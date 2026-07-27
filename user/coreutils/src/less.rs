//! less — simplified pager
//!
//! Usage: less [file]
//!
//! Reads a file and displays it page by page.
//! Navigation:
//!   Enter/Space  next page
//!   q            quit
//!   h            help

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use core::alloc::{GlobalAlloc, Layout};

use common::{args, exit, format_u64, stderrln, stdout, stdoutln};
use openos_sdk::{console, fs};

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

/// Default page size (lines per page).
const PAGE_LINES: usize = 20;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut path: Option<&str> = None;

    for arg in args() {
        path = Some(arg);
        break;
    }

    let Some(file_path) = path else {
        stderrln("less: missing file operand");
        exit(1);
    };

    let fd = match fs::open(file_path) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("less: cannot open file");
            exit(1);
        }
    };

    // Read the entire file into a buffer.
    let mut file_buf = [0u8; 16384];
    let total_bytes = fs::read(fd, &mut file_buf).unwrap_or(0);
    let _ = fs::close(fd);
    let data = &file_buf[..total_bytes];

    // Split into lines.
    let mut lines: [&str; 1024] = [""; 1024];
    let line_count = split_lines(data, &mut lines);

    if line_count == 0 {
        exit(0);
    }

    // Display page by page.
    let mut offset = 0usize;

    loop {
        // Show current page.
        let end = if offset + PAGE_LINES < line_count {
            offset + PAGE_LINES
        } else {
            line_count
        };

        for i in offset..end {
            stdoutln(lines[i]);
        }

        // Show status line.
        stdout("--MORE-- (");
        let mut num_buf = [0u8; 20];
        let s = format_u64(end as u64, &mut num_buf);
        stdout(core::str::from_utf8(s).unwrap_or("?"));
        stdout("/");
        let s = format_u64(line_count as u64, &mut num_buf);
        stdout(core::str::from_utf8(s).unwrap_or("?"));
        stdout(" lines, q to quit)");

        // Read a key.
        let mut key = [0u8; 1];
        match console::read(&mut key, true) {
            Ok(0) => break,
            Ok(_) => {
                match key[0] {
                    b'q' | b'Q' => {
                        stdout("\n");
                        break;
                    }
                    b'\n' | b'\r' | b' ' => {
                        stdout("\n");
                        if end >= line_count {
                            break;
                        }
                        offset = end;
                    }
                    b'h' | b'H' => {
                        stdout("\n");
                        stdoutln("  h/H  help");
                        stdoutln("  q/Q  quit");
                        stdoutln("  SPACE/Enter  next page");
                    }
                    _ => {
                        // Any other key advances one page.
                        stdout("\n");
                        if end >= line_count {
                            break;
                        }
                        offset = end;
                    }
                }
            }
            Err(_) => break,
        }
    }
    exit(0);
}

fn split_lines<'a>(data: &'a [u8], lines: &mut [&'a str; 1024]) -> usize {
    let mut count = 0;
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            if count < 1024 {
                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                    lines[count] = line;
                    count += 1;
                }
            }
            start = i + 1;
        }
    }
    // Last line without trailing newline.
    if start < data.len() && count < 1024 {
        if let Ok(line) = core::str::from_utf8(&data[start..]) {
            lines[count] = line;
            count += 1;
        }
    }
    count
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
