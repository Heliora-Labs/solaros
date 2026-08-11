use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};

use crate::heap::ALLOCATOR;

const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_NULL: u64 = 0;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const R_X86_64_RELATIVE: u32 = 8;

const PAGE: u64 = 0x1000;
const USER_STACK_BYTES: u64 = 64 * 1024;

pub struct LoadedUserProgram {
    pub entry: u64,
    pub stack_top: u64,
}

#[derive(Clone, Copy)]
struct Phdr {
    p_type: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
}

fn get16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn get32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn get64(b: &[u8], off: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(v)
}

/// Loads a PIE (ET_DYN) ELF64 image into kernel heap memory and maps every
/// image page plus a fresh user stack as ring-3 accessible. The image is
/// linked at a fixed high virtual address but is position-independent:
/// `base + vaddr` (with `base = alloc_addr - min_vaddr`) puts every segment
/// at its runtime address, and R_X86_64_RELATIVE relocations are applied
/// against that base. Returns the entry point and the user stack top.
pub fn load_user_elf(blob: &[u8]) -> Option<LoadedUserProgram> {
    if blob.len() < 64 || blob[0..4] != [0x7f, b'E', b'L', b'F'] {
        crate::serial::write_fmt(format_args!("[elf] bad ELF magic\n"));
        return None;
    }
    if get16(blob, 0x10) != ET_DYN {
        crate::serial::write_fmt(format_args!("[elf] not a PIE (ET_DYN) image\n"));
        return None;
    }
    let e_entry = get64(blob, 0x18);
    let e_phoff = get64(blob, 0x20) as usize;
    let e_phentsize = get16(blob, 0x36) as usize;
    let e_phnum = get16(blob, 0x38) as usize;

    let mut phdrs: Vec<Phdr> = Vec::new();
    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + 56 > blob.len() {
            return None;
        }
        phdrs.push(Phdr {
            p_type: get32(blob, off),
            p_offset: get64(blob, off + 8),
            p_vaddr: get64(blob, off + 0x10),
            p_filesz: get64(blob, off + 0x20),
            p_memsz: get64(blob, off + 0x28),
        });
    }

    let mut min_vaddr: u64 = u64::MAX;
    let mut max_end: u64 = 0;
    for ph in &phdrs {
        if ph.p_type != PT_LOAD {
            continue;
        }
        let start = ph.p_vaddr & !(PAGE - 1);
        let end = ph.p_vaddr.saturating_add(ph.p_memsz);
        if start < min_vaddr {
            min_vaddr = start;
        }
        if end > max_end {
            max_end = end;
        }
    }
    if min_vaddr == u64::MAX {
        crate::serial::write_fmt(format_args!("[elf] no loadable segments\n"));
        return None;
    }
    let image_size = ((max_end - min_vaddr) + PAGE - 1) & !(PAGE - 1);

    let block = unsafe {
        ALLOCATOR.alloc(Layout::from_size_align_unchecked(
            image_size as usize,
            PAGE as usize,
        ))
    } as u64;
    if block == 0 {
        crate::serial::write_fmt(format_args!("[elf] image alloc failed\n"));
        return None;
    }
    // The image is linked in the high half (e.g. 0xffff_8000_1000_0000), so
    // `min_vaddr` is huge and `block` is small: the base ends up in the PMO
    // window again, and every base-relative address must be computed with
    // wrapping arithmetic (debug builds panic on the overflow otherwise).
    let base = block.wrapping_sub(min_vaddr);

    for ph in &phdrs {
        if ph.p_type != PT_LOAD {
            continue;
        }
        let dst = base.wrapping_add(ph.p_vaddr) as usize;
        let src = ph.p_offset as usize;
        if src.checked_add(ph.p_filesz as usize).map_or(true, |s| s > blob.len()) {
            crate::serial::write_fmt(format_args!("[elf] segment exceeds image\n"));
            return None;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                blob.as_ptr().add(src),
                dst as *mut u8,
                ph.p_filesz as usize,
            );
            if ph.p_memsz > ph.p_filesz {
                core::ptr::write_bytes(
                    (dst as u64 + ph.p_filesz) as *mut u8,
                    0,
                    (ph.p_memsz - ph.p_filesz) as usize,
                );
            }
        }
    }

    for ph in &phdrs {
        if ph.p_type == PT_DYNAMIC {
            apply_relocations(base, ph);
        }
    }

    if !crate::mem::mark_user_pages(block, image_size) {
        crate::serial::write_fmt(format_args!("[elf] marking image pages failed\n"));
        return None;
    }

    let stack = unsafe {
        ALLOCATOR.alloc(Layout::from_size_align_unchecked(
            USER_STACK_BYTES as usize,
            PAGE as usize,
        ))
    } as u64;
    if stack == 0 {
        crate::serial::write_fmt(format_args!("[elf] user stack alloc failed\n"));
        return None;
    }
    crate::mem::mark_user_pages(stack, USER_STACK_BYTES);

    crate::serial::write_fmt(format_args!(
        "[elf] loaded PIE: {} bytes @ {:#x}, entry {:#x}, stack top {:#x}\n",
        image_size,
        block,
        base.wrapping_add(e_entry),
        stack + USER_STACK_BYTES
    ));
    Some(LoadedUserProgram {
        entry: base.wrapping_add(e_entry),
        stack_top: stack + USER_STACK_BYTES,
    })
}

/// Applies the dynamic relocations of a PIE image: R_X86_64_RELATIVE writes
/// `base + addend` at `base + r_offset`. Only that type can occur in a fully
/// static PIE (no external symbols). The dynamic table itself was copied into
/// memory by the load loop above.
fn apply_relocations(base: u64, dyn_ph: &Phdr) {
    let dyn_base = base.wrapping_add(dyn_ph.p_vaddr) as *const u64;
    let mut rela_addr: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut rela_ent: u64 = 24;
    for i in 0..dyn_ph.p_memsz / 16 {
        let tag = unsafe { dyn_base.add(i as usize * 2).read() };
        let val = unsafe { dyn_base.add(i as usize * 2 + 1).read() };
        if tag == DT_NULL {
            break;
        }
        match tag {
            DT_RELA => rela_addr = val,
            DT_RELASZ => rela_size = val,
            DT_RELAENT => rela_ent = val,
            _ => {}
        }
    }
    if rela_size == 0 || rela_ent < 24 {
        return;
    }

    let mut off: u64 = 0;
    let mut relocs: u64 = 0;
    while off + 24 <= rela_size {
        let r = base.wrapping_add(rela_addr).wrapping_add(off) as *const u8;
        let r_offset = unsafe { (r as *const u64).read() };
        let r_info = unsafe { (r.add(8) as *const u64).read() };
        let r_addend = unsafe { (r.add(16) as *const u64).read() };
        if (r_info & 0xffff_ffff) as u32 == R_X86_64_RELATIVE {
            unsafe {
                (base.wrapping_add(r_offset) as *mut u64).write(base.wrapping_add(r_addend));
            }
            relocs += 1;
        }
        off += rela_ent;
    }
    if relocs > 0 {
        crate::serial::write_fmt(format_args!("[elf] applied {} relative relocs\n", relocs));
    }
}
