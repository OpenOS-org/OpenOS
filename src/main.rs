//! `OpenOS` — A microkernel operating system written in Rust.
//!
//! This crate is the kernel binary. It is `#![no_std]` and `#![no_main]` because
//! there is no C runtime or standard library available at boot; the bootloader
//! jumps directly to `_start` after setting up identity-mapped paging.

#![no_std]
#![no_main]
// Required for `extern "x86-interrupt"` calling convention on ISRs.
#![feature(abi_x86_interrupt)]
// Required because we use `panic = "abort"` — the default unwinding-based
// `alloc_error_handler` is not available without the `unwind` runtime.
#![feature(alloc_error_handler)]
// Lint policy: warn on everything clippy considers, then suppress the specific
// lints that fire on scaffolding code we haven't wired up yet.
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::module_inception,
    clippy::similar_names,          // user_cs/user_ss is intentional naming
    clippy::items_after_statements, // static arrays in lazy_static blocks
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    dead_code,
    unused_imports,
    unused_variables,
    clippy::missing_const_for_fn,
    clippy::used_underscore_items
)]

extern crate alloc;

use core::panic::PanicInfo;

mod arch;
mod drivers;
mod fs;
mod ipc;
mod memory;
mod syscall;
mod task;

/// Kernel entry point.
///
/// The bootloader (crate `bootloader 0.9`) loads the kernel at physical address
/// `0x100000`, sets up identity-mapped page tables, and jumps here with:
///   - Interrupts disabled
///   - A valid GDT loaded (bootloader's own)
///   - No IDT, no PIC, no heap
///
/// The init order is deliberate:
/// 1. VGA first — we need `println!` for all subsequent diagnostics.
/// 2. GDT/IDT/PIC before anything that might fault or use interrupts.
/// 3. Heap before anything that allocates (IPC, scheduler).
/// 4. Syscall/IPC/scheduler last — they depend on everything above.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Initialize serial port first — VGA may not be available in headless QEMU.
    drivers::serial::SERIAL1.lock();

    drivers::vga::init();

    println!("=================================");
    println!("  OpenOS Microkernel v0.1.0");
    println!("=================================");
    println!();
    serial_println!("=================================");
    serial_println!("  OpenOS Microkernel v0.1.0");
    serial_println!("=================================");
    serial_println!();

    arch::x86_64::init();
    memory::init();
    ipc::init();
    task::init();

    println!("[OK] Kernel initialization complete");
    println!("[OK] Microkernel ready");
    println!();
    serial_println!("[OK] Kernel initialization complete");
    serial_println!("[OK] Microkernel ready");
    serial_println!();

    // Launch the first user-mode process.
    serial_println!("[...] Launching first user process");
    task::user::launch_first_process();

    // Should never reach here — the user process runs until exit.
    println!("[OK] First user process exited");
    serial_println!("[OK] First user process exited");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Global panic handler.
///
/// With `panic = "abort"`, unwinding is disabled, so this function is the
/// final destination for any panic. We print the panic info to VGA (which is
/// always available) and then halt — there is no recovery path.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[PANIC] {info}");
    serial_println!("[PANIC] {info}");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Allocation failure handler.
///
/// Called by the `alloc` crate when `Box::new`, `Vec::push`, etc. fail because
/// the heap is exhausted. We treat this as a panic because a kernel that cannot
/// allocate is in an unrecoverable state.
#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation error: {layout:?}");
}
