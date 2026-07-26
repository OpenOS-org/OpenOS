//! Round-robin task scheduler.
//!
//! Tasks are stored in a FIFO queue. Each timer tick moves the front task
//! to the back. A global `CURRENT_TASK_ID` allows syscall handlers to
//! access the running task's handle table.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

use super::task::{Task, TaskId, TaskState};
use crate::println;

/// ID of the task currently on the `CPU`.
static CURRENT_TASK_ID: AtomicU64 = AtomicU64::new(0);

lazy_static::lazy_static! {
    static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

struct Scheduler {
    ready_queue: VecDeque<Task>,
    current_task: Option<TaskId>,
}

impl Scheduler {
    fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current_task: None,
        }
    }

    fn add_task(&mut self, task: Task) {
        self.ready_queue.push_back(task);
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

/// Wake a task by ID (currently a no-op since we don't have real blocking).
pub fn wake_task_by_id(_id: TaskId) {
    // In a full implementation, this would move the task from blocked to ready.
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
        // Reset to 0.
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
        // Create a scheduler with a task.
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
