use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{read_volatile, write_volatile};

use spin::Mutex;
use x86_64::instructions::interrupts::enable_and_hlt;

use crate::acpi::PMO;
use crate::ata::AtaDevice;

// ---- HBA generic registers ----
const HBA_CAP: usize = 0x00;
const HBA_GHC: usize = 0x04;
const HBA_PI: usize = 0x0C;

// ---- port registers (base + port * 0x80) ----
const PORT_OFF: usize = 0x100;
const PORT_STRIDE: usize = 0x80;
const PxCLB: usize = 0x00;
const PxFB: usize = 0x08;
const PxIS: usize = 0x10;
const PxIE: usize = 0x14;
const PxCMD: usize = 0x18;
const PxTFD: usize = 0x20;
const PxSIG: usize = 0x24;
const PxSSTS: usize = 0x28;
const PxSCTL: usize = 0x2C;
const PxSERR: usize = 0x30;
const PxCI: usize = 0x38;

const GHC_HR: u32 = 0x0000_0001;
const GHC_AE: u32 = 0x8000_0000;
const PxCMD_ST: u32 = 0x0000_0001;
const PxCMD_FRE: u32 = 0x0000_0010;
const PxCMD_CR: u32 = 0x0000_8000;
const PxCMD_POD: u32 = 0x0000_0004;
const PxCMD_SUD: u32 = 0x0000_0002;
const PxIS_TFES: u32 = 0x4000_0000;
const PxTFD_ERR: u32 = 0x0000_0001;
const PxTFD_BSY: u32 = 0x0000_0080;

const SIG_ATA: u32 = 0x0000_0101;
const SIG_ATAPI: u32 = 0xEB14_0101;

const CMD_IDENTIFY: u8 = 0xEC;
const CMD_READ_DMA_EXT: u8 = 0x25;
const CMD_WRITE_DMA_EXT: u8 = 0x35;

/// ATA device slots reserved for AHCI disks (PIO fills 0..4).
pub const ATA_BASE_SLOT: usize = 4;
const MAX_PORTS: usize = 4;
const TIMEOUT_TICKS: u64 = 40;

#[derive(Clone, Copy)]
struct Port {
    active: bool,
    port_base: usize,
    clb_virt: usize,
    ct_virt: usize,
    dma_virt: usize,
}

struct Ahci {
    abar: usize,
    ports: [Port; MAX_PORTS],
    disk_count: usize,
}

static AHCI: Mutex<Option<Ahci>> = Mutex::new(None);

impl Ahci {
    fn port_mut(&mut self, ata_slot: usize) -> Option<&mut Port> {
        if ata_slot < ATA_BASE_SLOT || ata_slot >= ATA_BASE_SLOT + MAX_PORTS {
            return None;
        }
        let i = ata_slot - ATA_BASE_SLOT;
        if self.ports[i].active {
            Some(&mut self.ports[i])
        } else {
            None
        }
    }
}

// ---- MMIO access ----

fn mmio_read(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

fn mmio_write(addr: usize, v: u32) {
    unsafe { write_volatile(addr as *mut u32, v) }
}

fn hba_reg(abar: usize, off: usize) -> usize {
    abar + off
}

fn port_reg(port_base: usize, off: usize) -> usize {
    port_base + off
}

/// Allocates an aligned DMA block. Heap memory lives in the PMO window,
/// so phys = virt - PMO and QEMU can DMA into it.
fn alloc_dma(size: usize) -> Option<usize> {
    let layout = unsafe { Layout::from_size_align_unchecked(size, 4096) };
    let virt = unsafe { crate::heap::ALLOCATOR.alloc(layout) } as usize;
    if virt == 0 {
        return None;
    }
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, size);
    }
    Some(virt)
}

fn virt_to_phys(virt: usize) -> u64 {
    virt as u64 - PMO
}

/// Finds the first AHCI SATA controller in the PCI device list.
fn find_sata_controller() -> Option<crate::pci::PciDevice> {
    for i in 0..crate::pci::device_count() {
        let d = crate::pci::device(i)?;
        if d.class == 0x01 && d.subclass == 0x06 {
            return Some(d);
        }
    }
    None
}

pub fn init() -> usize {
    let Some(ctlr) = find_sata_controller() else {
        crate::serial::write_fmt(format_args!("[AHCI] no SATA controller found\n"));
        return 0;
    };

    let bar_low = crate::pci::config_read_u32(ctlr.bus, ctlr.dev, ctlr.func, 0x24);
    let bar_high = crate::pci::config_read_u32(ctlr.bus, ctlr.dev, ctlr.func, 0x28);
    let abar_phys = (bar_low as u64 & 0xFFFF_FFF0) | ((bar_high as u64) << 32);
    if abar_phys == 0 {
        crate::serial::write_fmt(format_args!("[AHCI] ABAR is 0\n"));
        return 0;
    }

    // enable memory space + bus mastering
    let cmd = crate::pci::config_read_u16(ctlr.bus, ctlr.dev, ctlr.func, 0x04);
    crate::pci::config_write_u16(ctlr.bus, ctlr.dev, ctlr.func, 0x04, cmd | 0x06);

    let abar = (PMO + abar_phys) as usize;
    crate::serial::write_fmt(format_args!(
        "[AHCI] HBA at {:02x}:{:02x}.{} BAR5 phys {:#x}\n",
        ctlr.bus, ctlr.dev, ctlr.func, abar_phys
    ));

    // reset HBA
    mmio_write(hba_reg(abar, HBA_GHC), GHC_HR);
    let deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
    while mmio_read(hba_reg(abar, HBA_GHC)) & GHC_HR != 0 {
        if crate::interrupts::ticks() >= deadline {
            crate::serial::write_fmt(format_args!("[AHCI] HBA reset timeout\n"));
            return 0;
        }
        enable_and_hlt();
    }

    // AHCI enable + interrupts off
    let ghc = mmio_read(hba_reg(abar, HBA_GHC));
    mmio_write(hba_reg(abar, HBA_GHC), ghc | GHC_AE);

    let pi = mmio_read(hba_reg(abar, HBA_PI));
    let cap_np = ((mmio_read(hba_reg(abar, HBA_CAP)) & 0x1F) + 1) as usize;

    let mut ahci = Ahci {
        abar,
        ports: [Port {
            active: false,
            port_base: 0,
            clb_virt: 0,
            ct_virt: 0,
            dma_virt: 0,
        }; MAX_PORTS],
        disk_count: 0,
    };

    for p in 0..cap_np.min(MAX_PORTS) {
        if pi & (1 << p) == 0 {
            continue;
        }
        let pb = abar + PORT_OFF + p * PORT_STRIDE;

        // port startup: stop engines, clear state, assign memory
        let mut cmd = mmio_read(port_reg(pb, PxCMD));
        cmd &= !(PxCMD_ST | PxCMD_FRE);
        mmio_write(port_reg(pb, PxCMD), cmd);
        let deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
        while mmio_read(port_reg(pb, PxCMD)) & PxCMD_CR != 0 {
            if crate::interrupts::ticks() >= deadline {
                crate::serial::write_fmt(format_args!("[AHCI] port {} stuck in CR\n", p));
                break;
            }
            enable_and_hlt();
        }

        let (Some(clb), Some(fb), Some(ct), Some(dma)) = (
            alloc_dma(4096),
            alloc_dma(4096),
            alloc_dma(4096),
            alloc_dma(512),
        ) else {
            crate::serial::write_fmt(format_args!("[AHCI] port {} DMA alloc failed\n", p));
            continue;
        };

        let clb_phys = virt_to_phys(clb);
        let fb_phys = virt_to_phys(fb);
        mmio_write(port_reg(pb, PxCLB), clb_phys as u32);
        mmio_write(port_reg(pb, PxCLB + 4), (clb_phys >> 32) as u32);
        mmio_write(port_reg(pb, PxFB), fb_phys as u32);
        mmio_write(port_reg(pb, PxFB + 4), (fb_phys >> 32) as u32);
        mmio_write(port_reg(pb, PxIS), 0xFFFF_FFFF);
        mmio_write(port_reg(pb, PxIE), 0);

        mmio_write(port_reg(pb, PxSERR), 0xFFFF_FFFF);

        cmd = mmio_read(port_reg(pb, PxCMD));
        cmd |= PxCMD_FRE | PxCMD_POD | PxCMD_SUD;
        mmio_write(port_reg(pb, PxCMD), cmd);
        cmd |= PxCMD_ST;
        mmio_write(port_reg(pb, PxCMD), cmd);

        // wait for the link to come up (DET=3) before trusting SIG
        let mut deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
        loop {
            let ssts = mmio_read(port_reg(pb, PxSSTS));
            if ssts & 0x0F == 3 {
                break;
            }
            if crate::interrupts::ticks() >= deadline {
                crate::serial::write_fmt(format_args!(
                    "[AHCI] port {} link down (ssts={:08x})\n",
                    p, ssts
                ));
                break;
            }
            enable_and_hlt();
        }
        if mmio_read(port_reg(pb, PxSSTS)) & 0x0F != 3 {
            continue;
        }

        let sig = mmio_read(port_reg(pb, PxSIG));
        if sig != SIG_ATA && sig != SIG_ATAPI {
            crate::serial::write_fmt(format_args!(
                "[AHCI] port {} unexpected signature {:08x}\n",
                p, sig
            ));
            continue;
        }

        ahci.ports[ahci.disk_count] = Port {
            active: true,
            port_base: pb,
            clb_virt: clb,
            ct_virt: ct,
            dma_virt: dma,
        };

        let adev = identify(&mut ahci.ports[ahci.disk_count], p);
        if adev.present {
            let slot = ATA_BASE_SLOT + ahci.disk_count;
            let mut stored = adev;
            stored.index = slot;
            stored.via_ahci = true;
            crate::ata::set_device(slot, stored);
            ahci.disk_count += 1;
        } else {
            ahci.ports[ahci.disk_count].active = false;
        }
    }

    let count = ahci.disk_count;
    if count > 0 {
        *AHCI.lock() = Some(ahci);
    }
    count
}

fn identify(port: &mut Port, hba_port: usize) -> AtaDevice {
    let mut dev = AtaDevice {
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

    if !issue(port, false, CMD_IDENTIFY, 0, 0, 511) {
        crate::serial::write_fmt(format_args!(
            "[AHCI] port {} identify failed\n",
            hba_port
        ));
        return dev;
    }

    // POST-IDENTIFY IO TEST: PIO state on this QEMU build appears to get
    // poisoned after the first PIO command (prepare uses a stale offset).
    // Check whether DMA commands (which reset the offset in start_dma) still
    // transfer full sectors.
    if issue(port, false, CMD_READ_DMA_EXT, 42, 1, 511) {
        let dbg = unsafe { core::slice::from_raw_parts(port.dma_virt as *const u8, 512) };
        crate::serial::write_fmt(format_args!(
            "[AHCI] dma-read-ok lba42 head={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} tail={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} bytes/8={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}\n",
            dbg[0], dbg[1], dbg[2], dbg[3], dbg[4], dbg[5], dbg[6], dbg[7],
            dbg[504], dbg[505], dbg[506], dbg[507], dbg[508], dbg[509], dbg[510], dbg[511],
            dbg[8], dbg[9], dbg[10], dbg[11], dbg[12], dbg[13], dbg[14], dbg[15]
        ));
    } else {
        crate::serial::write_fmt(format_args!("[AHCI] dma-read FAILED\n"));
    }
    unsafe {
        core::ptr::write_bytes(port.dma_virt as *mut u8, 0xAA, 512);
    }
    if issue(port, true, CMD_WRITE_DMA_EXT, 42, 1, 511) {
        crate::serial::write_fmt(format_args!("[AHCI] dma-write-ok\n"));
    } else {
        crate::serial::write_fmt(format_args!("[AHCI] dma-write FAILED\n"));
    }
    unsafe {
        core::ptr::write_bytes(port.dma_virt as *mut u8, 0, 512);
    }
    if issue(port, false, CMD_READ_DMA_EXT, 42, 1, 511) {
        let dbg = unsafe { core::slice::from_raw_parts(port.dma_virt as *const u8, 512) };
        crate::serial::write_fmt(format_args!(
            "[AHCI] dma-reread lba42 head={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} tail={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} bytes/8={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}\n",
            dbg[0], dbg[1], dbg[2], dbg[3], dbg[4], dbg[5], dbg[6], dbg[7],
            dbg[504], dbg[505], dbg[506], dbg[507], dbg[508], dbg[509], dbg[510], dbg[511],
            dbg[8], dbg[9], dbg[10], dbg[11], dbg[12], dbg[13], dbg[14], dbg[15]
        ));
    } else {
        crate::serial::write_fmt(format_args!("[AHCI] dma-reread FAILED\n"));
    }

    let dbg = unsafe { core::slice::from_raw_parts(port.dma_virt as *const u8, 512) };
    crate::serial::write_fmt(format_args!(
        "[AHCI] dbg dma_virt={:#x} dba={:#x} dma[0..32]={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}\n",
        port.dma_virt,
        virt_to_phys(port.dma_virt),
        dbg[0], dbg[1], dbg[2], dbg[3], dbg[4], dbg[5], dbg[6], dbg[7],
        dbg[8], dbg[9], dbg[10], dbg[11], dbg[12], dbg[13], dbg[14], dbg[15],
        dbg[16], dbg[17], dbg[18], dbg[19], dbg[20], dbg[21], dbg[22], dbg[23],
        dbg[24], dbg[25], dbg[26], dbg[27], dbg[28], dbg[29], dbg[30], dbg[31]
    ));
    crate::serial::write_fmt(format_args!(
        "[AHCI] dbg tfd={:08x} is={:08x} cmd={:08x}\n",
        mmio_read(port_reg(port.port_base, PxTFD)),
        mmio_read(port_reg(port.port_base, PxIS)),
        mmio_read(port_reg(port.port_base, PxCMD))
    ));
    unsafe {
        let clb = port.clb_virt as *const u32;
        let ct = port.ct_virt as *const u32;
        let prd = (port.ct_virt + 128) as *const u32;
        crate::serial::write_fmt(format_args!(
            "[AHCI] dbg clb={:#x} ct={:#x} hdr={:08x} {:08x} {:08x} {:08x} prd={:08x} {:08x} {:08x} {:08x}\n",
            port.clb_virt,
            port.ct_virt,
            *clb.add(0),
            *clb.add(1),
            *clb.add(2),
            *clb.add(3),
            *prd.add(0),
            *prd.add(1),
            *prd.add(2),
            *prd.add(3)
        ));
    }

    let mut tmp = [0u16; 256];
    unsafe {
        core::ptr::copy_nonoverlapping(port.dma_virt as *const u8, tmp.as_mut_ptr() as *mut u8, 512);
    }

    dev.lba_supported = tmp[49] & (1 << 9) != 0;
    dev.sectors = (tmp[60] as u64) | ((tmp[61] as u64) << 16);
    if tmp[49] & (1 << 10) != 0 {
        dev.sectors = (tmp[100] as u64)
            | ((tmp[101] as u64) << 16)
            | ((tmp[102] as u64) << 32)
            | ((tmp[103] as u64) << 48);
    }
    dev.capacity_mb = dev.sectors / 2048;
    let mut at = 0usize;
    for i in 27..47 {
        let w = tmp[i];
        dev.model[at] = (w >> 8) as u8;
        dev.model[at + 1] = (w & 0xFF) as u8;
        at += 2;
    }
    dev.present = true;
    dev
}

/// Issues a single-sector DMA command on the given port (slot 0).
/// `write` selects the data direction; `count` is in sectors (0 = 256 for
/// legacy identify semantics, but we only ever use 1 or 0).
fn issue(port: &mut Port, write: bool, command: u8, lba: u64, count: u16, dbc: u32) -> bool {
    let pb = port.port_base;

    // wait for previous command / busy
    let deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
    while mmio_read(port_reg(pb, PxTFD)) & PxTFD_BSY != 0 || mmio_read(port_reg(pb, PxCI)) != 0 {
        if crate::interrupts::ticks() >= deadline {
            crate::serial::write_fmt(format_args!(
                "[AHCI] port busy timeout tfd={:08x}\n",
                mmio_read(port_reg(pb, PxTFD))
            ));
            return false;
        }
        enable_and_hlt();
    }

    // ---- command table: H2D FIS (20 bytes) at ct + 0 ----
    let ct = port.ct_virt;
    unsafe {
        core::ptr::write_bytes(ct as *mut u8, 0, 128);
        let f = ct as *mut u8;
        *f.add(0) = 0x27; // H2D FIS type
        *f.add(1) = 0x80; // command flag (c bit)
        *f.add(2) = command;
        *f.add(3) = 0; // features
        *f.add(4) = (lba & 0xFF) as u8;
        *f.add(5) = ((lba >> 8) & 0xFF) as u8;
        *f.add(6) = ((lba >> 16) & 0xFF) as u8;
        *f.add(7) = 0x40; // device: LBA
        *f.add(8) = ((lba >> 24) & 0xFF) as u8;
        *f.add(9) = ((lba >> 32) & 0xFF) as u8;
        *f.add(10) = ((lba >> 40) & 0xFF) as u8;
        *f.add(12) = (count & 0xFF) as u8;
        *f.add(13) = ((count >> 8) & 0xFF) as u8;

        // ---- PRD at ct + 128 ----
        let prd = (ct + 128) as *mut u32;
        let dba = virt_to_phys(port.dma_virt);
        *prd.add(0) = dba as u32;
        *prd.add(1) = (dba >> 32) as u32;
        *prd.add(2) = dbc;
        *prd.add(3) = 0;
    }

    // ---- command list entry (slot 0) at clb + 0 ----
    let clb = port.clb_virt;
    let cfl = 5u32; // 5 dwords H2D FIS
    let w = if write { 1u32 << 6 } else { 0 };
    let prdtl = 1u32 << 16;
    let ctba = virt_to_phys(ct);
    unsafe {
        let e = clb as *mut u32;
        *e.add(0) = prdtl | w | cfl;
        *e.add(1) = 0; // PRDBC
        *e.add(2) = ctba as u32;
        *e.add(3) = (ctba >> 32) as u32;
        for i in 4..8 {
            *e.add(i) = 0;
        }
    }

    // ---- issue ----
    unsafe {
        let prd = (ct + 128) as *const u32;
        let e = clb as *const u32;
        crate::serial::write_fmt(format_args!(
            "[AHCI] dbg pre-ci hdr={:08x} {:08x} {:08x} {:08x} prd={:08x} {:08x} {:08x} {:08x}\n",
            read_volatile(e.add(0)),
            read_volatile(e.add(1)),
            read_volatile(e.add(2)),
            read_volatile(e.add(3)),
            read_volatile(prd.add(0)),
            read_volatile(prd.add(1)),
            read_volatile(prd.add(2)),
            read_volatile(prd.add(3))
        ));
    }
    crate::serial::write_fmt(format_args!("[AHCI] dbg WAIT-MARKER-BEFORE-CI\n"));
    let mark = crate::interrupts::ticks() + 3000;
    while crate::interrupts::ticks() < mark {
        enable_and_hlt();
    }
    mmio_write(port_reg(pb, PxIS), 0xFFFF_FFFF);
    mmio_write(port_reg(pb, PxCI), 1 << 0);

    let deadline = crate::interrupts::ticks() + TIMEOUT_TICKS;
    while mmio_read(port_reg(pb, PxCI)) & 1 != 0 {
        if crate::interrupts::ticks() >= deadline {
            crate::serial::write_fmt(format_args!(
                "[AHCI] cmd {:02x} lba {} timeout tfd={:08x} is={:08x}\n",
                command,
                lba,
                mmio_read(port_reg(pb, PxTFD)),
                mmio_read(port_reg(pb, PxIS))
            ));
            return false;
        }
        enable_and_hlt();
    }
    crate::serial::write_fmt(format_args!("[AHCI] dbg WAIT-MARKER-AFTER-CMD\n"));
    let mark2 = crate::interrupts::ticks() + 3000;
    while crate::interrupts::ticks() < mark2 {
        enable_and_hlt();
    }

    let is = mmio_read(port_reg(pb, PxIS));
    let tfd = mmio_read(port_reg(pb, PxTFD));
    if is & PxIS_TFES != 0 || tfd & PxTFD_ERR != 0 {
        crate::serial::write_fmt(format_args!(
            "[AHCI] cmd {:02x} lba {} error is={:08x} tfd={:08x}\n",
            command, lba, is, tfd
        ));
        return false;
    }
    true
}

pub fn read_sector(ata_slot: usize, lba: u32, buf: &mut [u8; 512]) -> bool {
    let mut guard = AHCI.lock();
    let Some(ahci) = guard.as_mut() else { return false };
    let Some(port) = ahci.port_mut(ata_slot) else { return false };
    if !issue(port, false, CMD_READ_DMA_EXT, lba as u64, 1, 511) {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(port.dma_virt as *const u8, buf.as_mut_ptr(), 512);
    }
    true
}

pub fn write_sector(ata_slot: usize, lba: u32, buf: &[u8; 512]) -> bool {
    let mut guard = AHCI.lock();
    let Some(ahci) = guard.as_mut() else { return false };
    let Some(port) = ahci.port_mut(ata_slot) else { return false };
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), port.dma_virt as *mut u8, 512);
    }
    issue(port, true, CMD_WRITE_DMA_EXT, lba as u64, 1, 511)
}
