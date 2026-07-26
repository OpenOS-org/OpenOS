//! top — display system summary and process list
//!
//! Usage: top
//!
//! Displays system uptime, CPU count, memory usage, and a list of
//! running processes. The display refreshes every 3 seconds.
//! Press Ctrl-C to exit.

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

/// Read the contents of a procfs file into a buffer. Returns the number of
/// bytes read, or 0 on failure.
fn read_proc_file(path: &str, buf: &mut [u8]) -> usize {
    let fd = match openos_sdk::fs::open(path) {
        Ok(fd) => fd,
        Err(_) => return 0,
    };
    let mut total = 0;
    loop {
        match openos_sdk::fs::read(fd, &mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if total >= buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = openos_sdk::fs::close(fd);
    total
}

/// Find a value after a key prefix in a `Key: Value kB` style line.
/// Returns the numeric value or 0 if not found.
fn find_mem_value(data: &[u8], key: &[u8]) -> u64 {
    let mut i = 0;
    while i + key.len() <= data.len() {
        if &data[i..i + key.len()] == key {
            // Skip past the key and any whitespace/colon.
            let mut j = i + key.len();
            while j < data.len() && (data[j] == b' ' || data[j] == b':') {
                j += 1;
            }
            // Parse the numeric value.
            let mut val: u64 = 0;
            while j < data.len() && data[j].is_ascii_digit() {
                val = val * 10 + (data[j] - b'0') as u64;
                j += 1;
            }
            return val;
        }
        // Advance to next line.
        while i < data.len() && data[i] != b'\n' {
            i += 1;
        }
        i += 1; // skip newline
    }
    0
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
    let s = unsafe { core::str::from_utf8_unchecked(digits) };
    stdout(s);
}

/// Format seconds into `HH:MM:SS` and write it.
fn write_uptime(secs: u64) {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    let mut buf = [0u8; 20];
    let h = format_u64(hours, &mut buf);
    let h_str = unsafe { core::str::from_utf8_unchecked(h) };
    stdout(h_str);
    stdout("h:");

    let m = format_u64(minutes, &mut buf);
    let m_str = unsafe { core::str::from_utf8_unchecked(m) };
    if minutes < 10 {
        stdout("0");
    }
    stdout(m_str);
    stdout("m:");

    let s = format_u64(seconds, &mut buf);
    let s_str = unsafe { core::str::from_utf8_unchecked(s) };
    if seconds < 10 {
        stdout("0");
    }
    stdout(s_str);
    stdout("s");
}

/// Return a short label for the task state.
fn state_label(state: openos_sdk::process::TaskState) -> &'static str {
    match state {
        openos_sdk::process::TaskState::Ready => "Ready",
        openos_sdk::process::TaskState::Running => "Run",
        openos_sdk::process::TaskState::Blocked => "Block",
        openos_sdk::process::TaskState::Terminated => "Term",
    }
}

/// Render one frame of the top display.
fn render_frame() {
    // Clear screen and move cursor to top-left.
    let _ = openos_sdk::console::write("\x1b[2J\x1b[H");

    // --- System summary ---
    let uptime_secs = openos_sdk::time::clock_gettime(openos_sdk::time::CLOCK_MONOTONIC)
        .map(|ts| ts.sec)
        .unwrap_or(0);

    stdout("OpenOS top - up ");
    write_uptime(uptime_secs);
    stdoutln("");

    // --- Memory summary from /proc/meminfo ---
    let mut mem_buf = [0u8; 1024];
    let mem_len = read_proc_file("/proc/meminfo", &mut mem_buf);
    let mem_data = &mem_buf[..mem_len];

    let mem_total = find_mem_value(mem_data, b"MemTotal");
    let mem_free = find_mem_value(mem_data, b"MemFree");
    let mem_used = find_mem_value(mem_data, b"MemUsed");

    stdout("Mem: ");
    write_u64_aligned(mem_total, 8);
    stdout(" kB total, ");
    write_u64_aligned(mem_used, 8);
    stdout(" kB used, ");
    write_u64_aligned(mem_free, 8);
    stdoutln(" kB free");

    // --- CPU info from /proc/cpuinfo ---
    let mut cpu_buf = [0u8; 512];
    let cpu_len = read_proc_file("/proc/cpuinfo", &mut cpu_buf);
    let cpu_data = &cpu_buf[..cpu_len];

    // Count "processor" lines to determine CPU count.
    let mut cpu_count: u64 = 0;
    let mut i = 0;
    while i + 9 <= cpu_data.len() {
        if &cpu_data[i..i + 9] == b"processor" {
            cpu_count += 1;
        }
        while i < cpu_data.len() && cpu_data[i] != b'\n' {
            i += 1;
        }
        i += 1;
    }

    stdout("CPUs: ");
    let mut buf2 = [0u8; 20];
    let c = format_u64(cpu_count, &mut buf2);
    let c_str = unsafe { core::str::from_utf8_unchecked(c) };
    stdoutln(c_str);
    stdoutln("");

    // --- Process list header ---
    write_padded("PID", 6);
    write_padded("STATE", 8);
    write_padded("PRI", 5);
    stdoutln("NAME");

    // --- Process list ---
    let tasks = match openos_sdk::process::list_tasks() {
        Ok(t) => t,
        Err(_) => return,
    };

    for task in &tasks {
        write_u64_aligned(task.id, 6);
        stdout("  ");
        write_padded(state_label(task.state), 6);
        write_u64_aligned(task.priority as u64, 5);
        stdout("  ");
        stdoutln(&task.name);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Run 10 iterations, then exit. In a real system this would loop
    // until interrupted, but we cap it to avoid hanging the shell.
    for _ in 0..10 {
        render_frame();
        openos_sdk::time::sleep_ms(3000);
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
