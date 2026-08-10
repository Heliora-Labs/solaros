use core::fmt;
use core::fmt::Write;

use spin::Mutex;
use uart_16550::backend::PioBackend;
use uart_16550::spec::registers::IER;
use uart_16550::Config;
use uart_16550::Uart16550Tty;

use x86_64::instructions::interrupts::without_interrupts;

static SERIAL1: Mutex<Option<Uart16550Tty<PioBackend>>> = Mutex::new(None);

pub fn init() {
    let mut serial = SERIAL1.lock();
    let mut cfg = Config {
        interrupts: IER::empty(),
        ..Config::default()
    };
    cfg.fifo_trigger_level = Some(uart_16550::spec::registers::FifoTriggerLevel::Fourteen);
    *serial = unsafe { Uart16550Tty::new_port(0x3F8, cfg).ok() };
}

#[doc(hidden)]
pub fn write_fmt(args: fmt::Arguments) {
    without_interrupts(|| {
        let mut serial = SERIAL1.lock();
        if let Some(s) = serial.as_mut() {
            let _ = s.write_fmt(args);
        }
    });
}

/// Enables the "received data available" interrupt (IRQ4): FIFO with a 1-byte
/// trigger so each incoming byte raises an interrupt immediately. The console
/// input path drains the receiver from the IRQ handler into a software ring.
pub fn enable_rx_irq() {
    without_interrupts(|| {
        let mut fcr = unsafe { x86_64::instructions::port::Port::<u8>::new(0x3FA) };
        let mut ier = unsafe { x86_64::instructions::port::Port::<u8>::new(0x3F9) };
        unsafe {
            fcr.write(0x01);
            ier.write(0x01);
        }
    });
}

/// Peeks the COM1 line status register and returns one byte if the receiver
/// holds data. Polled by the console input path; no UART IRQ is enabled.
pub fn try_read_byte() -> Option<u8> {
    without_interrupts(|| {
        let mut lsr = unsafe { x86_64::instructions::port::Port::<u8>::new(0x3FD) };
        if unsafe { lsr.read() } & 0x01 == 0 {
            return None;
        }
        let mut dr = unsafe { x86_64::instructions::port::Port::<u8>::new(0x3F8) };
        Some(unsafe { dr.read() })
    })
}