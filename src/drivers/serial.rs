//! UART 16550 serial port driver (COM1).
//!
//! The serial port is essential for kernel debugging: QEMU captures writes to
//! the serial port and forwards them to the host's stdout (with `-serial stdio`),
//! so we can see kernel output even when VGA is unavailable or corrupted.
//!
//! Port 0x3F8 is the standard I/O address for COM1. The UART 16550 has a
//! 16-byte FIFO; `uart_16550::SerialPort` handles FIFO setup and byte-level
//! I/O for us.

use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;
use x86_64::instructions::interrupts;

lazy_static! {
    /// Global COM1 serial port. Mutex-protected for the same reason as VGA:
    /// `serial_print!` may be called from any context, including ISR handlers.
    pub static ref SERIAL1: Mutex<SerialPort> = {
        // SAFETY: Port 0x3F8 is the fixed I/O address for COM1. No other
        // device claims this range on standard PC hardware. `init()` sends
        // configuration bytes to the UART registers — safe because the UART
        // is a stateless device at well-known ports.
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

/// Write formatted output to COM1. Use via `serial_print!` / `serial_println!`.
///
/// Interrupts are disabled while holding the lock to prevent the same
/// deadlock scenario as VGA: an interrupt handler calling `serial_print!`
/// while the main loop holds the serial lock.
#[doc(hidden)]
pub fn _serial_print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        SERIAL1
            .lock()
            .write_fmt(args)
            .expect("Printing to serial failed");
    });
}

/// Print to serial port (QEMU `-serial stdio` output).
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::drivers::serial::_serial_print(format_args!($($arg)*));
    };
}

/// Print a line to serial port.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}
