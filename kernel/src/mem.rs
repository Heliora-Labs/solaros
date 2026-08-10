use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page_table::{PageTable, PageTableIndex};
use x86_64::structures::paging::PageTableFlags;

use crate::acpi::PMO;

const PAGE: u64 = 0x1000;

fn phys_to_virt(paddr: u64) -> u64 {
    PMO + paddr
}

/// Marks the page table chain so that ring 3 can access the 4 KiB page
/// containing `vaddr`. Every level of the walk must carry the user bit, so
/// the PML4E (which spans the entire 512 GiB PMO window) becomes
/// user-accessible too — a known limitation of this stage: physical memory
/// inside the window is visible to user mode until 2d introduces
/// per-process page tables.
fn mark_one_page(vaddr: u64) -> bool {
    let (root, _) = Cr3::read();
    let root_va = phys_to_virt(root.start_address().as_u64());

    let pml4_idx = ((vaddr >> 39) & 0x1FF) as usize;
    let table = unsafe { &mut *(root_va as *mut PageTable) };
    let pml4e = &mut table[PageTableIndex::new(pml4_idx as u16)];
    if !pml4e.flags().contains(PageTableFlags::PRESENT) {
        crate::serial::write_fmt(format_args!("[mem] mark fail: PML4E[{}] not present @ {:#x}\n", pml4_idx, vaddr));
        return false;
    }
    pml4e.set_flags((pml4e.flags() & !PageTableFlags::NO_EXECUTE) | PageTableFlags::USER_ACCESSIBLE);

    let pdpt_idx = ((vaddr >> 30) & 0x1FF) as usize;
    let pdpt_va = phys_to_virt(pml4e.addr().as_u64());
    let table = unsafe { &mut *(pdpt_va as *mut PageTable) };
    let pdpte = &mut table[PageTableIndex::new(pdpt_idx as u16)];
    if !pdpte.flags().contains(PageTableFlags::PRESENT) {
        crate::serial::write_fmt(format_args!("[mem] mark fail: PDPTE[{}] not present @ {:#x}\n", pdpt_idx, vaddr));
        return false;
    }
    pdpte.set_flags((pdpte.flags() & !PageTableFlags::NO_EXECUTE) | PageTableFlags::USER_ACCESSIBLE);
    if pdpte.flags().contains(PageTableFlags::HUGE_PAGE) {
        return true;
    }

    let pd_idx = ((vaddr >> 21) & 0x1FF) as usize;
    let pd_va = phys_to_virt(pdpte.addr().as_u64());
    let table = unsafe { &mut *(pd_va as *mut PageTable) };
    let pde = &mut table[PageTableIndex::new(pd_idx as u16)];
    if !pde.flags().contains(PageTableFlags::PRESENT) {
        crate::serial::write_fmt(format_args!("[mem] mark fail: PDE[{}] not present @ {:#x}\n", pd_idx, vaddr));
        return false;
    }
    pde.set_flags((pde.flags() & !PageTableFlags::NO_EXECUTE) | PageTableFlags::USER_ACCESSIBLE);
    if pde.flags().contains(PageTableFlags::HUGE_PAGE) {
        return true;
    }

    let pt_idx = ((vaddr >> 12) & 0x1FF) as usize;
    let pt_va = phys_to_virt(pde.addr().as_u64());
    let table = unsafe { &mut *(pt_va as *mut PageTable) };
    let pte = &mut table[PageTableIndex::new(pt_idx as u16)];
    if !pte.flags().contains(PageTableFlags::PRESENT) {
        crate::serial::write_fmt(format_args!("[mem] mark fail: PTE[{}] not present @ {:#x}\n", pt_idx, vaddr));
        return false;
    }
    pte.set_flags((pte.flags() & !PageTableFlags::NO_EXECUTE) | PageTableFlags::USER_ACCESSIBLE);
    true
}

/// Marks the (4 KiB aligned) range `[vaddr, vaddr + len)` as user-accessible
/// so ring 3 code can execute from and write to it. Flushes the TLB so stale
/// supervisor-only translations cached by earlier kernel accesses are dropped.
pub fn mark_user_pages(vaddr: u64, len: u64) -> bool {
    let start = vaddr & !(PAGE - 1);
    let end = ((vaddr + len) + PAGE - 1) & !(PAGE - 1);
    let mut ok = true;
    let mut page = start;
    while page < end {
        if !mark_one_page(page) {
            ok = false;
        }
        page += PAGE;
    }
    x86_64::instructions::tlb::flush_all();
    ok
}
