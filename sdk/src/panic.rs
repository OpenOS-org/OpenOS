//! Panic handler for user-space programs.
//!
//! When a user-space program panics (via `panic!`, `unwrap()` on None, etc.),
//! this handler:
//!   1. Prints the panic message to the kernel console via `SYS_WRITE`
//!   2. Exits the process with status 1
//!
//! This prevents user-space panics from crashing the kernel — the process
//! is terminated cleanly and the kernel continues running.
//!
//! ## Custom Panic Handler
//!
//! Programs can override this by defining their own `#[panic_handler]`.
//! The SDK's handler is a safe default that works for most programs.

use core::panic::PanicInfo;

/// Global panic handler for user-space programs.
///
/// Prints the panic message and exits. This is the last resort —
/// there is no unwinding, no recovery, no second chances.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Print panic header.
    let _ = crate::io::write(b"\n[USER PANIC] ");

    // Print the panic location if available.
    if let Some(location) = info.location() {
        let _ = crate::io::write(location.file().as_bytes());
        let _ = crate::io::write(b":");
        let line = location.line();
        let mut buf = [0u8; 10];
        let s = u64_to_str(u64::from(line), &mut buf);
        let _ = crate::io::write(s);
        let _ = crate::io::write(b": ");
    }

    // Print the payload message using Display formatting.
    {
        use core::fmt::Write;
        struct FmtWriter;
        impl Write for FmtWriter {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let _ = crate::io::write(s.as_bytes());
                Ok(())
            }
        }
        #[allow(clippy::incompatible_msrv)]
        let _ = FmtWriter.write_fmt(format_args!("{}", info.message()));
    }

    let _ = crate::io::write(b"\n");

    // Exit with error status.
    crate::process::exit(1);
}

/// Convert a u64 to a decimal string in the provided buffer.
/// Returns a slice of the populated portion.
fn u64_to_str(mut n: u64, buf: &mut [u8]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }

    let mut i = buf.len();
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &buf[i..]
}
