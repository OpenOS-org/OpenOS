//! PS/2 keyboard driver.
//!
//! Decodes PS/2 scancodes (set 1) into ASCII characters and stores them
//! in a kernel-global input buffer. The `pc-keyboard` crate handles
//! the complex scancode-to-keycode mapping, including shift/caps lock
//! state tracking.
//!
//! ## Architecture
//!
//! ```text
//!   PS/2 Controller (port 0x60)
//!        │
//!   IRQ 1 interrupt handler
//!        │
//!   ScancodeDecoded (key events)
//!        │
//!   Input buffer (VecDeque<u8>)
//!        │
//!   sys_read() → user-space
//! ```
//!
//! ## Limitations
//!
//! - US QWERTY layout only (no international layouts)
//! - No extended key support (arrows, function keys)
//! - No key repeat handling
//! - No caps lock LED control

use alloc::collections::VecDeque;

use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use spin::Mutex;

/// Maximum input buffer size in bytes.
const INPUT_BUFFER_SIZE: usize = 256;

/// Global keyboard state — protected by a spinlock because the interrupt
/// handler and syscall handler run on different stacks/contexts.
///
/// The keyboard object stores the scancode decoder state (shift, caps lock, etc.).
/// The buffer stores decoded ASCII characters ready for consumption by `sys_read`.
static KEYBOARD: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());

/// Keyboard state including the decoder and input buffer.
struct KeyboardState {
    /// The `pc-keyboard` decoder — tracks modifier state (shift, ctrl, etc.)
    /// and translates scancodes to key events.
    keyboard: Keyboard<layouts::Us104Key, ScancodeSet1>,
    /// Decoded character buffer. Characters are pushed at the back (by the
    /// interrupt handler) and popped from the front (by `sys_read`).
    buffer: VecDeque<u8>,
}

impl KeyboardState {
    /// Create a new keyboard state with an empty buffer.
    ///
    /// `const` because `pc_keyboard::Keyboard::new()` is const-compatible.
    const fn new() -> Self {
        Self {
            keyboard: Keyboard::new(
                ScancodeSet1::new(),
                layouts::Us104Key,
                HandleControl::MapLettersToUnicode,
            ),
            buffer: VecDeque::new(),
        }
    }

    /// Push a byte into the input buffer, dropping the oldest byte if full.
    fn push(&mut self, byte: u8) {
        if self.buffer.len() >= INPUT_BUFFER_SIZE {
            self.buffer.pop_front();
        }
        self.buffer.push_back(byte);
    }

    /// Pop a byte from the input buffer. Returns `None` if empty.
    fn pop(&mut self) -> Option<u8> {
        self.buffer.pop_front()
    }

    /// Check if the buffer is empty.
    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Process a raw PS/2 scancode from the keyboard controller.
///
/// Called from the IRQ 1 interrupt handler in `interrupts.rs`.
/// Decodes the scancode using the US QWERTY layout and pushes the
/// resulting character into the input buffer.
///
/// # Arguments
/// - `scancode`: Raw byte read from port 0x60.
///
/// # Behavior
/// - Printable characters (letters, digits, symbols) are pushed as ASCII bytes
/// - Enter (0x1C) is translated to newline (0x0A)
/// - Backspace (0x0E) is pushed as 0x08 (ASCII backspace)
/// - Key release events are ignored
/// - Modifier key state is tracked internally by `pc-keyboard`
pub fn process_scancode(scancode: u8) {
    let mut state = KEYBOARD.lock();

    if let Ok(Some(key_event)) = state.keyboard.add_byte(scancode) {
        if let Some(key) = state.keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(ch) => {
                    match ch {
                        // Enter → newline
                        '\r' | '\n' => {
                            state.push(b'\n');
                        }
                        // Backspace → ASCII 0x08
                        '\x08' => {
                            // Only pop if there is at least one character in the
                            // buffer. This intentionally guards against popping
                            // from an empty VecDeque — backspace at column 0 is
                            // a no-op.
                            if state.buffer.back().is_some() {
                                state.buffer.pop_back();
                            }
                        }
                        // Regular printable character
                        c if c.is_ascii_graphic() || c == ' ' => {
                            state.push(c as u8);
                        }
                        // Tab
                        '\t' => {
                            state.push(b'\t');
                        }
                        // Other control characters (ignore)
                        _ => {}
                    }
                }
                DecodedKey::RawKey(_raw_key) => {
                    // Function keys, arrows, etc. — not supported yet
                }
            }
        }
    }
}

/// Read up to `len` bytes from the keyboard input buffer into `dst`.
///
/// Returns the number of bytes actually read. If the buffer is empty,
/// returns 0 (non-blocking) or blocks until data is available depending
/// on the `blocking` parameter.
///
/// # Safety
/// - `dst` must point to a valid, writable buffer of at least `len` bytes
/// - The caller must ensure the pointer is in user-space (for syscall use)
pub fn read(dst: *mut u8, len: usize, blocking: bool) -> usize {
    if len == 0 {
        return 0;
    }

    // If non-blocking and buffer is empty, return immediately
    if !blocking {
        let state = KEYBOARD.lock();
        if state.is_empty() {
            return 0;
        }
    }

    // Block until we have at least one byte
    if blocking {
        loop {
            {
                let state = KEYBOARD.lock();
                if !state.is_empty() {
                    break;
                }
            }
            // Sleep until an interrupt wakes us
            x86_64::instructions::hlt();
        }
    }

    // Copy bytes from buffer to destination
    let mut state = KEYBOARD.lock();
    let mut count = 0;
    while count < len {
        match state.pop() {
            Some(byte) => {
                // SAFETY: Caller guarantees dst is valid for `len` bytes.
                unsafe {
                    core::ptr::write(dst.add(count), byte);
                }
                count += 1;
            }
            None => break,
        }
    }

    count
}

/// Check if there are bytes available in the keyboard input buffer.
pub fn has_data() -> bool {
    let state = KEYBOARD.lock();
    !state.is_empty()
}

/// Get the number of bytes currently in the keyboard input buffer.
pub fn buffer_len() -> usize {
    let state = KEYBOARD.lock();
    state.buffer.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyboard_state_new() {
        let state = KeyboardState::new();
        assert!(state.is_empty());
        assert_eq!(state.buffer.len(), 0);
    }

    #[test]
    fn test_keyboard_state_push_pop() {
        let mut state = KeyboardState::new();
        state.push(b'A');
        state.push(b'B');
        state.push(b'C');

        assert_eq!(state.pop(), Some(b'A'));
        assert_eq!(state.pop(), Some(b'B'));
        assert_eq!(state.pop(), Some(b'C'));
        assert_eq!(state.pop(), None);
    }

    #[test]
    fn test_keyboard_state_buffer_full() {
        let mut state = KeyboardState::new();

        // Fill the buffer
        for i in 0..INPUT_BUFFER_SIZE {
            state.push(i as u8);
        }

        assert_eq!(state.buffer.len(), INPUT_BUFFER_SIZE);

        // Push one more — should drop the oldest
        state.push(0xFF);
        assert_eq!(state.buffer.len(), INPUT_BUFFER_SIZE);
        assert_eq!(state.pop(), Some(1)); // The 0 was dropped
    }

    #[test]
    fn test_keyboard_state_is_empty() {
        let mut state = KeyboardState::new();
        assert!(state.is_empty());

        state.push(b'X');
        assert!(!state.is_empty());

        state.pop();
        assert!(state.is_empty());
    }
}
