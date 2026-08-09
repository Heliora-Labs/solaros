#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod ata;
mod boot;
mod acpi;
mod apic;
mod commands;
mod crc;
mod ext4;
mod fat;
mod framebuffer;
mod fs;
mod interrupts;
mod keyboard;
mod ps2;
mod serial;
mod settings;
mod terminal;
mod users;
mod vga_font;

use core::fmt;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use bootloader_api::entry_point;
use bootloader_api::BootInfo;
use x86_64::instructions::hlt;

pub const OS_NAME: &str = "SolarOS";
pub const OS_VERSION: &str = "26.1";
pub const KERNEL_NAME: &str = "solarcore";
pub const KERNEL_VERSION: &str = "1.0";

static USABLE_MB: AtomicU64 = AtomicU64::new(0);

pub fn usable_mem_mb() -> u64 {
    USABLE_MB.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    framebuffer::write_fmt(args);
    serial::write_fmt(args);
}

pub struct Utf8Chars<'a>(pub &'a [char]);

impl fmt::Display for Utf8Chars<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 4];
        for &c in self.0 {
            let encoded = c.encode_utf8(&mut buf);
            f.write_str(encoded)?;
        }
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    framebuffer::set_colors(framebuffer::RED, framebuffer::BG);
    println!("==================================================");
    println!("  !! SOLAROS PANIC !!");
    println!("  {}", info);
    println!("  System halted.");
    println!("==================================================");
    serial::write_fmt(format_args!("PANIC: {info}\n"));
    loop {
        hlt();
    }
}

struct CpuIdResult {
    eax: u32,
    ebx: u32,
    ecx: u32,
    edx: u32,
}

fn cpuid(leaf: u32) -> CpuIdResult {
    let mut eax: u32 = leaf;
    let mut rbx: u64;
    let mut ecx: u32 = 0;
    let mut edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov rbx, 0",
            "cpuid",
            "mov {0}, rbx",
            "pop rbx",
            out(reg) rbx,
            inout("eax") eax,
            inout("ecx") ecx,
            out("edx") edx,
        );
    }
    CpuIdResult {
        eax,
        ebx: rbx as u32,
        ecx,
        edx,
    }
}

pub fn cpu_vendor() -> [u8; 12] {
    let res = cpuid(0);
    let mut v = [0u8; 12];
    v[0..4].copy_from_slice(&res.ebx.to_ne_bytes());
    v[4..8].copy_from_slice(&res.edx.to_ne_bytes());
    v[8..12].copy_from_slice(&res.ecx.to_ne_bytes());
    v
}

pub fn cpu_model() -> u32 {
    cpuid(1).eax
}

pub fn cpu_brand() -> [u8; 48] {
    if cpuid(0x8000_0000).eax >= 0x8000_0004 {
        let r0 = cpuid(0x8000_0002);
        let r1 = cpuid(0x8000_0003);
        let r2 = cpuid(0x8000_0004);
        let mut b = [0u8; 48];
        b[0..4].copy_from_slice(&r0.eax.to_ne_bytes());
        b[4..8].copy_from_slice(&r0.ebx.to_ne_bytes());
        b[8..12].copy_from_slice(&r0.ecx.to_ne_bytes());
        b[12..16].copy_from_slice(&r0.edx.to_ne_bytes());
        b[16..20].copy_from_slice(&r1.eax.to_ne_bytes());
        b[20..24].copy_from_slice(&r1.ebx.to_ne_bytes());
        b[24..28].copy_from_slice(&r1.ecx.to_ne_bytes());
        b[28..32].copy_from_slice(&r1.edx.to_ne_bytes());
        b[32..36].copy_from_slice(&r2.eax.to_ne_bytes());
        b[36..40].copy_from_slice(&r2.ebx.to_ne_bytes());
        b[40..44].copy_from_slice(&r2.ecx.to_ne_bytes());
        b[44..48].copy_from_slice(&r2.edx.to_ne_bytes());
        b
    } else {
        [0u8; 48]
    }
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();

    let fb_info = if let Some(fb) = boot_info.framebuffer.take() {
        let info = fb.info();
        let buffer: &'static mut [u8] = fb.into_buffer();
        framebuffer::init(buffer, info);
        Some(info)
    } else {
        None
    };

    let memory: &[bootloader_api::info::MemoryRegion] = &*boot_info.memory_regions;
    let usable_bytes: u64 = memory
        .iter()
        .filter(|r| r.kind == bootloader_api::info::MemoryRegionKind::Usable)
        .map(|r| r.end - r.start)
        .sum();
    USABLE_MB.store(usable_bytes / 1024 / 1024, Ordering::Relaxed);

    boot::info(format_args!("Booting SolarOS {}...", OS_VERSION));

    let ps2 = ps2::init();
    if ps2.controller_ok && ps2.device_ok {
        boot::ok(format_args!(
            "PS/2: controller self-test passed, scancode set 1, IRQ1 armed"
        ));
    } else {
        boot::fail(format_args!(
            "PS/2: degraded mode (ctrl ok: {}, device ok: {})",
            ps2.controller_ok, ps2.device_ok
        ));
    }

    interrupts::init();
    interrupts::sleep_ms(120);

    let acpi = acpi::init(boot_info);
    if let Some(a) = &acpi {
        let _ = apic::init(a);
    }

    match fb_info {
        Some(info) => boot::ok(format_args!(
            "Framebuffer: {}x{} px, {} BPP ({:?}), stride {}",
            info.width,
            info.height,
            info.bytes_per_pixel * 8,
            info.pixel_format,
            info.stride
        )),
        None => boot::fail(format_args!("No framebuffer available")),
    }
    interrupts::sleep_ms(110);

    let vendor_bytes = cpu_vendor();
    let vendor = core::str::from_utf8(&vendor_bytes).unwrap_or("Unknown");
    let f = cpuid(1);
    boot::ok(format_args!("CPU: {:#x} model, vendor {}", f.eax, vendor));
    interrupts::sleep_ms(90);
    boot::ok(format_args!(
        "CPU features: FPU:{0} PSE:{1} TSC:{2} PAE:{3} SSE:{4} SSE2:{5} SSE3:{6} SSSE3:{7} SSE4.1:{8} SSE4.2:{9}",
        f.edx & 1 != 0,
        f.edx & (1 << 3) != 0,
        f.edx & (1 << 4) != 0,
        f.edx & (1 << 6) != 0,
        f.edx & (1 << 25) != 0,
        f.edx & (1 << 26) != 0,
        f.ecx & 1 != 0,
        f.ecx & (1 << 9) != 0,
        f.ecx & (1 << 19) != 0,
        f.ecx & (1 << 20) != 0,
    ));
    interrupts::sleep_ms(90);

    boot::ok(format_args!(
        "Memory map: {} MB usable, {} regions",
        usable_bytes / 1024 / 1024,
        memory.len()
    ));
    for (i, r) in memory.iter().enumerate() {
        interrupts::sleep_ms(70);
        boot::ok(format_args!(
            "  Region {:02}: {:#018x} - {:#018x} ({:?}, {} KB)",
            i,
            r.start,
            r.end,
            r.kind,
            (r.end - r.start) / 1024
        ));
    }

    interrupts::sleep_ms(70);
    boot::ok(format_args!(
        "Interrupts: IDT + PIC initialized (IRQs remapped to 32-47)"
    ));

    interrupts::sleep_ms(70);
    ata::init();
    let disk_count = ata::device_count();
    boot::ok(format_args!(
        "ATA: scanned {} devices, {} present",
        ata::MAX_DEVICES,
        disk_count
    ));
    for i in 0..ata::MAX_DEVICES {
        let d = ata::device(i);
        let slot = if d.is_secondary { "secondary" } else { "primary" };
        let pos = if d.master { "master" } else { "slave" };
        if d.present {
            interrupts::sleep_ms(90);
            boot::ok(format_args!(
                "Disk {}: {}:{} - {} sectors ({} MB), LBA {}, {}",
                i,
                slot,
                pos,
                d.sectors,
                d.capacity_mb,
                if d.lba_supported { "yes" } else { "no" },
                d.model_str()
            ));
        } else {
            interrupts::sleep_ms(40);
            boot::fail(format_args!("Disk {}: {}:{} - no device", i, slot, pos));
        }
    }

    interrupts::sleep_ms(90);
    boot::ok(format_args!("Keyboard: {}", settings::layout_code()));

    interrupts::sleep_ms(90);
    users::init();
    match fs::mount() {
        Ok(()) => {
            let g = crate::ext4::groups();
            boot::ok(format_args!(
                "Filesystem: {} mounted at / ({} {})",
                fs::fs_name(),
                g,
                if g == 1 { "group" } else { "groups" }
            ));
            users::load_from_fs();
        }
        Err(fs::FsErr::NoDevice) => boot::fail(format_args!("Filesystem: no data disk")),
        Err(fs::FsErr::NotFormatted) => {
            boot::fail(format_args!(
                "Filesystem: data disk not formatted (run 'mkfs')"
            ));
        }
        Err(_) => boot::fail(format_args!("Filesystem: I/O error")),
    }

    interrupts::sleep_ms(90);
    boot::ok(format_args!("Terminal ready"));
    interrupts::sleep_ms(250);

    framebuffer::clear();
    println!();
    framebuffer::set_colors(framebuffer::ACCENT, framebuffer::BG);
    println!("          S O L A R   O S   {}", OS_VERSION);
    framebuffer::reset_colors();
    println!();

    terminal::run();
}

static BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    use bootloader_api::config::Mapping;
    let mut c = bootloader_api::BootloaderConfig::new_default();
    c.kernel_stack_size = 1024 * 1024;
    c.mappings.physical_memory = Some(Mapping::FixedAddress(0xFFFF_8000_0000_0000));
    c
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);
