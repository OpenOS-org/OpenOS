//! `x86_64` architecture initialization.
//!
//! This module orchestrates the low-level CPU setup that must happen before
//! any other kernel subsystem can run. The order matters:
//!   1. GDT — the CPU needs a valid code segment before we can load the IDT.
//!   2. IDT — exception handlers must be in place before we enable interrupts.
//!   3. PIC — remap IRQs so they don't collide with CPU exception vectors.
//!   4. sti — only after all three above are ready.

use crate::println;

pub mod gdt;
pub mod interrupts;

/// Initialize `x86_64` architecture: GDT → IDT → PIC → interrupts enabled.
pub fn init() {
    println!("[...] Initializing x86_64 architecture");

    gdt::init();
    println!("[OK] GDT initialized");

    interrupts::init_idt();
    println!("[OK] IDT initialized");

    // SAFETY: PIC initialization sends ICW1–ICW4 to the PIC command/data
    // ports. This is safe because the PIC is a stateless hardware device
    // at well-known I/O ports, and we do this exactly once.
    unsafe { interrupts::PICS.lock().initialize() };
    println!("[OK] PIC initialized");

    // sti — set interrupt flag. From this point on, the CPU will dispatch
    // hardware interrupts to our IDT handlers.
    x86_64::instructions::interrupts::enable();
    println!("[OK] Interrupts enabled");
}
