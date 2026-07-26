//! Task (process) abstraction.
//!
//! A task is the kernel's unit of execution. Each task has its own
//! `SavedContext` (registers), handle table, and scheduling state.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::handle::HandleTable;
use crate::net::socket::SocketTable;

/// A file descriptor entry mapping an fd number to a ramfs filename and offset.
pub struct FdEntry {
    /// Filename in the ramfs.
    pub name: String,
    /// Current read/write offset within the file.
    pub offset: usize,
}

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
    pub r11: u64,       // RFLAGS
    pub rcx: u64,       // RIP
    pub rsp: u64,       // RSP
    pub is_kernel: u64, // 1 = kernel task (use IRETQ), 0 = user task (use SYSRET)
    pub cr3: u64,       // Page table physical address (loaded into CR3 on switch)
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
            is_kernel: 0,
            cr3: 0,
        }
    }

    /// Create a context for a new user-space task (Ring 3, SYSRET).
    pub fn user_mode(entry: u64, stack_top: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rcx = entry;
        ctx.rsp = stack_top;
        ctx.r11 = 0x202; // RFLAGS with IF=1
        ctx.is_kernel = 0;
        ctx
    }

    /// Create a context for a new kernel-space task (Ring 0, IRETQ).
    pub fn kernel_mode(entry: u64, stack_top: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rcx = entry;
        ctx.rsp = stack_top;
        ctx.r11 = 0x202; // RFLAGS with IF=1
        ctx.is_kernel = 1; // Use IRETQ path
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
    /// Per-task service namespace. Maps service names to Channel handles.
    /// Populated by the parent process before `process_start`.
    pub namespace: BTreeMap<String, crate::handle::Handle>,
    /// Parent task ID. `None` for the root task (idle).
    pub parent_id: Option<TaskId>,
    /// Exit status. `None` while running, `Some(status)` after exit.
    pub exit_status: Option<u64>,
    /// Physical address of the task's P4 page table (CR3 value).
    /// `None` means the task uses the kernel's page table (idle task).
    /// User tasks get their own P4 with kernel entries copied from the
    /// kernel's P4 and user entries mapped independently.
    pub page_table: Option<u64>,
    /// Per-task file descriptor table. FD 0 (stdin) and FD 1 (stdout) are
    /// special-cased and not stored here. Real file descriptors start at 2.
    pub fd_table: BTreeMap<u64, FdEntry>,
    /// Per-task socket table. Maps socket descriptors to socket state.
    pub socket_table: SocketTable,
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
            namespace: BTreeMap::new(),
            parent_id: None,
            exit_status: None,
            page_table: None,
            fd_table: BTreeMap::new(),
            socket_table: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_ordering() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert!(id1 < id2);
    }

    #[test]
    fn test_task_id_debug() {
        let id = TaskId::new();
        let debug = format!("{:?}", id);
        assert!(!debug.is_empty());
    }

    #[test]
    fn test_task_state_all_variants() {
        let states = [
            TaskState::Ready,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::Terminated,
        ];
        for i in 0..states.len() {
            for j in i + 1..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[test]
    fn test_saved_context_new_zeroed() {
        let ctx = SavedContext::new();
        assert_eq!(ctx.rax, 0);
        assert_eq!(ctx.rbx, 0);
        assert_eq!(ctx.rcx, 0);
        assert_eq!(ctx.rdx, 0);
        assert_eq!(ctx.rsi, 0);
        assert_eq!(ctx.rdi, 0);
        assert_eq!(ctx.rsp, 0);
        assert_eq!(ctx.rbp, 0);
        assert_eq!(ctx.r11, 0);
        assert_eq!(ctx.is_kernel, 0);
        assert_eq!(ctx.cr3, 0);
    }

    #[test]
    fn test_saved_context_user_mode() {
        let ctx = SavedContext::user_mode(0x401000, 0x800000000000);
        assert_eq!(ctx.rcx, 0x401000); // entry point
        assert_eq!(ctx.rsp, 0x800000000000); // stack top
        assert_eq!(ctx.r11, 0x202); // RFLAGS with IF
        assert_eq!(ctx.is_kernel, 0); // user mode
    }

    #[test]
    fn test_saved_context_kernel_mode() {
        let ctx = SavedContext::kernel_mode(0x100000, 0x200000);
        assert_eq!(ctx.rcx, 0x100000);
        assert_eq!(ctx.rsp, 0x200000);
        assert_eq!(ctx.is_kernel, 1); // kernel mode
    }

    #[test]
    fn test_saved_context_repr_c() {
        // Verify the struct is laid out correctly for the assembly stub.
        let ctx = SavedContext::new();
        let base = &ctx as *const _ as u64;
        let r9_ptr = &ctx.r9 as *const _ as u64;
        assert_eq!(r9_ptr - base, 0); // r9 is at offset 0
    }

    #[test]
    fn test_task_new() {
        let task = Task::new("test", 5);
        assert_eq!(task.name, "test");
        assert_eq!(task.priority, 5);
        assert_eq!(task.state, TaskState::Ready);
        assert!(task.context.is_none());
    }

    #[test]
    fn test_task_id_unique() {
        let t1 = Task::new("a", 0);
        let t2 = Task::new("b", 0);
        assert_ne!(t1.id, t2.id);
    }
}
