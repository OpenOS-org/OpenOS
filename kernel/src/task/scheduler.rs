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

/// ID of the task currently on the CPU.
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

    fn schedule(&mut self) -> Option<&Task> {
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

    fn block_task(&mut self, id: TaskId) -> Option<Task> {
        if let Some(pos) = self.ready_queue.iter().position(|t| t.id == id) {
            let mut task = self.ready_queue.remove(pos).unwrap();
            task.state = TaskState::Blocked;
            Some(task)
        } else {
            None
        }
    }
}

/// Initialize the scheduler with a single idle task (priority 0).
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

/// Get the ID of the currently running task.
pub fn current_task_id() -> TaskId {
    TaskId::from_u64(CURRENT_TASK_ID.load(Ordering::Acquire))
}

/// Set the current task ID (called when launching a process).
pub fn set_current_task(id: TaskId) {
    CURRENT_TASK_ID.store(id.as_u64(), Ordering::Release);
}

/// Block the current task (remove from ready queue).
pub fn block_current_task() {
    let id = current_task_id();
    SCHEDULER.lock().block_task(id);
}

/// Wake a task by ID (add back to ready queue).
pub fn wake_task_by_id(id: TaskId) {
    let mut scheduler = SCHEDULER.lock();
    for task in &mut scheduler.ready_queue {
        if task.id == id {
            task.state = TaskState::Ready;
            return;
        }
    }
    // Task not in ready queue — it was blocked and removed.
    // For Phase 3, we silently ignore. A full implementation would
    // store blocked tasks in a separate list and re-add them here.
    drop(scheduler);
}

/// Execute a closure with a shared reference to the current task's handle table.
/// Returns `None` if the current task is not found.
pub fn with_current_task<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&Task) -> R,
{
    let id = current_task_id();
    let scheduler = SCHEDULER.lock();
    scheduler.find_task(id).map(f)
}

/// Execute a closure with a mutable reference to the current task's handle table.
/// Returns `None` if the current task is not found.
pub fn with_current_task_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Task) -> R,
{
    let id = current_task_id();
    let mut scheduler = SCHEDULER.lock();
    scheduler.find_task_mut(id).map(f)
}

impl Scheduler {
    fn find_task(&self, id: TaskId) -> Option<&Task> {
        self.ready_queue.iter().find(|t| t.id == id)
    }

    fn find_task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.ready_queue.iter_mut().find(|t| t.id == id)
    }
}
