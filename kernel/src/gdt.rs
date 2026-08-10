use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::instructions::segmentation::Segment;
use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::PrivilegeLevel;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const KERNEL_CS: SegmentSelector = SegmentSelector::new(1, PrivilegeLevel::Ring0);
pub const KERNEL_DS: SegmentSelector = SegmentSelector::new(2, PrivilegeLevel::Ring0);
pub const USER_DS: SegmentSelector = SegmentSelector::new(3, PrivilegeLevel::Ring3);
pub const USER_CS: SegmentSelector = SegmentSelector::new(4, PrivilegeLevel::Ring3);
pub const TSS_SEL: SegmentSelector = SegmentSelector::new(5, PrivilegeLevel::Ring0);

const DF_STACK_BYTES: usize = 16 * 1024;
/// Written by the CPU via IST 0 on double faults: must stay in writable
/// memory (`static mut`; a plain `static` would be placed in .rodata).
static mut DF_STACK: [u8; DF_STACK_BYTES] = [0; DF_STACK_BYTES];

static BOOT_STACK_TOP: AtomicU64 = AtomicU64::new(0);

static mut TSS: TaskStateSegment = TaskStateSegment::new();

static GDT: spin::LazyLock<GlobalDescriptorTable> = spin::LazyLock::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    gdt.append(Descriptor::kernel_code_segment());
    gdt.append(Descriptor::kernel_data_segment());
    gdt.append(Descriptor::user_data_segment());
    gdt.append(Descriptor::user_code_segment());
    gdt.append(unsafe { Descriptor::tss_segment_unchecked(core::ptr::addr_of!(TSS)) });
    gdt
});

pub fn boot_stack_top() -> u64 {
    BOOT_STACK_TOP.load(Ordering::Relaxed)
}

/// Loads our own GDT (kernel + ring 3 segments, TSS with IST for double
/// faults) and reloads the segment registers. Kernel CS/DS keep the same
/// selector indices the bootloader used (0x8/0x10), so the switch is safe
/// even though CS is reloaded with a far return.
pub fn init() {
    let rsp = unsafe { capture_rsp() };
    BOOT_STACK_TOP.store(rsp, Ordering::Relaxed);
    unsafe {
        TSS.privilege_stack_table[0] = x86_64::VirtAddr::new(rsp);
        TSS.interrupt_stack_table[0] =
            x86_64::VirtAddr::new(core::ptr::addr_of!(DF_STACK) as u64 + DF_STACK_BYTES as u64);
    }
    GDT.load();
    unsafe {
        CS::set_reg(KERNEL_CS);
        DS::set_reg(KERNEL_DS);
        ES::set_reg(KERNEL_DS);
        FS::set_reg(KERNEL_DS);
        GS::set_reg(KERNEL_DS);
        SS::set_reg(KERNEL_DS);
        load_tss(TSS_SEL);
    }
}

/// Updates the TSS privilege stack 0 (kernel stack pointer used by the CPU
/// when an interrupt or exception arrives while the current task is in
/// ring 3).
pub fn set_rsp0(top: u64) {
    unsafe {
        TSS.privilege_stack_table[0] = x86_64::VirtAddr::new(top);
    }
}

/// Switches to ring 3: pushes an iretq frame on the current (kernel) stack
/// and returns into the user entry point with interrupts enabled. Never
/// returns.
///
/// The user data segments are NOT preloaded here: long mode ignores
/// DS/ES/FS/GS privilege checks, so the user program runs fine with the
/// kernel selectors until the next syscall (whose entry reloads DS/ES) or
/// sysret (which restores the user SS/CS from the frame).
pub fn enter_user_mode(entry: u64, user_stack_top: u64) -> ! {
    unsafe {
        // iretq frame: SS, RSP, RFLAGS, CS, RIP
        core::arch::asm!(
            "push {ss}",
            "push {rsp}",
            "push 0x202",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss = const USER_DS.0,
            rsp = in(reg) user_stack_top,
            cs = const USER_CS.0,
            rip = in(reg) entry,
        );
    }
    unreachable!()
}

core::arch::global_asm!(
    ".global capture_rsp",
    "capture_rsp:",
    "mov rax, rsp",
    "ret"
);

unsafe extern "C" {
    fn capture_rsp() -> u64;
}
