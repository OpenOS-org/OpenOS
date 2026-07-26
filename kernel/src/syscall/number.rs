//! System call numbers — single source of truth.
//!
//! Number assignments follow INTERFACE.md §Appendix A.
//! Both kernel and user-space must use these constants.

// Channel (5)
pub const SYS_CHANNEL_CREATE: u64 = 0x01;
pub const SYS_CHANNEL_SEND: u64 = 0x02;
pub const SYS_CHANNEL_RECEIVE: u64 = 0x03;
pub const SYS_CHANNEL_CALL: u64 = 0x04;
pub const SYS_CHANNEL_REPLY: u64 = 0x05;

// Handle (3)
pub const SYS_HANDLE_CLOSE: u64 = 0x10;
pub const SYS_HANDLE_DUPLICATE: u64 = 0x11;
pub const SYS_HANDLE_TRANSFER: u64 = 0x12;

// Process (4)
pub const SYS_PROCESS_CREATE: u64 = 0x30;
pub const SYS_PROCESS_START: u64 = 0x31;
pub const SYS_PROCESS_EXIT: u64 = 0x32;
pub const SYS_PROCESS_WAIT: u64 = 0x33;

// Thread (3)
pub const SYS_THREAD_CREATE: u64 = 0x40;
pub const SYS_THREAD_EXIT: u64 = 0x41;
pub const SYS_THREAD_YIELD: u64 = 0x42;

// OpenOS-specific: kernel debug console output (not in INTERFACE.md)
pub const SYS_CONSOLE_WRITE: u64 = 0xF0;
