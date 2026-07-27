//! split — split a file into pieces
//!
//! Usage: split [-l N] [file [prefix]]
//!
//! Splits a file into N-line pieces. Default is 1000 lines per piece.
//! Output files are named `prefix` + "aa", "ab", "ac", etc.
//! Default prefix is "x".

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{exit, stderrln};
use openos_sdk::fs;

const DEFAULT_LINES: usize = 1000;
const DEFAULT_PREFIX: &str = "x";

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

    let (mut lines_per_file, file_idx, prefix) = parse_args(&args);

    if lines_per_file == 0 {
        lines_per_file = DEFAULT_LINES;
    }

    let path = if file_idx < args.len() {
        args[file_idx]
    } else {
        stderrln("split: missing file operand");
        exit(1);
    };

    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("split: cannot open file");
            exit(1);
        }
    };

    let mut buf = [0u8; 16384];
    let total = match fs::read(fd, &mut buf) {
        Ok(n) => n,
        Err(_) => {
            let _ = fs::close(fd);
            stderrln("split: read error");
            exit(1);
        }
    };
    let _ = fs::close(fd);

    let data = &buf[..total];
    let mut line_start = 0usize;
    let mut line_count = 0usize;
    let mut piece_num = 0usize;
    let mut piece_buf = [0u8; 16384];
    let mut piece_pos = 0usize;

    // Iterate through lines and flush every `lines_per_file` lines
    let mut i = 0usize;
    while i <= data.len() {
        if i == data.len() || data[i] == b'\n' {
            let line_end = if i < data.len() { i + 1 } else { i }; // include newline
            let line_len = line_end - line_start;

            // Copy the line into piece buffer
            if piece_pos + line_len <= piece_buf.len() {
                piece_buf[piece_pos..piece_pos + line_len].copy_from_slice(&data[line_start..line_end]);
                piece_pos += line_len;
            }

            line_count += 1;
            line_start = i + 1;

            // Flush when we hit the lines-per-file limit or end of data
            if line_count >= lines_per_file || (i == data.len() && piece_pos > 0) {
                write_piece(prefix, piece_num, &piece_buf[..piece_pos]);
                piece_num += 1;
                line_count = 0;
                piece_pos = 0;
            }

            if i == data.len() {
                break;
            }
        }
        i += 1;
    }

    exit(0);
}

fn write_piece(prefix: &str, num: usize, data: &[u8]) {
    // Generate suffix: aa, ab, ac, ... zz, aaa, aab, ...
    let suffix = make_suffix(num);
    let mut out_path = [0u8; 256];
    let mut pos = 0;

    let prefix_bytes = prefix.as_bytes();
    out_path[pos..pos + prefix_bytes.len()].copy_from_slice(prefix_bytes);
    pos += prefix_bytes.len();
    out_path[pos..pos + suffix.len()].copy_from_slice(&suffix);
    pos += suffix.len();

    let path_str = core::str::from_utf8(&out_path[..pos]).unwrap_or("xaa");

    match fs::open(path_str) {
        Ok(fd) => {
            let _ = fs::write(fd, data);
            let _ = fs::close(fd);
        }
        Err(_) => {
            // File doesn't exist — we'd need create, but for now write via open/create
            // Simplified: just try to write (assume filesystem supports create)
            // In practice we'd use a proper create syscall. For demo, skip.
            stderrln("split: cannot create output file");
        }
    }
}

fn make_suffix(num: usize) -> [u8; 3] {
    // Two-letter suffix: aa = 0, ab = 1, ..., zz = 675
    let first = (num / 26) as u8;
    let second = (num % 26) as u8;
    [b'a' + first.min(25), b'a' + second, 0]
}

fn parse_args<'a>(args: &'a [&'a str]) -> (usize, usize, &'a str) {
    let mut lines_per_file = DEFAULT_LINES;
    let mut prefix = DEFAULT_PREFIX;
    let mut file_arg_idx = 0usize;
    let mut i = 0usize;

    while i < args.len() {
        if args[i] == "-l" && i + 1 < args.len() {
            i += 1;
            lines_per_file = args[i].parse().unwrap_or(DEFAULT_LINES);
            i += 1;
        } else if file_arg_idx == 0 {
            file_arg_idx = i;
            i += 1;
        } else if prefix == DEFAULT_PREFIX {
            prefix = args[i];
            i += 1;
        } else {
            i += 1;
        }
    }

    (lines_per_file, file_arg_idx, prefix)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
