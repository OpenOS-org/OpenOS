//! Round-robin task scheduler.
//!
//! The simplest possible preemptive scheduler: tasks are stored in a FIFO
//! queue, and each timer tick moves the front task to the back. This gives
//! every task equal CPU time regardless of how long it runs.
//!
//! Limitations (to be addressed):
//!   - No actual context switching yet (no register save/restore)
//!   - Priority is ignored (strict round-robin)
//!   - No sleep/block/wake primitives
//!   - No per-CPU run queues (SMP support)

use alloc::collections::VecDeque;

use spin::Mutex;

use super::task::{Task, TaskId, TaskState};
use crate::println;

lazy_static::lazy_static! {
    static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

struct Scheduler {
    /// FIFO queue of tasks eligible to run. Front = currently running.
    ready_queue: VecDeque<Task>,
    /// ID of the task currently on the CPU. Used to identify the running task
    /// for accounting and future context-switch logic.
    current_task: Option<TaskId>,
}

impl Scheduler {
    fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current_task: None,
        }
    }

    /// Enqueue a task for scheduling. Called from `spawn_task` and during init.
    fn add_task(&mut self, task: Task) {
        self.ready_queue.push_back(task);
    }

    /// Select the next task to run. Round-robin: take from front, put at back,
    /// return a reference to the task that should now be running.
    fn schedule(&mut self) -> Option<&Task> {
        if let Some(mut task) = self.ready_queue.pop_front() {
            task.state = TaskState::Running;
            self.current_task = Some(task.id);
            self.ready_queue.push_back(task);
            self.ready_queue.back()
        } else {
            None
        }
    }
}

/// Initialize the scheduler with a single idle task (priority 0).
///
/// The idle task is always present in the run queue so `schedule()` never
/// returns `None` — the CPU can always fall back to the idle loop (`hlt`).
pub fn init() {
    let idle_task = Task::new("idle", 0);
    SCHEDULER.lock().add_task(idle_task);
    println!("[OK] Idle task created");
}

/// Spawn a new task and add it to the ready queue.
pub fn spawn_task(name: &str, priority: u8) {
    let task = Task::new(name, priority);
    SCHEDULER.lock().add_task(task);
}
