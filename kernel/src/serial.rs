use core::fmt;
use core::fmt::Write;

use spin::Mutex;
use uart_16550::backend::PioBackend;
use uart_16550::spec::registers::IER;
use uart_16550::Config;
use uart_16550::Uart16550Tty;

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
    let mut serial = SERIAL1.lock();
    if let Some(s) = serial.as_mut() {
        let _ = s.write_fmt(args);
    }
}