//! comm — compare two sorted files line by line
//!
//! Usage: comm [-1] [-2] [-3] FILE1 FILE2
//!
//! Outputs three columns:
//!   Column 1: lines unique to FILE1
//!   Column 2: lines unique to FILE2
//!   Column 3: lines common to both
//!
//! Options:
//!   -1  suppress column 1 (lines unique to FILE1)
//!   -2  suppress column 2 (lines unique to FILE2)
//!   -3  suppress column 3 (lines common to both)
//!
//! Columns are separated by tabs.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use core::alloc::{GlobalAlloc, Layout};

use common::{args, exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

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
    let mut suppress = [false; 3];
    let mut files = alloc::vec::Vec::new();

    for arg in args() {
        match arg {
            "-1" => suppress[0] = true,
            "-2" => suppress[1] = true,
            "-3" => suppress[2] = true,
            _ => files.push(arg),
        }
    }

    if files.len() < 2 {
        stderrln("comm: missing operands");
        exit(1);
    }

    let path1 = files[0];
    let path2 = files[1];

    let fd1 = match fs::open(path1) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("comm: cannot open file1");
            exit(1);
        }
    };
    let fd2 = match fs::open(path2) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("comm: cannot open file2");
            let _ = fs::close(fd1);
            exit(1);
        }
    };

    let mut buf1 = [0u8; 8192];
    let mut buf2 = [0u8; 8192];
    let n1 = fs::read(fd1, &mut buf1).unwrap_or(0);
    let n2 = fs::read(fd2, &mut buf2).unwrap_or(0);
    let _ = fs::close(fd1);
    let _ = fs::close(fd2);

    let data1 = &buf1[..n1];
    let data2 = &buf2[..n2];

    // Extract lines from both files.
    let mut lines1: [&str; 512] = [""; 512];
    let mut lines2: [&str; 512] = [""; 512];
    let count1 = extract_lines(data1, &mut lines1);
    let count2 = extract_lines(data2, &mut lines2);

    // Both files are assumed sorted; merge-iterate.
    let mut i1 = 0usize;
    let mut i2 = 0usize;

    while i1 < count1 || i2 < count2 {
        if i1 >= count1 {
            // Only file2 remains.
            if !suppress[1] {
                if !suppress[0] {
                    stdout("\t");
                }
                stdoutln(lines2[i2]);
            }
            i2 += 1;
        } else if i2 >= count2 {
            // Only file1 remains.
            if !suppress[0] {
                stdoutln(lines1[i1]);
            }
            i1 += 1;
        } else {
            let cmp = lines1[i1].cmp(lines2[i2]);
            match cmp {
                core::cmp::Ordering::Less => {
                    if !suppress[0] {
                        stdoutln(lines1[i1]);
                    }
                    i1 += 1;
                }
                core::cmp::Ordering::Greater => {
                    if !suppress[1] {
                        if !suppress[0] {
                            stdout("\t");
                        }
                        stdoutln(lines2[i2]);
                    }
                    i2 += 1;
                }
                core::cmp::Ordering::Equal => {
                    if !suppress[2] {
                        if !suppress[0] {
                            stdout("\t");
                        }
                        if !suppress[1] {
                            stdout("\t");
                        }
                        stdoutln(lines1[i1]);
                    }
                    i1 += 1;
                    i2 += 1;
                }
            }
        }
    }
    exit(0);
}

fn extract_lines<'a>(data: &'a [u8], lines: &mut [&'a str; 512]) -> usize {
    let mut count = 0;
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            if count < 512 {
                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                    lines[count] = line;
                    count += 1;
                }
            }
            start = i + 1;
        }
    }
    if start < data.len() && count < 512 {
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
