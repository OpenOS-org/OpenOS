//! Device drivers.
//!
//! Output devices (VGA, serial) and input devices (keyboard). The keyboard
//! driver is interrupt-driven: IRQ 1 fires on keypress, the scancode is
//! decoded, and the character is stored in an input buffer for user-space
//! consumption via `sys_read`.

pub mod keyboard;
pub mod net;
pub mod pci;
pub mod serial;
pub mod vga;
pub mod virtio_block;
pub mod virtio_net;
