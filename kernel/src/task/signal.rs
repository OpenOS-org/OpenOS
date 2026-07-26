//! POSIX-like signal definitions and per-task delivery state.
//!
//! Signals are identified by a small integer (1..31). Each task carries a
//! `SignalState` that tracks pending signals (bitmask), blocked signals
//! (bitmask), and per-signal handler addresses.
//!
//! Handler address conventions:
//! - `SIG_DFL (0)` — kernel default action (terminate for most fatal signals)
//! - `SIG_IGN (1)` — ignore the signal
//! - Any other value — user-space handler address

// ─── Signal constants ───

/// Hangup detected on controlling terminal or death of controlling process.
pub const SIGHUP: u8 = 1;
/// Interrupt from keyboard (Ctrl-C).
pub const SIGINT: u8 = 2;
/// Quit from keyboard (Ctrl-\).
pub const SIGQUIT: u8 = 3;
/// Illegal instruction.
pub const SIGILL: u8 = 4;
/// Trace/breakpoint trap.
pub const SIGTRAP: u8 = 5;
/// Abort signal.
pub const SIGABRT: u8 = 6;
/// Bus error (bad memory access).
pub const SIGBUS: u8 = 7;
/// Floating-point exception.
pub const SIGFPE: u8 = 8;
/// Kill signal (cannot be caught or ignored).
pub const SIGKILL: u8 = 9;
/// User-defined signal 1.
pub const SIGUSR1: u8 = 10;
/// Invalid memory reference (segmentation fault).
pub const SIGSEGV: u8 = 11;
/// User-defined signal 2.
pub const SIGUSR2: u8 = 12;
/// Broken pipe: write to pipe with no readers.
pub const SIGPIPE: u8 = 13;
/// Timer signal from alarm(2).
pub const SIGALRM: u8 = 14;
/// Termination signal.
pub const SIGTERM: u8 = 15;
/// Child stopped or terminated.
pub const SIGCHLD: u8 = 17;

/// Maximum signal number we support (signals 1..31 inclusive).
const MAX_SIGNUM: u8 = 31;

// ─── Handler special values ───

/// Default action — kernel handles the signal.
pub const SIG_DFL: u64 = 0;
/// Ignore the signal — silently discarded.
pub const SIG_IGN: u64 = 1;

/// Per-task signal delivery state.
///
/// Tracks which signals are pending, which are blocked, and what handler
/// is installed for each signal number.
pub struct SignalState {
    /// Bitmask of pending signals. Bit N is set when signal N is pending.
    pub pending: u64,
    /// Bitmask of blocked signals. Bit N is set when signal N is blocked.
    pub blocked: u64,
    /// Per-signal handler addresses. Index 0 is unused (signals start at 1).
    /// `SIG_DFL (0)` = default, `SIG_IGN (1)` = ignore, else = user handler addr.
    pub handlers: [u64; 32],
}

impl SignalState {
    /// Create a new signal state with all signals set to default handlers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: 0,
            blocked: 0,
            handlers: [SIG_DFL; 32],
        }
    }

    /// Send a signal by setting its bit in the pending bitmask.
    ///
    /// `sig` must be in the range 1..=31. Values outside this range are ignored.
    pub fn send_signal(&mut self, sig: u8) {
        if sig == 0 || sig > MAX_SIGNUM {
            return;
        }
        self.pending |= 1u64 << sig;
    }

    /// Check if any unblocked signal is pending.
    ///
    /// Returns `true` if there is at least one bit set in `pending & !blocked`.
    /// Signal 0 is never considered (it is not a valid signal number).
    #[must_use]
    pub fn has_pending(&self) -> bool {
        (self.pending & !self.blocked & !1u64) != 0
    }

    /// Dequeue the lowest-numbered pending unblocked signal.
    ///
    /// Clears the bit from `pending` and returns the signal number.
    /// Returns `None` if no unblocked signal is pending.
    #[must_use]
    pub fn next_pending(&mut self) -> Option<u8> {
        let actionable = self.pending & !self.blocked & !1u64;
        if actionable == 0 {
            return None;
        }
        // Find lowest set bit (excluding bit 0).
        #[allow(clippy::cast_possible_truncation)]
        let sig = actionable.trailing_zeros() as u8;
        self.pending &= !(1u64 << sig);
        Some(sig)
    }
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_no_pending_signals() {
        let state = SignalState::new();
        assert_eq!(state.pending, 0);
        assert_eq!(state.blocked, 0);
        assert!(!state.has_pending());
    }

    #[test]
    fn test_new_all_default_handlers() {
        let state = SignalState::new();
        for i in 0..32u64 {
            assert_eq!(state.handlers[i as usize], SIG_DFL);
        }
    }

    #[test]
    fn test_send_signal_sets_pending_bit() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        assert_eq!(state.pending & (1u64 << SIGINT), 1u64 << SIGINT);
    }

    #[test]
    fn test_send_signal_multiple() {
        let mut state = SignalState::new();
        state.send_signal(SIGHUP);
        state.send_signal(SIGTERM);
        assert!(state.has_pending());
        // Both should be pending.
        assert_ne!(state.pending & (1u64 << SIGHUP), 0);
        assert_ne!(state.pending & (1u64 << SIGTERM), 0);
    }

    #[test]
    fn test_send_signal_ignored_if_zero() {
        let mut state = SignalState::new();
        state.send_signal(0);
        assert_eq!(state.pending, 0);
    }

    #[test]
    fn test_send_signal_ignored_if_out_of_range() {
        let mut state = SignalState::new();
        state.send_signal(32);
        assert_eq!(state.pending, 0);
    }

    #[test]
    fn test_has_pending_blocked_signal() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        state.blocked = 1u64 << SIGINT;
        // SIGINT is pending but blocked — has_pending should be false.
        assert!(!state.has_pending());
    }

    #[test]
    fn test_has_pending_mixed_blocked_and_unblocked() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        state.send_signal(SIGTERM);
        state.blocked = 1u64 << SIGINT; // Only SIGINT is blocked.
                                        // SIGTERM is unblocked and pending.
        assert!(state.has_pending());
    }

    #[test]
    fn test_next_pending_returns_lowest() {
        let mut state = SignalState::new();
        state.send_signal(SIGTERM);
        state.send_signal(SIGHUP);
        // SIGHUP (1) is lower than SIGTERM (15).
        let sig = state.next_pending();
        assert_eq!(sig, Some(SIGHUP));
    }

    #[test]
    fn test_next_pending_clears_bit() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        let sig = state.next_pending();
        assert_eq!(sig, Some(SIGINT));
        // Should be cleared now.
        assert!(!state.has_pending());
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_next_pending_skips_blocked() {
        let mut state = SignalState::new();
        state.send_signal(SIGHUP);
        state.send_signal(SIGTERM);
        state.blocked = 1u64 << SIGHUP; // Block SIGHUP.
                                        // Should skip SIGHUP and return SIGTERM.
        let sig = state.next_pending();
        assert_eq!(sig, Some(SIGTERM));
    }

    #[test]
    fn test_next_pending_returns_none_when_empty() {
        let mut state = SignalState::new();
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_next_pending_returns_none_when_all_blocked() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        state.blocked = u64::MAX;
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_send_signal_same_twice_sets_bit_once() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        state.send_signal(SIGINT);
        let sig = state.next_pending();
        assert_eq!(sig, Some(SIGINT));
        // Second call should return None since the bit was set only once.
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_signal_constants() {
        assert_eq!(SIGHUP, 1);
        assert_eq!(SIGINT, 2);
        assert_eq!(SIGQUIT, 3);
        assert_eq!(SIGILL, 4);
        assert_eq!(SIGTRAP, 5);
        assert_eq!(SIGABRT, 6);
        assert_eq!(SIGBUS, 7);
        assert_eq!(SIGFPE, 8);
        assert_eq!(SIGKILL, 9);
        assert_eq!(SIGUSR1, 10);
        assert_eq!(SIGSEGV, 11);
        assert_eq!(SIGUSR2, 12);
        assert_eq!(SIGPIPE, 13);
        assert_eq!(SIGALRM, 14);
        assert_eq!(SIGTERM, 15);
        assert_eq!(SIGCHLD, 17);
    }

    #[test]
    fn test_handler_special_values() {
        assert_eq!(SIG_DFL, 0);
        assert_eq!(SIG_IGN, 1);
    }

    #[test]
    fn test_handler_install_and_read() {
        let mut state = SignalState::new();
        let handler_addr: u64 = 0x401_000;
        state.handlers[SIGINT as usize] = handler_addr;
        assert_eq!(state.handlers[SIGINT as usize], handler_addr);
        // Other signals still default.
        assert_eq!(state.handlers[SIGHUP as usize], SIG_DFL);
    }

    #[test]
    fn test_default_trait() {
        let state = SignalState::default();
        assert_eq!(state.pending, 0);
        assert_eq!(state.blocked, 0);
    }

    #[test]
    fn test_signal_1_through_31_all_sendable() {
        let mut state = SignalState::new();
        for sig in 1..=31u8 {
            state.send_signal(sig);
        }
        // All 31 signals should be pending (bit 0 excluded).
        assert_eq!(state.pending & !1u64, (1u64 << 32) - 2);
    }

    #[test]
    fn test_next_pending_fifo_order() {
        let mut state = SignalState::new();
        // Send in reverse order — next_pending returns lowest first.
        state.send_signal(SIGTERM);
        state.send_signal(SIGINT);
        state.send_signal(SIGHUP);
        assert_eq!(state.next_pending(), Some(SIGHUP));
        assert_eq!(state.next_pending(), Some(SIGINT));
        assert_eq!(state.next_pending(), Some(SIGTERM));
        assert_eq!(state.next_pending(), None);
    }
}
