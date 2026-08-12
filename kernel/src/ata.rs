use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;
use x86_64::instructions::interrupts::enable_and_hlt;
use x86_64::instructions::port::Port;

pub const SECTOR_SIZE: usize = 512;
/// 4 PIO slots + 4 AHCI slots.
pub const MAX_DEVICES: usize = 8;

const BASE_PRIMARY: u16 = 0x1F0;
const BASE_SECONDARY: u16 = 0x170;
const CMD_IDENTIFY: u8 = 0xEC;
const TIMEOUT_TICKS: u64 = 3000;

const DEV_NONE: AtaDevice = AtaDevice {
    present: false,
    is_secondary: false,
    master: false,
    lba_supported: false,
    sectors: 0,
    model: [0; 40],
    capacity_mb: 0,
    index: 0,
    via_ahci: false,
};

static DEVICES: Mutex<[AtaDevice; MAX_DEVICES]> = Mutex::new([DEV_NONE; MAX_DEVICES]);
static DEVICE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub struct AtaDevice {
    pub present: bool,
    pub is_secondary: bool,
    pub master: bool,
    pub lba_supported: bool,
    pub sectors: u64,
    pub model: [u8; 40],
    pub capacity_mb: u64,
    /// Slot index in the device table (route for AHCI reads/writes).
    pub index: usize,
    /// True when served by the AHCI driver (DMA) instead of PIO ATA.
    pub via_ahci: bool,
}

impl AtaDevice {
    pub fn model_str(&self) -> &str {
        let mut end = 40;
        while end > 0 && (self.model[end - 1] == b' ' || self.model[end - 1] == 0) {
            end -= 1;
        }
        core::str::from_utf8(&self.model[..end]).unwrap_or("?")
    }
}

pub fn device(index: usize) -> AtaDevice {
    DEVICES.lock()[index]
}

/// Registers a present device (used by the AHCI driver for slots 4..7).
pub fn set_device(index: usize, dev: AtaDevice) {
    let mut store = DEVICES.lock();
    store[index] = dev;
    DEVICE_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn device_count() -> usize {
    DEVICE_COUNT.load(Ordering::Relaxed)
}

pub fn init() {
    let mut index = 0usize;
    DEVICE_COUNT.store(0, Ordering::Relaxed);
    {
        let mut store = DEVICES.lock();
        for &secondary in &[false, true] {
            let base = if secondary { BASE_SECONDARY } else { BASE_PRIMARY };
            for &master in &[false, true] {
                let mut dev = DEV_NONE;
                dev.is_secondary = secondary;
                dev.master = master;
                identify(base, master, &mut dev);
                store[index] = dev;
                if dev.present {
                    DEVICE_COUNT.fetch_add(1, Ordering::Relaxed);
                }
                index += 1;
            }
        }
    }
}

fn out8(port: u16, value: u8) {
    unsafe {
        Port::new(port).write(value);
    }
}

fn in8(port: u16) -> u8 {
    unsafe { Port::new(port).read() }
}

fn in16(port: u16) -> u16 {
    unsafe { Port::new(port).read() }
}

fn poll_bsy_clear(base: u16) -> bool {
    let deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
    loop {
        let status = in8(base + 7);
        if status & 0x80 == 0 {
            return true;
        }
        if crate::interrupts::ticks() >= deadline {
            return false;
        }
        enable_and_hlt();
    }
}

fn poll_drq(base: u16) -> bool {
    let deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
    loop {
        let status = in8(base + 7);
        if status & 0x08 != 0 {
            return true;
        }
        if crate::interrupts::ticks() >= deadline {
            return false;
        }
        enable_and_hlt();
    }
}

fn identify(base: u16, master: bool, dev: &mut AtaDevice) {
    let drivesel: u8 = 0xA0 | if master { 0x00 } else { 0x10 };
    out8(base + 1, 0);
    out8(base + 2, 0);
    out8(base + 3, 0);
    out8(base + 4, 0);
    out8(base + 5, 0);
    out8(base + 6, drivesel);

    out8(base + 7, CMD_IDENTIFY);

    if !poll_bsy_clear(base) {
        return;
    }
    let status = in8(base + 7);
    if status & 0x01 != 0 {
        return;
    }
    if !poll_drq(base) {
        return;
    }

    let mut words = [0u16; 256];
    for w in words.iter_mut() {
        *w = in16(base);
    }

    dev.lba_supported = words[49] & (1 << 9) != 0;
    dev.sectors = (words[60] as u64) | ((words[61] as u64) << 16);
    dev.capacity_mb = dev.sectors / 2048;

    let mut at = 0usize;
    for i in 27..47 {
        let w = words[i];
        dev.model[at] = (w >> 8) as u8;
        dev.model[at + 1] = (w & 0xFF) as u8;
        at += 2;
    }

    dev.present = true;
}

#[allow(dead_code)]
pub fn read_sector(
    base: u16,
    secondary: bool,
    master: bool,
    lba: u32,
    buf: &mut [u8; SECTOR_SIZE],
) -> bool {
    let _ = secondary;
    if lba & 0x0FFF_FFFF != lba {
        return false;
    }
    let drivesel: u8 = 0xE0
        | if master { 0x00 } else { 0x10 }
        | ((lba >> 24) as u8 & 0x0F);
    out8(base + 6, drivesel);
    out8(base + 5, (lba >> 16) as u8);
    out8(base + 4, (lba >> 8) as u8);
    out8(base + 3, lba as u8);
    out8(base + 2, 0x01);
    out8(base + 7, 0x20);

    if !poll_bsy_clear(base) {
        crate::println!("[ATA] read lba={} bsy timeout st={:02x}", lba, in8(base + 7));
        return false;
    }
    let st = in8(base + 7);
    if st & 0x01 != 0 {
        crate::println!("[ATA] read lba={} error bit st={:02x} err={:02x}", lba, st, in8(base + 1));
        return false;
    }
    if !poll_drq(base) {
        crate::println!("[ATA] read lba={} drq timeout st={:02x}", lba, in8(base + 7));
        return false;
    }

    let mut at = 0usize;
    while at < SECTOR_SIZE {
        let word = in16(base);
        buf[at] = (word & 0xFF) as u8;
        buf[at + 1] = (word >> 8) as u8;
        at += 2;
    }
    if !poll_bsy_clear(base) {
        crate::println!("[ATA] read lba={} bsy after data st={:02x}", lba, in8(base + 7));
        return false;
    }
    let st = in8(base + 7);
    if st & 0x01 != 0 {
        crate::println!("[ATA] read lba={} error after data st={:02x} err={:02x}", lba, st, in8(base + 1));
        return false;
    }
    true
}

#[allow(dead_code)]
pub fn write_sector(
    base: u16,
    secondary: bool,
    master: bool,
    lba: u32,
    buf: &[u8; SECTOR_SIZE],
) -> bool {
    let _ = secondary;
    if lba & 0x0FFF_FFFF != lba {
        return false;
    }
    let drivesel: u8 = 0xE0
        | if master { 0x00 } else { 0x10 }
        | ((lba >> 24) as u8 & 0x0F);
    out8(base + 6, drivesel);
    out8(base + 5, (lba >> 16) as u8);
    out8(base + 4, (lba >> 8) as u8);
    out8(base + 3, lba as u8);
    out8(base + 2, 0x01);
    out8(base + 7, 0x30);

    if !poll_bsy_clear(base) {
        crate::println!("[ATA] write lba={} bsy timeout st={:02x}", lba, in8(base + 7));
        return false;
    }
    if !poll_drq(base) {
        crate::println!("[ATA] write lba={} drq timeout st={:02x}", lba, in8(base + 7));
        return false;
    }

    let mut at = 0usize;
    while at < SECTOR_SIZE {
        let word = (buf[at + 1] as u16) << 8 | buf[at] as u16;
        out16(base, word);
        at += 2;
    }
    if !poll_bsy_clear(base) {
        return false;
    }
    let st = in8(base + 7);
    if st & 0x01 != 0 {
        crate::println!("[ATA] write lba={} error bit st={:02x} err={:02x}", lba, st, in8(base + 1));
        return false;
    }
    if st & 0x08 != 0 {
        crate::println!("[ATA] write lba={} drq stuck st={:02x}", lba, st);
        return false;
    }
    true
}

fn out16(port: u16, value: u16) {
    unsafe {
        Port::new(port).write(value);
    }
}