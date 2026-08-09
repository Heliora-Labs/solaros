use pc_keyboard::{
    layouts::Us104Key, DecodedKey, HandleControl, KeyCode, KeyboardLayout, Modifiers, PS2Keyboard,
    PhysicalKeyboard, ScancodeSet1,
};
use spin::Mutex;

pub struct TurkishQ;

impl TurkishQ {
    const fn map2(plain: char, shifted: char, m: &Modifiers) -> DecodedKey {
        if m.is_shifted() {
            DecodedKey::Unicode(shifted)
        } else {
            DecodedKey::Unicode(plain)
        }
    }

    const fn letter(lower: char, upper: char, m: &Modifiers) -> DecodedKey {
        if m.is_caps() {
            DecodedKey::Unicode(upper)
        } else {
            DecodedKey::Unicode(lower)
        }
    }

    const fn tr_i(plain: char, m: &Modifiers) -> DecodedKey {
        if m.is_caps() {
            DecodedKey::Unicode('İ')
        } else {
            DecodedKey::Unicode(plain)
        }
    }
}

impl KeyboardLayout for TurkishQ {
    fn map_keycode(
        &self,
        keycode: KeyCode,
        modifiers: &Modifiers,
        _handle_ctrl: HandleControl,
    ) -> DecodedKey {
        match keycode {
            KeyCode::Escape => DecodedKey::Unicode('\u{001B}'),
            KeyCode::Key1 => Self::map2('1', '!', modifiers),
            KeyCode::Key2 => Self::map2('2', '\'', modifiers),
            KeyCode::Key3 => Self::map2('3', '^', modifiers),
            KeyCode::Key4 => Self::map2('4', '+', modifiers),
            KeyCode::Key5 => Self::map2('5', '%', modifiers),
            KeyCode::Key6 => Self::map2('6', '&', modifiers),
            KeyCode::Key7 => Self::map2('7', '/', modifiers),
            KeyCode::Key8 => Self::map2('8', '(', modifiers),
            KeyCode::Key9 => Self::map2('9', ')', modifiers),
            KeyCode::Key0 => Self::map2('0', '=', modifiers),
            KeyCode::OemMinus => Self::map2('*', '?', modifiers),
            KeyCode::OemPlus => Self::map2('-', '_', modifiers),
            KeyCode::Oem8 => Self::map2('"', 'é', modifiers),
            KeyCode::Backspace => DecodedKey::Unicode('\u{0008}'),
            KeyCode::Tab => DecodedKey::Unicode('\u{0009}'),
            KeyCode::Q => Self::letter('q', 'Q', modifiers),
            KeyCode::W => Self::letter('w', 'W', modifiers),
            KeyCode::E => Self::letter('e', 'E', modifiers),
            KeyCode::R => Self::letter('r', 'R', modifiers),
            KeyCode::T => Self::letter('t', 'T', modifiers),
            KeyCode::Y => Self::letter('y', 'Y', modifiers),
            KeyCode::U => Self::letter('u', 'U', modifiers),
            KeyCode::I => Self::tr_i('ı', modifiers),
            KeyCode::O => Self::letter('o', 'O', modifiers),
            KeyCode::P => Self::letter('p', 'P', modifiers),
            KeyCode::Oem4 => Self::map2('ğ', 'Ğ', modifiers),
            KeyCode::Oem6 => Self::map2('ü', 'Ü', modifiers),
            KeyCode::A => Self::letter('a', 'A', modifiers),
            KeyCode::S => Self::letter('s', 'S', modifiers),
            KeyCode::D => Self::letter('d', 'D', modifiers),
            KeyCode::F => Self::letter('f', 'F', modifiers),
            KeyCode::G => Self::letter('g', 'G', modifiers),
            KeyCode::H => Self::letter('h', 'H', modifiers),
            KeyCode::J => Self::letter('j', 'J', modifiers),
            KeyCode::K => Self::letter('k', 'K', modifiers),
            KeyCode::L => Self::letter('l', 'L', modifiers),
            KeyCode::Oem1 => Self::map2('ş', 'Ş', modifiers),
            KeyCode::Oem3 => Self::tr_i('i', modifiers),
            KeyCode::Return => DecodedKey::Unicode('\u{000A}'),
            KeyCode::Z => Self::letter('z', 'Z', modifiers),
            KeyCode::X => Self::letter('x', 'X', modifiers),
            KeyCode::C if modifiers.is_ctrl() => DecodedKey::Unicode('\u{0003}'),
            KeyCode::C => Self::letter('c', 'C', modifiers),
            KeyCode::V => Self::letter('v', 'V', modifiers),
            KeyCode::B => Self::letter('b', 'B', modifiers),
            KeyCode::N => Self::letter('n', 'N', modifiers),
            KeyCode::M => Self::letter('m', 'M', modifiers),
            KeyCode::OemComma => Self::map2('ö', 'Ö', modifiers),
            KeyCode::OemPeriod => Self::map2('ç', 'Ç', modifiers),
            KeyCode::Oem2 => Self::map2('.', ':', modifiers),
            KeyCode::Oem7 => Self::map2(',', ';', modifiers),
            KeyCode::Oem5 => Self::map2('<', '>', modifiers),
            KeyCode::Spacebar => DecodedKey::Unicode(' '),
            KeyCode::Delete => DecodedKey::Unicode('\u{007f}'),
            KeyCode::NumpadDivide => DecodedKey::Unicode('/'),
            KeyCode::NumpadMultiply => DecodedKey::Unicode('*'),
            KeyCode::NumpadSubtract => DecodedKey::Unicode('-'),
            KeyCode::NumpadAdd => DecodedKey::Unicode('+'),
            KeyCode::NumpadEnter => DecodedKey::Unicode('\u{000A}'),
            k => DecodedKey::RawKey(k),
        }
    }

    fn get_physical(&self) -> PhysicalKeyboard {
        PhysicalKeyboard::Iso
    }
}

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

const RAW_CAP: usize = 512;

static RAW_BUF: [AtomicU8; RAW_CAP] = [const { AtomicU8::new(0) }; RAW_CAP];
static RAW_W: AtomicUsize = AtomicUsize::new(0);
static RAW_R: AtomicUsize = AtomicUsize::new(0);

static ISR_PUSHED: AtomicU64 = AtomicU64::new(0);

fn push_byte(sc: u8) {
    let w = RAW_W.load(Ordering::SeqCst);
    let r = RAW_R.load(Ordering::SeqCst);
    let nw = (w + 1) % RAW_CAP;
    if nw == r {
        return;
    }
    RAW_BUF[w].store(sc, Ordering::SeqCst);
    RAW_W.store(nw, Ordering::SeqCst);
    ISR_PUSHED.fetch_add(1, Ordering::Relaxed);
}

fn pop_byte() -> Option<u8> {
    let r = RAW_R.load(Ordering::SeqCst);
    if r == RAW_W.load(Ordering::SeqCst) {
        return None;
    }
    let sc = RAW_BUF[r].load(Ordering::SeqCst);
    RAW_R.store((r + 1) % RAW_CAP, Ordering::SeqCst);
    Some(sc)
}

enum ActiveKeyboard {
    Tr(PS2Keyboard<TurkishQ, ScancodeSet1>),
    Us(PS2Keyboard<Us104Key, ScancodeSet1>),
}

static KEYBOARD: Mutex<ActiveKeyboard> = Mutex::new(ActiveKeyboard::Tr(PS2Keyboard::new(
    ScancodeSet1::new(),
    TurkishQ,
    HandleControl::Ignore,
)));

fn switch_layout_if_needed(kb: &mut ActiveKeyboard) {
    use crate::settings::KbdLayout;
    let want = crate::settings::layout();
    let wrong = match want {
        KbdLayout::TurkishQ => matches!(&*kb, ActiveKeyboard::Us(_)),
        KbdLayout::Us => matches!(&*kb, ActiveKeyboard::Tr(_)),
    };
    if wrong {
        *kb = match want {
            KbdLayout::TurkishQ => ActiveKeyboard::Tr(PS2Keyboard::new(
                ScancodeSet1::new(),
                TurkishQ,
                HandleControl::Ignore,
            )),
            KbdLayout::Us => ActiveKeyboard::Us(PS2Keyboard::new(
                ScancodeSet1::new(),
                Us104Key,
                HandleControl::MapLettersToUnicode,
            )),
        };
    }
}

pub fn process_scancode(scancode: u8) {
    push_byte(scancode);
}

pub fn drain<F: FnMut(DecodedKey)>(mut f: F) {
    loop {
        let decoded: Option<DecodedKey> = {
            let mut kb = KEYBOARD.lock();
            switch_layout_if_needed(&mut kb);
            let mut out: Option<DecodedKey> = None;
            if let Some(sc) = pop_byte() {
                let r = match &mut *kb {
                    ActiveKeyboard::Tr(k) => k.add_byte(sc).ok().flatten().and_then(|e| k.process_keyevent(e)),
                    ActiveKeyboard::Us(k) => k.add_byte(sc).ok().flatten().and_then(|e| k.process_keyevent(e)),
                };
                out = r;
            }
            out
        };
        match decoded {
            Some(key) => f(key),
            None => break,
        }
    }
}
