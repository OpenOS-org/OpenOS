//! Virtual File System (VFS) — ramfs and future filesystem services.
//!
//! A microkernel VFS runs as a user-space server that exposes a file-oriented
//! interface over IPC. Clients send open/read/write/close requests; the VFS
//! server dispatches to the appropriate filesystem driver.

pub mod ramfs;
pub mod vfs;

/// Initialize the VFS subsystem.
pub fn init() {
    ramfs::init();
}
