//! Virtual File System (VFS) — placeholder.
//!
//! A microkernel VFS runs as a user-space server that exposes a file-oriented
//! interface over IPC. Clients send open/read/write/close requests; the VFS
//! server dispatches to the appropriate filesystem driver (ramfs, ext2, etc.).
//!
//! This module will define the VFS IPC message types and the kernel-side
//! stub that routes file descriptor operations to the VFS port.

/// Initialize the VFS subsystem. Currently a no-op.
#[allow(dead_code)]
pub fn init() {}
