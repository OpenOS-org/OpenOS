//! `OpenOS` — A microkernel operating system written in Rust.
//!
//! This crate is the kernel binary. It is `#![no_std]` and `#![no_main]` because
//! there is no C runtime or standard library available at boot; the bootloader
//! jumps directly to the `kernel_main` entry point after setting up paging and
//! a framebuffer.

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

use bootloader_api::{entry_point, BootloaderConfig};

/// Bootloader configuration. The kernel stack size is set to 80 KiB to match
/// the original TSS RSP0 allocation. All other mappings use dynamic addresses.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 80 * 1024;
    config
};

// Kernel entry point.
//
// The bootloader (crate `bootloader_api 0.11`) loads the kernel, sets up
// higher-half paging, configures a framebuffer, and jumps here with:
//   - GDT loaded (basic kernel segments)
//   - Paging active (higher-half kernel mapped)
//   - A valid kernel stack
//   - Interrupts disabled
//
// The `BootInfo` struct provides the framebuffer, memory map, and physical
// memory offset — we no longer need to set up identity-mapped pages or
// access the VGA buffer at 0xB8000 directly.
//
// Init order: serial → framebuffer → GDT/IDT/PIC → heap → IPC → scheduler.
entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    // Initialize serial port first — framebuffer may not be available in
    // headless QEMU.
    drivers::serial::SERIAL1.lock();

    // Initialize framebuffer text renderer. The bootloader has already
    // configured the framebuffer; we just need to start writing to it.
    drivers::vga::init(boot_info);

    println!("=================================");
    println!("  OpenOS Microkernel v0.2.0");
    println!("=================================");
    println!();
    serial_println!("=================================");
    serial_println!("  OpenOS Microkernel v0.2.0");
    serial_println!("=================================");
    serial_println!();

    serial_println!("[...] Starting arch init");
    arch::x86_64::init();
    serial_println!("[OK] Arch init done");

    serial_println!("[...] Starting memory init");
    memory::init();
    serial_println!("[OK] Memory init done");

    serial_println!("[...] Starting IPC init");
    ipc::init();
    serial_println!("[OK] IPC init done");

    serial_println!("[...] Starting task init");
    task::init();
    serial_println!("[OK] Task init done");

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
/// final destination for any panic. We print the panic info to serial (always
/// available) and framebuffer (if initialized), then halt.
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
