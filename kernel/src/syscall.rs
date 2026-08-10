use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::structures::gdt::SegmentSelector;
use x86_64::PrivilegeLevel;
use x86_64::VirtAddr;

/// Kernel stack top used by the syscall entry (mirrors TSS.rsp0 for the
/// current task). Maintained by the scheduler.
#[unsafe(no_mangle)]
pub static mut SYS_KSTACK_TOP: u64 = 0;

/// User RSP saved on syscall entry, restored right before sysret. Written
/// by the `syscall_entry` assembly, so it must live in writable memory
/// (`static mut`; a plain `static` would be placed in .rodata).
#[unsafe(no_mangle)]
pub static mut SYS_USER_RSP: u64 = 0;

pub fn set_kstack_top(top: u64) {
    unsafe {
        SYS_KSTACK_TOP = top;
    }
}

/// Arms SYSCALL/SYSRET: EFER.SCE, STAR (segment pair), LSTAR (entry point),
/// SFMASK (clear IF and DF on entry). GS bases are left at 0 so the swapgs
/// on entry/exit is symmetric until per-CPU state exists.
pub fn init() {
    unsafe {
        let efer = Efer::read();
        Efer::write(efer | EferFlags::SYSTEM_CALL_EXTENSIONS);

        let cs_syscall = SegmentSelector::new(1, PrivilegeLevel::Ring0);
        let ss_syscall = SegmentSelector::new(2, PrivilegeLevel::Ring0);
        let cs_sysret = SegmentSelector::new(4, PrivilegeLevel::Ring3);
        let ss_sysret = SegmentSelector::new(3, PrivilegeLevel::Ring3);
        Star::write(cs_sysret, ss_sysret, cs_syscall, ss_syscall)
            .expect("invalid STAR segment selectors");

        LStar::write(VirtAddr::new(syscall_entry as *const () as usize as u64));

        SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG);
    }
}

/// Register save area pushed by `syscall_entry`, top-of-stack first.
#[repr(C)]
struct SyscallFrame {
    r9: u64,
    r8: u64,
    r10: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rcx: u64,
    rax: u64,
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    r11: u64,
}

static PING_COUNT: AtomicU64 = AtomicU64::new(0);

/// Syscall dispatcher. `rax` is the syscall number; the return value is
/// placed back into `rax` for the user. Returns 0 to continue in user mode
/// or 1 to terminate the calling task.
#[unsafe(no_mangle)]
extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) -> u8 {
    let f = unsafe { &mut *frame };
    match f.rax {
        1 => {
            let n = PING_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            f.rax = n;
            crate::serial::write_fmt(format_args!("[user] ping {}\n", n));
            0
        }
        2 => {
            crate::serial::write_fmt(format_args!("[user] bye (task exiting)\n"));
            1
        }
        _ => {
            crate::serial::write_fmt(format_args!("[user] unknown syscall {}\n", f.rax));
            f.rax = u64::MAX;
            0
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn syscall_terminate() -> ! {
    crate::sched::terminate_current()
}

core::arch::global_asm!(
    ".global syscall_entry",
    "syscall_entry:",
    "swapgs",
    "mov qword ptr [rip + SYS_USER_RSP], rsp",
    "mov rsp, qword ptr [rip + SYS_KSTACK_TOP]",
    "sub rsp, 8",
    "push r11",
    "push r15",
    "push r14",
    "push r13",
    "push r12",
    "push rbp",
    "push rbx",
    "push rax",
    "push rcx",
    "push rdi",
    "push rsi",
    "push rdx",
    "push r10",
    "push r8",
    "push r9",
    "mov rdi, rsp",
    "call syscall_dispatch",
    "test al, al",
    "jnz 1f",
    "pop r9",
    "pop r8",
    "pop r10",
    "pop rdx",
    "pop rsi",
    "pop rdi",
    "pop rcx",
    "pop rax",
    "pop rbx",
    "pop rbp",
    "pop r12",
    "pop r13",
    "pop r14",
    "pop r15",
    "pop r11",
    "add rsp, 8",
    "mov rsp, qword ptr [rip + SYS_USER_RSP]",
    "swapgs",
    "sysretq",
    "1:",
    "call syscall_terminate",
    "ud2"
);

unsafe extern "C" {
    fn syscall_entry();
}
