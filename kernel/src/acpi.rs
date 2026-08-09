use bootloader_api::BootInfo;

use crate::boot;

pub const PMO: u64 = 0xFFFF_8000_0000_0000;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SdtHeader {
    pub sig: [u8; 4],
    pub len: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem: [u8; 6],
    pub oem_id: [u8; 8],
    pub creator: [u8; 4],
    pub creator_rev: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpiTable {
    pub sig: [u8; 4],
    pub addr: u64,
}

pub struct Acpi {
    pub revision: u8,
    pub xsdt: bool,
    pub tables: [AcpiTable; 32],
    pub table_count: usize,
}

pub fn ptr(phys_addr: u64) -> *const u8 {
    (phys_addr + PMO) as *const u8
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)) == 0
}

fn sdt_at(addr: u64) -> Option<SdtHeader> {
    if addr == 0 {
        return None;
    }
    let p = ptr(addr);
    let header = unsafe { core::ptr::read_unaligned(p as *const SdtHeader) };
    if header.len as usize > 64 * 1024 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(p, header.len as usize) };
    if !checksum_ok(bytes) {
        return None;
    }
    Some(header)
}

fn find_root(boot: &BootInfo) -> Option<(u64, [u8; 4], u8)> {
    for r in (*boot.memory_regions).iter() {
        let mut a = r.end.saturating_sub(0x1_0000);
        while a < r.end && a < r.end.saturating_sub(0x10000) + 0x10000 && a + 256 >= r.start {
            let p = ptr(a);
            let sig = unsafe { core::slice::from_raw_parts(p, 4) };
            if sig == b"XSDT" || sig == b"RSDT" {
                if let Some(h) = sdt_at(a) {
                    return Some((a, h.sig, h.revision));
                }
            }
            if a < r.start {
                break;
            }
            a = a.saturating_sub(16);
            if a + 256 < r.end.saturating_sub(0x1_0000) {
                break;
            }
        }
    }
    None
}

pub fn init(boot: &BootInfo) -> Option<Acpi> {
    let (root_addr, sig, _rev) = if let Some(rsdp) = boot.rsdp_addr.into_option() {
        let p = ptr(rsdp);
        let hdr = unsafe { core::slice::from_raw_parts(p, 36) };
        if hdr[0..8] != *b"RSD PTR " || !checksum_ok(&hdr[0..20]) {
            find_root(boot)?
        } else {
            let rev = hdr[15];
            let xsdt_addr = if rev >= 2 {
                unsafe { core::ptr::read_unaligned(p.add(24) as *const u64) }
            } else {
                unsafe { core::ptr::read_unaligned(p.add(16) as *const u32) as u64 }
            };
            let h = sdt_at(xsdt_addr)?;
            (xsdt_addr, h.sig, h.revision)
        }
    } else {
        find_root(boot)?
    };
    let xsdt = sig == *b"XSDT";
    boot::ok(format_args!(
        "ACPI: root {} at {:#x}",
        core::str::from_utf8(&sig).unwrap_or("????"),
        root_addr
    ));

    let h = sdt_at(root_addr)?;
    let entries = if xsdt {
        (h.len as usize - 36) / 8
    } else {
        (h.len as usize - 36) / 4
    };
    let mut tables = [AcpiTable {
        sig: [0; 4],
        addr: 0,
    }; 32];
    let mut count = 0usize;
    for i in 0..entries.min(32) {
        let e = if xsdt {
            unsafe { core::ptr::read_unaligned(ptr(root_addr).add(36 + i * 8) as *const u64) }
        } else {
            unsafe { core::ptr::read_unaligned(ptr(root_addr).add(36 + i * 4) as *const u32) as u64 }
        };
        if let Some(t) = sdt_at(e) {
            tables[count] = AcpiTable {
                sig: t.sig,
                addr: e,
            };
            count += 1;
        }
    }

    for t in tables.iter().take(count) {
        boot::ok(format_args!(
            "ACPI: {}",
            core::str::from_utf8(&t.sig).unwrap_or("????")
        ));
    }

    Some(Acpi {
        revision: if xsdt { 2 } else { 1 },
        xsdt,
        tables,
        table_count: count,
    })
}

pub fn find<'a>(acpi: &'a Acpi, sig: &[u8; 4]) -> Option<&'a AcpiTable> {
    acpi.tables
        .iter()
        .take(acpi.table_count)
        .find(|t| &t.sig == sig)
}