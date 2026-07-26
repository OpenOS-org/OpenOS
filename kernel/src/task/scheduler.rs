//! Round-robin task scheduler with context switching.
//!
//! When a task blocks (e.g., on a channel receive), the scheduler saves
//! the current task's register `context` and switches to the next ready task.
//! The context switch happens at the end of the syscall handler — the
//! assembly stub checks `SWITCH_CONTEXT` and restores the new task's
//! registers instead of the original caller's.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

use super::task::{SavedContext, Task, TaskId, TaskState};
use crate::println;

/// ID of the task currently on the `CPU`.
static CURRENT_TASK_ID: AtomicU64 = AtomicU64::new(0);

/// Storage for the context of the task being switched TO.
/// The assembly stub reads from this pointer after the syscall handler returns.
/// Using a static ensures the pointer remains valid after the function returns.
static mut NEXT_CTX_STORAGE: SavedContext = SavedContext {
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
};

lazy_static::lazy_static! {
    static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

struct Scheduler {
    ready_queue: VecDeque<Task>,
    current_task: Option<TaskId>,
    /// Tasks that are blocked. Stored separately from the ready queue.
    blocked_tasks: Vec<Task>,
}

impl Scheduler {
    fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current_task: None,
            blocked_tasks: Vec::new(),
        }
    }

    fn add_task(&mut self, task: Task) {
        self.ready_queue.push_back(task);
    }

    /// Pick the next ready task and return its ID. Does not switch context.
    fn pick_next(&self) -> Option<TaskId> {
        // Round-robin: take from front.
        self.ready_queue.front().map(|t| t.id)
    }

    /// Remove the current task from the ready queue and store it as blocked.
    /// Returns the blocked task's `SavedContext`.
    fn block_current(&mut self) -> Option<SavedContext> {
        let id = self.current_task?;
        if let Some(pos) = self.ready_queue.iter().position(|t| t.id == id) {
            let mut task = self.ready_queue.remove(pos).unwrap();
            task.state = TaskState::Blocked;
            let ctx = task.context.take();
            self.blocked_tasks.push(task);
            ctx
        } else {
            None
        }
    }

    /// Wake a blocked task by ID. Moves it back to the ready queue.
    fn wake_task(&mut self, id: TaskId) {
        if let Some(pos) = self.blocked_tasks.iter().position(|t| t.id == id) {
            let mut task = self.blocked_tasks.remove(pos);
            task.state = TaskState::Ready;
            self.ready_queue.push_back(task);
        }
    }

    /// Switch from the current task to the next ready task.
    /// Returns the next task's `SavedContext` (to be restored by the assembly stub).
    fn switch_to_next(&mut self, current_ctx: SavedContext) -> Option<SavedContext> {
        // Save current task's context.
        let current_id = self.current_task?;
        if let Some(task) = self.ready_queue.iter_mut().find(|t| t.id == current_id) {
            task.context = Some(current_ctx);
        }

        // Pick next task (round-robin: rotate the queue).
        if self.ready_queue.len() > 1 {
            self.ready_queue.rotate_left(1);
        }
        let next = self.ready_queue.front()?;
        let next_id = next.id;
        let next_ctx = next.context?;

        self.current_task = Some(next_id);
        CURRENT_TASK_ID.store(next_id.as_u64(), Ordering::Release);
        Some(next_ctx)
    }

    fn find_task(&self, id: TaskId) -> Option<&Task> {
        self.ready_queue.iter().find(|t| t.id == id)
    }

    fn find_task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.ready_queue.iter_mut().find(|t| t.id == id)
    }
}

/// Initialize the scheduler with a single idle task.
pub fn init() {
    let idle_task = Task::new("idle", 0);
    CURRENT_TASK_ID.store(idle_task.id.as_u64(), Ordering::Release);
    SCHEDULER.lock().add_task(idle_task);
    println!("[OK] Idle task created");
}

/// Spawn a new task and add it to the ready queue.
pub fn spawn_task(name: &str, priority: u8) {
    let task = Task::new(name, priority);
    SCHEDULER.lock().add_task(task);
}

/// Spawn a task with a specific user-mode entry point and stack.
pub fn spawn_user_task(name: &str, entry: u64, stack_top: u64) -> TaskId {
    let mut task = Task::new(name, 0);
    task.context = Some(SavedContext::user_mode(entry, stack_top));
    let id = task.id;
    SCHEDULER.lock().add_task(task);
    id
}

/// Get the ID of the currently running task.
pub fn current_task_id() -> TaskId {
    TaskId::from_u64(CURRENT_TASK_ID.load(Ordering::Acquire))
}

/// Set the current task ID (called when launching a process).
pub fn set_current_task(id: TaskId) {
    CURRENT_TASK_ID.store(id.as_u64(), Ordering::Release);
}

/// Block the current task and switch to the next ready task.
///
/// Returns `true` if a context switch happened (the caller should restore
/// the new context), `false` if no other task is ready.
#[allow(static_mut_refs)]
pub fn block_and_switch(current_ctx: SavedContext) -> bool {
    let mut scheduler = SCHEDULER.lock();

    // Save current task and remove from ready queue.
    let _blocked_ctx = scheduler.block_current();

    // Try to switch to the next ready task.
    #[allow(static_mut_refs)]
    scheduler
        .switch_to_next(current_ctx)
        .is_some_and(|new_ctx| {
            // Store the new context in a static so the pointer remains valid
            // after this function returns. The assembly stub reads from
            // SWITCH_CONTEXT during SYSRET.
            unsafe {
                NEXT_CTX_STORAGE = new_ctx;
                crate::arch::x86_64::syscall::SWITCH_CONTEXT =
                    core::ptr::addr_of!(NEXT_CTX_STORAGE);
            }
            true
        })
}

/// Wake a task by ID (add back to ready queue).
pub fn wake_task_by_id(id: TaskId) {
    SCHEDULER.lock().wake_task(id);
}

/// Execute a closure with a shared reference to the current task.
pub fn with_current_task<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&Task) -> R,
{
    let id = current_task_id();
    let scheduler = SCHEDULER.lock();
    scheduler.find_task(id).map(f)
}

/// Execute a closure with a mutable reference to the current task.
pub fn with_current_task_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Task) -> R,
{
    let id = current_task_id();
    let mut scheduler = SCHEDULER.lock();
    scheduler.find_task_mut(id).map(f)
}
