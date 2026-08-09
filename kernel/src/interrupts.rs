use core::sync::atomic::{AtomicU64, Ordering};

use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::instructions::hlt;
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn rdtsc() -> u64 {
    let lo: u64;
    let hi: u64;
    unsafe {
        core::arch::asm!("lfence", "rdtsc", out("eax") lo, out("edx") hi);
    }
    (hi << 32) | lo
}

pub fn calibrate_smoke() -> bool {
    let t0 = rdtsc();
    sleep_ticks(5);
    rdtsc().wrapping_sub(t0) > 0
}

fn sleep_ticks(n: u64) {
    let start = ticks();
    while ticks().wrapping_sub(start) < n {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

pub fn rearm() {
    crate::serial::write_fmt(format_args!("[WD] rearm\n"));
    unsafe {
        PICS.lock().initialize();
    }
    let mut cmd = Port::new(0x43);
    let mut data = Port::new(0x40);
    unsafe {
        cmd.write(0x34u8);
        data.write(0x00u8);
        data.write(0x00u8);
    }
    x86_64::instructions::interrupts::enable();
    TICKS.store(0, Ordering::Relaxed);
}

pub fn sleep_ms(ms: u64) {
    let target = ticks() + ms.div_ceil(55).max(1);
    while ticks() < target {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

macro_rules! register_ate_irq {
    ($($name:ident = $vec:literal),* $(,)?) => {
        $(
            #[allow(non_snake_case, dead_code)]
            extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) {
                unsafe {
                    PICS.lock().notify_end_of_interrupt($vec);
                }
            }
        )*
    };
}

// Unexpected IRQ handlers for PIC vectors (34..=47).
register_ate_irq!(
    ate_irq_34 = 34, ate_irq_35 = 35, ate_irq_36 = 36, ate_irq_37 = 37,
    ate_irq_38 = 38, ate_irq_39 = 39, ate_irq_40 = 40, ate_irq_41 = 41,
    ate_irq_42 = 42, ate_irq_43 = 43, ate_irq_44 = 44, ate_irq_45 = 45,
    ate_irq_46 = 46, ate_irq_47 = 47
);

static IDT: spin::LazyLock<InterruptDescriptorTable> = spin::LazyLock::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.general_protection_fault
        .set_handler_fn(general_protection_handler);
    idt.double_fault.set_handler_fn(double_fault_handler);
    idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
    idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
    for vec in 34u8..=47 {
        let handler: extern "x86-interrupt" fn(InterruptStackFrame) = match vec {
            34 => ate_irq_34,
            35 => ate_irq_35,
            36 => ate_irq_36,
            37 => ate_irq_37,
            38 => ate_irq_38,
            39 => ate_irq_39,
            40 => ate_irq_40,
            41 => ate_irq_41,
            42 => ate_irq_42,
            43 => ate_irq_43,
            44 => ate_irq_44,
            45 => ate_irq_45,
            46 => ate_irq_46,
            47 => ate_irq_47,
            _ => unreachable!(),
        };
        idt[vec].set_handler_fn(handler);
    }
    idt
});

pub fn init() {
    IDT.load();
    unsafe {
        PICS.lock().initialize();
    }
    x86_64::instructions::interrupts::enable();
}

extern "x86-interrupt" fn divide_error_handler(frame: InterruptStackFrame) {
    exception("DIVIDE ERROR", &frame)
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    exception("INVALID OPCODE", &frame)
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    exception("BREAKPOINT", &frame)
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, error: PageFaultErrorCode) {
    let addr = x86_64::registers::control::Cr2::read()
        .map(|a| a.as_u64())
        .unwrap_or(0);
    crate::println!(
        "  Faulting address: {:#x}  Error code: {:?}",
        addr,
        error
    );
    exception("PAGE FAULT", &frame)
}

extern "x86-interrupt" fn general_protection_handler(frame: InterruptStackFrame, _error: u64) {
    exception("GENERAL PROTECTION FAULT", &frame)
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _error: u64) -> ! {
    exception("DOUBLE FAULT", &frame)
}

extern "x86-interrupt" fn timer_interrupt_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_frame: InterruptStackFrame) {
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::keyboard::process_scancode(scancode);
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

fn exception(name: &str, frame: &InterruptStackFrame) -> ! {
    use crate::println;
    println!();
    println!("==================================================");
    println!("  !!! SOLAROS EXCEPTION !!!");
    println!("  {}", name);
    println!("  RIP: {:#x}", frame.instruction_pointer.as_u64());
    println!("  CS : {:#x}", frame.code_segment.0);
    println!("  RSP: {:#x}", frame.stack_pointer.as_u64());
    println!("  System halted.");
    println!("==================================================");
    loop {
        hlt();
    }
}
