//! Round-robin task scheduler with context switching.
//!
//! Tasks are stored in a FIFO queue. When a task blocks (e.g., on `channel_receive`),
//! the scheduler saves its context and switches to the next ready task.
//! The context switch happens via the `SWITCH_CONTEXT` global in the syscall entry stub.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

use super::task::{SavedContext, Task, TaskId, TaskState};
use crate::println;

/// ID of the task currently on the CPU.
static CURRENT_TASK_ID: AtomicU64 = AtomicU64::new(0);

/// Storage for the context of the task being switched TO.
/// The assembly stub reads from this pointer after the syscall handler returns.
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
    is_kernel: 0,
};

lazy_static::lazy_static! {
    static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

struct Scheduler {
    ready_queue: VecDeque<Task>,
    blocked_queue: VecDeque<Task>,
    current_task: Option<TaskId>,
}

impl Scheduler {
    fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            blocked_queue: VecDeque::new(),
            current_task: None,
        }
    }

    fn add_task(&mut self, task: Task) {
        self.ready_queue.push_back(task);
    }

    fn find_task(&self, id: TaskId) -> Option<&Task> {
        self.ready_queue
            .iter()
            .find(|t| t.id == id)
            .or_else(|| self.blocked_queue.iter().find(|t| t.id == id))
    }

    fn find_task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.ready_queue
            .iter_mut()
            .find(|t| t.id == id)
            .or_else(|| self.blocked_queue.iter_mut().find(|t| t.id == id))
    }

    /// Pick the next ready task and make it current.
    fn schedule_next(&mut self) -> Option<&Task> {
        if let Some(mut task) = self.ready_queue.pop_front() {
            task.state = TaskState::Running;
            let id = task.id;
            self.current_task = Some(id);
            CURRENT_TASK_ID.store(id.as_u64(), Ordering::Release);
            self.ready_queue.push_back(task);
            self.ready_queue.back()
        } else {
            None
        }
    }

    /// Move a task from blocked to ready queue.
    fn wake_task(&mut self, id: TaskId) {
        if let Some(pos) = self.blocked_queue.iter().position(|t| t.id == id) {
            let mut task = self.blocked_queue.remove(pos).unwrap();
            task.state = TaskState::Ready;
            self.ready_queue.push_back(task);
        }
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

/// Spawn a new task and return its ID.
pub fn spawn_task_with_id(name: &str, priority: u8) -> TaskId {
    let task = Task::new(name, priority);
    let id = task.id;
    SCHEDULER.lock().add_task(task);
    id
}

/// Spawn a pre-constructed task.
pub fn spawn_task_from(task: Task) {
    SCHEDULER.lock().add_task(task);
}

/// Get the ID of the currently running task.
pub fn current_task_id() -> TaskId {
    TaskId::from_u64(CURRENT_TASK_ID.load(Ordering::Acquire))
}

/// Set the current task ID (called when launching a process).
pub fn set_current_task(id: TaskId) {
    CURRENT_TASK_ID.store(id.as_u64(), Ordering::Release);
}

/// Wake a blocked task by ID (move from blocked to ready queue).
pub fn wake_task_by_id(id: TaskId) {
    SCHEDULER.lock().wake_task(id);
}

/// Block the current task and switch to the next ready task.
///
/// Saves the current task's context, moves it to the blocked queue,
/// picks the next ready task, and sets `SWITCH_CONTEXT` so the syscall
/// entry stub restores the new task's context.
///
/// Returns `true` if a context switch happened, `false` if no other task
/// is ready (current task stays running).
#[allow(static_mut_refs)]
pub fn block_and_switch(current_ctx: SavedContext) -> bool {
    let mut scheduler = SCHEDULER.lock();

    let Some(current_id) = scheduler.current_task else {
        return false;
    };

    // Save current task's context and move to blocked queue.
    if let Some(pos) = scheduler
        .ready_queue
        .iter()
        .position(|t| t.id == current_id)
    {
        let mut task = scheduler.ready_queue.remove(pos).unwrap();
        task.state = TaskState::Blocked;
        task.context = Some(current_ctx);
        scheduler.blocked_queue.push_back(task);
    }

    // Pick the next ready task.
    if let Some(next) = scheduler.schedule_next() {
        let next_id = next.id;
        if let Some(ctx) = next.context {
            // Store context for the assembly stub to restore.
            unsafe {
                NEXT_CTX_STORAGE = ctx;
                crate::arch::x86_64::syscall::SWITCH_CONTEXT =
                    core::ptr::addr_of!(NEXT_CTX_STORAGE);
            }
            crate::serial_println!(
                "[SCHED] switch {} -> {}",
                current_id.as_u64(),
                next_id.as_u64()
            );
            return true;
        }
    }

    false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_unique() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_task_id_monotonic() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert!(id2.as_u64() > id1.as_u64());
    }

    #[test]
    fn test_task_id_roundtrip() {
        let id = TaskId::new();
        let raw = id.as_u64();
        let id2 = TaskId::from_u64(raw);
        assert_eq!(id, id2);
    }

    #[test]
    fn test_task_state_transitions() {
        assert_ne!(TaskState::Ready, TaskState::Running);
        assert_ne!(TaskState::Running, TaskState::Blocked);
        assert_ne!(TaskState::Blocked, TaskState::Terminated);
    }

    #[test]
    fn test_task_creation() {
        let task = Task::new("test", 5);
        assert_eq!(task.name, "test");
        assert_eq!(task.priority, 5);
        assert_eq!(task.state, TaskState::Ready);
        assert!(task.context.is_none());
    }

    #[test]
    fn test_scheduler_add_and_find() {
        let mut scheduler = Scheduler::new();
        let task = Task::new("test_task", 0);
        let id = task.id;
        scheduler.add_task(task);

        assert!(scheduler.find_task(id).is_some());
        assert_eq!(scheduler.find_task(id).unwrap().name, "test_task");
    }

    #[test]
    fn test_scheduler_find_nonexistent() {
        let scheduler = Scheduler::new();
        let fake_id = TaskId::new();
        assert!(scheduler.find_task(fake_id).is_none());
    }

    #[test]
    fn test_scheduler_find_mut() {
        let mut scheduler = Scheduler::new();
        let task = Task::new("mutable", 0);
        let id = task.id;
        scheduler.add_task(task);

        let task = scheduler.find_task_mut(id).unwrap();
        task.state = TaskState::Running;
        assert_eq!(scheduler.find_task(id).unwrap().state, TaskState::Running);
    }

    #[test]
    fn test_current_task_id_default() {
        CURRENT_TASK_ID.store(0, Ordering::Release);
        let id = current_task_id();
        assert_eq!(id.as_u64(), 0);
    }

    #[test]
    fn test_set_current_task() {
        let id = TaskId::new();
        set_current_task(id);
        assert_eq!(current_task_id(), id);
    }

    #[test]
    fn test_with_current_task() {
        let mut scheduler = SCHEDULER.lock();
        let task = Task::new("current", 0);
        let id = task.id;
        scheduler.add_task(task);
        drop(scheduler);

        set_current_task(id);
        let name = with_current_task(|t| t.name.clone());
        assert_eq!(name, Some("current".to_string()));
    }

    #[test]
    fn test_with_current_task_not_found() {
        let fake_id = TaskId::new();
        set_current_task(fake_id);
        let result = with_current_task(|t| t.name.clone());
        assert!(result.is_none());
    }

    #[test]
    fn test_with_current_task_mut() {
        let mut scheduler = SCHEDULER.lock();
        let task = Task::new("mutable_task", 0);
        let id = task.id;
        scheduler.add_task(task);
        drop(scheduler);

        set_current_task(id);
        with_current_task_mut(|t| {
            t.priority = 10;
        });
        let priority = with_current_task(|t| t.priority);
        assert_eq!(priority, Some(10));
    }
}
