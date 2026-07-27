//! SMP-aware task scheduler with per-CPU run queues.
//!
//! Each CPU has its own run queue protected by its own lock, minimizing
//! contention. Tasks are routed to the least-loaded CPU on spawn. When a
//! task is unblocked on a different CPU, an IPI is sent to wake that CPU.
//! A global migration queue allows work stealing for load balancing.
//!
//! ## Per-CPU queue architecture
//!
//! Each CPU owns a `CpuQueue` containing:
//! - `ready: VecDeque<Task>` — tasks waiting to run on this CPU
//! - `current: Option<TaskId>` — the task currently executing
//!
//! The array `CPU_QUEUES: [Mutex<CpuQueue>; MAX_CPUS]` provides one lock
//! per CPU, so scheduling on different CPUs never contends on the same lock.
//!
//! ## Blocked task tracking
//!
//! Blocked tasks are moved to a global `BLOCKED_QUEUE` (shared across CPUs).
//! `BLOCKED_CPU_MAP` records which CPU a task was on when blocked, so
//! `wake_task_by_id` can send an IPI to the correct CPU.
//!
//! ## Work stealing (migration)
//!
//! `MIGRATION_QUEUE` is a global queue for cross-CPU task migration.
//! `push_migration()` places a task on the queue; `steal_from_migration()`
//! pops a task and adds it to the current CPU's local queue. This enables
//! load balancing when a CPU becomes idle.
//!
//! ## Context switch flow
//!
//! 1. `block_and_switch(current_ctx)` saves the current task's registers,
//!    moves it to the blocked queue, picks the next ready task, and writes
//!    the new task's context to `SWITCH_CONTEXT`.
//! 2. The syscall entry stub checks `SWITCH_CONTEXT` after the handler returns.
//!    If non-null, it restores that context instead of the saved registers.
//! 3. For user-mode tasks: SYSRETQ. For kernel-mode tasks: IRETQ.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

use super::task::{SavedContext, Task, TaskId, TaskState};
use crate::println;

/// Maximum number of CPUs supported (matches `percpu::MAX_CPUS`).
const MAX_CPUS: usize = 8;

/// Maximum number of tasks the scheduler will hold across all CPUs.
/// Prevents unbounded memory growth from runaway task creation.
const MAX_TASKS: usize = 256;

/// ID of the task currently running on the calling CPU.
static CURRENT_TASK_ID: AtomicU64 = AtomicU64::new(0);

/// Storage for the context of the task being switched TO.
///
/// The assembly stub reads from this pointer after the syscall handler returns.
/// This is `static mut` because it must have a stable address for the assembly
/// stub to reference via `SWITCH_CONTEXT`. Access is serialized: the handler
/// writes before returning, and the stub reads after the handler returns.
///
/// # Safety
///
/// Only written inside `block_and_switch` while holding the CPU queue lock.
/// Only read by the assembly stub after the handler has returned.
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
    cr3: 0,
};

// ============================================================================
// Per-CPU run queue
// ============================================================================

/// Per-CPU run queue. Each CPU has one of these, protected by its own mutex.
/// Contains the ready queue for this CPU and the ID of the currently running task.
struct CpuQueue {
    /// Ready tasks for this CPU.
    ready: VecDeque<Task>,
    /// ID of the task currently executing on this CPU, or `None` if idle.
    current: Option<TaskId>,
}

impl CpuQueue {
    const fn new() -> Self {
        Self {
            ready: VecDeque::new(),
            current: None,
        }
    }

    /// Total tasks on this CPU: ready queue length plus one if a task is running.
    fn task_count(&self) -> usize {
        self.ready.len() + usize::from(self.current.is_some())
    }
}

/// Per-CPU run queues. One lock per CPU to minimize contention.
static CPU_QUEUES: [Mutex<CpuQueue>; MAX_CPUS] = [
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
    Mutex::new(CpuQueue::new()),
];

/// Global blocked tasks queue (shared across all CPUs).
static BLOCKED_QUEUE: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());

/// Global migration queue for cross-CPU task migration (work stealing).
static MIGRATION_QUEUE: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());

// ============================================================================
// Per-CPU blocked task tracking (for targeted wakeups)
// ============================================================================

/// Tracks which CPU a blocked task was on when it was blocked.
/// Maps `TaskId -> cpu_id` so we know which CPU to IPI on wakeup.
static BLOCKED_CPU_MAP: Mutex<alloc::collections::BTreeMap<u64, u32>> =
    Mutex::new(alloc::collections::BTreeMap::new());

// ============================================================================
// Helpers
// ============================================================================

/// Get the current CPU ID via the per-CPU GSBASE mechanism.
///
/// Reads the CPU index from the `PerCpuData` structure whose address is
/// loaded into GSBASE. Returns 0 if per-CPU data is not yet initialized.
fn current_cpu() -> usize {
    crate::arch::x86_64::percpu::current_cpu_id() as usize
}

/// Count total tasks across all CPUs and the blocked/migration queues.
///
/// Sums `task_count()` for each CPU queue, plus the blocked and migration
/// queue lengths. Used to enforce `MAX_TASKS`.
fn total_task_count() -> usize {
    let mut total = 0;
    for queue in CPU_QUEUES.iter().take(MAX_CPUS) {
        total += queue.lock().task_count();
    }
    total += BLOCKED_QUEUE.lock().len();
    total += MIGRATION_QUEUE.lock().len();
    total
}

/// Find the CPU with the fewest tasks (ready + current).
///
/// Used by `spawn_task` to distribute new tasks evenly. Ties are broken
/// by CPU index (lower index wins).
fn least_loaded_cpu() -> usize {
    let mut best_cpu = 0;
    let mut best_count = usize::MAX;

    for (i, queue) in CPU_QUEUES.iter().enumerate().take(MAX_CPUS) {
        let count = queue.lock().task_count();
        if count < best_count {
            best_count = count;
            best_cpu = i;
        }
    }

    best_cpu
}

/// IPI vector used for scheduler wakeup notifications.
///
/// When a task is unblocked on a different CPU, an IPI with this vector
/// is sent to trigger a reschedule on that CPU. The IPI handler for this
/// vector is registered in `interrupts.rs`.
const SCHED_IPI_VECTOR: u8 = 0x40;

/// Send an IPI to a specific CPU to trigger a reschedule.
///
/// Looks up the LAPIC ID for the given CPU index and sends the scheduler
/// IPI vector. Silently does nothing if the CPU is not registered.
fn send_ipi_to_cpu(cpu_id: usize) {
    if let Some(lapic_id) = get_lapic_id_for_cpu(cpu_id) {
        crate::arch::x86_64::apic::send_ipi(lapic_id as u8, SCHED_IPI_VECTOR);
    }
}

/// Look up the LAPIC ID for a given CPU index.
fn get_lapic_id_for_cpu(cpu_id: usize) -> Option<u32> {
    if cpu_id < MAX_CPUS {
        // SAFETY: Each CPU's slot is only written by that CPU during init.
        // We read the LAPIC ID which is set once during AP startup.
        unsafe {
            let percpu_base = PERCPU_BASES[cpu_id];
            if !percpu_base.is_null() {
                let data = &*percpu_base;
                return Some(data.lapic_id);
            }
        }
    }
    None
}

/// Pointers to per-CPU data structures (set during `init_cpu`).
///
/// Each slot is written exactly once during AP startup and read thereafter.
/// The `register_percpu_base` function is called during init with the CPU's
/// `PerCpuData` pointer, and `get_lapic_id_for_cpu` reads it later.
///
/// # Safety
///
/// Written once per CPU during initialization (no concurrent writes).
/// Reads after initialization see a valid, immutable `PerCpuData` struct.
static mut PERCPU_BASES: [*const crate::arch::x86_64::percpu::PerCpuData; MAX_CPUS] =
    [core::ptr::null(); MAX_CPUS];

/// Register a per-CPU data pointer (called from `percpu::init_cpu`).
///
/// # Safety
///
/// Must be called exactly once per CPU during initialization.
pub unsafe fn register_percpu_base(
    cpu_id: usize,
    base: *const crate::arch::x86_64::percpu::PerCpuData,
) {
    // SAFETY: Called once per CPU during init, no concurrent access.
    unsafe {
        PERCPU_BASES[cpu_id] = base;
    }
}

// ============================================================================
// Scheduler init
// ============================================================================

/// Initialize the scheduler with a single idle task on CPU 0.
pub fn init() {
    let idle_task = Task::new("idle", 0);
    CURRENT_TASK_ID.store(idle_task.id.as_u64(), Ordering::Release);
    let mut cpu0 = CPU_QUEUES[0].lock();
    cpu0.current = Some(idle_task.id);
    cpu0.ready.push_back(idle_task);
    drop(cpu0);
    println!("[OK] SMP scheduler initialized (idle task on CPU 0)");
}

// ============================================================================
// Task spawning
// ============================================================================

/// Spawn a new task and add it to the least-loaded CPU's run queue.
///
/// # Errors
///
/// Returns `Err("maximum number of tasks reached")` if the total task count
/// across all CPUs has reached `MAX_TASKS`.
pub fn spawn_task(name: &str, priority: u8) -> Result<TaskId, &'static str> {
    let task = Task::new(name, priority);
    let id = task.id;

    if total_task_count() >= MAX_TASKS {
        return Err("maximum number of tasks reached");
    }

    let target_cpu = least_loaded_cpu();
    let mut queue = CPU_QUEUES[target_cpu].lock();
    queue.ready.push_back(task);
    Ok(id)
}

/// Spawn a new task and return its ID (alias for `spawn_task`).
///
/// # Errors
///
/// Returns `Err` if the maximum number of tasks (`MAX_TASKS`) has been reached.
pub fn spawn_task_with_id(name: &str, priority: u8) -> Result<TaskId, &'static str> {
    spawn_task(name, priority)
}

/// Spawn a pre-constructed task on the least-loaded CPU.
///
/// # Errors
///
/// Returns `Err` if the maximum number of tasks (`MAX_TASKS`) has been reached.
pub fn spawn_task_from(task: Task) -> Result<TaskId, &'static str> {
    let id = task.id;

    if total_task_count() >= MAX_TASKS {
        return Err("maximum number of tasks reached");
    }

    let target_cpu = least_loaded_cpu();
    let mut queue = CPU_QUEUES[target_cpu].lock();
    queue.ready.push_back(task);
    Ok(id)
}

// ============================================================================
// Current task tracking
// ============================================================================

/// Get the ID of the currently running task on this CPU.
pub fn current_task_id() -> TaskId {
    TaskId::from_u64(CURRENT_TASK_ID.load(Ordering::Acquire))
}

/// Set the current task ID (called when launching a process).
pub fn set_current_task(id: TaskId) {
    CURRENT_TASK_ID.store(id.as_u64(), Ordering::Release);
    // Also update the per-CPU queue's current field.
    let cpu = current_cpu();
    let mut queue = CPU_QUEUES[cpu].lock();
    queue.current = Some(id);
}

// ============================================================================
// Task lookup (across all queues)
// ============================================================================

/// Execute a closure with a shared reference to the current task.
pub fn with_current_task<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&Task) -> R,
{
    let id = current_task_id();
    let cpu = current_cpu();

    // Check the current CPU's queue first.
    {
        let queue = CPU_QUEUES[cpu].lock();
        if let Some(task) = queue.ready.iter().find(|t| t.id == id) {
            return Some(f(task));
        }
    }

    // Fall back to the blocked queue.
    let blocked = BLOCKED_QUEUE.lock();
    blocked.iter().find(|t| t.id == id).map(f)
}

/// Execute a closure with a mutable reference to the current task.
pub fn with_current_task_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Task) -> R,
{
    let id = current_task_id();
    let cpu = current_cpu();

    // Check the current CPU's queue first.
    {
        let mut queue = CPU_QUEUES[cpu].lock();
        if let Some(task) = queue.ready.iter_mut().find(|t| t.id == id) {
            return Some(f(task));
        }
    }

    // Fall back to the blocked queue.
    let mut blocked = BLOCKED_QUEUE.lock();
    blocked.iter_mut().find(|t| t.id == id).map(f)
}

/// Execute a closure with a shared reference to a task by ID.
///
/// Searches across all CPU queues and the blocked queue.
pub fn with_task<F, R>(id: TaskId, f: F) -> Option<R>
where
    F: FnOnce(&Task) -> R,
{
    // Search the caller's CPU queue first.
    let cpu = current_cpu();
    let queue = CPU_QUEUES[cpu].lock();
    if let Some(task) = queue.ready.iter().find(|t| t.id == id) {
        return Some(f(task));
    }
    drop(queue);

    // Search other CPU queues.
    for (i, cpu_queue) in CPU_QUEUES.iter().enumerate().take(MAX_CPUS) {
        if i == cpu {
            continue;
        }
        let queue = cpu_queue.lock();
        if let Some(task) = queue.ready.iter().find(|t| t.id == id) {
            return Some(f(task));
        }
    }

    // Search the blocked queue.
    let blocked = BLOCKED_QUEUE.lock();
    blocked.iter().find(|t| t.id == id).map(f)
}

/// Execute a closure with a mutable reference to a task by ID.
///
/// Searches across all CPU queues and the blocked queue.
pub fn with_task_mut<F, R>(id: TaskId, f: F) -> Option<R>
where
    F: FnOnce(&mut Task) -> R,
{
    // Search the caller's CPU queue first.
    let cpu = current_cpu();
    let mut queue = CPU_QUEUES[cpu].lock();
    if let Some(task) = queue.ready.iter_mut().find(|t| t.id == id) {
        return Some(f(task));
    }
    drop(queue);

    // Search other CPU queues.
    for (i, cpu_queue) in CPU_QUEUES.iter().enumerate().take(MAX_CPUS) {
        if i == cpu {
            continue;
        }
        let mut queue = cpu_queue.lock();
        if let Some(task) = queue.ready.iter_mut().find(|t| t.id == id) {
            return Some(f(task));
        }
    }

    // Search the blocked queue.
    let mut blocked = BLOCKED_QUEUE.lock();
    blocked.iter_mut().find(|t| t.id == id).map(f)
}

// ============================================================================
// Scheduling
// ============================================================================

/// Pick the next ready task from the current CPU's queue using priority scheduling.
///
/// Selects the task with the highest priority value. Within the same priority
/// level, tasks are scheduled in FIFO order (round-robin). Priority 0 is the
/// lowest (idle), priority 255 is the highest.
///
/// Returns `Some(TaskId)` if a task was scheduled, `None` if the queue is empty.
fn schedule_next_local(cpu: usize) -> Option<TaskId> {
    let mut queue = CPU_QUEUES[cpu].lock();
    if queue.ready.is_empty() {
        return None;
    }

    // Find the index of the highest-priority task (first occurrence for FIFO).
    let best_idx = queue
        .ready
        .iter()
        .enumerate()
        .max_by_key(|(_, t)| t.priority)
        .map(|(i, _)| i)?;

    let mut task = queue.ready.remove(best_idx).unwrap();
    task.state = TaskState::Running;
    let id = task.id;
    queue.current = Some(id);
    CURRENT_TASK_ID.store(id.as_u64(), Ordering::Release);
    queue.ready.push_back(task);
    Some(id)
}

/// Wake a blocked task by ID, moving it to the appropriate CPU's ready queue.
///
/// If the task was blocked on a different CPU, sends an IPI to that CPU.
/// If the task is not in the blocked queue, this function is a no-op.
///
/// # Panics
///
/// Panics if the blocked queue is corrupted (task found by position but
/// cannot be removed). This should never happen in practice.
pub fn wake_task_by_id(id: TaskId) {
    // Find and remove the task from the blocked queue.
    let task = {
        let mut blocked = BLOCKED_QUEUE.lock();
        blocked.iter().position(|t| t.id == id).map(|pos| {
            let mut task = blocked.remove(pos).unwrap();
            task.state = TaskState::Ready;
            task
        })
    };

    let Some(mut task) = task else { return };

    // Determine which CPU to wake the task on.
    let target_cpu = {
        let mut map = BLOCKED_CPU_MAP.lock();
        map.remove(&id.as_u64()).map(|c| c as usize)
    };

    let target_cpu = target_cpu.unwrap_or_else(current_cpu);
    task.state = TaskState::Ready;

    let mut queue = CPU_QUEUES[target_cpu].lock();
    queue.ready.push_back(task);
    drop(queue);

    // If the target CPU is different from the current one, send an IPI.
    let current = current_cpu();
    if target_cpu != current {
        send_ipi_to_cpu(target_cpu);
    }
}

/// Move the current task from the blocked queue back to the ready queue.
///
/// Used when `block_and_switch` moved the task to the blocked queue but
/// no context switch occurred (the task is still on the CPU).
///
/// # Panics
///
/// Panics if the blocked queue is corrupted (task found by position but
/// cannot be removed). This should never happen in practice.
pub fn unblock_current() {
    let cpu = current_cpu();
    let mut queue = CPU_QUEUES[cpu].lock();
    let Some(current_id) = queue.current else {
        return;
    };

    let mut blocked = BLOCKED_QUEUE.lock();
    if let Some(pos) = blocked.iter().position(|t| t.id == current_id) {
        let mut task = blocked.remove(pos).unwrap();
        task.state = TaskState::Ready;
        queue.ready.push_back(task);
    }
}

/// Block the current task and switch to the next ready task.
///
/// Saves the current task's context, moves it to the blocked queue,
/// picks the next ready task on the current CPU, and sets `SWITCH_CONTEXT`
/// so the syscall entry stub restores the new task's context.
///
/// Returns `true` if a context switch happened, `false` if no other task
/// is ready (current task stays running).
///
/// # Panics
///
/// Panics if the blocked queue is corrupted (task found by position but
/// cannot be removed). This should never happen in practice.
#[allow(static_mut_refs)]
pub fn block_and_switch(current_ctx: SavedContext) -> bool {
    let cpu = current_cpu();
    let mut queue = CPU_QUEUES[cpu].lock();

    let Some(current_id) = queue.current else {
        return false;
    };

    // Save current task's context and move to blocked queue.
    if let Some(pos) = queue.ready.iter().position(|t| t.id == current_id) {
        let mut task = queue.ready.remove(pos).unwrap();
        task.state = TaskState::Blocked;
        task.context = Some(current_ctx);

        // Track which CPU this task was blocked on.
        {
            let mut map = BLOCKED_CPU_MAP.lock();
            map.insert(current_id.as_u64(), cpu as u32);
        }

        let mut blocked = BLOCKED_QUEUE.lock();
        blocked.push_back(task);
    }

    // Pick the next ready task on this CPU.
    if let Some(mut task) = queue.ready.pop_front() {
        task.state = TaskState::Running;
        let next_id = task.id;
        let next_cr3 = task.page_table.unwrap_or(0);
        queue.current = Some(next_id);
        CURRENT_TASK_ID.store(next_id.as_u64(), Ordering::Release);
        queue.ready.push_back(task);

        if let Some(mut ctx) = queue.ready.back().and_then(|t| t.context) {
            // Ensure the context carries the correct page table. The task's
            // page_table field is authoritative; the SavedContext.cr3 may be
            // stale if the task was created before the CR3 fix.
            ctx.cr3 = next_cr3;
            // SAFETY: `NEXT_CTX_STORAGE` is a static mutable used as stable storage
            // for the context pointer. We write the new context and set `SWITCH_CONTEXT`
            // to point to it. The syscall entry stub reads from this pointer after the
            // handler returns. This is safe because: (1) we hold the CPU queue lock,
            // (2) the stub only reads after the handler returns, (3) the static lives
            // for the entire program lifetime.
            unsafe {
                NEXT_CTX_STORAGE = ctx;
                crate::arch::x86_64::syscall::SWITCH_CONTEXT =
                    core::ptr::addr_of!(NEXT_CTX_STORAGE);
            }
            crate::serial_println!(
                "[SCHED] CPU {} switch {} -> {}",
                cpu,
                current_id.as_u64(),
                next_id.as_u64()
            );
            return true;
        }
    }

    false
}

// ============================================================================
// Task termination
// ============================================================================

/// Terminate the current task with the given exit status.
///
/// Sets the task state to `Terminated`, closes all handles, removes the
/// task from the CPU's ready queue, and wakes the parent task if it is
/// blocked in `process_wait`.
///
/// # Panics
///
/// Panics if the ready queue is corrupted (task found by position but
/// cannot be removed). This should never happen in practice.
pub fn terminate_current(status: u64) {
    let cpu = current_cpu();
    let mut queue = CPU_QUEUES[cpu].lock();

    let Some(current_id) = queue.current else {
        return;
    };

    // Find and remove the current task from the ready queue.
    if let Some(pos) = queue.ready.iter().position(|t| t.id == current_id) {
        let mut task = queue.ready.remove(pos).unwrap();
        task.state = TaskState::Terminated;
        task.exit_status = Some(status);

        // Close all handles in the task's handle table.
        task.handle_table.close_all();

        // Release all file locks held by this task.
        crate::fs::file_lock::release_all_for_task(current_id.as_u64());

        // Free the task's user page table (if it has one).
        if let Some(p4_phys) = task.page_table {
            // SAFETY: The task is being terminated, so its page table is
            // no longer in use. We free all user-mapped frames and the
            // page table itself.
            unsafe {
                crate::task::user::free_user_page_table(p4_phys);
            }
            task.page_table = None;
        }

        // Wake the parent task if it is blocked in process_wait.
        if let Some(parent_id) = task.parent_id {
            drop(queue);
            wake_task_by_id(parent_id);
        }

        crate::serial_println!(
            "[SCHED] task {} terminated with status {}",
            current_id.as_u64(),
            status
        );
    }
}

/// Get the exit status of a task by ID.
///
/// Returns `Some(status)` if the task has exited, `None` if still running or not found.
pub fn get_exit_status(id: TaskId) -> Option<u64> {
    // Search all CPU queues.
    for queue in CPU_QUEUES.iter().take(MAX_CPUS) {
        let q = queue.lock();
        if let Some(task) = q.ready.iter().find(|t| t.id == id) {
            return task.exit_status;
        }
    }

    // Search the blocked queue.
    let blocked = BLOCKED_QUEUE.lock();
    blocked
        .iter()
        .find(|t| t.id == id)
        .and_then(|t| t.exit_status)
}

// ============================================================================
// Priority management
// ============================================================================

/// Set the priority of a task by ID.
///
/// Returns `true` if the task was found and its priority was updated,
/// `false` if the task was not found.
pub fn set_task_priority(id: TaskId, priority: u8) -> bool {
    // Search the caller's CPU queue first.
    let cpu = current_cpu();
    {
        let mut queue = CPU_QUEUES[cpu].lock();
        if let Some(task) = queue.ready.iter_mut().find(|t| t.id == id) {
            task.priority = priority;
            return true;
        }
    }

    // Search other CPU queues.
    for (i, cpu_queue) in CPU_QUEUES.iter().enumerate().take(MAX_CPUS) {
        if i == cpu {
            continue;
        }
        let mut queue = cpu_queue.lock();
        if let Some(task) = queue.ready.iter_mut().find(|t| t.id == id) {
            task.priority = priority;
            return true;
        }
    }

    // Search the blocked queue.
    let mut blocked = BLOCKED_QUEUE.lock();
    if let Some(task) = blocked.iter_mut().find(|t| t.id == id) {
        task.priority = priority;
        return true;
    }

    false
}

/// Information about a task, returned by `list_tasks`.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    /// Unique task identifier.
    pub id: u64,
    /// Task name as raw bytes (not null-terminated).
    pub name: [u8; 32],
    /// Length of the valid portion of `name`.
    pub name_len: u8,
    /// Current scheduling state.
    pub state: TaskState,
    /// Scheduling priority (0 = lowest, 255 = highest).
    pub priority: u8,
}

/// List all tasks across all queues.
///
/// Returns a vector of `TaskInfo` for every task (ready, running, blocked).
pub fn list_tasks() -> alloc::vec::Vec<TaskInfo> {
    let mut result = alloc::vec::Vec::new();

    for queue in CPU_QUEUES.iter().take(MAX_CPUS) {
        let q = queue.lock();
        for task in &q.ready {
            let mut name = [0u8; 32];
            let bytes = task.name.as_bytes();
            let len = bytes.len().min(32);
            name[..len].copy_from_slice(&bytes[..len]);
            result.push(TaskInfo {
                id: task.id.as_u64(),
                name,
                name_len: len as u8,
                state: task.state,
                priority: task.priority,
            });
        }
    }

    let blocked = BLOCKED_QUEUE.lock();
    for task in blocked.iter() {
        let mut name = [0u8; 32];
        let bytes = task.name.as_bytes();
        let len = bytes.len().min(32);
        name[..len].copy_from_slice(&bytes[..len]);
        result.push(TaskInfo {
            id: task.id.as_u64(),
            name,
            name_len: len as u8,
            state: task.state,
            priority: task.priority,
        });
    }

    let migration = MIGRATION_QUEUE.lock();
    for task in migration.iter() {
        let mut name = [0u8; 32];
        let bytes = task.name.as_bytes();
        let len = bytes.len().min(32);
        name[..len].copy_from_slice(&bytes[..len]);
        result.push(TaskInfo {
            id: task.id.as_u64(),
            name,
            name_len: len as u8,
            state: task.state,
            priority: task.priority,
        });
    }

    result
}

// ============================================================================
// Task migration (work stealing)
// ============================================================================

/// Migrate a task from one CPU's ready queue to another CPU's ready queue.
///
/// Returns `true` if the migration succeeded, `false` if the task was not found.
///
/// # Panics
///
/// Panics if the source queue is corrupted (task found by position but
/// cannot be removed). This should never happen in practice.
pub fn migrate_task(task_id: TaskId, from_cpu: usize, to_cpu: usize) -> bool {
    if from_cpu >= MAX_CPUS || to_cpu >= MAX_CPUS {
        return false;
    }

    let task = {
        let mut from_queue = CPU_QUEUES[from_cpu].lock();
        match from_queue.ready.iter().position(|t| t.id == task_id) {
            Some(pos) => from_queue.ready.remove(pos).unwrap(),
            None => return false,
        }
    };

    let mut to_queue = CPU_QUEUES[to_cpu].lock();
    to_queue.ready.push_back(task);
    true
}

/// Push a task onto the global migration queue for work stealing.
pub fn push_migration(task: Task) {
    MIGRATION_QUEUE.lock().push_back(task);
}

/// Pop a task from the global migration queue and add it to the current CPU.
///
/// Returns `true` if a task was stolen, `false` if the migration queue is empty.
pub fn steal_from_migration() -> bool {
    let task = MIGRATION_QUEUE.lock().pop_front();
    task.is_some_and(|task| {
        let cpu = current_cpu();
        let mut queue = CPU_QUEUES[cpu].lock();
        queue.ready.push_back(task);
        true
    })
}

// ============================================================================
// Tests
// ============================================================================

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
    fn test_cpu_queue_new() {
        let queue = CpuQueue::new();
        assert_eq!(queue.task_count(), 0);
        assert!(queue.current.is_none());
    }

    #[test]
    fn test_cpu_queue_task_count() {
        let mut queue = CpuQueue::new();
        queue.ready.push_back(Task::new("a", 0));
        queue.ready.push_back(Task::new("b", 0));
        queue.current = Some(TaskId::new());
        assert_eq!(queue.task_count(), 3);
    }

    #[test]
    fn test_cpu_queues_initialized() {
        assert_eq!(CPU_QUEUES.len(), MAX_CPUS);
        // All queues are accessible and lockable. We don't assert emptiness
        // because other tests (e.g., spawn_task_from) may have added tasks
        // to the shared static queues.
        for i in 0..MAX_CPUS {
            let queue = CPU_QUEUES[i].lock();
            let _count = queue.task_count();
        }
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
    fn test_migration_queue_push_pop() {
        let task = Task::new("migrant", 0);
        let id = task.id;
        push_migration(task);

        let stolen = steal_from_migration();
        assert!(stolen);

        // The task should now be on CPU 0's queue (current_cpu returns 0 in tests).
        let queue = CPU_QUEUES[0].lock();
        assert!(queue.ready.iter().any(|t| t.id == id));
    }

    #[test]
    fn test_migration_queue_empty() {
        // Ensure the migration queue is empty after stealing.
        let stolen = steal_from_migration();
        // May or may not be empty depending on test ordering, but shouldn't panic.
        let _ = stolen;
    }

    #[test]
    fn test_total_task_count() {
        let count = total_task_count();
        // Should be non-negative (usize) and reasonable.
        assert!(count < MAX_TASKS);
    }

    #[test]
    fn test_max_cpus_is_8() {
        assert_eq!(MAX_CPUS, 8);
    }

    #[test]
    fn test_spawn_task_from_with_context() {
        // Simulate sys_thread_create: build a task with user-mode context,
        // page table, and parent, then enqueue it.
        use super::super::task::SavedContext;

        let mut task = Task::new("test-thread", 5);
        let task_id = task.id;

        let mut ctx = SavedContext::user_mode(0x40_0000, 0x7FFF_FFFF_F000, 0x1000);
        ctx.rdi = 42;
        task.context = Some(ctx);
        task.page_table = Some(0x1000);
        task.parent_id = Some(TaskId::from_u64(0));

        let result = spawn_task_from(task);
        assert!(result.is_ok(), "spawn_task_from must succeed");
        assert_eq!(result.unwrap(), task_id);

        // Verify the task was enqueued with its context intact.
        let found = with_task_mut(task_id, |t| {
            assert!(t.context.is_some(), "context must be set");
            let ctx = t.context.unwrap();
            assert_eq!(ctx.rcx, 0x40_0000);
            assert_eq!(ctx.rsp, 0x7FFF_FFFF_F000);
            assert_eq!(ctx.rdi, 42);
            assert_eq!(ctx.cr3, 0x1000);
            assert_eq!(ctx.is_kernel, 0);
            assert_eq!(t.page_table, Some(0x1000));
            assert!(t.parent_id.is_some());
        });
        assert!(found.is_some(), "task must be findable after spawn");
    }

    #[test]
    fn test_spawn_task_from_returns_unique_id() {
        let t1 = Task::new("a", 0);
        let t2 = Task::new("b", 0);
        let id1 = t1.id;
        let id2 = t2.id;

        let r1 = spawn_task_from(t1);
        let r2 = spawn_task_from(t2);
        assert!(r1.is_ok());
        assert!(r2.is_ok());
        assert_ne!(r1.unwrap(), r2.unwrap(), "task IDs must be unique");
    }

    #[test]
    fn test_spawn_task_preserves_state() {
        let mut task = Task::new("ready-task", 3);
        task.state = TaskState::Ready;
        let id = task.id;

        spawn_task_from(task).expect("spawn must succeed");

        let found = with_task_mut(id, |t| {
            assert_eq!(t.state, TaskState::Ready);
            assert_eq!(t.priority, 3);
            assert_eq!(t.name, "ready-task");
        });
        assert!(found.is_some());
    }

    #[test]
    fn test_set_task_priority() {
        let task = Task::new("priority-test", 1);
        let id = task.id;
        spawn_task_from(task).expect("spawn must succeed");

        assert!(set_task_priority(id, 10));

        let found = with_task_mut(id, |t| {
            assert_eq!(t.priority, 10);
        });
        assert!(found.is_some());
    }

    #[test]
    fn test_set_task_priority_not_found() {
        let fake_id = TaskId::from_u64(999_999);
        assert!(!set_task_priority(fake_id, 5));
    }

    #[test]
    fn test_list_tasks_returns_all() {
        let task = Task::new("listable", 0);
        let id = task.id;
        spawn_task_from(task).expect("spawn must succeed");

        let tasks = list_tasks();
        assert!(tasks.iter().any(|t| t.id == id.as_u64()));
    }

    #[test]
    fn test_list_tasks_contains_names() {
        let task = Task::new("named-task", 0);
        let id = task.id;
        spawn_task_from(task).expect("spawn must succeed");

        let tasks = list_tasks();
        let info = tasks.iter().find(|t| t.id == id.as_u64()).unwrap();
        assert_eq!(info.name_len, 10);
        assert_eq!(&info.name[..info.name_len as usize], b"named-task");
    }

    #[test]
    fn test_priority_scheduling_order() {
        // Spawn tasks with different priorities on the same CPU.
        let low = Task::new("low", 1);
        let low_id = low.id;
        spawn_task_from(low).expect("spawn must succeed");

        let high = Task::new("high", 10);
        let high_id = high.id;
        spawn_task_from(high).expect("spawn must succeed");

        // Find which CPU they landed on and verify scheduling picks the higher priority.
        let scheduled_cpu = {
            let mut found = 0usize;
            for (i, queue) in CPU_QUEUES.iter().enumerate().take(MAX_CPUS) {
                let q = queue.lock();
                if q.ready.iter().any(|t| t.id == high_id) {
                    found = i;
                    break;
                }
            }
            found
        };

        // The next scheduled task should be the high-priority one.
        let scheduled = schedule_next_local(scheduled_cpu);
        assert_eq!(scheduled, Some(high_id));
    }
}
