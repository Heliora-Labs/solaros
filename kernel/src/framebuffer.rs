use core::fmt;
use core::fmt::Write;

use bootloader_api::info::FrameBufferInfo;
use bootloader_api::info::PixelFormat;
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

pub const CELL_W: usize = 8;
pub const CELL_H: usize = 16;

// Scrollback: at most 256 columns (2048px) x 2000 rows. ~4MB bss.
const COLS_MAX: usize = 256;
const SCR_ROWS: usize = 2000;

#[derive(Clone, Copy, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const BG: Rgb = Rgb(10, 12, 28);
pub const FG: Rgb = Rgb(226, 232, 255);
pub const ACCENT: Rgb = Rgb(93, 220, 255);
pub const RED: Rgb = Rgb(255, 90, 90);

const EMPTY_CELL: Cell = Cell { c: ' ', fg: FG };

#[derive(Clone, Copy)]
struct Cell {
    c: char,
    fg: Rgb,
}

struct Console {
    buffer: Option<&'static mut [u8]>,
    info: Option<FrameBufferInfo>,
    fg: Rgb,
    bg: Rgb,
    cols: usize,   // visible column count (pixels/8)
    vis: usize,    // visible row count (pixels/16)
    cells: [Cell; COLS_MAX * SCR_ROWS],
    rows: usize,   // total logical rows in the buffer (0 = empty)
    view: usize,   // rows scrolled back from the end (0 = follow)
    curs_col: usize,
}

impl Console {
    const fn empty() -> Self {
        Console {
            buffer: None,
            info: None,
            fg: FG,
            bg: BG,
            cols: 80,
            vis: 25,
            cells: [EMPTY_CELL; COLS_MAX * SCR_ROWS],
            rows: 0,
            view: 0,
            curs_col: 0,
        }
    }

    fn width(&self) -> usize {
        self.info.map(|i| i.width).unwrap_or(640)
    }

    fn height(&self) -> usize {
        self.info.map(|i| i.height).unwrap_or(400)
    }

    fn ready(&self) -> bool {
        self.buffer.is_some() && self.info.is_some()
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: Rgb) {
        let Some(info) = self.info.as_ref() else { return };
        let Some(buf) = self.buffer.as_deref_mut() else { return };
        if x >= info.width || y >= info.height {
            return;
        }
        let offset = y * info.stride * info.bytes_per_pixel + x * info.bytes_per_pixel;
        let bytes = [color.0, color.1, color.2];
        match info.pixel_format {
            PixelFormat::Rgb => {
                buf[offset] = bytes[0];
                buf[offset + 1] = bytes[1];
                buf[offset + 2] = bytes[2];
            }
            PixelFormat::Bgr => {
                buf[offset] = bytes[2];
                buf[offset + 1] = bytes[1];
                buf[offset + 2] = bytes[0];
            }
            _ => {}
        }
    }

    fn get_pixel(&self, x: usize, y: usize) -> Rgb {
        let Some(info) = self.info.as_ref() else { return self.bg };
        let Some(buf) = self.buffer.as_deref() else { return self.bg };
        if x >= info.width || y >= info.height {
            return self.bg;
        }
        let offset = y * info.stride * info.bytes_per_pixel + x * info.bytes_per_pixel;
        match info.pixel_format {
            PixelFormat::Rgb => Rgb(buf[offset], buf[offset + 1], buf[offset + 2]),
            PixelFormat::Bgr => Rgb(buf[offset + 2], buf[offset + 1], buf[offset]),
            _ => self.bg,
        }
    }

    // Shift the framebuffer up by one row, clearing the bottom row.
    fn scroll_px(&mut self) {
        let w = self.width();
        let h = self.height();
        for y in CELL_H..h {
            for x in 0..w {
                let c = self.get_pixel(x, y);
                self.put_pixel(x, y - CELL_H, c);
            }
        }
        self.fill_rect(0, h - CELL_H, w, h, self.bg);
    }

    fn fill_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, color: Rgb) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.put_pixel(x, y, color);
            }
        }
    }

    // Before dropping the oldest row: shift the "rows" buffer rows
    // [1..rows) -> [0..rows-1)
    fn drop_oldest(&mut self) {
        let stride = COLS_MAX;
        self.cells
            .copy_within(stride..self.rows * stride, 0);
        self.rows -= 1;
        if self.view > 0 {
            self.view -= 1;
        }
    }

    // Move to a new row. When following the bottom (view==0) and the screen
    // is full, scroll the display up by one row.
    fn new_line(&mut self) {
        if self.rows == 0 {
            self.rows = 1;
            self.curs_col = 0;
            return;
        }
        let need_px = self.view == 0 && self.rows >= self.vis;
        self.rows += 1;
        if self.rows > SCR_ROWS {
            self.drop_oldest();
        }
        // Keep stale character tails out of the reused row.
        let cols = self.scr_cols();
        let line_start = (self.rows - 1) * COLS_MAX;
        self.cells[line_start..line_start + cols].fill(EMPTY_CELL);
        self.curs_col = 0;
        if need_px {
            self.scroll_px();
        }
    }

    // Start index of the visible rows in the buffer
    fn window_start(&self) -> usize {
        if self.rows == 0 {
            return 0;
        }
        if self.rows >= self.vis {
            self.rows - self.vis - self.view.min(self.rows - self.vis)
        } else {
            0
        }
    }

    fn draw_cell_at(&mut self, sx: usize, sy: usize, cell: Cell) {
        if !self.ready() {
            return;
        }
        let x = sx * CELL_W;
        let y = sy * CELL_H;
        self.fill_rect(x, y, x + CELL_W, y + CELL_H, self.bg);
        if cell.c != ' ' {
            let glyph = crate::vga_font::get(cell.c).unwrap_or(crate::vga_font::BLOCK);
            for (row, line) in glyph.iter().enumerate() {
                for col in 0..CELL_W {
                    let bit = (line >> (7 - col)) & 1;
                    if bit == 1 {
                        self.put_pixel(x + col, y + row, cell.fg);
                    }
                }
            }
        }
    }

    // Cursor row/column on screen (meaningful when view==0)
    fn cursor_location(&self) -> (usize, usize) {
        if self.rows == 0 {
            return (self.curs_col.min(self.scr_cols().saturating_sub(1)), 0);
        }
        let final_row = self.rows - 1;
        let sy = if final_row >= self.vis { self.vis - 1 } else { final_row };
        (self.curs_col.min(self.scr_cols().saturating_sub(1)), sy)
    }

    fn scr_cols(&self) -> usize {
        self.cols.min(COLS_MAX)
    }

    fn wrap_if_needed(&mut self) {
        if self.curs_col >= self.scr_cols() {
            self.new_line();
        }
    }

    fn write_char(&mut self, c: char) {
        // Any key input while scrolled back returns to the bottom.
        if self.view != 0 {
            self.view = 0;
            self.paint();
        }
        match c {
            '\n' => {
                self.new_line();
                if self.rows > 0 {
                    let (sx, sy) = self.cursor_location();
                    self.curs_line_bg(sx, sy);
                }
            }
            '\r' => self.curs_col = 0,
            c => {
                self.wrap_if_needed();
                if self.rows == 0 {
                    self.rows = 1;
                }
                let idx = (self.rows - 1) * COLS_MAX + self.curs_col;
                self.cells[idx] = Cell { c, fg: self.fg };
                let (sx, sy) = self.cursor_location();
                self.draw_cell_at(sx, sy, self.cells[idx]);
                self.curs_col = (self.curs_col + 1).min(self.scr_cols());
            }
        }
    }

    fn curs_line_bg(&mut self, sx: usize, sy: usize) {
        // Clear the next cell on the line to be written (for convenience)
        if self.rows > 0 {
            let idx = (self.rows - 1) * COLS_MAX + sx;
            self.cells[idx] = EMPTY_CELL;
        }
        self.draw_cell_at(sx, sy, EMPTY_CELL);
    }

    fn paint(&mut self) {
        if !self.ready() {
            return;
        }
        self.fill_rect(0, 0, self.width(), self.height(), self.bg);
        let a = self.window_start();
        for sy in 0..self.vis {
            let row = a + sy;
            if row >= self.rows {
                break;
            }
            for sx in 0..self.scr_cols() {
                let cell = self.cells[row * COLS_MAX + sx];
                if cell.c != ' ' {
                    self.draw_cell_at(sx, sy, cell);
                }
            }
        }
    }

    fn clear_screen(&mut self) {
        if !self.ready() {
            return;
        }
        self.fill_rect(0, 0, self.width(), self.height(), self.bg);
        self.cells.fill(EMPTY_CELL);
        self.rows = 0;
        self.view = 0;
        self.curs_col = 0;
    }

    fn backspace_at(&mut self) {
        if self.rows == 0 {
            return;
        }
        if self.curs_col > 0 {
            self.curs_col -= 1;
            let idx = (self.rows - 1) * COLS_MAX + self.curs_col;
            self.cells[idx] = EMPTY_CELL;
            let (sx, sy) = self.cursor_location();
            self.draw_cell_at(sx, sy, EMPTY_CELL);
        }
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.write_char(c);
        }
        Ok(())
    }
}

static CONSOLE: Mutex<Console> = Mutex::new(Console::empty());

pub fn init(buffer: &'static mut [u8], info: FrameBufferInfo) {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        console.buffer = Some(buffer);
        console.info = Some(info);
        console.cols = (info.width / CELL_W).clamp(1, COLS_MAX);
        console.vis = (info.height / CELL_H).clamp(1, SCR_ROWS).min(SCR_ROWS);
        console.clear_screen();
    });
}

pub fn set_colors(fg: Rgb, bg: Rgb) {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        console.fg = fg;
        console.bg = bg;
    });
}

pub fn set_fg(fg: Rgb) {
    without_interrupts(|| {
        CONSOLE.lock().fg = fg;
    });
}

pub fn info() -> Option<FrameBufferInfo> {
    without_interrupts(|| CONSOLE.lock().info)
}

pub fn backspace() {
    without_interrupts(|| {
        CONSOLE.lock().backspace_at();
    });
}

pub fn reset_colors() {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        console.fg = FG;
        console.bg = BG;
    });
}

pub fn clear() {
    without_interrupts(|| {
        CONSOLE.lock().clear_screen();
    });
}

// ---- TTY scrollback ----

pub fn tty_page_up() {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        let pg = console.vis;
        console.view = (console.view + pg).min(console.rows.saturating_sub(console.vis));
        console.paint();
    });
}

pub fn tty_page_down() {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        console.view = console.view.saturating_sub(console.vis);
        console.paint();
    });
}

pub fn tty_home() {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        console.view = console.rows.saturating_sub(console.vis);
        console.paint();
    });
}

pub fn tty_end() {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        console.view = 0;
        console.paint();
    });
}

#[doc(hidden)]
pub fn write_fmt(args: fmt::Arguments) {
    without_interrupts(|| {
        let mut console = CONSOLE.lock();
        let _ = console.write_fmt(args);
    });
}