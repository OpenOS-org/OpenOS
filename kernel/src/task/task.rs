//! Task (process) abstraction.
//!
//! A task is the kernel's unit of execution. In a full microkernel, each task
//! has its own address space, kernel stack, and set of capabilities. For now,
//! we model only the scheduling metadata — context switching will be added
//! when we implement preemptive scheduling.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

/// Globally unique task identifier.
///
/// Uses an atomic counter so `TaskId::new()` is lock-free and safe to call
/// from any context (including interrupt handlers, once we have preemptive
/// scheduling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    /// Allocate the next available task ID. IDs are monotonically increasing
    /// and never reused (u64 overflow is not a practical concern).
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Execution state of a task.
///
/// The scheduler uses this to decide which tasks are eligible to run.
/// `Blocked` is for tasks waiting on I/O or IPC; `Terminated` marks tasks
/// that have exited but haven't been cleaned up yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

/// Task control block (TCB). Contains all metadata the scheduler needs to
/// make scheduling decisions. Context-switch state (registers, page table)
/// will be added here when we implement preemptive multitasking.
pub struct Task {
    /// Monotonically increasing, globally unique.
    pub id: TaskId,
    /// Human-readable label (e.g., "idle", "`fs_server`").
    pub name: String,
    /// Current scheduling state.
    pub state: TaskState,
    /// Higher value = higher priority. The scheduler doesn't use this yet.
    pub priority: u8,
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
        }
    }
}
