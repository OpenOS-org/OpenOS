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

// ─── sigprocmask operations ───

/// Block the signals in `set` (add to blocked mask).
pub const SIG_BLOCK: u64 = 0;
/// Unblock the signals in `set` (remove from blocked mask).
pub const SIG_UNBLOCK: u64 = 1;
/// Set the blocked mask to exactly `set`.
pub const SIG_SETMASK: u64 = 2;

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

    /// Get the handler address for a signal.
    ///
    /// Returns `SIG_DFL`, `SIG_IGN`, or a user-space handler address.
    #[must_use]
    pub fn get_handler(&self, sig: u8) -> u64 {
        if sig == 0 || sig > MAX_SIGNUM {
            return SIG_DFL;
        }
        self.handlers[sig as usize]
    }
}

/// Default action for a signal.
///
/// Determines what happens when a signal has `SIG_DFL` as its handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    /// Terminate the process (e.g., SIGKILL, SIGTERM, SIGINT).
    Terminate,
    /// Ignore the signal (e.g., SIGCHLD, SIGURG).
    Ignore,
    /// Stop the process (not yet implemented).
    Stop,
    /// Continue the process (not yet implemented).
    Continue,
}

/// Get the default action for a signal number.
///
/// Most fatal signals default to `Terminate`. A few signals are ignored
/// by default (SIGCHLD, SIGURG). SIGSTOP/SIGCONT have special semantics
/// but are not yet implemented.
#[must_use]
pub fn default_action(sig: u8) -> DefaultAction {
    match sig {
        // Signals that are ignored by default.
        SIGCHLD => DefaultAction::Ignore,
        // All other signals terminate by default.
        _ => DefaultAction::Terminate,
    }
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

/// Signal trampoline frame pushed onto the user stack during signal delivery.
///
/// When the kernel delivers a signal, it saves the interrupted context onto
/// the user stack and redirects execution to the signal handler. When the
/// handler calls `sigreturn`, the kernel restores the saved context from
/// this frame.
///
/// Layout on the user stack (addresses grow downward):
///
/// ```text
///   ┌─────────────────────┐  ← RSP after signal delivery
///   │  saved_r11          │  (RFLAGS)
///   │  saved_rcx          │  (RIP from SYSCALL)
///   │  saved_rsp          │  (user RSP)
///   │  saved_rdi          │  (handler argument = signal number)
///   │  sig_num            │  (which signal)
///   │  ret_addr           │  (address of sigreturn trampoline)
///   └─────────────────────┘
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalFrame {
    /// Address of the `sigreturn` trampoline in user-space (so the handler
    /// returns to it, which then calls `SYS_SIGRETURN`).
    pub ret_addr: u64,
    /// The signal number being delivered.
    pub sig_num: u64,
    /// Saved RDI (passed as first argument to handler = signal number).
    pub saved_rdi: u64,
    /// Saved user RSP before signal delivery.
    pub saved_rsp: u64,
    /// Saved RCX (user RIP from SYSCALL instruction).
    pub saved_rcx: u64,
    /// Saved R11 (user RFLAGS from SYSCALL instruction).
    pub saved_r11: u64,
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

    #[test]
    fn test_default_action_sigchld_is_ignore() {
        assert_eq!(default_action(SIGCHLD), DefaultAction::Ignore);
    }

    #[test]
    fn test_default_action_fatal_signals_terminate() {
        assert_eq!(default_action(SIGHUP), DefaultAction::Terminate);
        assert_eq!(default_action(SIGINT), DefaultAction::Terminate);
        assert_eq!(default_action(SIGKILL), DefaultAction::Terminate);
        assert_eq!(default_action(SIGSEGV), DefaultAction::Terminate);
        assert_eq!(default_action(SIGTERM), DefaultAction::Terminate);
    }

    #[test]
    fn test_default_action_variants_unique() {
        assert_ne!(DefaultAction::Terminate, DefaultAction::Ignore);
        assert_ne!(DefaultAction::Terminate, DefaultAction::Stop);
    }

    #[test]
    fn test_get_handler_zero_returns_dfl() {
        let state = SignalState::new();
        assert_eq!(state.get_handler(0), SIG_DFL);
    }

    #[test]
    fn test_get_handler_out_of_range() {
        let state = SignalState::new();
        assert_eq!(state.get_handler(32), SIG_DFL);
        assert_eq!(state.get_handler(255), SIG_DFL);
    }

    #[test]
    fn test_sigprocmask_constants() {
        assert_eq!(SIG_BLOCK, 0);
        assert_eq!(SIG_UNBLOCK, 1);
        assert_eq!(SIG_SETMASK, 2);
    }

    // ─── Signal frame tests ───

    #[test]
    fn test_signal_frame_layout() {
        // Verify the struct is repr(C) and correctly sized.
        let frame = SignalFrame {
            ret_addr: 0x1234_5678,
            sig_num: 11,
            saved_rdi: 11,
            saved_rsp: 0x7fff_0000,
            saved_rcx: 0x4000_1000,
            saved_r11: 0x202,
        };
        assert_eq!(frame.sig_num, 11);
        assert_eq!(frame.ret_addr, 0x1234_5678);
        // Check size: 6 * 8 = 48 bytes.
        assert_eq!(core::mem::size_of::<SignalFrame>(), 48);
        // Check alignment.
        assert_eq!(core::mem::align_of::<SignalFrame>(), 8);
    }

    #[test]
    fn test_signal_frame_zeroed() {
        // Default zeroed SignalFrame (repr(C) so zero-initialized fields are 0).
        let frame: SignalFrame = unsafe { core::mem::zeroed() };
        assert_eq!(frame.ret_addr, 0);
        assert_eq!(frame.sig_num, 0);
        assert_eq!(frame.saved_rdi, 0);
        assert_eq!(frame.saved_rsp, 0);
        assert_eq!(frame.saved_rcx, 0);
        assert_eq!(frame.saved_r11, 0);
    }

    // ─── Block/unblock signal tests ───

    #[test]
    fn test_block_signal_hides_from_has_pending() {
        let mut state = SignalState::new();
        state.send_signal(SIGALRM);
        assert!(state.has_pending());
        // Block the signal.
        state.blocked |= 1u64 << SIGALRM;
        assert!(!state.has_pending());
        // Unblock it.
        state.blocked &= !(1u64 << SIGALRM);
        assert!(state.has_pending());
    }

    #[test]
    fn test_block_multiple_signals() {
        let mut state = SignalState::new();
        state.send_signal(SIGINT);
        state.send_signal(SIGTERM);
        state.send_signal(SIGHUP);
        // Block two out of three.
        state.blocked = (1u64 << SIGINT) | (1u64 << SIGTERM);
        // Only SIGHUP should be actionable.
        assert_eq!(state.next_pending(), Some(SIGHUP));
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_block_all_signals_without_mask_zero() {
        let mut state = SignalState::new();
        // Send all valid signals.
        for sig in 1..=31u8 {
            state.send_signal(sig);
        }
        // Block all.
        state.blocked = u64::MAX;
        assert!(!state.has_pending());
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_setmask_replaces_blocked() {
        let mut state = SignalState::new();
        state.blocked = 1u64 << 17; // Block SIGCHLD.
                                    // Replace with a new mask (SIG_SETMASK equivalent: just set it).
        state.blocked = (1u64 << SIGHUP) | (1u64 << SIGINT);
        assert_eq!(state.blocked, (1u64 << SIGHUP) | (1u64 << SIGINT));
        // SIGCHLD should no longer be blocked.
        state.send_signal(SIGCHLD);
        assert!(state.has_pending());
    }

    #[test]
    fn test_sigkill_blocked_full_mask() {
        // SIGKILL is always deliverable per POSIX, but our implementation
        // does not enforce this. Verify current blocking behavior.
        let mut state = SignalState::new();
        state.send_signal(SIGKILL);
        // When all signals are blocked, has_pending returns false
        // because pending & !blocked & !1 == 0.
        state.blocked = u64::MAX;
        assert!(!state.has_pending());
        // next_pending also returns None since no signal is actionable.
        assert_eq!(state.next_pending(), None);
    }

    #[test]
    fn test_handler_install_via_sigaction_pattern() {
        // Simulate the sigaction pattern: install handler, then check.
        let mut state = SignalState::new();
        let handler_addr: u64 = 0x8000_0000;
        state.handlers[SIGINT as usize] = handler_addr;
        assert_eq!(state.get_handler(SIGINT), handler_addr);
        // Install SIG_IGN to ignore a signal.
        state.handlers[SIGTERM as usize] = SIG_IGN;
        assert_eq!(state.get_handler(SIGTERM), SIG_IGN);
        // Other signals unaffected.
        assert_eq!(state.get_handler(SIGHUP), SIG_DFL);
    }

    #[test]
    fn test_pending_cleared_after_next_pending() {
        let mut state = SignalState::new();
        state.send_signal(SIGQUIT);
        assert_ne!(state.pending & (1u64 << SIGQUIT), 0);
        let _ = state.next_pending();
        assert_eq!(state.pending & (1u64 << SIGQUIT), 0);
    }

    #[test]
    fn test_default_action_all_terminate_except_chld() {
        // Test all known signals.
        let ignore_signals = [SIGCHLD];
        let terminate_signals = [
            SIGHUP, SIGINT, SIGQUIT, SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGKILL, SIGUSR1,
            SIGSEGV, SIGUSR2, SIGPIPE, SIGALRM, SIGTERM,
        ];
        for &sig in &ignore_signals {
            assert_eq!(
                default_action(sig),
                DefaultAction::Ignore,
                "signal {} should be Ignore",
                sig
            );
        }
        for &sig in &terminate_signals {
            assert_eq!(
                default_action(sig),
                DefaultAction::Terminate,
                "signal {} should be Terminate",
                sig
            );
        }
    }

    #[test]
    fn test_signal_17_meaning() {
        // SIGCHLD = 17 is a POSIX standard.
        assert_eq!(SIGCHLD, 17);
        // Verify struct layout: pending(u64)=8, blocked(u64)=8, handlers([u64;32])=256
        let expected_size = 8 + 8 + 32 * 8; // 272
        assert_eq!(core::mem::size_of::<SignalState>(), expected_size as usize);
    }
}
