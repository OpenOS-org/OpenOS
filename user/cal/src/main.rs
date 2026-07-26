//! cal — display a monthly calendar
//!
//! Usage: cal [YYYY MM]
//!
//! Without arguments, displays the calendar for the current month
//! (computed from boot epoch + monotonic clock). The current day
//! is highlighted with inverse video.
//!
//! With arguments, displays the calendar for the specified month.
//!
//! Examples:
//!   cal           — current month
//!   cal 2026 7    — July 2026

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, env, process, time};

// ---------------------------------------------------------------------------
// Allocator
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in cal!");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Assumed boot epoch: January 1, 2026 (Thursday).
/// The kernel provides only monotonic ticks since boot, so we assume
/// the system was booted at this point in wall-clock time.
const BOOT_EPOCH_YEAR: u32 = 2026;
const BOOT_EPOCH_MONTH: u32 = 1;
const BOOT_EPOCH_DAY: u32 = 1;

/// Timer frequency (ticks per second).
const TIMER_HZ: u64 = 100;

/// Days in each month (non-leap year).
const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Month names.
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Day-of-week header.
const DAY_HEADER: &str = "Su Mo Tu We Th Fr Sa";

// ---------------------------------------------------------------------------
// Date calculations
// ---------------------------------------------------------------------------

/// Returns true if the given year is a leap year.
const fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Returns the number of days in the given month (1-based).
const fn days_in_month(year: u32, month: u32) -> u32 {
    if month == 2 && is_leap_year(year) {
        29
    } else {
        DAYS_IN_MONTH[(month - 1) as usize]
    }
}

/// Calculate the total number of days from year 1-01-01 to the given date.
/// Uses a formula that accounts for leap years correctly.
fn days_from_epoch(year: u32, month: u32, day: u32) -> u64 {
    // Adjust months so that March = month 0, February = month 11.
    // This places the leap day at the end of the year, simplifying the formula.
    let m = if month <= 2 { month + 12 } else { month };
    let y = if month <= 2 { year - 1 } else { year };

    // Days from the epoch to the given date.
    // Using Rata Die / Julian Day Number derivation.
    let era = y / 400;
    let yoe = y - era * 400; // year of era [0, 399]
    let doy = (153 * (m - 3) + 2) / 5 + day - 1; // day of year [0, 365]
    let era_days = era as u64 * 146_097; // days per 400-year era
    let yoe_days = yoe as u64 * 365 + (yoe / 4) as u64 - (yoe / 100) as u64;
    era_days + yoe_days + doy as u64 - 306 // subtract offset to make 0001-01-01 = day 0
}

/// Returns the day of week (0 = Sunday, 6 = Saturday) for a given date.
fn day_of_week(year: u32, month: u32, day: u32) -> u32 {
    // 0001-01-01 was a Monday (day 1), so offset = 1.
    let d = (days_from_epoch(year, month, day) + 1) % 7;
    d as u32
}

/// Convert monotonic boot time to a wall-clock date.
/// Assumes the system booted at `BOOT_EPOCH_YEAR/MONTH/DAY`.
fn boot_time_to_date() -> (u32, u32, u32) {
    let ticks = time::ticks();
    let elapsed_seconds = ticks / TIMER_HZ;

    // Start from boot epoch.
    let mut year = BOOT_EPOCH_YEAR;
    let mut month = BOOT_EPOCH_MONTH;
    let mut day = BOOT_EPOCH_DAY;

    // Add whole days (ignore sub-day remainder).
    let days_elapsed = (elapsed_seconds / 86_400) as u32;

    // Advance by days_elapsed.
    let mut days_left = days_elapsed;
    while days_left > 0 {
        let days_this_month = days_in_month(year, month);
        let days_remaining_in_month = days_this_month - day + 1;

        if days_left < days_remaining_in_month {
            day += days_left;
            break;
        }

        days_left -= days_remaining_in_month;
        month += 1;
        day = 1;

        if month > 12 {
            month = 1;
            year += 1;
        }
    }

    (year, month, day)
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

/// Parse arguments from the `__ARGS__` environment variable.
/// Returns `Some((year, month))` if valid arguments are provided,
/// or `None` for default (current month).
fn parse_args() -> Option<(u32, u32)> {
    let raw = env::get("__ARGS__").ok().flatten()?;
    let args = raw.trim();
    if args.is_empty() {
        return None;
    }

    // Split into tokens.
    let mut parts = args.splitn(3, ' ');
    let year_str = parts.next()?.trim();
    let month_str = parts.next()?.trim();

    // Parse year.
    let year = parse_u32(year_str)?;
    if !(1..=9999).contains(&year) {
        return None;
    }

    // Parse month.
    let month = parse_u32(month_str)?;
    if !(1..=12).contains(&month) {
        return None;
    }

    Some((year, month))
}

/// Parse a decimal u32 from a string without std.
fn parse_u32(s: &str) -> Option<u32> {
    let mut val: u32 = 0;
    for &b in s.as_bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    if s.is_empty() {
        None
    } else {
        Some(val)
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a u32 into a buffer, return as &str.
fn format_u32(buf: &mut [u8; 10], val: u32) -> &str {
    if val == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut n = val;
    let mut pos = 0;
    let mut digits = [0u8; 10];
    let mut dcount = 0;
    while n > 0 {
        digits[dcount] = b'0' + (n % 10) as u8;
        n /= 10;
        dcount += 1;
    }
    for i in (0..dcount).rev() {
        buf[pos] = digits[i];
        pos += 1;
    }
    core::str::from_utf8(&buf[..pos]).unwrap_or("?")
}

// ---------------------------------------------------------------------------
// Calendar rendering
// ---------------------------------------------------------------------------

/// Render a calendar for the given month, optionally highlighting `today`.
/// `today` is `Some((day, month, year))` if the current day should be highlighted.
fn render_calendar(year: u32, month: u32, today: Option<(u32, u32, u32)>) {
    // Print the month/year header, centered over the 20-char day grid.
    let month_name = MONTH_NAMES[(month - 1) as usize];
    let header = month_name;
    let mut year_buf = [0u8; 10];
    let year_str = format_u32(&mut year_buf, year);

    // Calculate padding to center "July 2026" over a 20-char wide grid.
    // The day header is 20 chars wide (Su Mo Tu We Th Fr Sa).
    let title_len = header.len() + 1 + year_str.len();
    let pad = if title_len < 20 {
        (20 - title_len) / 2
    } else {
        0
    };

    for _ in 0..pad {
        let _ = console::write(" ");
    }
    let _ = console::write(header);
    let _ = console::write(" ");
    let _ = console::writeln(year_str);
    let _ = console::writeln(DAY_HEADER);

    // First day of the month.
    let first_dow = day_of_week(year, month, 1);
    let total_days = days_in_month(year, month);

    // Leading spaces.
    for _ in 0..first_dow {
        let _ = console::write("   ");
    }

    // Print each day.
    let mut dow = first_dow;
    for day in 1..=total_days {
        let is_today = today.is_some_and(|(td, tm, ty)| td == day && tm == month && ty == year);

        if is_today {
            // Inverse video for highlighted day: ESC[7m ... ESC[0m
            let _ = console::write("\x1b[7m");
        }

        let mut day_buf = [0u8; 10];
        let day_str = format_u32(&mut day_buf, day);
        if day < 10 {
            let _ = console::write(" ");
        }
        let _ = console::write(day_str);

        if is_today {
            let _ = console::write("\x1b[0m");
        }

        dow += 1;
        if dow >= 7 {
            dow = 0;
            let _ = console::write("\n");
        } else {
            let _ = console::write(" ");
        }
    }

    // Trailing newline if the last day wasn't Saturday.
    if dow != 0 {
        let _ = console::write("\n");
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let (year, month, today_info) = match parse_args() {
        Some((y, m)) => {
            // Specific month requested; no "today" highlight.
            (y, m, None)
        }
        None => {
            // Current month based on boot time.
            let (y, m, d) = boot_time_to_date();
            (y, m, Some((d, m, y)))
        }
    };

    render_calendar(year, month, today_info);
    process::exit(0);
}
