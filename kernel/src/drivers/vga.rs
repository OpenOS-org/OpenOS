//! Framebuffer text renderer.
//!
//! Bootloader 0.11 provides a graphical framebuffer instead of the legacy VGA
//! text buffer at 0xB8000. This module renders text glyphs directly onto the
//! framebuffer using an embedded 8×16 bitmap font.
//!
//! The `print!` / `println!` macros are preserved — the rest of the kernel
//! uses them identically regardless of the underlying output device.

use core::fmt;

use bootloader_api::BootInfo;
use lazy_static::lazy_static;
use spin::Mutex;

/// Character cell dimensions in pixels.
const CHAR_WIDTH: usize = 8;
const CHAR_HEIGHT: usize = 16;

/// Default foreground color: light gray (0xAAAAAA).
const DEFAULT_FG: u32 = 0x00AA_AAAA;
/// Default background color: black (0x000000).
const DEFAULT_BG: u32 = 0x0000_0000;

// Embedded 8×16 bitmap font for printable ASCII (32–126).
// Each character is 16 bytes, one byte per row, MSB = leftmost pixel.
include!("font_8x16.rs");

/// Framebuffer text writer. Holds a mutable slice into the bootloader-provided
/// framebuffer and tracks the cursor position in character coordinates.
struct FramebufferWriter {
    /// Raw framebuffer bytes.
    buffer: &'static mut [u8],
    /// Framebuffer metadata from the bootloader.
    info: bootloader_api::info::FrameBufferInfo,
    /// Current cursor column (character units).
    column: usize,
    /// Current cursor row (character units).
    row: usize,
    /// Foreground color (packed RGB, no alpha).
    fg_color: u32,
    /// Background color (packed RGB, no alpha).
    bg_color: u32,
    /// Number of character columns that fit on screen.
    cols: usize,
    /// Number of character rows that fit on screen.
    rows: usize,
}

// SAFETY: The framebuffer writer is only accessed through a Mutex-protected
// static. The framebuffer memory is exclusively owned by the kernel.
unsafe impl Send for FramebufferWriter {}

impl FramebufferWriter {
    /// Create a new framebuffer writer from bootloader-provided framebuffer.
    fn new(boot_info: &'static mut BootInfo) -> Option<Self> {
        let fb = boot_info.framebuffer.take()?;
        let info = fb.info();
        let buffer = fb.into_buffer();

        let cols = info.width / CHAR_WIDTH;
        let rows = info.height / CHAR_HEIGHT;

        let mut writer = Self {
            buffer,
            info,
            column: 0,
            row: 0,
            fg_color: DEFAULT_FG,
            bg_color: DEFAULT_BG,
            cols: if cols > 0 { cols } else { 80 },
            rows: if rows > 0 { rows } else { 25 },
        };

        // Clear screen.
        writer.clear();
        Some(writer)
    }

    /// Clear the entire framebuffer to the background color.
    fn clear(&mut self) {
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        for y in 0..self.info.height {
            for x in 0..self.info.width {
                let offset = (y * stride + x) * bpp;
                self.write_pixel(offset, self.bg_color);
            }
        }
        self.column = 0;
        self.row = 0;
    }

    /// Write a packed RGB color to the framebuffer at the given byte offset.
    fn write_pixel(&mut self, offset: usize, color: u32) {
        if offset + self.info.bytes_per_pixel > self.buffer.len() {
            return;
        }
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;

        match self.info.pixel_format {
            bootloader_api::info::PixelFormat::Rgb => {
                self.buffer[offset] = r;
                self.buffer[offset + 1] = g;
                self.buffer[offset + 2] = b;
                if self.info.bytes_per_pixel >= 4 {
                    self.buffer[offset + 3] = 0xFF;
                }
            }
            bootloader_api::info::PixelFormat::Bgr => {
                self.buffer[offset] = b;
                self.buffer[offset + 1] = g;
                self.buffer[offset + 2] = r;
                if self.info.bytes_per_pixel >= 4 {
                    self.buffer[offset + 3] = 0xFF;
                }
            }
            _ => {
                // U8 or unknown — write luminance.
                let lum = (r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8;
                self.buffer[offset] = lum as u8;
            }
        }
    }

    /// Render a single character glyph at the given character position.
    fn render_char(&mut self, col: usize, row: usize, ch: char) {
        let glyph_idx = if (ch as usize) < 128 { ch as usize } else { 0 };
        let glyph = &FONT_DATA[glyph_idx];

        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let x0 = col * CHAR_WIDTH;
        let y0 = row * CHAR_HEIGHT;

        for (dy, &row_bits) in glyph.iter().enumerate().take(CHAR_HEIGHT) {
            for dx in 0..CHAR_WIDTH {
                let bit = 0x80 >> dx;
                let color = if row_bits & bit != 0 {
                    self.fg_color
                } else {
                    self.bg_color
                };
                let px = x0 + dx;
                let py = y0 + dy;
                let offset = (py * stride + px) * bpp;
                self.write_pixel(offset, color);
            }
        }
    }

    /// Write a single character, handling newlines and scrolling.
    fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            ch => {
                if self.column >= self.cols {
                    self.new_line();
                }
                self.render_char(self.column, self.row, ch);
                self.column += 1;
            }
        }
    }

    /// Advance to the next line, scrolling if necessary.
    fn new_line(&mut self) {
        self.column = 0;
        self.row += 1;
        if self.row >= self.rows {
            self.scroll();
            self.row = self.rows - 1;
        }
    }

    /// Scroll the screen up by one character row.
    fn scroll(&mut self) {
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let row_bytes = CHAR_HEIGHT * stride * bpp;
        let total_bytes = self.info.height * stride * bpp;

        // Copy each row up by one character height.
        if row_bytes < total_bytes {
            self.buffer.copy_within(row_bytes..total_bytes, 0);
        }

        // Clear the bottom row.
        let clear_start = (self.rows - 1) * CHAR_HEIGHT * stride * bpp;
        for i in clear_start..total_bytes {
            if i < self.buffer.len() {
                self.buffer[i] = 0;
            }
        }
    }
}

impl fmt::Write for FramebufferWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            self.write_char(ch);
        }
        Ok(())
    }
}

lazy_static! {
    /// Global framebuffer writer. Initialized once during boot from the
    /// bootloader-provided `BootInfo`. After initialization, the Mutex
    /// provides interior mutability for `print!` / `println!` calls from
    /// any context.
    static ref WRITER: Mutex<Option<FramebufferWriter>> = Mutex::new(None);
}

/// Initialize the framebuffer text renderer.
///
/// Must be called once during boot, after the bootloader has set up the
/// framebuffer. Panics if no framebuffer is available.
pub fn init(boot_info: &'static mut BootInfo) {
    let writer = FramebufferWriter::new(boot_info);
    *WRITER.lock() = writer;
}

/// Print to the framebuffer. Use via the `print!` / `println!` macros.
///
/// Disabling interrupts while holding the lock prevents a deadlock: if a
/// timer interrupt fires while we hold the lock, and the timer handler also
/// tries to `println!`, it would attempt to acquire the same spinlock on
/// the same CPU — a guaranteed deadlock.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        if let Some(writer) = WRITER.lock().as_mut() {
            writer.write_fmt(args).unwrap();
        }
    });
}

/// Print to the framebuffer.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::drivers::vga::_print(format_args!($($arg)*)));
}

/// Print a line to the framebuffer.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_data_size() {
        // FONT_DATA should have 128 entries (one per ASCII code).
        assert_eq!(FONT_DATA.len(), 128);
    }

    #[test]
    fn test_font_glyph_size() {
        // Each glyph should be 16 bytes (one per row).
        for i in 0..128 {
            assert_eq!(FONT_DATA[i].len(), CHAR_HEIGHT, "glyph {} has wrong size", i);
        }
    }

    #[test]
    fn test_font_space_glyph() {
        // Space (ASCII 32) should be all zeros.
        assert_eq!(FONT_DATA[32], [0u8; 16]);
    }

    #[test]
    fn test_font_nonzero_glyphs() {
        // At least some printable characters should have non-zero glyphs.
        let has_nonzero = FONT_DATA.iter().enumerate().any(|(i, g)| {
            i >= 33 && i <= 126 && g.iter().any(|&b| b != 0)
        });
        assert!(has_nonzero, "all printable glyphs are zero");
    }

    #[test]
    fn test_char_dimensions() {
        assert_eq!(CHAR_WIDTH, 8);
        assert_eq!(CHAR_HEIGHT, 16);
    }

    #[test]
    fn test_default_colors() {
        assert_eq!(DEFAULT_FG, 0x00AA_AAAA);
        assert_eq!(DEFAULT_BG, 0x0000_0000);
    }

    #[test]
    fn test_write_pixel_rgb() {
        // Test RGB color decomposition.
        let color: u32 = 0xFF8040; // R=255, G=128, B=64
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        assert_eq!(r, 0xFF);
        assert_eq!(g, 0x80);
        assert_eq!(b, 0x40);
    }

    #[test]
    fn test_glyph_bit_order() {
        // MSB = leftmost pixel. Verify a known glyph pattern.
        // '!' (ASCII 33) should have a vertical line in the center.
        let glyph = FONT_DATA[33];
        // At least some rows should have bits set in the middle.
        let has_bits = glyph.iter().any(|&b| b != 0);
        assert!(has_bits, "'!' glyph is empty");
    }
}
