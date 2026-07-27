//! column — columnate lists
//!
//! Usage: column [file]
//!
//! Reads lines from a file (or stdin placeholder) and formats them into
//! columns filling rows first (left-to-right, top-to-bottom).

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use core::alloc::{GlobalAlloc, Layout};

use common::{exit, stdout_byte, stdout_bytes, stderrln};
use openos_sdk::fs;

const TERM_WIDTH: usize = 80;

/// Simple bump allocator for user-space (64 KiB heap).
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let args: alloc::vec::Vec<&str> = common::args().collect();

    let path = if !args.is_empty() { args[0] } else { "/disk/test.txt" };

    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("column: cannot open file");
            exit(1);
        }
    };

    let mut buf = [0u8; 8192];
    let n = match fs::read(fd, &mut buf) {
        Ok(n) => n,
        Err(_) => {
            let _ = fs::close(fd);
            stderrln("column: read error");
            exit(1);
        }
    };
    let _ = fs::close(fd);

    let data = &buf[..n];

    // Extract all lines into a fixed-size array
    let mut lines: [&str; 512] = [""; 512];
    let count = extract_lines(data, &mut lines);

    if count == 0 {
        exit(0);
    }

    // Find the longest line width
    let max_width = lines[..count]
        .iter()
        .map(|l| l.len())
        .max()
        .unwrap_or(0);

    // Column width = max_width + 2 spaces padding
    let col_width = (max_width + 2).min(TERM_WIDTH);
    let cols_per_row = if col_width > 0 {
        let c = TERM_WIDTH / col_width;
        if c < 1 { 1 } else { c }
    } else {
        1
    };
    let rows = (count + cols_per_row - 1) / cols_per_row;

    // Print in row-major order (fill rows)
    for row in 0..rows {
        for col in 0..cols_per_row {
            let idx = row + col * rows;
            if idx < count {
                let line = lines[idx];
                stdout_bytes(line.as_bytes());
                // Pad to col_width
                if col < cols_per_row - 1 {
                    let padding = col_width - line.len().min(col_width);
                    for _ in 0..padding {
                        stdout_byte(b' ');
                    }
                }
            }
        }
        stdout_byte(b'\n');
    }

    exit(0);
}

/// Extract lines from a byte buffer into a fixed-size array.
fn extract_lines<'a>(data: &'a [u8], lines: &mut [&'a str; 512]) -> usize {
    let mut count = 0;
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            if count < 512 {
                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                    // Skip empty lines
                    if !line.is_empty() {
                        lines[count] = line;
                        count += 1;
                    }
                }
            }
            start = i + 1;
        }
    }
    if start < data.len() && count < 512 {
        if let Ok(line) = core::str::from_utf8(&data[start..]) {
            if !line.is_empty() {
                lines[count] = line;
                count += 1;
            }
        }
    }
    count
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
