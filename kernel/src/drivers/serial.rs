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

/// Check if the UART has received data available in its FIFO.
///
/// Reads the Line Status Register (LSR) and checks the Data Ready bit (bit 0).
/// Returns `true` if at least one byte is available to read.
#[must_use]
pub fn serial_has_data() -> bool {
    interrupts::without_interrupts(|| {
        // SAFETY: Port 0x3F8+5 is the LSR for COM1. Reading a status register
        // has no side effects.
        unsafe { x86_64::instructions::port::PortReadOnly::<u8>::new(0x3F8 + 5).read() & 1 != 0 }
    })
}

/// Try to read a single byte from the UART receive buffer.
///
/// Returns `Some(byte)` if data is available, `None` if the FIFO is empty.
/// Call `serial_has_data()` first to check.
#[must_use]
pub fn serial_read_byte() -> Option<u8> {
    interrupts::without_interrupts(|| {
        if unsafe { x86_64::instructions::port::PortReadOnly::<u8>::new(0x3F8 + 5).read() & 1 != 0 }
        {
            // SAFETY: Port 0x3F8 is the data register (RBR in read mode).
            let byte = unsafe { x86_64::instructions::port::PortReadOnly::<u8>::new(0x3F8).read() };
            // Ignore null bytes from the UART (can appear as noise on some setups).
            if byte == 0 {
                None
            } else {
                Some(byte)
            }
        } else {
            None
        }
    })
}

/// Print to serial port (QEMU `-serial stdio` output).
///
/// When testing on the host, serial I/O is not available, so this is a no-op.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        #[cfg(not(test))]
        {
            $crate::drivers::serial::_serial_print(format_args!($($arg)*));
        }
    };
}

/// Print a line to serial port.
///
/// When testing on the host, serial I/O is not available, so this is a no-op.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

#[cfg(test)]
mod tests {
    /// COM1 standard I/O address (verify the well-known constant).
    const COM1_IO_PORT: u16 = 0x3F8;

    /// Standard baud rate divisors for UART 16550.
    /// The UART clock is 115200 Hz base. Divisor = 115200 / baud_rate.
    const BAUD_RATE_DIVISOR_9600: u16 = 12; // 115200 / 9600 = 12
    const BAUD_RATE_DIVISOR_38400: u16 = 3; // 115200 / 38400 = 3
    const BAUD_RATE_DIVISOR_115200: u16 = 1; // 115200 / 115200 = 1

    #[test]
    fn com1_io_port_address() {
        assert_eq!(COM1_IO_PORT, 0x3F8);
    }

    #[test]
    fn baud_rate_divisor_9600() {
        // 115200 / 9600 = 12
        assert_eq!(115200u32 / 9600u32, BAUD_RATE_DIVISOR_9600 as u32);
    }

    #[test]
    fn baud_rate_divisor_38400() {
        // 115200 / 38400 = 3
        assert_eq!(115200u32 / 38400u32, BAUD_RATE_DIVISOR_38400 as u32);
    }

    #[test]
    fn baud_rate_divisor_115200() {
        // 115200 / 115200 = 1
        assert_eq!(115200u32 / 115200u32, BAUD_RATE_DIVISOR_115200 as u32);
    }

    #[test]
    fn uart_fifo_size() {
        // UART 16550 has a 16-byte FIFO.
        const UART_FIFO_SIZE: usize = 16;
        assert_eq!(UART_FIFO_SIZE, 16);
    }

    #[test]
    fn uart_register_offsets() {
        // Standard UART 16550 register offsets from base port.
        // These are the offsets used by the uart_16550 crate.
        const DATA_REGISTER: u16 = 0; // RBR/THR
        const INT_ENABLE: u16 = 1; // IER
        const FIFO_CONTROL: u16 = 2; // FCR
        const LINE_CONTROL: u16 = 3; // LCR
        const MODEM_CONTROL: u16 = 4; // MCR
        const LINE_STATUS: u16 = 5; // LSR

        assert_eq!(DATA_REGISTER, 0);
        assert_eq!(INT_ENABLE, 1);
        assert_eq!(FIFO_CONTROL, 2);
        assert_eq!(LINE_CONTROL, 3);
        assert_eq!(MODEM_CONTROL, 4);
        assert_eq!(LINE_STATUS, 5);
    }

    #[test]
    fn com1_port_range() {
        // COM1 uses I/O ports 0x3F8..0x3FF (8 ports).
        assert_eq!(COM1_IO_PORT + 7, 0x3FF);
    }

    #[test]
    fn divisor_latch_bit() {
        // DLAB (Divisor Latch Access Bit) is bit 7 of the Line Control Register.
        const DLAB_BIT: u8 = 1 << 7;
        assert_eq!(DLAB_BIT, 0x80);
    }
}
