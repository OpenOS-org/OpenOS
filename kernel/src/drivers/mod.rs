//! Device drivers.
//!
//! Currently only output devices (VGA, serial). Input drivers (keyboard,
//! mouse) will be added as interrupt-driven handlers in `arch::interrupts`.

pub mod serial;
pub mod vga;
