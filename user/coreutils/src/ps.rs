//! ps — process status
//!
//! Usage: ps

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use core::alloc::{GlobalAlloc, Layout};

use common::{exit, format_u64, stdout, stdoutln};

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
        // Align
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

/// Return a short label for the task state.
fn state_label(state: openos_sdk::process::TaskState) -> &'static str {
    match state {
        openos_sdk::process::TaskState::Ready => "Ready",
        openos_sdk::process::TaskState::Running => "Running",
        openos_sdk::process::TaskState::Blocked => "Blocked",
        openos_sdk::process::TaskState::Terminated => "Term",
    }
}

/// Write a string padded with spaces to `width` columns.
fn write_padded(s: &str, width: usize) {
    stdout(s);
    if s.len() < width {
        for _ in 0..(width - s.len()) {
            stdout(" ");
        }
    }
}

/// Write a u64 right-aligned in a field of `width` columns.
fn write_u64_aligned(val: u64, width: usize) {
    let mut buf = [0u8; 20];
    let digits = format_u64(val, &mut buf);
    let len = digits.len();
    if len < width {
        for _ in 0..(width - len) {
            stdout(" ");
        }
    }
    // SAFETY: digits are ASCII decimal characters.
    let s = unsafe { core::str::from_utf8_unchecked(digits) };
    stdout(s);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let tasks = match openos_sdk::process::list_tasks() {
        Ok(t) => t,
        Err(_) => {
            stdoutln("ps: failed to list tasks");
            exit(1);
        }
    };

    // Header: "PID  STATE     PRI NAME"
    write_padded("PID", 6);
    write_padded("STATE", 11);
    write_padded("PRI", 4);
    stdoutln("NAME");

    for task in &tasks {
        write_u64_aligned(task.id, 6);
        stdout("  ");
        write_padded(state_label(task.state), 9);
        write_u64_aligned(task.priority as u64, 4);
        stdout("  ");
        stdoutln(&task.name);
    }

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
