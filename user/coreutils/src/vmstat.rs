//! vmstat — virtual memory statistics
//!
//! Usage: vmstat
//!
//! Displays a one-shot summary of virtual memory and system statistics,
//! similar to Linux `vmstat` (first line only).

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{exit, format_u64, stdout, stdoutln};

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
            let mut j = i + key.len();
            while j < data.len() && (data[j] == b' ' || data[j] == b':') {
                j += 1;
            }
            let mut val: u64 = 0;
            while j < data.len() && data[j].is_ascii_digit() {
                val = val * 10 + (data[j] - b'0') as u64;
                j += 1;
            }
            return val;
        }
        while i < data.len() && data[i] != b'\n' {
            i += 1;
        }
        i += 1;
    }
    0
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // --- Read /proc/meminfo ---
    let mut mem_buf = [0u8; 1024];
    let mem_len = read_proc_file("/proc/meminfo", &mut mem_buf);
    let mem_data = &mem_buf[..mem_len];

    let mem_total = find_mem_value(mem_data, b"MemTotal");
    let mem_free = find_mem_value(mem_data, b"MemFree");
    let mem_used = find_mem_value(mem_data, b"MemUsed");

    // Convert kB to pages (4 kB each) for vmstat-style output.
    let total_pages = mem_total / 4;
    let free_pages = mem_free / 4;
    let used_pages = mem_used / 4;

    // --- Count processes ---
    let task_count = match openos_sdk::process::list_tasks() {
        Ok(tasks) => tasks.len() as u64,
        Err(_) => 0,
    };

    // --- Read uptime ---
    let uptime_secs = openos_sdk::time::clock_gettime(openos_sdk::time::CLOCK_MONOTONIC)
        .map(|ts| ts.sec)
        .unwrap_or(0);

    // --- Read CPU count from /proc/cpuinfo ---
    let mut cpu_buf = [0u8; 512];
    let cpu_len = read_proc_file("/proc/cpuinfo", &mut cpu_buf);
    let cpu_data = &cpu_buf[..cpu_len];

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
    if cpu_count == 0 {
        cpu_count = 1; // fallback
    }

    // --- Header ---
    stdoutln("------ Memory (pages) ------");
    stdout("  total: ");
    write_u64_aligned(total_pages, 10);
    stdoutln("");
    stdout("   used: ");
    write_u64_aligned(used_pages, 10);
    stdoutln("");
    stdout("   free: ");
    write_u64_aligned(free_pages, 10);
    stdoutln("");
    stdoutln("");

    stdoutln("------ System ------");
    stdout("  uptime(s): ");
    write_u64_aligned(uptime_secs, 10);
    stdoutln("");
    stdout("   CPUs:     ");
    write_u64_aligned(cpu_count, 10);
    stdoutln("");
    stdout("   procs:    ");
    write_u64_aligned(task_count, 10);
    stdoutln("");
    stdoutln("");

    // --- Memory in kB ---
    stdoutln("------ Memory (kB) ------");
    stdout("  total: ");
    write_u64_aligned(mem_total, 10);
    stdoutln("");
    stdout("   used: ");
    write_u64_aligned(mem_used, 10);
    stdoutln("");
    stdout("   free: ");
    write_u64_aligned(mem_free, 10);
    stdoutln("");

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
