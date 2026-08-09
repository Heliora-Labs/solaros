use core::fmt;

use crate::framebuffer;
use crate::{print, println};

const OK_COLOR: framebuffer::Rgb = framebuffer::Rgb(110, 220, 120);
const FAIL_COLOR: framebuffer::Rgb = framebuffer::RED;
const INFO_COLOR: framebuffer::Rgb = framebuffer::Rgb(150, 170, 215);

fn line(color: framebuffer::Rgb, marker: &str, args: fmt::Arguments) {
    framebuffer::set_colors(color, framebuffer::BG);
    print!("[{:^6}] ", marker);
    framebuffer::set_fg(framebuffer::FG);
    println!("{}", args);
    framebuffer::reset_colors();
}

pub fn info(args: fmt::Arguments) {
    framebuffer::set_colors(INFO_COLOR, framebuffer::BG);
    println!("{}", args);
    framebuffer::reset_colors();
}

pub fn ok(args: fmt::Arguments) {
    line(OK_COLOR, "OK", args);
}

pub fn fail(args: fmt::Arguments) {
    line(FAIL_COLOR, "FAIL", args);
}
