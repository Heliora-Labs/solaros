use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use pc_keyboard::DecodedKey;

/// Unified console input: decoded characters from the serial port (COM1) and
/// the PS/2 keyboard. `read_char` blocks until a character is available; while
/// waiting it enables interrupts and halts, so the scheduler keeps running.
///
/// Serial bytes are pushed into a lock-free ring by the COM1 IRQ handler
/// (`serial_irq`) as they arrive; `read_char` also drains the receiver
/// directly as a fallback (e.g. bytes that arrived before the IRQ was armed).
/// The keyboard path reuses the raw scancode ring from `keyboard` and decodes
/// lazily. The console has a single reader at a time.
pub fn read_char() -> char {
    loop {
        if let Some(c) = poll_serial_char() {
            return c;
        }
        drain_receiver();
        if let Some(c) = poll_serial_char() {
            return c;
        }
        if let Some(c) = poll_keyboard_char() {
            return c;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

/// Serial input is ASCII-only: CR/LF map to newline, backspace and Ctrl-C pass
/// through as control characters, printable ASCII maps directly. Bytes outside
/// this range are dropped.
fn poll_serial_char() -> Option<char> {
    let b = serial_pop()?;
    match b {
        0x0D | 0x0A => Some('\n'),
        0x08 => Some('\u{0008}'),
        0x03 => Some('\u{0003}'),
        0x20..=0x7E => Some(b as char),
        _ => None,
    }
}

fn poll_keyboard_char() -> Option<char> {
    match crate::keyboard::decode_one()? {
        DecodedKey::Unicode(c) => Some(c),
        DecodedKey::RawKey(_) => None,
    }
}

// --- Lock-free serial ring (same pattern as the keyboard raw ring) ---------

const SERIAL_CAP: usize = 256;

static SBUF: [AtomicU8; SERIAL_CAP] = [const { AtomicU8::new(0) }; SERIAL_CAP];
static SW: AtomicUsize = AtomicUsize::new(0);
static SR: AtomicUsize = AtomicUsize::new(0);

/// Called from the COM1 IRQ handler: drains the receiver into the ring.
pub fn serial_irq() {
    while let Some(b) = crate::serial::try_read_byte() {
        let w = SW.load(Ordering::SeqCst);
        let r = SR.load(Ordering::SeqCst);
        let nw = (w + 1) % SERIAL_CAP;
        if nw == r {
            return;
        }
        SBUF[w].store(b, Ordering::SeqCst);
        SW.store(nw, Ordering::SeqCst);
    }
}

fn drain_receiver() {
    while let Some(b) = crate::serial::try_read_byte() {
        let w = SW.load(Ordering::SeqCst);
        let r = SR.load(Ordering::SeqCst);
        let nw = (w + 1) % SERIAL_CAP;
        if nw == r {
            return;
        }
        SBUF[w].store(b, Ordering::SeqCst);
        SW.store(nw, Ordering::SeqCst);
    }
}

fn serial_pop() -> Option<u8> {
    let r = SR.load(Ordering::SeqCst);
    if r == SW.load(Ordering::SeqCst) {
        return None;
    }
    let b = SBUF[r].load(Ordering::SeqCst);
    SR.store((r + 1) % SERIAL_CAP, Ordering::SeqCst);
    Some(b)
}
