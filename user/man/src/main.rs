//! man — display manual pages
//!
//! Usage: man [command]
//!
//! Without arguments, lists all available manual pages in /disk/man/.
//! With an argument, reads and displays the corresponding page.
//!
//! Manual pages are plain text files stored in /disk/man/<command>.txt
//! with sections: NAME, SYNOPSIS, DESCRIPTION, EXAMPLES.
//!
//! Examples:
//!   man          — list available pages
//!   man ls       — display the ls(1) manual page

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, env, fs, process};

// ---------------------------------------------------------------------------
// Allocator
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in man!");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAN_DIR: &str = "/disk/man";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write(s: &str) {
    let _ = console::write(s);
}

fn writeln(s: &str) {
    let _ = console::writeln(s);
}

/// Parse the first argument from `__ARGS__`.
fn get_page_name() -> Option<alloc::string::String> {
    let raw = env::get("__ARGS__").ok().flatten()?;
    let args = raw.trim();
    if args.is_empty() {
        return None;
    }
    // Take the first token.
    let end = args.find(' ').unwrap_or(args.len());
    Some(alloc::string::String::from(&args[..end]))
}

/// Read the entire contents of a file into a heap-allocated byte vector.
fn read_file(path: &str) -> Result<alloc::vec::Vec<u8>, ()> {
    let fd = fs::open(path).map_err(|_| ())?;
    let mut data = alloc::vec::Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match fs::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(_) => {
                let _ = fs::close(fd);
                return Err(());
            }
        }
    }
    let _ = fs::close(fd);
    Ok(data)
}

// ---------------------------------------------------------------------------
// Display manual page
// ---------------------------------------------------------------------------

fn display_page(name: &str) {
    let path = alloc::format!("{}/{}.txt", MAN_DIR, name);
    match read_file(&path) {
        Ok(data) => {
            // Parse and render sections with formatting.
            let text = core::str::from_utf8(&data).unwrap_or("");
            let mut in_section = false;

            for line in text.lines() {
                let trimmed = line.trim();

                // Detect section headers (lines that are all-uppercase section names).
                if is_section_header(trimmed) {
                    writeln("");
                    // Bold section header using ANSI escape.
                    write("\x1b[1m");
                    writeln(trimmed);
                    write("\x1b[0m");
                    in_section = true;
                } else if trimmed.starts_with('-') && trimmed.contains(' ') && in_section {
                    // Option lines: highlight the option flag.
                    write("  ");
                    if let Some(pos) = trimmed.find(' ') {
                        write("\x1b[1m");
                        write(&trimmed[..pos]);
                        write("\x1b[0m");
                        writeln(&trimmed[pos..]);
                    } else {
                        writeln(trimmed);
                    }
                } else {
                    writeln(line);
                }
            }
            writeln("");
        }
        Err(()) => {
            write("man: no manual entry for ");
            writeln(name);
            writeln("");
            writeln("Available pages:");
            list_pages();
        }
    }
}

/// Check if a line looks like a section header (e.g., NAME, SYNOPSIS, etc.).
fn is_section_header(line: &str) -> bool {
    if line.is_empty() || line.len() > 40 {
        return false;
    }
    // Must be all uppercase ASCII letters, digits, spaces, or colons.
    line.bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ' || b == b':')
        && line.bytes().any(|b| b.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// List available pages
// ---------------------------------------------------------------------------

fn list_pages() {
    match fs::open(MAN_DIR) {
        Ok(fd) => {
            let mut buf = [0u8; 4096];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut i = 0;
                    let mut count = 0;
                    while i < data.len() {
                        let end = data[i..]
                            .iter()
                            .position(|&b| b == 0)
                            .unwrap_or(data.len() - i);
                        if end > 0 {
                            if let Ok(name) = core::str::from_utf8(&data[i..i + end]) {
                                // Strip .txt suffix for display.
                                let display = if let Some(stripped) = name.strip_suffix(".txt") {
                                    stripped
                                } else {
                                    name
                                };
                                if !display.is_empty() {
                                    write("  ");
                                    writeln(display);
                                    count += 1;
                                }
                            }
                        }
                        i += end + 1;
                    }
                    if count == 0 {
                        writeln("  (no manual pages found)");
                    }
                }
                Err(_) => {
                    writeln("man: error reading manual page directory");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            writeln("man: manual page directory not found (/disk/man/)");
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    match get_page_name() {
        Some(name) => display_page(&name),
        None => {
            writeln("Available manual pages:");
            writeln("");
            list_pages();
        }
    }
    process::exit(0);
}
