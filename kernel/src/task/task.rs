//! Task (process) abstraction.
//!
//! A task is the kernel's unit of execution. Each task has its own
//! `SavedContext` (registers), handle table, and scheduling state.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::handle::HandleTable;
use crate::memory::vma::VmaList;
use crate::net::socket::SocketTable;
use crate::task::signal::SignalState;

/// Default umask value (0o022). New files are created with permissions
/// that have the group-write and other-write bits cleared.
pub const DEFAULT_UMASK: u16 = 0o022;

const ENV_HOME: &str = "/";
const ENV_PATH: &str = "/bin:/usr/bin";
const ENV_SHELL: &str = "/bin/sh";
const ENV_USER: &str = "root";

/// Create the default environment variables for a new task.
#[must_use]
pub fn default_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(String::from("HOME"), String::from(ENV_HOME));
    env.insert(String::from("PATH"), String::from(ENV_PATH));
    env.insert(String::from("SHELL"), String::from(ENV_SHELL));
    env.insert(String::from("USER"), String::from(ENV_USER));
    env
}

/// A shared memory attachment record for a task.
///
/// Tracks which shared memory segments are mapped into the task's address
/// space so that `shmdt` can look up the correct segment by virtual address
/// and cleanup can detach all segments on task exit.
pub struct ShmAttachment {
    /// Shared memory segment ID (from `shmget`).
    pub shmid: u32,
    /// Virtual address where the segment is mapped.
    pub virt_addr: u64,
    /// Size of the attached segment in bytes.
    pub size: u64,
}

/// A file descriptor entry mapping an fd number to a VFS path, inode, and offset.
pub struct FdEntry {
    /// Full path (with mount prefix) for VFS dispatch.
    pub path: String,
    /// Inode number returned by the filesystem's `open`.
    pub ino: u64,
    /// Current read/write offset within the file.
    pub offset: usize,
}

/// Globally unique task identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    /// Create a new globally unique task ID.
    #[must_use]
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Get the raw `u64` value of this task ID.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Create a task ID from a `u64` value.
    #[must_use]
    pub fn from_u64(val: u64) -> Self {
        Self(val)
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// Execution state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is ready to run.
    Ready,
    /// Task is currently running.
    Running,
    /// Task is blocked waiting for an event.
    Blocked,
    /// Task has terminated.
    Terminated,
}

/// Saved register state for context switching.
///
/// Layout matches the syscall entry assembly stub — registers are pushed
/// in this order, and the same order is used for restoration.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SavedContext {
    /// Register R9.
    pub r9: u64,
    /// Register R8.
    pub r8: u64,
    /// Register RDX.
    pub rdx: u64,
    /// Register RSI.
    pub rsi: u64,
    /// Register RDI.
    pub rdi: u64,
    /// Register RAX.
    pub rax: u64,
    /// Register R15.
    pub r15: u64,
    /// Register R14.
    pub r14: u64,
    /// Register R13.
    pub r13: u64,
    /// Register R12.
    pub r12: u64,
    /// Register RBX.
    pub rbx: u64,
    /// Register RBP.
    pub rbp: u64,
    /// Register R11 (RFLAGS on SYSRET).
    pub r11: u64,
    /// Register RCX (RIP on SYSRET).
    pub rcx: u64,
    /// Stack pointer (RSP).
    pub rsp: u64,
    /// 1 = kernel task (use IRETQ), 0 = user task (use SYSRET).
    pub is_kernel: u64,
    /// Page table physical address (loaded into CR3 on switch).
    pub cr3: u64,
}

impl SavedContext {
    /// Create a zeroed context.
    #[must_use]
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
    ///
    /// `cr3` is the physical address of the task's P4 page table. If zero,
    /// the current page table is retained on context switch (kernel task).
    #[must_use]
    pub fn user_mode(entry: u64, stack_top: u64, cr3: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rcx = entry;
        ctx.rsp = stack_top;
        ctx.r11 = 0x202; // RFLAGS with IF=1
        ctx.is_kernel = 0;
        ctx.cr3 = cr3;
        ctx
    }

    /// Create a context for a new kernel-space task (Ring 0, IRETQ).
    #[must_use]
    pub fn kernel_mode(entry: u64, stack_top: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rcx = entry;
        ctx.rsp = stack_top;
        ctx.r11 = 0x202; // RFLAGS with IF=1
        ctx.is_kernel = 1; // Use IRETQ path
        ctx
    }
}

impl Default for SavedContext {
    fn default() -> Self {
        Self::new()
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
    /// Per-task virtual memory area list for address space validation.
    pub vma_list: VmaList,
    /// Current program break address (heap end). 0 if not set.
    pub brk: u64,
    /// Per-task environment variables.
    pub env: BTreeMap<String, String>,
    /// Current working directory. Defaults to `"/"`.
    pub cwd: String,
    /// Per-task signal state (pending, blocked, handlers).
    pub signal_state: SignalState,
    /// File mode creation mask (umask). Defaults to 0o022.
    /// Bits set in the umask are cleared from newly created file permissions.
    pub umask: u16,
    /// Process group ID. Defaults to the task's own ID.
    pub pgid: u64,
    /// Session ID. Defaults to the task's own ID.
    pub sid: u64,
    /// Per-task shared memory attachments. Tracks segments mapped via `shmat`
    /// so that `shmdt` can look up the correct segment by virtual address
    /// and cleanup can detach all segments on task exit.
    pub shm_attachments: alloc::vec::Vec<ShmAttachment>,
}

impl Task {
    /// Create a new task in the `Ready` state with a fresh ID.
    #[must_use]
    pub fn new(name: &str, priority: u8) -> Self {
        let id = TaskId::new();
        Self {
            pgid: id.as_u64(),
            sid: id.as_u64(),
            id,
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
            vma_list: VmaList::new(),
            brk: 0,
            env: default_env(),
            cwd: String::from("/"),
            signal_state: SignalState::new(),
            umask: DEFAULT_UMASK,
            shm_attachments: alloc::vec::Vec::new(),
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
        let ctx = SavedContext::user_mode(0x401000, 0x800000000000, 0x1234_0000);
        assert_eq!(ctx.rcx, 0x401000); // entry point
        assert_eq!(ctx.rsp, 0x800000000000); // stack top
        assert_eq!(ctx.r11, 0x202); // RFLAGS with IF
        assert_eq!(ctx.is_kernel, 0); // user mode
        assert_eq!(ctx.cr3, 0x1234_0000); // page table
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

    #[test]
    fn test_saved_context_user_mode_thread_setup() {
        // Simulate what sys_thread_create does: build a user-mode context
        // then set rdi to the thread argument.
        let entry = 0x40_1000_u64;
        let stack = 0x7FFF_FFFF_F000_u64;
        let cr3 = 0x1000_u64;
        let arg = 0xDEAD_BEEF_u64;

        let mut ctx = SavedContext::user_mode(entry, stack, cr3);
        ctx.rdi = arg;

        // Verify all fields for SYSRET-based thread launch.
        assert_eq!(ctx.rcx, entry, "rcx must be entry point (RIP for SYSRET)");
        assert_eq!(ctx.rsp, stack, "rsp must be the thread stack pointer");
        assert_eq!(ctx.rdi, arg, "rdi must hold the thread argument");
        assert_eq!(ctx.cr3, cr3, "cr3 must be the shared page table");
        assert_eq!(ctx.is_kernel, 0, "is_kernel must be 0 for user mode");
        assert_eq!(ctx.r11, 0x202, "r11 must be RFLAGS with IF=1");
    }

    #[test]
    fn test_saved_context_user_mode_rdi_independent() {
        // Verify that rdi does not leak from a prior context.
        let mut ctx = SavedContext::user_mode(0x40_0000, 0x8000_0000, 0x2000);
        assert_eq!(ctx.rdi, 0, "rdi must start at 0 in fresh user_mode context");
        ctx.rdi = 99;
        assert_eq!(ctx.rdi, 99);
    }

    #[test]
    fn test_task_thread_fields_default() {
        // A freshly created task should have no context, no page table,
        // and no parent — these are set by sys_thread_create.
        let task = Task::new("thread", 5);
        assert!(task.context.is_none());
        assert!(task.page_table.is_none());
        assert!(task.parent_id.is_none());
        assert_eq!(task.state, TaskState::Ready);
    }

    #[test]
    fn test_task_pgid_sid_default_to_own_id() {
        // pgid and sid should default to the task's own ID.
        let task = Task::new("leader", 5);
        assert_eq!(task.pgid, task.id.as_u64());
        assert_eq!(task.sid, task.id.as_u64());
    }

    #[test]
    fn test_task_pgid_sid_can_be_changed() {
        // pgid and sid should be mutable.
        let mut task = Task::new("member", 5);
        let leader_id = 42u64;
        task.pgid = leader_id;
        task.sid = leader_id;
        assert_eq!(task.pgid, leader_id);
        assert_eq!(task.sid, leader_id);
    }

    #[test]
    fn test_default_umask_value() {
        // Default umask should be 0o022 (group-write and other-write cleared).
        assert_eq!(DEFAULT_UMASK, 0o022);
    }

    #[test]
    fn test_task_umask_default() {
        // A freshly created task should have the default umask.
        let task = Task::new("test", 5);
        assert_eq!(task.umask, DEFAULT_UMASK);
    }

    #[test]
    fn test_task_umask_can_be_changed() {
        // umask should be mutable.
        let mut task = Task::new("test", 5);
        task.umask = 0o077;
        assert_eq!(task.umask, 0o077);
    }

    #[test]
    fn test_default_env_values() {
        let env = default_env();
        assert_eq!(env.len(), 4);
        assert_eq!(env["HOME"], "/");
        assert_eq!(env["PATH"], "/bin:/usr/bin");
        assert_eq!(env["SHELL"], "/bin/sh");
        assert_eq!(env["USER"], "root");
    }

    #[test]
    fn test_task_new_has_default_env() {
        let task = Task::new("test", 5);
        assert_eq!(task.env.len(), 4);
    }

    #[test]
    fn test_task_env_overwrite() {
        let mut task = Task::new("test", 5);
        task.env.insert(String::from("HOME"), String::from("/c"));
        assert_eq!(task.env.len(), 4);
    }

    #[test]
    fn test_task_env_inheritance() {
        let mut p = Task::new("p", 5);
        p.env.insert(String::from("X"), String::from("1"));
        let mut c = Task::new("c", 5);
        c.env = p.env.clone();
        assert_eq!(c.env["X"], "1");
    }

    #[test]
    fn test_task_env_missing_key() {
        assert!(Task::new("t", 5).env.get("NOPE").is_none());
    }
}
