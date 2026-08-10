#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

/// Raw syscall trampoline. The kernel entry preserves rdi/rsi/rdx; rax
/// carries the syscall number in and the return value out. rcx/r11 are
/// clobbered by the `syscall` instruction itself.
fn syscall(n: u64, a: u64, b: u64, c: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

fn write(s: &[u8]) {
    syscall(3, 1, s.as_ptr() as u64, s.len() as u64);
}

fn exit(code: u64) -> ! {
    syscall(2, code, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

fn delay() {
    let mut x: u64 = 0;
    loop {
        unsafe {
            if core::ptr::read_volatile(&x) >= 0x100_0000 {
                break;
            }
            core::ptr::write_volatile(&mut x, x + 1);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    write(b"[user] hello from ELF (write syscall)\n");
    let mut i: u64 = 0;
    while i < 5 {
        write(b"[user] loop iteration via write\n");
        delay();
        i += 1;
    }
    write(b"[user] ELF program done, exiting\n");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(255);
}
