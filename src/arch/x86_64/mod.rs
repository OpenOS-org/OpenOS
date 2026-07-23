//! `x86_64` architecture initialization.
//!
//! Init order: GDT → IDT → PIC → SYSCALL MSRs → interrupts enabled.
//! The SYSCALL MSRs must be configured before user-mode transitions,
//! but can be done before or after enabling interrupts.

use crate::println;

pub mod gdt;
pub mod interrupts;
pub mod syscall;

/// Initialize `x86_64` architecture.
pub fn init() {
    println!("[...] Initializing x86_64 architecture");

    gdt::init();
    println!("[OK] GDT initialized (kernel + user segments, TSS)");

    interrupts::init_idt();
    println!("[OK] IDT initialized");

    // SAFETY: PIC initialization sends ICW1-ICW4 to remap IRQs.
    unsafe { interrupts::PICS.lock().initialize() };
    println!("[OK] PIC initialized");

    // SAFETY: Writing MSRs to configure SYSCALL/SYSRET.
    syscall::init();

    x86_64::instructions::interrupts::enable();
    println!("[OK] Interrupts enabled");
}
