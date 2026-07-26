//! Task (process) abstraction.
//!
//! A task is the kernel's unit of execution. Each task has its own
//! `SavedContext` (registers), handle table, and scheduling state.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::handle::HandleTable;

/// Globally unique task identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn from_u64(val: u64) -> Self {
        Self(val)
    }
}

/// Execution state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

/// Saved register state for context switching.
///
/// Layout matches the syscall entry assembly stub — registers are pushed
/// in this order, and the same order is used for restoration.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SavedContext {
    // Saved by assembly stub (syscall entry)
    pub r9: u64,
    pub r8: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rax: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r11: u64, // user RFLAGS
    pub rcx: u64, // user RIP
    pub rsp: u64, // user RSP (saved separately by syscall handler)
}

impl SavedContext {
    /// Create a zeroed context.
    pub fn new() -> Self {
        Self {
            r9: 0,
            r8: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rax: 0,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbx: 0,
            rbp: 0,
            r11: 0,
            rcx: 0,
            rsp: 0,
        }
    }

    /// Create a context for a new user-space task.
    pub fn user_mode(entry: u64, stack_top: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rcx = entry; // user RIP (restored by SYSRET)
        ctx.rsp = stack_top; // user RSP
        ctx.r11 = 0x202; // RFLAGS with IF=1 (interrupts enabled)
        ctx.rax = 0;
        ctx
    }
}

/// Task control block (TCB).
pub struct Task {
    /// Monotonically increasing, globally unique.
    pub id: TaskId,
    /// Human-readable label.
    pub name: String,
    /// Current scheduling state.
    pub state: TaskState,
    /// Higher value = higher priority.
    pub priority: u8,
    /// Per-task handle table — the capability set for this task.
    pub handle_table: HandleTable,
    /// Saved register state for context switching.
    /// `None` for tasks that haven't run yet.
    pub context: Option<SavedContext>,
}

impl Task {
    /// Create a new task in the `Ready` state with a fresh ID.
    #[must_use]
    pub fn new(name: &str, priority: u8) -> Self {
        Self {
            id: TaskId::new(),
            name: String::from(name),
            state: TaskState::Ready,
            priority,
            handle_table: HandleTable::new(),
            context: None,
        }
    }
}
