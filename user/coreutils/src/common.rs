//! Common utilities shared across all coreutils commands.

#![allow(dead_code)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::str;

/// Simple bump allocator (64 KiB heap) for coreutils.
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 65536]>,
    offset: core::cell::Cell<usize>,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut off = self.offset.get();
        off = (off + align - 1) & !(align - 1);
        if off + size > 65536 {
            return core::ptr::null_mut();
        }
        let ptr = (*self.heap.get()).as_mut_ptr().add(off);
        self.offset.set(off + size);
        ptr
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: no-op dealloc.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0u8; 65536]),
    offset: core::cell::Cell::new(0),
};

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

/// Get command-line arguments.
///
/// In OpenOS, arguments are stored in the `__ARGS__` environment variable
/// by the shell before launching a program. Returns an iterator over the
/// space-separated arguments (skipping the program name).
pub fn args() -> ArgsIter {
    let raw = openos_sdk::env::get("__ARGS__").ok().flatten();
    ArgsIter { data: raw, pos: 0 }
}

/// Iterator over space-separated arguments from `__ARGS__`.
pub struct ArgsIter {
    data: Option<alloc::string::String>,
    pos: usize,
}

impl Iterator for ArgsIter {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        let data = self.data.as_ref()?;
        let bytes = data.as_bytes();

        // Skip whitespace.
        while self.pos < bytes.len() && bytes[self.pos] == b' ' {
            self.pos += 1;
        }

        if self.pos >= bytes.len() {
            return None;
        }

        let start = self.pos;
        while self.pos < bytes.len() && bytes[self.pos] != b' ' {
            self.pos += 1;
        }

        // SAFETY: We're extending the lifetime of a string slice that's stored
        // in the ArgsIter. The data lives as long as the iterator.
        let slice = unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                bytes.as_ptr().add(start),
                self.pos - start,
            ))
        };
        Some(slice)
    }
}

/// Format an error message with a path argument to stderr.
pub fn stderr_fmt(msg: &str, path: &str) {
    stderr(msg);
    stderr(": ");
    stderrln(path);
}

/// Copy all data from one fd to another. Returns bytes copied or error.
pub fn copy_fd(src_fd: u64, dst_fd: u64) -> Result<usize, i32> {
    let mut total = 0usize;
    let mut buf = [0u8; 2048];
    loop {
        match openos_sdk::fs::read(src_fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                openos_sdk::fs::write(dst_fd, &buf[..n]).map_err(|_| -1i32)?;
                total += n;
            }
            Err(_) => return Err(-1),
        }
    }
    Ok(total)
}
