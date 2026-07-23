//! VGA text-mode display driver.
//!
//! The VGA text buffer is a memory-mapped I/O region at physical address
//! `0xB8000`. Each character cell is 2 bytes: one byte of ASCII, one byte of
//! color attributes. The hardware scans this buffer at ~60 Hz and renders it
//! to the screen — no GPU or framebuffer driver needed.
//!
//! We use `Volatile` wrappers because the compiler must not reorder or elide
//! writes to MMIO regions: the hardware reads them independently of the CPU.

use core::fmt;

use lazy_static::lazy_static;
use spin::Mutex;
use volatile::Volatile;

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

/// VGA hardware color palette. Values 0–15 match the CGA/EGA color encoding
/// that the VGA text-mode hardware expects in the attribute byte.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// Packed foreground (low nibble) + background (high nibble) color attribute.
///
/// `#[repr(transparent)]` guarantees this is laid out identically to its
/// inner `u8`, so we can write it directly to the VGA attribute byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    fn new(foreground: Color, background: Color) -> Self {
        // VGA attribute format: bits 0-3 = foreground, bits 4-6 = background.
        Self((background as u8) << 4 | (foreground as u8))
    }
}

/// One cell in the VGA text buffer. `#[repr(C)]` ensures the compiler doesn't
/// reorder the fields — the hardware expects ASCII first, then attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

/// The full 80×25 VGA text buffer. `#[repr(transparent)]` so we can cast a
/// raw pointer to `0xB8000` directly to `&mut Buffer`.
#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// Writer that owns the VGA buffer and tracks the cursor position.
pub struct VgaWriter {
    column_position: usize,
    color_code: ColorCode,
    /// Pointer to the MMIO buffer at 0xB8000. The `'static` lifetime is a
    /// lie in the Rust sense — this memory is not owned by the kernel — but
    /// it's correct because the VGA buffer exists for the entire duration of
    /// the machine's power-on session.
    buffer: &'static mut Buffer,
}

impl VgaWriter {
    /// Write a single byte to the current cursor position, advancing the cursor.
    /// Newlines trigger a scroll; printable ASCII is written directly.
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;
                let color_code = self.color_code;

                // Volatile write — the compiler must not elide or reorder this,
                // because the VGA hardware reads the buffer independently.
                self.buffer.chars[row][col].write(ScreenChar {
                    ascii_character: byte,
                    color_code,
                });
                self.column_position += 1;
            }
        }
    }

    /// Write a string, replacing non-printable bytes with ■ (0xFE) so the
    /// kernel can display arbitrary byte sequences (e.g., panic payloads)
    /// without corrupting the display.
    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
        }
    }

    /// Scroll the screen up one line by copying each row to the one above it,
    /// then clear the bottom row. This is the classic VGA text-mode scroll
    /// — no hardware scroll support, just memory copies.
    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = self.buffer.chars[row][col].read();
                self.buffer.chars[row - 1][col].write(character);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }
}

/// Implement `core::fmt::Write` so we can use `write!` / `format_args!` with
/// the VGA writer. This is the bridge that makes `print!` / `println!` work.
impl fmt::Write for VgaWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

lazy_static! {
    /// Global VGA writer. Mutex-protected because `print!` may be called from
    /// any context (main loop, interrupt handler). The interrupt-safety is
    /// handled in `_print` by disabling interrupts around the lock.
    pub static ref WRITER: Mutex<VgaWriter> = Mutex::new(VgaWriter {
        column_position: 0,
        color_code: ColorCode::new(Color::LightGreen, Color::Black),
        // SAFETY: 0xB8000 is the fixed physical address of the VGA text buffer
        // on all x86 PCs. The bootloader has identity-mapped this region, so
        // the virtual address equals the physical address. The cast to `*mut
        // Buffer` is valid because Buffer is `#[repr(transparent)]` over an
        // array of Volatile<ScreenChar>, which matches the hardware layout.
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}

/// Clear the screen by writing blank cells to every position.
pub fn init() {
    let mut writer = WRITER.lock();
    for row in 0..BUFFER_HEIGHT {
        writer.clear_row(row);
    }
}

/// Print to the VGA buffer. Use via the `print!` / `println!` macros.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::drivers::vga::_print(format_args!($($arg)*)));
}

/// Print a line to the VGA buffer.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Internal entry point for `print!` / `println!`.
///
/// Disabling interrupts while holding the VGA lock prevents a deadlock:
/// if a timer interrupt fires while we hold the lock, and the timer handler
/// also tries to `println!`, it would attempt to acquire the same spinlock
/// on the same CPU — a guaranteed deadlock.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
    });
}
