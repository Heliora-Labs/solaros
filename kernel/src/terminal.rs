use crate::framebuffer;

pub const READ_CAP: usize = 64;

/// Reads a line of input from the console (keyboard or serial). If `echo` is
/// false, characters are masked with '*'. Returns Some((line, len)) on Enter,
/// None on Ctrl-C. Used by kernel-side commands (passwd/login); the interactive
/// shell itself lives in user space.
pub fn read_line(prompt: &str, echo: bool) -> Option<([char; READ_CAP], usize)> {
    crate::print!("{}", prompt);
    let mut line = ['\0'; READ_CAP];
    let mut len = 0usize;
    loop {
        let c = crate::input::read_char();
        match c {
            '\n' | '\r' => {
                crate::println!();
                return Some((line, len));
            }
            '\u{0003}' => {
                crate::println!();
                return None;
            }
            '\u{0008}' => {
                if len > 0 {
                    len -= 1;
                    framebuffer::backspace();
                }
            }
            c if (c as u32) >= 0x20 => {
                if len < READ_CAP {
                    line[len] = c;
                    len += 1;
                    crate::print!("{}", if echo { c } else { '*' });
                }
            }
            _ => {}
        }
    }
}
