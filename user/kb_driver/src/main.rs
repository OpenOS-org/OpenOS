//! User-space keyboard driver for OpenOS.
//!
//! Receives raw PS/2 scancodes from the kernel via IRQ 1 forwarding,
//! decodes them using US QWERTY scancode set 1, tracks shift state,
//! and sends decoded characters to the console service via channel.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use openos_sdk::{channel, console, device, process, service};

/// PS/2 keyboard I/O port.
const KB_DATA_PORT: u16 = 0x60;

/// Scancode set 1 make codes for the US QWERTY layout.
/// Index = scancode, value = ASCII character (0 = no mapping).
const SC1_LOWER: [u8; 128] = [
    0, 0, b'1', b'2', b'3', b'4', b'5', b'6', // 0x00-0x07
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t', // 0x08-0x0F
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', // 0x10-0x17
    b'o', b'p', b'[', b']', b'\n', 0, b'a', b's', // 0x18-0x1F
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', // 0x20-0x27
    b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v', // 0x28-0x2F
    b'b', b'n', b'm', b',', b'.', b'/', 0, b'*', // 0x30-0x37
    0, b' ', 0, 0, 0, 0, 0, 0, // 0x38-0x3F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x40-0x47
    0, 0, b'-', 0, 0, 0, b'+', 0, // 0x48-0x4F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x50-0x57
    0, 0, 0, 0, 0, 0, 0, 0, // 0x58-0x5F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x60-0x67
    0, 0, 0, 0, 0, 0, 0, 0, // 0x68-0x6F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x70-0x77
    0, 0, 0, 0, 0, 0, 0, 0, // 0x78-0x7F
];

/// Scancode set 1 with Shift held.
const SC1_UPPER: [u8; 128] = [
    0, 0, b'!', b'@', b'#', b'$', b'%', b'^', // 0x00-0x07
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t', // 0x08-0x0F
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', // 0x10-0x17
    b'O', b'P', b'{', b'}', b'\n', 0, b'A', b'S', // 0x18-0x1F
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', // 0x20-0x27
    b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V', // 0x28-0x2F
    b'B', b'N', b'M', b'<', b'>', b'?', 0, b'*', // 0x30-0x37
    0, b' ', 0, 0, 0, 0, 0, 0, // 0x38-0x3F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x40-0x47
    0, 0, b'-', 0, 0, 0, b'+', 0, // 0x48-0x4F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x50-0x57
    0, 0, 0, 0, 0, 0, 0, 0, // 0x58-0x5F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x60-0x67
    0, 0, 0, 0, 0, 0, 0, 0, // 0x68-0x6F
    0, 0, 0, 0, 0, 0, 0, 0, // 0x70-0x77
    0, 0, 0, 0, 0, 0, 0, 0, // 0x78-0x7F
];

/// Make code for left shift.
const SCANCODE_LSHIFT: u8 = 0x2A;
/// Make code for right shift.
const SCANCODE_RSHIFT: u8 = 0x36;
/// Break code prefix (make code | 0x80).
const BREAK_BIT: u8 = 0x80;

/// Keyboard driver state.
struct KeyboardState {
    /// Whether either shift key is currently held.
    shift: bool,
    /// Whether caps lock is toggled on.
    caps_lock: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            shift: false,
            caps_lock: false,
        }
    }

    /// Process a raw scancode and return the decoded ASCII character, if any.
    fn process_scancode(&mut self, scancode: u8) -> Option<u8> {
        // Break code (key release).
        if scancode & BREAK_BIT != 0 {
            let make = scancode & !BREAK_BIT;
            if make == SCANCODE_LSHIFT || make == SCANCODE_RSHIFT {
                self.shift = false;
            }
            return None;
        }

        // Make code (key press).
        match scancode {
            SCANCODE_LSHIFT | SCANCODE_RSHIFT => {
                self.shift = true;
                None
            }
            0x3A => {
                // Caps Lock toggle.
                self.caps_lock = !self.caps_lock;
                None
            }
            _ => {
                if (scancode as usize) < SC1_LOWER.len() {
                    let lower = SC1_LOWER[scancode as usize];
                    let upper = SC1_UPPER[scancode as usize];
                    if lower == 0 {
                        return None;
                    }
                    // Shift XOR CapsLock determines case for letters.
                    let use_upper = if lower >= b'a' && lower <= b'z' {
                        self.shift != self.caps_lock
                    } else {
                        self.shift
                    };
                    Some(if use_upper { upper } else { lower })
                } else {
                    None
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in kb_driver!");
    process::exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("[kb_driver] Keyboard driver starting...");

    // The IRQ 1 event handle is passed in RDI by the kernel.
    // For now, we assume the kernel has set up IRQ 1 forwarding and
    // we read scancodes directly from port 0x60 after waiting for
    // the IRQ event via the port-based approach.
    //
    // Since we don't have the IRQ event handle passed to us yet,
    // we use the direct port polling approach: read port 0x60
    // and process scancodes in a loop.
    let _ = console::writeln("[kb_driver] Polling keyboard port 0x60...");

    let mut state = KeyboardState::new();

    loop {
        // Read scancode from the keyboard data port.
        // The kernel's IRQ handler reads port 0x60 and stores the raw
        // scancode in the IrqEvent. When sys_irq_wait returns, the
        // return value IS the scancode byte. We use port_in as a
        // fallback/alternative approach.
        match device::port_in(KB_DATA_PORT, 1) {
            Ok(scancode) => {
                let sc = scancode as u8;
                // Skip invalid/zero scancodes.
                if sc == 0 {
                    // Brief yield to avoid busy-spinning when no key is pressed.
                    openos_sdk::thread::yield_();
                    continue;
                }

                if let Some(ch) = state.process_scancode(sc) {
                    // Send the decoded character to the console service.
                    let msg = [ch];
                    // Try to send to the console service if it exists.
                    if let Ok(console_handle) = service::discover("console") {
                        let _ = channel::send(console_handle, &msg);
                    } else {
                        // Fallback: write directly to the debug console.
                        let _ = console::write(core::str::from_utf8(&[ch]).unwrap_or("?"));
                    }
                }
            }
            Err(_) => {
                // Port read failed — yield and retry.
                openos_sdk::thread::yield_();
            }
        }
    }
}
