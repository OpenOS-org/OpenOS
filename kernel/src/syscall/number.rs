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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Channel syscall numbers ───
    #[test]
    fn test_channel_numbers_sequential() {
        assert_eq!(SYS_CHANNEL_CREATE, 0x01);
        assert_eq!(SYS_CHANNEL_SEND, 0x02);
        assert_eq!(SYS_CHANNEL_RECEIVE, 0x03);
        assert_eq!(SYS_CHANNEL_CALL, 0x04);
        assert_eq!(SYS_CHANNEL_REPLY, 0x05);
    }

    // ─── Handle syscall numbers ───
    #[test]
    fn test_handle_numbers_sequential() {
        assert_eq!(SYS_HANDLE_CLOSE, 0x10);
        assert_eq!(SYS_HANDLE_DUPLICATE, 0x11);
        assert_eq!(SYS_HANDLE_TRANSFER, 0x12);
    }

    // ─── Process syscall numbers ───
    #[test]
    fn test_process_numbers_sequential() {
        assert_eq!(SYS_PROCESS_CREATE, 0x30);
        assert_eq!(SYS_PROCESS_START, 0x31);
        assert_eq!(SYS_PROCESS_EXIT, 0x32);
        assert_eq!(SYS_PROCESS_WAIT, 0x33);
    }

    // ─── Thread syscall numbers ───
    #[test]
    fn test_thread_numbers_sequential() {
        assert_eq!(SYS_THREAD_CREATE, 0x40);
        assert_eq!(SYS_THREAD_EXIT, 0x41);
        assert_eq!(SYS_THREAD_YIELD, 0x42);
    }

    // ─── Console syscall number ───
    #[test]
    fn test_console_write_number() {
        assert_eq!(SYS_CONSOLE_WRITE, 0xF0);
    }

    // ─── No overlaps between groups ───
    #[test]
    fn test_no_overlaps() {
        let all = [
            SYS_CHANNEL_CREATE,
            SYS_CHANNEL_SEND,
            SYS_CHANNEL_RECEIVE,
            SYS_CHANNEL_CALL,
            SYS_CHANNEL_REPLY,
            SYS_HANDLE_CLOSE,
            SYS_HANDLE_DUPLICATE,
            SYS_HANDLE_TRANSFER,
            SYS_PROCESS_CREATE,
            SYS_PROCESS_START,
            SYS_PROCESS_EXIT,
            SYS_PROCESS_WAIT,
            SYS_THREAD_CREATE,
            SYS_THREAD_EXIT,
            SYS_THREAD_YIELD,
            SYS_CONSOLE_WRITE,
        ];
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(
                    all[i], all[j],
                    "syscall numbers {} and {} overlap",
                    all[i], all[j]
                );
            }
        }
    }

    // ─── Channel group is in range 0x01-0x0F ───
    #[test]
    fn test_channel_range() {
        let channel_numbers = [
            SYS_CHANNEL_CREATE,
            SYS_CHANNEL_SEND,
            SYS_CHANNEL_RECEIVE,
            SYS_CHANNEL_CALL,
            SYS_CHANNEL_REPLY,
        ];
        for &n in &channel_numbers {
            assert!(n >= 0x01 && n <= 0x0F, "channel syscall {} out of range", n);
        }
    }

    // ─── Handle group is in range 0x10-0x1F ───
    #[test]
    fn test_handle_range() {
        let handle_numbers = [SYS_HANDLE_CLOSE, SYS_HANDLE_DUPLICATE, SYS_HANDLE_TRANSFER];
        for &n in &handle_numbers {
            assert!(n >= 0x10 && n <= 0x1F, "handle syscall {} out of range", n);
        }
    }

    // ─── Process group is in range 0x30-0x3F ───
    #[test]
    fn test_process_range() {
        let process_numbers = [
            SYS_PROCESS_CREATE,
            SYS_PROCESS_START,
            SYS_PROCESS_EXIT,
            SYS_PROCESS_WAIT,
        ];
        for &n in &process_numbers {
            assert!(n >= 0x30 && n <= 0x3F, "process syscall {} out of range", n);
        }
    }

    // ─── Thread group is in range 0x40-0x4F ───
    #[test]
    fn test_thread_range() {
        let thread_numbers = [SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD];
        for &n in &thread_numbers {
            assert!(n >= 0x40 && n <= 0x4F, "thread syscall {} out of range", n);
        }
    }
}
