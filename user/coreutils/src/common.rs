//! Common utilities shared across all coreutils commands.

#![allow(dead_code)]

use core::str;

/// Parse command-line arguments from the raw args buffer.
/// Returns a vector of argument strings (skipping argv[0] which is the program name).
pub fn parse_args() -> ([u8; 4096], usize) {
    let mut buf = [0u8; 4096];
    // In OpenOS, args are passed via the console. For now, return empty.
    (buf, 0)
}

/// Write a string to stdout (console).
pub fn stdout(s: &str) {
    let _ = openos_sdk::console::write(s);
}

/// Write a string followed by newline to stdout.
pub fn stdoutln(s: &str) {
    let _ = openos_sdk::console::writeln(s);
}

/// Write a byte to stdout.
pub fn stdout_byte(b: u8) {
    let s = [b];
    let _ = openos_sdk::console::write(core::str::from_utf8(&s).unwrap_or("?"));
}

/// Write bytes to stdout.
pub fn stdout_bytes(data: &[u8]) {
    if let Ok(s) = core::str::from_utf8(data) {
        let _ = openos_sdk::console::write(s);
    }
}

/// Write a string to stderr (using console for now).
pub fn stderr(s: &str) {
    let _ = openos_sdk::console::write(s);
}

/// Write a string followed by newline to stderr.
pub fn stderrln(s: &str) {
    let _ = openos_sdk::console::writeln(s);
}

/// Format a u64 as decimal string into buffer, returns the formatted slice.
pub fn format_u64(val: u64, buf: &mut [u8; 20]) -> &[u8] {
    if val == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut tmp = val;
    let mut pos = 19;
    while tmp > 0 {
        buf[pos] = b'0' + (tmp % 10) as u8;
        tmp /= 10;
        if pos == 0 {
            break;
        }
        pos -= 1;
    }
    &buf[pos..20]
}

/// Format a u64 as hex string into buffer, returns the formatted slice.
pub fn format_hex(val: u64, buf: &mut [u8; 18]) -> &[u8] {
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..16 {
        let nibble = (val >> (60 - i * 4)) & 0xF;
        buf[2 + i] = if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'a' + (nibble - 10) as u8
        };
    }
    &buf[..18]
}

/// Count lines, words, and bytes in a buffer.
pub fn count_lwb(data: &[u8]) -> (usize, usize, usize) {
    let mut lines = 0;
    let mut words = 0;
    let mut in_word = false;
    for &b in data {
        if b == b'\n' {
            lines += 1;
        }
        if b.is_ascii_whitespace() {
            if in_word {
                words += 1;
                in_word = false;
            }
        } else {
            in_word = true;
        }
    }
    if in_word {
        words += 1;
    }
    (lines, words, data.len())
}

/// Check if a byte matches a simple pattern (supports * at start/end).
pub fn pattern_match(pattern: &[u8], text: &[u8]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    if pattern == b"*" {
        return true;
    }
    // Simple prefix/suffix matching
    if pattern[0] == b'*' && pattern[pattern.len() - 1] == b'*' {
        let inner = &pattern[1..pattern.len() - 1];
        return find_subsequence(text, inner).is_some();
    }
    if pattern[0] == b'*' {
        let suffix = &pattern[1..];
        return text.ends_with(suffix);
    }
    if pattern[pattern.len() - 1] == b'*' {
        let prefix = &pattern[..pattern.len() - 1];
        return text.starts_with(prefix);
    }
    text == pattern
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Exit the process with a status code.
pub fn exit(code: i32) -> ! {
    openos_sdk::process::exit(code as u64);
}
