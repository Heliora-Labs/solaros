use core::hint::spin_loop;

use pc_keyboard::{DecodedKey, KeyCode};

use crate::commands;
use crate::framebuffer;
use crate::println;

const PROMPT: &str = "solaros> ";
const LINE_CAP: usize = 256;
pub const READ_CAP: usize = 64;

pub struct Terminal {
    line: [char; LINE_CAP],
    len: usize,
}

impl Terminal {
    const fn new() -> Self {
        Terminal {
            line: ['\0'; LINE_CAP],
            len: 0,
        }
    }

    fn push(&mut self, c: char) {
        if self.len < LINE_CAP {
            self.line[self.len] = c;
            self.len += 1;
            crate::print!("{}", c);
        }
    }

    fn backspace(&mut self) {
        if self.len == 0 {
            return;
        }
        self.len -= 1;
        framebuffer::backspace();
    }

    fn cancel(&mut self) {
        if self.len > 0 {
            while self.len > 0 {
                self.backspace();
            }
            println!();
        } else {
            println!();
        }
    }

    fn execute(&mut self) {
        println!();
        if self.len > 0 {
            commands::execute(&self.line[..self.len]);
        }
        self.len = 0;
    }

    fn handle_char(&mut self, c: char) -> bool {
        match c {
            '\n' | '\r' => {
                self.execute();
                true
            }
            '\u{0008}' => {
                self.backspace();
                false
            }
            '\u{0009}' | '\u{001B}' | '\u{007F}' => false,
            '\u{0003}' => {
                self.cancel();
                false
            }
            '\u{0000}'..='\u{001F}' => false,
            c => {
                self.push(c);
                false
            }
        }
    }

    fn handle_raw(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::PageUp => framebuffer::tty_page_up(),
            KeyCode::PageDown => framebuffer::tty_page_down(),
            KeyCode::Home => framebuffer::tty_home(),
            KeyCode::End => framebuffer::tty_end(),
            _ => {}
        }
        false
    }
}

fn print_prompt() {
    framebuffer::set_fg(framebuffer::ACCENT);
    crate::print!("{}", PROMPT);
    framebuffer::reset_colors();
}

/// Reads a line of input from the keyboard. If `echo` is false, characters are
/// masked with '*'. Returns Some((line, len)) on Enter, None on Ctrl-C.
pub fn read_line(prompt: &str, echo: bool) -> Option<([char; READ_CAP], usize)> {
    crate::print!("{}", prompt);
    let mut line = ['\0'; READ_CAP];
    let mut len = 0usize;
    loop {
        let mut done = false;
        let mut result: Option<bool> = None;
        crate::keyboard::drain(|key| {
            if done {
                return;
            }
            match key {
                DecodedKey::Unicode('\n') | DecodedKey::Unicode('\r') => {
                    done = true;
                    result = Some(true);
                }
                DecodedKey::Unicode('\u{0003}') => {
                    done = true;
                    result = Some(false);
                }
                DecodedKey::Unicode('\u{0008}') => {
                    if len > 0 {
                        len -= 1;
                        framebuffer::backspace();
                    }
                }
                DecodedKey::Unicode(c) if (c as u32) >= 0x20 => {
                    if len < READ_CAP {
                        line[len] = c;
                        len += 1;
                        crate::print!("{}", if echo { c } else { '*' });
                    }
                }
                _ => {}
            }
        });
        if let Some(enter) = result {
            crate::println!();
            return if enter { Some((line, len)) } else { None };
        }
        spin_loop();
    }
}

pub fn run() -> ! {
    let mut terminal = Terminal::new();
    let _has_tsc = crate::interrupts::calibrate_smoke();
    let mut last_ticks = crate::interrupts::ticks();
    let mut last_tsc = crate::interrupts::rdtsc();
    print_prompt();
    loop {
        let mut done = false;
        crate::keyboard::drain(|key| {
            if done {
                return;
            }
            match key {
                DecodedKey::Unicode(c) => {
                    if terminal.handle_char(c) {
                        done = true;
                    }
                }
                DecodedKey::RawKey(k) => {
                    if terminal.handle_raw(k) {
                        done = true;
                    }
                }
            }
        });
        if done {
            print_prompt();
        }
        let now_ticks = crate::interrupts::ticks();
        let now_tsc = crate::interrupts::rdtsc();
        if now_ticks != last_ticks {
            last_ticks = now_ticks;
            last_tsc = now_tsc;
        } else if now_tsc.wrapping_sub(last_tsc) > 3_000_000_000 {
            crate::interrupts::rearm();
            last_tsc = crate::interrupts::rdtsc();
        }
        spin_loop();
    }
}
