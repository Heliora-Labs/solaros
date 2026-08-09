use crate::framebuffer;
use crate::framebuffer::Rgb;
use crate::interrupts;
use crate::print;
use crate::println;
use crate::settings;
use crate::settings::KbdLayout;
use crate::OS_NAME;
use crate::OS_VERSION;
use core::fmt;
use spin::Mutex;

const MAX_ARGS: usize = 8;
const MAX_ARG_LEN: usize = 64;

struct Command {
    name: &'static str,
    desc: &'static str,
}

const COMMANDS: &[Command] = &[
    Command {
        name: "help",
        desc: "shows the list of commands",
    },
    Command {
        name: "clear",
        desc: "clears the screen",
    },
    Command {
        name: "echo",
        desc: "prints the given text: echo <text>",
    },
    Command {
        name: "version",
        desc: "shows the SolarOS version",
    },
    Command {
        name: "solarfetch",
        desc: "shows system info in a fastfetch-style layout",
    },
    Command {
        name: "count",
        desc: "counts from 1 to n: count <n>",
    },
    Command {
        name: "color",
        desc: "changes the text color: color <color> (colors: red, green, blue, yellow, purple, cyan, orange, pink, white, default)",
    },
    Command {
        name: "status",
        desc: "shows the system status (uptime, framebuffer, settings)",
    },
    Command {
        name: "loadkeys",
        desc: "changes the keyboard layout like Linux: loadkeys <tr|trq|us|qwerty>",
    },
    Command {
        name: "help2",
        desc: "hidden command",
    },
    Command {
        name: "wtest",
        desc: "writes a pattern file and verifies it: wtest <path> <size> | verify",
    },
    Command {
        name: "diskinfo",
        desc: "shows the detected ATA disks",
    },
    Command {
        name: "ls",
        desc: "lists files: ls [path]",
    },
    Command {
        name: "cd",
        desc: "changes the directory: cd <path>",
    },
    Command {
        name: "pwd",
        desc: "prints the current directory",
    },
    Command {
        name: "mkdir",
        desc: "creates a directory: mkdir <path>",
    },
    Command {
        name: "touch",
        desc: "creates an empty file: touch <path>",
    },
    Command {
        name: "cat",
        desc: "prints a file: cat <path>",
    },
    Command {
        name: "rm",
        desc: "removes a file: rm <path>",
    },
    Command {
        name: "rmdir",
        desc: "removes an empty directory: rmdir <path>",
    },
    Command {
        name: "mv",
        desc: "moves/renames: mv <old> <new>",
    },
    Command {
        name: "mkfs",
        desc: "formats the data disk: mkfs [ext4] (default FAT32)",
    },
    Command {
        name: "whoami",
        desc: "prints the current user",
    },
    Command {
        name: "users",
        desc: "lists all users",
    },
    Command {
        name: "adduser",
        desc: "creates a user: adduser <name>",
    },
    Command {
        name: "passwd",
        desc: "sets a password: passwd [user]",
    },
    Command {
        name: "login",
        desc: "switches the current user: login <user>",
    },
    Command {
        name: "su",
        desc: "switches the current user: su <user>",
    },
];

fn parse_args(chars: &[char]) -> ([char; MAX_ARGS * MAX_ARG_LEN], [usize; MAX_ARGS], usize) {
    let mut args = ['\0'; MAX_ARGS * MAX_ARG_LEN];
    let mut lens = [0usize; MAX_ARGS];
    let mut count = 0usize;
    let mut in_arg = false;
    let mut pos = 0usize;

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
            pos = count * MAX_ARG_LEN;
        }
        if lens[count] < MAX_ARG_LEN {
            args[pos + lens[count]] = c;
            lens[count] += 1;
        }
    }
    if in_arg && count < MAX_ARGS {
        count += 1;
    }
    (args, lens, count)
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

pub fn execute(chars: &[char]) {
    let (args, lens, count) = parse_args(chars);
    if count == 0 {
        return;
    }
    let name = &args[0..lens[0]];

    let arg_str = |i: usize| -> &[char] {
        if i < count {
            &args[i * MAX_ARG_LEN..i * MAX_ARG_LEN + lens[i]]
        } else {
            &[]
        }
    };

    if matches(name, "help") {
        cmd_help();
    } else if matches(name, "clear") {
        framebuffer::clear();
    } else if matches(name, "echo") {
        cmd_echo(&chars[lens[0]..]);
    } else if matches(name, "version") {
        println!("SolarOS {}", OS_VERSION);
    } else if matches(name, "solarfetch") {
        cmd_solarfetch();
    } else if matches(name, "count") {
        cmd_count(arg_str(1));
    } else if matches(name, "color") {
        cmd_color(arg_str(1));
    } else if matches(name, "status") {
        cmd_status();
    } else if matches(name, "loadkeys") {
        cmd_loadkeys(arg_str(1));
    } else if matches(name, "help2") {
        println!("Hidden command found! More is coming here in the future.");
    } else if matches(name, "diskinfo") {
        cmd_diskinfo();
    } else if matches(name, "ls") {
        cmd_ls(arg_str(1));
    } else if matches(name, "cd") {
        cmd_cd(arg_str(1));
    } else if matches(name, "pwd") {
        cmd_pwd();
    } else if matches(name, "mkdir") {
        cmd_mkdir(arg_str(1));
    } else if matches(name, "touch") {
        cmd_touch(arg_str(1));
    } else if matches(name, "cat") {
        cmd_cat(arg_str(1));
    } else if matches(name, "rm") {
        cmd_rm(arg_str(1));
    } else if matches(name, "rmdir") {
        cmd_rmdir(arg_str(1));
    } else if matches(name, "mv") {
        cmd_mv(arg_str(1), arg_str(2));
    } else if matches(name, "wtest") {
        cmd_wtest(arg_str(1), arg_str(2));
    } else if matches(name, "mkfs") {
        cmd_mkfs(arg_str(1));
    } else if matches(name, "whoami") {
        cmd_whoami();
    } else if matches(name, "users") {
        cmd_users();
    } else if matches(name, "adduser") {
        cmd_adduser(arg_str(1));
    } else if matches(name, "passwd") {
        cmd_passwd(arg_str(1));
    } else if matches(name, "login") || matches(name, "su") {
        cmd_login(arg_str(1));
    } else {
        let mut s = ['\0'; 32];
        let n = lens[0].min(32);
        s[..n].copy_from_slice(&name[..n]);
        println!(
            "Unknown command: '{}'. Type 'help' for a list of commands.",
            crate::Utf8Chars(&s[..n])
        );
    }
}

fn cmd_help() {
    println!("SolarOS commands:");
    for c in COMMANDS {
        println!("  {:<10} - {}", c.name, c.desc);
    }
}

fn cmd_echo(chars: &[char]) {
    let mut trimmed = chars;
    while let Some(&c) = trimmed.first() {
        if c == ' ' || c == '\t' {
            trimmed = &trimmed[1..];
        } else {
            break;
        }
    }
    if trimmed.is_empty() {
        return;
    }
    let mut s = ['\0'; 256];
    let n = trimmed.len().min(256);
    s[..n].copy_from_slice(&trimmed[..n]);
    println!("{}", crate::Utf8Chars(&s[..n]));
}

const LOGO_WIDTH: usize = 36;

const LOGO: [&str; 19] = [
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢠⡀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣷⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣄",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣾⡟⠀⠀⠀⠀⠀⠀⠀⠀⠀⣸⡿",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⠀⠀⠀⢹⣿⣄⠀⠀⣄⠀⠀⠀⣠⣾⡟⠁",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⢳⣄⠀⠀⢻⣿⡇⠰⠿⠀⠀⢀⣿⣿",
    "⠀⠀⠤⣀⣀⣀⠀⠀⠀⠀⠀⠙⠆⠀⣼⣿⡷⠠⡦⠀⣤⣾⣿⠇⠀⣠⠆",
    "⠀⠀⠀⠈⠛⢿⣶⣤⣤⣤⣀⣀⠴⠇⠈⢀⣀⣠⣤⣤⡉⠛⠋⣤⡘⠋⠀⠀⢀⣠⣤⣄⣠⣤⡴⠂",
    "⠀⠀⠀⠀⠀⠀⠉⠛⠛⢿⣿⡿⠀⣰⣾⡿⠟⠛⠛⠛⢿⣷⣄⠈⣵⣤⣤⣶⣿⠟⠛⠻⠛⠙",
    "⠀⠀⠀⠀⠠⢤⣤⣤⠀⢀⣉⠁⣼⣿⠋⣠⣶⣿⣿⣷⣄⠙⣿⣆⠘⠿⠟⠋⠁",
    "⠀⠀⠀⠀⠀⠀⠀⠀⢀⣈⣍⠀⣿⡏⠈⣿⣿⣁⡈⠹⣿⡇⢻⣿⠀⠺⠆⠀⣶⠶⠦⠂",
    "⠀⠀⠀⠀⠀⠀⢠⣾⣿⣿⣿⡇⠻⣿⣦⡈⠛⠋⣁⣴⣿⢃⣽⣿⠀⣤⡀",
    "⢀⣤⣾⣿⠿⣿⡿⠟⠁⠀⠠⣶⢀⡈⠻⠿⣿⣿⠿⠏⣡⣾⡿⠃⣸⣿⣿⣿⣿⣿⣷⣦",
    "⠀⠀⠀⠀⠀⠀⠀⢀⣴⠟⠀⠀⣿⣿⣿⣷⣶⣶⣾⡿⠟⠋⢀⣤⠈⠉⠉⠉⠁⠀⠛⠿⣶⣤⣄⡀",
    "⠀⠀⠀⠀⠀⠀⠀⠈⠀⠀⠀⢸⣿⡟⠁⠀⣥⡁⢠⣤⣴⣶⠀⠁⢶⣆",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣸⣿⡇⠀⢀⣌⠀⠈⣿⣿⡇⠀⠀⠀⠉⠃",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣼⣿⠋⠀⠀⠰⠗⠀⠀⠘⣿⣿⡀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⡿⠁⠀⠀⠀⠈⠀⠀⠀⠀⠈⠻⣿⡄",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠟⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢿⡇",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠘⠅",];

const LOGO_COLORS: [Rgb; 19] = [
    Rgb(255, 255, 200),
    Rgb(255, 240, 191),
    Rgb(255, 225, 182),
    Rgb(255, 210, 173),
    Rgb(255, 195, 164),
    Rgb(255, 180, 155),
    Rgb(255, 165, 146),
    Rgb(255, 150, 137),
    Rgb(255, 135, 128),
    Rgb(255, 120, 119),
    Rgb(255, 105, 110),
    Rgb(255, 90, 101),
    Rgb(255, 75, 92),
    Rgb(255, 60, 83),
    Rgb(255, 45, 74),
    Rgb(255, 40, 65),
    Rgb(255, 40, 56),
    Rgb(255, 40, 47),
    Rgb(255, 40, 38),
];

fn about_logo(line: usize) {
    framebuffer::set_fg(LOGO_COLORS[line]);
    print!("{:<w$}  ", LOGO[line], w = LOGO_WIDTH);
    framebuffer::reset_colors();
}

fn about_row(logo_line: usize, key: &str, value: fmt::Arguments) {
    about_logo(logo_line);
    framebuffer::set_fg(framebuffer::ACCENT);
    print!("{:>10}", key);
    print!(": ");
    framebuffer::reset_colors();
    println!("{}", value);
}

fn cmd_solarfetch() {
    about_logo(0);
    framebuffer::set_fg(framebuffer::ACCENT);
    let (uname, ulen) = crate::users::user_name(crate::users::current_user());
    println!("{}@solaros", crate::Utf8Chars(&uname[..ulen]));
    about_logo(1);
    framebuffer::set_fg(framebuffer::ACCENT);
    println!("----------------");

    about_row(2, "OS", format_args!("{} {} x86_64", OS_NAME, OS_VERSION));
    about_row(3, "Kernel", format_args!("{} {}", crate::KERNEL_NAME, crate::KERNEL_VERSION));

    let secs = interrupts::ticks() * 55 / 1000;
    if secs >= 3600 {
        about_row(4, "Uptime", format_args!("{}h {}m", secs / 3600, (secs % 3600) / 60));
    } else if secs >= 60 {
        about_row(4, "Uptime", format_args!("{}m {}s", secs / 60, secs % 60));
    } else {
        about_row(4, "Uptime", format_args!("{}s", secs));
    }

    let brand = crate::cpu_brand();
    let brand_str = core::str::from_utf8(&brand).unwrap_or("");
    let trimmed = brand_str.trim_end_matches(|c| c == ' ' || c == '\0');
    if trimmed.is_empty() {
        let v = crate::cpu_vendor();
        let vs = core::str::from_utf8(&v).unwrap_or("Unknown").trim_end_matches('\0');
        about_row(5, "CPU", format_args!("{} (model {:#x})", vs, crate::cpu_model()));
    } else {
        about_row(5, "CPU", format_args!("{}", trimmed));
    }

    about_row(6, "Memory", format_args!("{} MB", crate::usable_mem_mb()));

    if let Some(info) = framebuffer::info() {
        about_row(7, "Resolution", format_args!("{}x{}", info.width, info.height));
    } else {
        about_row(7, "Resolution", format_args!("unknown"));
    }

    let mut total_mb: u64 = 0;
    for i in 0..crate::ata::MAX_DEVICES {
        let d = crate::ata::device(i);
        if d.present {
            total_mb += d.capacity_mb;
        }
    }
    about_row(
        8,
        "Disks",
        format_args!("{} devices ({} MB)", crate::ata::device_count(), total_mb),
    );

    about_row(
        9,
        "Keyboard",
        format_args!("{}", settings::layout_code()),
    );

    about_row(10, "Terminal", format_args!("tty"));

    for line in 11..19 {
        about_logo(line);
        println!();
    }
}

fn cmd_count(arg: &[char]) {
    if arg.is_empty() {
        println!("Usage: count <n>");
        return;
    }
    let mut n: u64 = 0;
    for &c in arg {
        if !c.is_ascii_digit() {
            println!("Error: enter a number.");
            return;
        }
        n = n.saturating_mul(10).saturating_add((c as u64) - ('0' as u64));
    }
    if n == 0 || n > 100 {
        println!("Counting to {} is too long! (try 1-100)", n);
        return;
    }
    for i in 1..=n {
        println!("{}", i);
    }
}

fn cmd_color(arg: &[char]) {
    let mut s = ['\0'; 16];
    let n = arg.len().min(16);
    s[..n].copy_from_slice(&arg[..n]);

    let color: Option<Rgb> = if eq_ci_str(&s[..n], "red") {
        Some(Rgb(255, 90, 90))
    } else if eq_ci_str(&s[..n], "green") {
        Some(Rgb(120, 255, 140))
    } else if eq_ci_str(&s[..n], "blue") {
        Some(Rgb(90, 160, 255))
    } else if eq_ci_str(&s[..n], "yellow") {
        Some(Rgb(255, 210, 110))
    } else if eq_ci_str(&s[..n], "purple") {
        Some(Rgb(200, 120, 255))
    } else if eq_ci_str(&s[..n], "cyan") {
        Some(Rgb(90, 255, 220))
    } else if eq_ci_str(&s[..n], "orange") {
        Some(Rgb(255, 150, 60))
    } else if eq_ci_str(&s[..n], "pink") {
        Some(Rgb(255, 120, 180))
    } else if eq_ci_str(&s[..n], "white") {
        Some(Rgb(255, 255, 255))
    } else if eq_ci_str(&s[..n], "default") || eq_ci_str(&s[..n], "reset") {
        None
    } else {
        println!(
            "Unknown color: '{}'. (colors: red, green, blue, yellow, purple, cyan, orange, pink, white, default)",
            crate::Utf8Chars(&s[..n])
        );
        return;
    };

    match color {
        Some(c) => {
            framebuffer::set_fg(c);
            println!("Text color changed.");
        }
        None => {
            framebuffer::reset_colors();
            println!("Back to default colors.");
        }
    }
}

fn cmd_status() {
    let uptime_secs = interrupts::ticks() / 100;
    println!("System status:");
    println!("  {:<14}: {} seconds", "Uptime", uptime_secs);
    if let Some(info) = framebuffer::info() {
        println!(
            "  {:<14}: {}x{} px, {:?}",
            "Framebuffer",
            info.width,
            info.height,
            info.pixel_format
        );
    } else {
        println!("  {:<14}: NONE", "Framebuffer");
    }
    println!("  {:<14}: {}", "Command count", COMMANDS.len());
    let (uname, ulen) = crate::users::user_name(crate::users::current_user());
    println!("  {:<14}: {}", "User", crate::Utf8Chars(&uname[..ulen]));
    println!("  {:<14}: {}", "Keyboard", settings::layout_code());
}

fn cmd_loadkeys(arg: &[char]) {
    let mut s = ['\0'; 16];
    let n = arg.len().min(16);
    s[..n].copy_from_slice(&arg[..n]);

    if eq_ci_str(&s[..n], "tr")
        || eq_ci_str(&s[..n], "trq")
        || eq_ci_str(&s[..n], "turkish")
        || eq_ci_str(&s[..n], "trf")
    {
        settings::set_layout(KbdLayout::TurkishQ);
        println!("Keyboard layout: trq");
    } else if eq_ci_str(&s[..n], "us")
        || eq_ci_str(&s[..n], "qwerty")
        || eq_ci_str(&s[..n], "english")
        || eq_ci_str(&s[..n], "us-intl")
    {
        settings::set_layout(KbdLayout::Us);
        println!("Keyboard layout: en-us");
    } else {
        println!("Usage: loadkeys <tr|trq|us|qwerty>");
    }
}

fn cmd_diskinfo() {
    println!("ATA devices:");
    for i in 0..crate::ata::MAX_DEVICES {
        let d = crate::ata::device(i);
        let slot = if d.is_secondary { "secondary" } else { "primary" };
        let pos = if d.master { "master" } else { "slave" };
        if d.present {
            println!(
                "  Disk {} ({}:{}): {} MB, {} sectors, LBA {} - {}",
                i,
                slot,
                pos,
                d.capacity_mb,
                d.sectors,
                if d.lba_supported { "yes" } else { "no" },
                d.model_str()
            );
        } else {
            println!("  Disk {} ({}:{}): not found", i, slot, pos);
        }
    }
}

fn fs_err(cmd: &str, e: crate::fs::FsErr) {
    println!("{}: {}", cmd, crate::fs::err_str(e));
}

fn cmd_ls(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("ls", crate::fs::FsErr::NotFormatted);
        return;
    }
    if let Err(e) = crate::fs::ls(arg) {
        fs_err("ls", e);
    }
}

fn cmd_cd(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("cd", crate::fs::FsErr::NotFormatted);
        return;
    }
    if let Err(e) = crate::fs::cd(arg) {
        fs_err("cd", e);
    }
}

fn cmd_pwd() {
    crate::fs::pwd();
}

fn cmd_mkdir(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("mkdir", crate::fs::FsErr::NotFormatted);
        return;
    }
    if arg.is_empty() {
        println!("Usage: mkdir <path>");
        return;
    }
    if let Err(e) = crate::fs::mkdir(arg) {
        fs_err("mkdir", e);
    }
}

fn cmd_touch(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("touch", crate::fs::FsErr::NotFormatted);
        return;
    }
    if arg.is_empty() {
        println!("Usage: touch <path>");
        return;
    }
    if let Err(e) = crate::fs::touch(arg) {
        fs_err("touch", e);
    }
}

fn cmd_cat(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("cat", crate::fs::FsErr::NotFormatted);
        return;
    }
    if arg.is_empty() {
        println!("Usage: cat <path>");
        return;
    }
    if let Err(e) = crate::fs::cat(arg) {
        fs_err("cat", e);
    }
}

fn cmd_rm(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("rm", crate::fs::FsErr::NotFormatted);
        return;
    }
    if arg.is_empty() {
        println!("Usage: rm <path>");
        return;
    }
    if let Err(e) = crate::fs::rm(arg) {
        fs_err("rm", e);
    }
}

fn cmd_rmdir(arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("rmdir", crate::fs::FsErr::NotFormatted);
        return;
    }
    if arg.is_empty() {
        println!("Usage: rmdir <path>");
        return;
    }
    if let Err(e) = crate::fs::rmdir(arg) {
        fs_err("rmdir", e);
    }
}

fn cmd_mv(a: &[char], b: &[char]) {
    if !crate::fs::mounted() {
        fs_err("mv", crate::fs::FsErr::NotFormatted);
        return;
    }
    if a.is_empty() || b.is_empty() {
        println!("Usage: mv <old> <new>");
        return;
    }
    if let Err(e) = crate::fs::mv(a, b) {
        fs_err("mv", e);
    }
}

fn cmd_mkfs(arg: &[char]) {
    let ext4 = !arg.is_empty() && arg[0] == 'e';
    let res = if ext4 {
        crate::fs::mkfs_ext4()
    } else {
        crate::fs::mkfs()
    };
    match res {
        Ok(()) => {
            if ext4 {
                println!("mkfs: data disk formatted as ext4");
            } else {
                println!("mkfs: data disk formatted as FAT32");
            }
        }
        Err(e) => fs_err("mkfs", e),
    }
}

const WTEST_MAX: usize = 4 * 1024 * 1024;
static WTEST_BUF: Mutex<[u8; WTEST_MAX]> = Mutex::new([0; WTEST_MAX]);

fn wtest_pattern(i: usize) -> u8 {
    ((i as u64 * 2654435761) >> 21) as u8 ^ (i as u8).wrapping_mul(31)
}

fn cmd_wtest(arg: &[char], size_arg: &[char]) {
    if !crate::fs::mounted() {
        fs_err("wtest", crate::fs::FsErr::NotFormatted);
        return;
    }
    if arg.is_empty() || size_arg.is_empty() {
        println!("Usage: wtest <path> <size|verify>");
        return;
    }
    if eq_ci_str(size_arg, "verify") {
        let mut buf = WTEST_BUF.lock();
        let n = match crate::fs::read_file(arg, &mut buf[..]) {
            Ok(n) => n,
            Err(e) => {
                fs_err("wtest", e);
                return;
            }
        };
        for i in 0..n {
            if buf[i] != wtest_pattern(i) {
                println!("wtest: MISMATCH at byte {}", i);
                return;
            }
        }
        println!("wtest: verify OK ({} bytes)", n);
        return;
    }
    let mut size = 0usize;
    for &c in size_arg {
        if !c.is_ascii_digit() {
            println!("wtest: invalid size");
            return;
        }
        size = size
            .saturating_mul(10)
            .saturating_add(c as usize - '0' as usize);
        if size > WTEST_MAX {
            println!("wtest: size must be <= {}", WTEST_MAX);
            return;
        }
    }
    if size == 0 {
        println!("wtest: size must be > 0");
        return;
    }
    {
        let mut buf = WTEST_BUF.lock();
        for i in 0..size {
            buf[i] = wtest_pattern(i);
        }
        let r = crate::fs::write_file(arg, &buf[..size]);
        let _ = crate::ext4::jbd_commit();
        match r {
            Ok(()) => {
                println!(
                    "wtest: wrote {} bytes to {}",
                    size,
                    crate::Utf8Chars(arg)
                );
            }
            Err(e) => fs_err("wtest", e),
        }
    }
}

fn cmd_whoami() {
    let (name, len) = crate::users::user_name(crate::users::current_user());
    println!("{}", crate::Utf8Chars(&name[..len]));
}

fn cmd_users() {
    let count = crate::users::count();
    println!("User accounts ({}):", count);
    for i in 0..count {
        let (name, len) = crate::users::user_name(i);
        println!(
            "  {:<12} uid={:<5} gid={}",
            crate::Utf8Chars(&name[..len]),
            crate::users::user_uid(i),
            crate::users::user_gid(i)
        );
    }
}

fn cmd_adduser(arg: &[char]) {
    if arg.is_empty() {
        println!("Usage: adduser <name>");
        return;
    }
    match crate::users::add_user(arg) {
        Ok(idx) => {
            let (name, len) = crate::users::user_name(idx);
            println!(
                "user '{}' added with uid {}",
                crate::Utf8Chars(&name[..len]),
                crate::users::user_uid(idx)
            );
        }
        Err(msg) => println!("adduser: {}", msg),
    }
}

fn cmd_passwd(arg: &[char]) {
    let idx = if arg.is_empty() {
        crate::users::current_user()
    } else {
        match crate::users::find(arg) {
            Some(i) => i,
            None => {
                println!("passwd: no such user");
                return;
            }
        }
    };
    let first = match crate::terminal::read_line("New password: ", false) {
        Some(v) => v,
        None => {
            println!("passwd: cancelled");
            return;
        }
    };
    let second = match crate::terminal::read_line("Repeat password: ", false) {
        Some(v) => v,
        None => {
            println!("passwd: cancelled");
            return;
        }
    };
    if !first
        .0
        .iter()
        .zip(second.0.iter())
        .all(|(a, b)| a == b)
        || first.1 != second.1
    {
        println!("passwd: passwords do not match");
        return;
    }
    match crate::users::set_password(idx, &first.0[..first.1]) {
        Ok(()) => println!("passwd: password updated"),
        Err(msg) => println!("passwd: {}", msg),
    }
}

fn cmd_login(arg: &[char]) {
    if arg.is_empty() {
        println!("Usage: login <user>");
        return;
    }
    let Some(idx) = crate::users::find(arg) else {
        println!("login: no such user '{}'", crate::Utf8Chars(arg));
        return;
    };
    let mut pass = ['\0'; crate::terminal::READ_CAP];
    let mut pass_len = 0usize;
    if crate::users::has_password(idx) {
        match crate::terminal::read_line("Password: ", false) {
            Some(v) => {
                pass = v.0;
                pass_len = v.1;
            }
            None => {
                println!("login: cancelled");
                return;
            }
        }
    }
    if !crate::users::check_password(idx, &pass[..pass_len]) {
        println!("login: authentication failure");
        return;
    }
    crate::users::set_current(idx);
    let (name, len) = crate::users::user_name(idx);
    println!("logged in as {}", crate::Utf8Chars(&name[..len]));
}