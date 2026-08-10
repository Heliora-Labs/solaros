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

// Syscall numbers: 2=exit, 3=write, 4=read, 7=console, 8=exec.

fn write(s: &[u8]) {
    syscall(3, 1, s.as_ptr() as u64, s.len() as u64);
}

fn exit(code: u64) -> ! {
    syscall(2, code, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Blocks until one decoded character is available and returns it.
fn read_char() -> char {
    let buf = [0u8; 64];
    loop {
        let n = syscall(4, 0, buf.as_ptr() as u64, buf.len() as u64);
        if n == 0 {
            continue;
        }
        if let Ok(s) = core::str::from_utf8(&buf[..n as usize]) {
            if let Some(c) = s.chars().next() {
                return c;
            }
        }
    }
}

fn console(op: u64, a: u64) {
    syscall(7, op, a, 0);
}

fn exec(line: &[u8]) {
    syscall(8, line.as_ptr() as u64, line.len() as u64, 0);
}

fn put_char(c: char) {
    let mut buf = [0u8; 4];
    write(c.encode_utf8(&mut buf).as_bytes());
}

fn put_chars(cs: &[char]) {
    for &c in cs {
        put_char(c);
    }
}

fn puts(s: &str) {
    write(s.as_bytes());
}

/// Prints a number followed by two spaces (for history/help lists).
fn write_ordinal(mut v: u64) {
    let mut tmp = [0u8; 24];
    let mut tlen = 0usize;
    loop {
        tmp[tlen] = b'0' + (v % 10) as u8;
        tlen += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let mut buf = [0u8; 26];
    let mut len = 0usize;
    while tlen > 0 {
        tlen -= 1;
        buf[len] = tmp[tlen];
        len += 1;
    }
    buf[len] = b' ';
    buf[len + 1] = b' ';
    write(&buf[..len + 2]);
}

fn eq_ci_str(a: &[char], b: &str) -> bool {
    let mut it = b.chars();
    if a.len() != b.chars().count() {
        return false;
    }
    for &x in a {
        match it.next() {
            Some(y) => {
                if x.to_ascii_lowercase() != y.to_ascii_lowercase() {
                    return false;
                }
            }
            None => return false,
        }
    }
    it.next().is_none()
}

fn matches(name: &[char], cmd: &str) -> bool {
    eq_ci_str(name, cmd)
}

// ---------------------------------------------------------------------------
// History (fixed-size, no heap)
// ---------------------------------------------------------------------------

const HIST_CAP: usize = 50;
const HIST_LEN_CAP: usize = 64;

static mut HIST: [[char; HIST_LEN_CAP]; HIST_CAP] = [['\0'; HIST_LEN_CAP]; HIST_CAP];
static mut HLEN: [usize; HIST_CAP] = [0; HIST_CAP];
static mut HCOUNT: usize = 0;

fn history_push(line: &[char]) {
    unsafe {
        if HCOUNT > 0
            && HLEN[HCOUNT - 1] == line.len()
            && HIST[HCOUNT - 1][..line.len()] == line[..]
        {
            return;
        }
        if HCOUNT == HIST_CAP {
            for i in 0..HIST_CAP - 1 {
                HIST[i] = HIST[i + 1];
                HLEN[i] = HLEN[i + 1];
            }
            HCOUNT -= 1;
        }
        let n = line.len().min(HIST_LEN_CAP);
        HIST[HCOUNT][..n].copy_from_slice(&line[..n]);
        HLEN[HCOUNT] = n;
        HCOUNT += 1;
    }
}

// ---------------------------------------------------------------------------
// Argument parsing (ported from the kernel command parser)
// ---------------------------------------------------------------------------

const MAX_ARGS: usize = 8;
const MAX_ARG_LEN: usize = 64;
const LINE_CAP: usize = 128;

fn parse_args(
    chars: &[char],
    args: &mut [char; MAX_ARGS * MAX_ARG_LEN],
    lens: &mut [usize; MAX_ARGS],
) -> usize {
    let mut count = 0usize;
    let mut in_arg = false;
    for &c in chars {
        if c == ' ' || c == '\t' {
            if in_arg {
                if count < MAX_ARGS {
                    count += 1;
                }
                in_arg = false;
            }
            continue;
        }
        if !in_arg {
            if count >= MAX_ARGS {
                break;
            }
            in_arg = true;
        }
        let pos = count * MAX_ARG_LEN;
        if lens[count] < MAX_ARG_LEN {
            args[pos + lens[count]] = c;
            lens[count] += 1;
        }
    }
    if in_arg && count < MAX_ARGS {
        count += 1;
    }
    count
}

fn encode_line(line: &[char], out: &mut [u8; 512]) -> usize {
    let mut n = 0usize;
    for &c in line {
        if n + 4 > out.len() {
            break;
        }
        let mut buf = [0u8; 4];
        let enc = c.encode_utf8(&mut buf);
        if n + enc.len() > out.len() {
            break;
        }
        out[n..n + enc.len()].copy_from_slice(enc.as_bytes());
        n += enc.len();
    }
    n
}

fn arg<'a>(args: &'a [char; MAX_ARGS * MAX_ARG_LEN], lens: &[usize; MAX_ARGS], count: usize, i: usize) -> &'a [char] {
    if i < count {
        &args[i * MAX_ARG_LEN..i * MAX_ARG_LEN + lens[i]]
    } else {
        &[]
    }
}

// ---------------------------------------------------------------------------
// Builtin commands (executed entirely in ring 3)
// ---------------------------------------------------------------------------

fn cmd_help() {
    puts("SolarOS shell commands (user-space builtins):\n");
    puts("  help       - shows this list\n");
    puts("  clear      - clears the screen\n");
    puts("  color      - changes the text color: color <name>\n");
    puts("  count      - counts from 1 to n: count <n>\n");
    puts("  echo       - prints the given text: echo <text>\n");
    puts("  history    - lists recently executed commands\n");
    puts("  version    - shows the SolarOS version\n");
    puts("  help2      - hidden command\n");
    puts("Other commands (solarfetch, status, diskinfo, pci, ls, cat, mkfs,\n");
    puts("whoami, users, adduser, passwd, login, su, loadkeys, wtest, ...)\n");
    puts("run in the kernel service via the exec syscall.\n");
}

fn cmd_echo(rest: &[char]) {
    let mut trimmed = rest;
    while let Some(&c) = trimmed.first() {
        if c == ' ' || c == '\t' {
            trimmed = &trimmed[1..];
        } else {
            break;
        }
    }
    put_chars(trimmed);
    put_char('\n');
}

fn cmd_count(arg: &[char]) {
    if arg.is_empty() {
        puts("Usage: count <n>\n");
        return;
    }
    let mut n: u64 = 0;
    for &c in arg {
        if !c.is_ascii_digit() {
            puts("Error: enter a number.\n");
            return;
        }
        n = n.saturating_mul(10).saturating_add((c as u64) - ('0' as u64));
    }
    if n == 0 || n > 100 {
        puts("Counting to that is too long! (try 1-100)\n");
        return;
    }
    for i in 1..=n {
        write_ordinal(i);
        put_char('\n');
    }
}

fn cmd_version() {
    puts("SolarOS 26.1\n");
}

fn cmd_color(arg: &[char]) {
    let n = arg.len().min(16);
    let mut s = ['\0'; 16];
    s[..n].copy_from_slice(&arg[..n]);
    let color: Option<(u64, u64, u64)> = if eq_ci_str(&s[..n], "red") {
        Some((255, 90, 90))
    } else if eq_ci_str(&s[..n], "green") {
        Some((120, 255, 140))
    } else if eq_ci_str(&s[..n], "blue") {
        Some((90, 160, 255))
    } else if eq_ci_str(&s[..n], "yellow") {
        Some((255, 210, 110))
    } else if eq_ci_str(&s[..n], "purple") {
        Some((200, 120, 255))
    } else if eq_ci_str(&s[..n], "cyan") {
        Some((90, 255, 220))
    } else if eq_ci_str(&s[..n], "orange") {
        Some((255, 150, 60))
    } else if eq_ci_str(&s[..n], "pink") {
        Some((255, 120, 180))
    } else if eq_ci_str(&s[..n], "white") {
        Some((255, 255, 255))
    } else if eq_ci_str(&s[..n], "default") || eq_ci_str(&s[..n], "reset") {
        None
    } else {
        puts("Unknown color. (red, green, blue, yellow, purple, cyan, orange, pink, white, default)\n");
        return;
    };
    match color {
        Some((r, g, b)) => {
            console(1, (r << 16) | (g << 8) | b);
            puts("Text color changed.\n");
        }
        None => {
            console(2, 0);
            puts("Back to default colors.\n");
        }
    }
}

fn cmd_history() {
    unsafe {
        if HCOUNT == 0 {
            puts("History is empty.\n");
            return;
        }
        for i in 0..HCOUNT {
            write_ordinal((i + 1) as u64);
            put_chars(&HIST[i][..HLEN[i]]);
            put_char('\n');
        }
    }
}

fn execute(line: &[char], len: usize) {
    if len == 0 {
        return;
    }
    history_push(&line[..len]);

    let mut args = ['\0'; MAX_ARGS * MAX_ARG_LEN];
    let mut lens = [0usize; MAX_ARGS];
    let count = parse_args(&line[..len], &mut args, &mut lens);
    if count == 0 {
        return;
    }
    let name = &args[0..lens[0]];

    if matches(name, "help") {
        cmd_help();
    } else if matches(name, "clear") {
        console(0, 0);
    } else if matches(name, "echo") {
        cmd_echo(&line[lens[0]..len]);
    } else if matches(name, "version") {
        cmd_version();
    } else if matches(name, "count") {
        cmd_count(arg(&args, &lens, count, 1));
    } else if matches(name, "color") {
        cmd_color(arg(&args, &lens, count, 1));
    } else if matches(name, "history") {
        cmd_history();
    } else if matches(name, "help2") {
        puts("Hidden command found! More is coming here in the future.\n");
    } else {
        // Kernel service bridge: forward the whole line as-is.
        let mut buf = [0u8; 512];
        let n = encode_line(&line[..len], &mut buf);
        exec(&buf[..n]);
    }
}

fn print_prompt() {
    console(1, (0x5D << 16) | (0xDC << 8) | 0xFF);
    puts("solaros> ");
    console(2, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    console(1, (0x5D << 16) | (0xDC << 8) | 0xFF);
    puts("          S O L A R   O S   26.1\n");
    console(2, 0);
    puts("\nWelcome to SolarOS. Type 'help' for a list of commands.\n\n");

    let mut line = ['\0'; LINE_CAP];
    let mut len = 0usize;
    print_prompt();
    loop {
        let c = read_char();
        match c {
            '\n' | '\r' => {
                put_char('\n');
                execute(&line, len);
                len = 0;
                print_prompt();
            }
            '\u{0008}' => {
                if len > 0 {
                    len -= 1;
                    console(3, 0);
                }
            }
            '\u{0003}' => {
                while len > 0 {
                    len -= 1;
                    console(3, 0);
                }
                put_char('\n');
                print_prompt();
            }
            c if (c as u32) >= 0x20 => {
                if len < LINE_CAP {
                    line[len] = c;
                    len += 1;
                    put_char(c);
                }
            }
            _ => {}
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(255);
}
