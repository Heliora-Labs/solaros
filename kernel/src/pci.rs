use crate::println;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::port::Port;

pub const MAX_DEVICES: usize = 64;

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub progif: u8,
    pub header_type: u8,
    pub secondary_bus: u8,
    pub irq_line: u8,
}

static DEVICES: Mutex<Vec<PciDevice>> = Mutex::new(Vec::new());

pub fn config_read_u32(bus: u8, dev: u8, func: u8, offset: u16) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        let mut c = Port::new(0xCF8);
        c.write(addr);
        let mut d = Port::new(0xCFC);
        d.read()
    }
}

pub fn config_read_u16(bus: u8, dev: u8, func: u8, offset: u16) -> u16 {
    (config_read_u32(bus, dev, func, offset) >> (((offset & 2) as u32) * 8)) as u16
}

pub fn config_read_u8(bus: u8, dev: u8, func: u8, offset: u16) -> u8 {
    (config_read_u32(bus, dev, func, offset) >> (((offset & 3) as u32) * 8)) as u8
}

pub fn config_write_u16(bus: u8, dev: u8, func: u8, offset: u16, value: u16) {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        let mut c = Port::new(0xCF8);
        c.write(addr);
        let mut d = Port::new(0xCFC);
        d.write(value as u32);
    }
}

/// Full enumeration: scan bus 0, then any buses behind PCI-PCI bridges
/// (secondary bus number from the bridge header). Config space is accessed
/// through the legacy PIO mechanism (0xCF8/0xCFC) - sufficient for QEMU's
/// `pc` machine and for bus 0 on real hardware. ECAM (MCFG) remains future
/// work for multi-root-port real hardware.
pub fn enumerate() {
    let mut list = DEVICES.lock();
    list.clear();
    let mut buses: Vec<u16> = alloc::vec![0];
    let mut idx = 0usize;
    while idx < buses.len() {
        let bus = buses[idx] as u8;
        idx += 1;
        for dev in 0..32u8 {
            let mut multifunction = false;
            for func in 0..8u8 {
                let hdr = config_read_u8(bus, dev, func, 0x0E);
                if func == 0 {
                    multifunction = hdr & 0x80 != 0;
                } else if !multifunction {
                    break;
                }
                if config_read_u16(bus, dev, func, 0x00) == 0xFFFF {
                    break;
                }
                if list.len() >= MAX_DEVICES {
                    return;
                }
                let htype = hdr & 0x7F;
                let mut secondary = 0u8;
                if htype == 0x01 {
                    secondary = config_read_u8(bus, dev, func, 0x19);
                    if !buses.contains(&(secondary as u16)) {
                        buses.push(secondary as u16);
                    }
                }
                list.push(PciDevice {
                    bus,
                    dev,
                    func,
                    vendor: config_read_u16(bus, dev, func, 0x00),
                    device: config_read_u16(bus, dev, func, 0x02),
                    class: config_read_u8(bus, dev, func, 0x0B),
                    subclass: config_read_u8(bus, dev, func, 0x0A),
                    progif: config_read_u8(bus, dev, func, 0x09),
                    header_type: htype,
                    secondary_bus: secondary,
                    irq_line: config_read_u8(bus, dev, func, 0x3C),
                });
            }
        }
    }
}

pub fn device_count() -> usize {
    DEVICES.lock().len()
}

pub fn device(i: usize) -> Option<PciDevice> {
    DEVICES.lock().get(i).copied()
}

pub fn class_name(d: &PciDevice) -> &'static str {
    match d.class {
        0x00 => "legacy",
        0x01 => match d.subclass {
            0x00 => "storage/SCSI",
            0x01 => "storage/IDE",
            0x02 => "storage/Floppy",
            0x03 => "storage/IPI",
            0x04 => "storage/RAID",
            0x05 => "storage/ATA",
            0x06 => "storage/SATA",
            0x07 => "storage/SAS",
            0x08 => "storage/NVMe",
            _ => "storage",
        },
        0x02 => "network",
        0x03 => match d.subclass {
            0x00 => "display/VGA",
            0x01 => "display/XGA",
            0x02 => "display/3D",
            _ => "display",
        },
        0x04 => match d.subclass {
            0x00 => "multimedia/Video",
            0x01 => "multimedia/Audio",
            0x03 => "multimedia/HDA",
            _ => "multimedia",
        },
        0x05 => "memory",
        0x06 => match d.subclass {
            0x00 => "bridge/Host",
            0x01 => "bridge/ISA",
            0x02 => "bridge/EISA",
            0x03 => "bridge/MCA",
            0x04 => "bridge/PCI-PCI",
            0x05 => "bridge/CardBus",
            0x06 => "bridge/RACEWAY",
            0x0A => "bridge/PCI-PCI sub",
            _ => "bridge",
        },
        0x07 => "comm",
        0x08 => match d.subclass {
            0x00 => "system/PIC",
            0x01 => "system/DMA",
            0x03 => "system/Timer",
            0x04 => "system/RTC",
            _ => "system",
        },
        0x09 => match d.subclass {
            0x00 => "input/Keyboard",
            0x01 => "input/Pen",
            0x02 => "input/Mouse",
            _ => "input",
        },
        0x0A => "docking",
        0x0B => "processor",
        0x0C => match d.subclass {
            0x00 => "serialbus/FireWire",
            0x03 => "serialbus/USB",
            0x05 => "serialbus/SMBus",
            0x06 => "serialbus/InfiniBand",
            _ => "serialbus",
        },
        0x0D => "wireless",
        0x0E => "intelligent-io",
        0x0F => "satellite",
        0x10 => "encryption",
        0x11 => "signal-proc",
        0x12 => "accel",
        0x13 => "nonessential",
        _ => "unknown",
    }
}

pub fn device_name(d: &PciDevice) -> &'static str {
    match (d.vendor, d.device) {
        (0x8086, 0x1237) => "Intel 82371AB/PIIX3 host bridge",
        (0x8086, 0x7000) => "Intel 82371SB PIIX3 ISA bridge",
        (0x8086, 0x7010) => "Intel 82371SB PIIX3 IDE",
        (0x8086, 0x7111) => "Intel PIIX4 IDE",
        (0x8086, 0x7113) => "Intel PIIX4 ACPI",
        (0x8086, 0x2415) => "Intel 82801AA AC97 audio",
        (0x8086, 0x7020) => "Intel UHCI USB controller",
        (0x8086, 0x100E) => "Intel PRO/1000 (e1000)",
        (0x8086, 0x10D3) => "Intel 82574L (e1000e)",
        (0x1234, 0x1111) => "QEMU standard VGA",
        (0x1AF4, _) => "VirtIO device",
        (0x10EC, 0x8029) => "Realtek RTL8029 (ne2k)",
        (0x1013, 0x00B8) => "Cirrus CLGD5446 VGA",
        (0x1022, _) => "AMD device",
        _ => "",
    }
}

pub fn print_devices() {
    let list = DEVICES.lock();
    if list.is_empty() {
        println!("PCI: no devices found.");
        return;
    }
    println!("PCI devices ({}):", list.len());
    for d in list.iter() {
        let name = device_name(d);
        if name.is_empty() {
            println!(
                "  {:02x}:{:02x}.{}  {:04x}:{:04x}  {}",
                d.bus, d.dev, d.func, d.vendor, d.device, class_name(d)
            );
        } else {
            println!(
                "  {:02x}:{:02x}.{}  {:04x}:{:04x}  {:<16}  {}",
                d.bus,
                d.dev,
                d.func,
                d.vendor,
                d.device,
                class_name(d),
                name
            );
        }
    }
}
