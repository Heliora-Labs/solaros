use crate::acpi::{ptr, Acpi};
use crate::boot;

const LAPIC_DEF: u64 = 0xFEE0_0000;
const IOAPIC_DEF: u64 = 0xFEC0_0000;

const IOREGSEL: u64 = 0x00;
const IOREGWIN: u64 = 0x10;
const IOAPIC_VER: u32 = 0x01;
const IOAPIC_REDIR: u32 = 0x10;
const MAX_PINS: u32 = 24;

const REDIR_MASK: u64 = 1 << 16;
const REDIR_POLARITY: u64 = 1 << 12;
const REDIR_TRIGGER: u64 = 1 << 13;

struct State {
    iso: [(u32, u16); 16],
    ioapic_base: u64,
}

static STATE: spin::Mutex<State> = spin::Mutex::new(State {
    iso: [(u32::MAX, u16::MAX); 16],
    ioapic_base: IOAPIC_DEF,
});

fn lapic(off: u64) -> *mut u32 {
    (LAPIC_DEF + off + crate::acpi::PMO) as *mut u32
}

fn ioapic_read(reg: u32) -> u32 {
    let base = STATE.lock().ioapic_base;
    let sel = (base + IOREGSEL + crate::acpi::PMO) as *mut u32;
    let win = (base + IOREGWIN + crate::acpi::PMO) as *mut u32;
    unsafe {
        sel.write_volatile(reg);
        win.read_volatile()
    }
}

fn ioapic_write(reg: u32, value: u32) {
    let base = STATE.lock().ioapic_base;
    let sel = (base + IOREGSEL + crate::acpi::PMO) as *mut u32;
    let win = (base + IOREGWIN + crate::acpi::PMO) as *mut u32;
    unsafe {
        sel.write_volatile(reg);
        win.write_volatile(value);
    }
}

pub fn eoi() {
    unsafe {
        lapic(0xB0).write_volatile(0);
    }
}

fn gsi_of(irq: u8) -> (u32, u16) {
    let e = STATE.lock().iso[irq as usize & 0xF];
    if e.1 == u16::MAX && e.0 == u32::MAX {
        (irq as u32, 0)
    } else {
        e
    }
}

fn program_pin(pin: u32, vector: u8, flags: u16) {
    let entry = vector as u64
        | (flags as u64 & 0x3F)
        | ((flags as u64 >> 7) & REDIR_POLARITY)
        | ((flags as u64 >> 3) & REDIR_TRIGGER);
    ioapic_write(IOAPIC_REDIR + pin * 2, entry as u32);
    ioapic_write(IOAPIC_REDIR + pin * 2 + 1, (entry >> 32) as u32);
}

pub fn init(acpi: &Acpi) -> bool {
    let madt = match crate::acpi::find(acpi, b"APIC") {
        Some(t) => t,
        None => {
            boot::fail(format_args!("APIC: MADT not found"));
            return false;
        }
    };
    let base = ptr(madt.addr);
    let hdr_len = unsafe { core::ptr::read_unaligned(base.add(4) as *const u32) as usize };
    let lapic_addr_hw = unsafe { core::ptr::read_unaligned(base.add(36) as *const u32) as u64 };

    let mut ioapic_addr = IOAPIC_DEF;
    let mut ioapic_id = 0u8;
    let mut gsi_base = 0u32;
    let mut off = 0x2C;
    while off + 2 <= hdr_len {
        let rec = unsafe { core::ptr::read_unaligned(base.add(off) as *const u8) };
        let len = unsafe { core::ptr::read_unaligned(base.add(off + 1) as *const u8) } as usize;
        if len < 2 || off + len > hdr_len {
            break;
        }
        match rec {
            1 => {
                ioapic_id = unsafe { core::ptr::read_unaligned(base.add(off + 2) as *const u8) };
                ioapic_addr =
                    unsafe { core::ptr::read_unaligned(base.add(off + 4) as *const u32) as u64 };
                gsi_base = unsafe { core::ptr::read_unaligned(base.add(off + 8) as *const u32) };
            }
            2 => {
                let irq = unsafe { core::ptr::read_unaligned(base.add(off + 3) as *const u8) };
                let gsi = unsafe { core::ptr::read_unaligned(base.add(off + 4) as *const u32) };
                let flags = unsafe { core::ptr::read_unaligned(base.add(off + 8) as *const u16) };
                if irq < 16 {
                    STATE.lock().iso[irq as usize] = (gsi, flags);
                }
            }
            4 => {
                let _ = unsafe { core::ptr::read_unaligned(base.add(off + 4) as *const u64) };
            }
            _ => {}
        }
        off += len;
    }

    {
        let mut s = STATE.lock();
        s.ioapic_base = ioapic_addr;
    }

    unsafe {
        let svr = lapic(0xF0);
        svr.write_volatile(svr.read_volatile() | 0x100);
    }

    let ver = ioapic_read(IOAPIC_VER);
    let pins = ((ver >> 16) & 0xFF) + 1;

    let (pit_gsi, pit_flags) = gsi_of(0);
    let (kbd_gsi, kbd_flags) = gsi_of(1);

    for pin in 0..pins.min(MAX_PINS) {
        if pin == pit_gsi || pin == kbd_gsi {
            continue;
        }
        let entry = ioapic_read(IOAPIC_REDIR + pin * 2) as u64
            | ((ioapic_read(IOAPIC_REDIR + pin * 2 + 1) as u64) << 32);
        if entry & REDIR_MASK == 0 {
            program_pin(pin, 0xEF, 0x8000);
        }
    }

    program_pin(pit_gsi.min(pins.saturating_sub(1)), 32, pit_flags);
    program_pin(kbd_gsi.min(pins.saturating_sub(1)), 33, kbd_flags);

    crate::interrupts::pic_mask_all();

    boot::ok(format_args!(
        "APIC: LAPIC {:#x} IOAPIC {:#x} (id {}, {} pins, GSI base {})",
        lapic_addr_hw,
        ioapic_addr,
        ioapic_id,
        pins,
        gsi_base
    ));
    boot::ok(format_args!(
        "APIC: IRQ0 -> GSI{} (PIT), IRQ1 -> GSI{} (kbd); PIC masked",
        pit_gsi, kbd_gsi
    ));
    true
}