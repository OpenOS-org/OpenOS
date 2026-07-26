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
//! ## Extended Keys
//!
//! Extended scancodes (prefixed with 0xE0) are decoded by `pc-keyboard` and
//! returned as `DecodedKey::RawKey(KeyCode::...)`. We map these to ANSI escape
//! sequences so that terminal applications can interpret arrow keys, function
//! keys, and navigation keys.
//!
//! ## Key Repeat
//!
//! When a key is held down, the driver repeats it after an initial delay
//! (~500 ms) and then at ~55 ms intervals (one timer tick at 18.2 Hz).
//! Repeat is polled from the blocking `read()` loop to avoid deadlock
//! with the keyboard IRQ handler (both share the `KEYBOARD` lock).
//!
//! ## Limitations
//!
//! - US QWERTY layout only (no international layouts)
//! - No caps lock LED control

use alloc::collections::VecDeque;

use pc_keyboard::{layouts, DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1};
use spin::Mutex;

/// Maximum input buffer size in bytes.
const INPUT_BUFFER_SIZE: usize = 256;

/// Number of timer ticks before key repeat starts (~500 ms at 18.2 Hz).
/// 18.2 Hz ≈ 55 ms per tick, so 9 ticks ≈ 495 ms.
const REPEAT_DELAY_TICKS: u64 = 9;

/// Number of timer ticks between repeated key events (~55 ms at 18.2 Hz).
/// One tick is the fastest we can repeat given the PIT resolution.
const REPEAT_INTERVAL_TICKS: u64 = 1;

/// Global keyboard state — protected by a spinlock because the interrupt
/// handler and syscall handler run on different stacks/contexts.
///
/// The keyboard object stores the scancode decoder state (shift, caps lock, etc.).
/// The buffer stores decoded ASCII characters ready for consumption by `sys_read`.
static KEYBOARD: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());

/// Keyboard state including the decoder, input buffer, and repeat tracking.
struct KeyboardState {
    /// The `pc-keyboard` decoder — tracks modifier state (shift, ctrl, etc.)
    /// and translates scancodes to key events.
    keyboard: Keyboard<layouts::Us104Key, ScancodeSet1>,
    /// Decoded character buffer. Characters are pushed at the back (by the
    /// interrupt handler) and popped from the front (by `sys_read`).
    buffer: VecDeque<u8>,
    /// Currently held key for repeat. `None` if no key is being held.
    held_key: Option<HeldKey>,
}

/// Tracks a key that is currently held down for repeat purposes.
struct HeldKey {
    /// The key code of the held key.
    code: KeyCode,
    /// The `TICKS` value when the key was first pressed.
    pressed_tick: u64,
    /// The `TICKS` value when the last repeat event was generated.
    last_repeat_tick: u64,
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
            held_key: None,
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
/// - Extended keys (arrows, function keys, navigation) are mapped to ANSI
///   escape sequences
/// - Key repeat state is tracked for held keys
pub fn process_scancode(scancode: u8) {
    let mut state = KEYBOARD.lock();

    if let Ok(Some(key_event)) = state.keyboard.add_byte(scancode) {
        // Track key release to clear held_key state.
        if key_event.state == pc_keyboard::KeyState::Up {
            // If the released key matches the held key, clear repeat state.
            if let Some(ref held) = state.held_key {
                if held.code == key_event.code {
                    state.held_key = None;
                }
            }
        }

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
                        // ESC character (0x1B) — push directly
                        '\x1B' => {
                            state.push(0x1B);
                        }
                        // Delete (0x7F) — push directly
                        '\x7F' => {
                            state.push(0x7F);
                        }
                        // Other control characters (ignore)
                        _ => {}
                    }
                }
                DecodedKey::RawKey(raw_key) => {
                    push_raw_key(&mut state, raw_key);
                }
            }
        }
    }
}

/// Map a `KeyCode` to its ANSI escape sequence and push it into the buffer.
///
/// Extended keys (arrows, navigation, function keys) produce multi-byte
/// ANSI escape sequences that terminal applications can interpret.
fn push_raw_key(state: &mut KeyboardState, code: KeyCode) {
    // ANSI escape sequences use ESC (0x1B) as the prefix.
    const ESC: u8 = 0x1B;

    // Track the key for repeat if it is a repeatable extended key.
    let is_repeatable = matches!(
        code,
        KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::ArrowLeft
            | KeyCode::ArrowRight
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Insert
            | KeyCode::Delete
    );

    if is_repeatable {
        let now =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        state.held_key = Some(HeldKey {
            code,
            pressed_tick: now,
            last_repeat_tick: now,
        });
    }

    let seq: &[u8] = match code {
        // Arrow keys: ESC [ A/B/C/D
        KeyCode::ArrowUp => &[ESC, b'[', b'A'],
        KeyCode::ArrowDown => &[ESC, b'[', b'B'],
        KeyCode::ArrowRight => &[ESC, b'[', b'C'],
        KeyCode::ArrowLeft => &[ESC, b'[', b'D'],
        // Home/End: ESC [ H / ESC [ F
        KeyCode::Home => &[ESC, b'[', b'H'],
        KeyCode::End => &[ESC, b'[', b'F'],
        // Tilde sequences: ESC [ <param> ~
        KeyCode::PageUp => &[ESC, b'[', b'5', b'~'],
        KeyCode::PageDown => &[ESC, b'[', b'6', b'~'],
        KeyCode::Insert => &[ESC, b'[', b'2', b'~'],
        KeyCode::Delete => &[ESC, b'[', b'3', b'~'],
        // Function keys: ESC [ <param> ~ (F1-F4 use SS3, but CSI is more
        // common in modern terminals and avoids SS3/CSI ambiguity)
        KeyCode::F1 => &[ESC, b'[', b'1', b'1', b'~'],
        KeyCode::F2 => &[ESC, b'[', b'1', b'2', b'~'],
        KeyCode::F3 => &[ESC, b'[', b'1', b'3', b'~'],
        KeyCode::F4 => &[ESC, b'[', b'1', b'4', b'~'],
        KeyCode::F5 => &[ESC, b'[', b'1', b'5', b'~'],
        KeyCode::F6 => &[ESC, b'[', b'1', b'7', b'~'],
        KeyCode::F7 => &[ESC, b'[', b'1', b'8', b'~'],
        KeyCode::F8 => &[ESC, b'[', b'1', b'9', b'~'],
        KeyCode::F9 => &[ESC, b'[', b'2', b'0', b'~'],
        KeyCode::F10 => &[ESC, b'[', b'2', b'1', b'~'],
        KeyCode::F11 => &[ESC, b'[', b'2', b'3', b'~'],
        KeyCode::F12 => &[ESC, b'[', b'2', b'4', b'~'],
        // Unmapped raw keys — ignore silently.
        _ => return,
    };

    for &byte in seq {
        state.push(byte);
    }
}

/// Read up to `len` bytes from the keyboard input buffer into `dst`.
///
/// Returns the number of bytes actually read. If the buffer is empty,
/// returns 0 (non-blocking) or blocks until data is available depending
/// on the `blocking` parameter.
///
/// When blocking, key repeat is checked on each wake: if a key is held
/// and enough timer ticks have elapsed, the key's ANSI escape sequence
/// is re-injected into the buffer before returning.
///
/// # Safety
/// - `dst` must point to a valid, writable buffer of at least `len` bytes
/// - The caller must ensure the pointer is in user-space (for syscall use)
pub unsafe fn read(dst: *mut u8, len: usize, blocking: bool) -> usize {
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
                // Check for key repeat before testing the buffer. This handles
                // the case where a key is held and the timer has ticked enough
                // to warrant a repeat, but no new scancode IRQ has fired.
                check_and_repeat();
                let state = KEYBOARD.lock();
                if !state.is_empty() {
                    break;
                }
            }
            // Sleep until an interrupt wakes us (timer or keyboard IRQ).
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

/// Check if a held key should be repeated and inject the ANSI sequence.
///
/// Called from `read()` during the blocking loop. This avoids calling from
/// the timer interrupt handler, which could deadlock if the keyboard IRQ
/// handler is interrupted while holding the `KEYBOARD` lock.
fn check_and_repeat() {
    let mut state = KEYBOARD.lock();
    let held = match state.held_key {
        Some(ref h) => (h.code, h.pressed_tick, h.last_repeat_tick),
        None => return,
    };

    let (code, pressed_tick, last_repeat_tick) = held;
    let now = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    // Not yet past the initial delay.
    let elapsed_since_press = now.saturating_sub(pressed_tick);
    if elapsed_since_press < REPEAT_DELAY_TICKS {
        return;
    }

    // Not yet past the repeat interval.
    let elapsed_since_last = now.saturating_sub(last_repeat_tick);
    if elapsed_since_last < REPEAT_INTERVAL_TICKS {
        return;
    }

    // Update the last repeat tick before pushing, so even if push drops
    // bytes we don't flood the buffer.
    if let Some(ref mut h) = state.held_key {
        h.last_repeat_tick = now;
    }

    // Re-inject the ANSI sequence for the held key.
    push_raw_key(&mut state, code);
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
        assert!(state.held_key.is_none());
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

    /// Helper to drain the buffer into a `Vec<u8>`.
    fn drain_buffer(state: &mut KeyboardState) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(b) = state.pop() {
            out.push(b);
        }
        out
    }

    #[test]
    fn test_arrow_up_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::ArrowUp);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'A']);
    }

    #[test]
    fn test_arrow_down_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::ArrowDown);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'B']);
    }

    #[test]
    fn test_arrow_right_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::ArrowRight);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'C']);
    }

    #[test]
    fn test_arrow_left_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::ArrowLeft);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'D']);
    }

    #[test]
    fn test_home_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::Home);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'H']);
    }

    #[test]
    fn test_end_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::End);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'F']);
    }

    #[test]
    fn test_page_up_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::PageUp);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'5', b'~']);
    }

    #[test]
    fn test_page_down_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::PageDown);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'6', b'~']);
    }

    #[test]
    fn test_insert_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::Insert);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'2', b'~']);
    }

    #[test]
    fn test_delete_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::Delete);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'3', b'~']);
    }

    #[test]
    fn test_f1_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::F1);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'1', b'1', b'~']);
    }

    #[test]
    fn test_f5_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::F5);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'1', b'5', b'~']);
    }

    #[test]
    fn test_f12_escape_sequence() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::F12);
        let bytes = drain_buffer(&mut state);
        assert_eq!(bytes, vec![0x1B, b'[', b'2', b'4', b'~']);
    }

    #[test]
    fn test_unmapped_raw_key_ignored() {
        let mut state = KeyboardState::new();
        // LShift is a modifier — should not produce any output.
        push_raw_key(&mut state, KeyCode::LShift);
        assert!(state.is_empty());
    }

    #[test]
    fn test_repeatable_key_sets_held_key() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::ArrowUp);
        assert!(state.held_key.is_some());
        assert_eq!(state.held_key.as_ref().unwrap().code, KeyCode::ArrowUp);
    }

    #[test]
    fn test_non_repeatable_key_does_not_set_held_key() {
        let mut state = KeyboardState::new();
        push_raw_key(&mut state, KeyCode::F1);
        // F1 is not in the repeatable set — it's a function key.
        assert!(state.held_key.is_none());
    }

    #[test]
    fn test_repeat_delay_constants() {
        // Initial delay should be around 500 ms (9 ticks at 18.2 Hz ≈ 495 ms).
        assert!(REPEAT_DELAY_TICKS >= 5);
        assert!(REPEAT_DELAY_TICKS <= 15);
        // Repeat interval should be at least 1 tick.
        assert!(REPEAT_INTERVAL_TICKS >= 1);
    }

    #[test]
    fn test_function_key_sequences() {
        // All F-keys should produce distinct sequences.
        let f_keys = [
            KeyCode::F1,
            KeyCode::F2,
            KeyCode::F3,
            KeyCode::F4,
            KeyCode::F5,
            KeyCode::F6,
            KeyCode::F7,
            KeyCode::F8,
            KeyCode::F9,
            KeyCode::F10,
            KeyCode::F11,
            KeyCode::F12,
        ];
        let mut sequences = Vec::new();
        for key in &f_keys {
            let mut state = KeyboardState::new();
            push_raw_key(&mut state, *key);
            let bytes = drain_buffer(&mut state);
            // All should start with ESC [ and end with ~
            assert!(
                bytes.starts_with(&[0x1B, b'[']),
                "{key:?} missing CSI prefix"
            );
            assert_eq!(bytes.last(), Some(&b'~'), "{key:?} missing ~ suffix");
            sequences.push(bytes);
        }
        // All sequences should be unique.
        sequences.sort();
        sequences.dedup();
        assert_eq!(sequences.len(), 12, "F-key sequences are not all unique");
    }
}
