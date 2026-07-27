//! xargs — build and execute command lines from stdin
//!
//! Usage: xargs [command] [initial-args...]
//!
//! Reads whitespace-separated tokens from stdin and appends them as
//! arguments to the given command, then executes it.
//!
//! If no command is given, defaults to "echo".

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use core::alloc::{GlobalAlloc, Layout};

use common::{args, exit, stderrln};
use openos_sdk::{console, process};

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
    let mut command = alloc::string::String::new();
    let mut initial_args = alloc::string::String::new();
    let mut first = true;

    for arg in args() {
        if first {
            command.push_str(arg);
            first = false;
        } else {
            if !initial_args.is_empty() {
                initial_args.push(' ');
            }
            initial_args.push_str(arg);
        }
    }

    if command.is_empty() {
        command.push_str("echo");
    }

    // Read stdin into a buffer.
    let mut stdin_buf = [0u8; 4096];
    let mut stdin_data = alloc::vec::Vec::new();
    loop {
        match console::read(&mut stdin_buf, false) {
            Ok(0) => break,
            Ok(n) => {
                stdin_data.extend_from_slice(&stdin_buf[..n]);
            }
            Err(_) => break,
        }
    }

    // If no stdin data and no initial args, nothing to do.
    if stdin_data.is_empty() && initial_args.is_empty() {
        exit(0);
    }

    // Build the full argument string.
    let mut full_args = alloc::string::String::new();
    if !initial_args.is_empty() {
        full_args.push_str(&initial_args);
    }

    // Append stdin tokens.
    if let Ok(stdin_str) = core::str::from_utf8(&stdin_data) {
        for token in stdin_str.split_whitespace() {
            if !full_args.is_empty() {
                full_args.push(' ');
            }
            full_args.push_str(token);
        }
    }

    // Execute the command via process create/start.
    match process::create(&command) {
        Ok(task_id) => {
            // Set __ARGS__ so the child can read its arguments.
            let _ = openos_sdk::env::set("__ARGS__", &full_args);
            if process::start(task_id, &command).is_err() {
                stderrln("xargs: failed to start command");
                exit(1);
            }
            let exit_code = process::wait(task_id, 5000).unwrap_or(1);
            exit(exit_code as i32);
        }
        Err(_) => {
            stderrln("xargs: failed to create process");
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
