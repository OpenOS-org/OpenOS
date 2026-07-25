//! Print macros for user-space output.
//!
//! Provides `print!`, `println!`, `eprint!`, `eprintln!` macros that
//! write to the kernel console via `SYS_WRITE`. Since there's no stdout/stderr
//! distinction in the microkernel, all macros write to the same destination.

/// Write formatted output to the console via `SYS_WRITE`.
///
/// This is the user-space equivalent of `std::print!`. It formats the
/// arguments into a stack buffer and writes them in a single syscall.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::_print(format_args!($($arg)*))
    };
}

/// Write formatted output followed by a newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Write formatted output to stderr (currently same as stdout).
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::_print(format_args!($($arg)*))
    };
}

/// Write formatted output followed by a newline to stderr.
#[macro_export]
macro_rules! eprintln {
    () => ($crate::eprint!("\n"));
    ($($arg:tt)*) => ($crate::eprint!("{}\n", format_args!($($arg)*)));
}

/// Internal implementation of the print macros.
///
/// Uses a stack buffer to format the output and writes it via `SYS_WRITE`.
/// The buffer size (512 bytes) limits a single print call — larger outputs
/// are silently truncated.
#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;

    struct Writer;

    impl Write for Writer {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            // Write in chunks to avoid oversized syscalls.
            for chunk in s.as_bytes().chunks(512) {
                let _ = crate::io::write(chunk);
            }
            Ok(())
        }
    }

    Writer.write_fmt(args).ok();
}
