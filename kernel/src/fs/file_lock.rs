//! Per-inode advisory file locking.
//!
//! Provides `flock()`-style advisory locks: shared (read) and exclusive (write).
//! Locks are managed per inode in a global table. The implementation follows
//! BSD `flock()` semantics:
//!
//! - `LOCK_SH` — shared (read) lock. Multiple tasks may hold a shared lock
//!   simultaneously. Shared locks block exclusive lock requests.
//! - `LOCK_EX` — exclusive (write) lock. At most one task may hold an
//!   exclusive lock, and no shared locks may be held concurrently.
//! - `LOCK_UN` — release the lock held by this task on this inode.
//! - `LOCK_NB` — non-blocking. Return `WouldBlock` instead of waiting.
//!
//! Locks are released automatically when the closing task closes all file
//! descriptors referencing the inode (via `close` syscall cleanup).

use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;

use spin::Mutex;

/// Shared lock — multiple readers allowed.
pub const LOCK_SH: u64 = 1;

/// Exclusive lock — single writer.
pub const LOCK_EX: u64 = 2;

/// Non-blocking flag — `ORed` with `LOCK_SH` or `LOCK_EX`.
pub const LOCK_NB: u64 = 4;

/// Unlock — release the lock on this inode.
pub const LOCK_UN: u64 = 8;

/// Lock type stored per-inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockType {
    /// Shared (read) lock.
    Shared,
    /// Exclusive (write) lock.
    Exclusive,
}

/// A single lock record for an inode.
#[derive(Debug, Clone)]
struct LockRecord {
    /// The type of lock held.
    lock_type: LockType,
    /// Globally-unique process (task) ID of the lock holder.
    pid: u64,
}

/// Lock state for a single inode.
#[derive(Debug, Clone)]
struct InodeLockState {
    /// Number of lock holders. For exclusive locks this is 1; for shared
    /// locks this is the count of concurrent holders.
    count: u32,
    /// The type of lock currently held. `None` when no locks are held
    /// (the entry will be cleaned up lazily).
    lock_type: Option<LockType>,
    /// List of lock records — used to detect duplicate lock requests
    /// from the same task (which are no-ops or upgrade attempts).
    holders: alloc::vec::Vec<LockRecord>,
}

/// Global file lock map. Maps `(filesystem_id, inode_number)` -> lock state.
///
/// The key uses a combination of a filesystem identifier (currently 0 for ramfs,
/// 1 for ext2, etc.) and the inode number to ensure uniqueness across mounted
/// filesystems.
static FILE_LOCKS: Mutex<BTreeMap<(u64, u64), InodeLockState>> = Mutex::new(BTreeMap::new());

/// The error returned when a non-blocking lock cannot be acquired immediately.
pub const ERR_WOULD_BLOCK: i64 = -7;

/// Attempt to acquire or release an advisory file lock.
///
/// `fs_id` is an opaque identifier for the filesystem (0 = ramfs, 1 = ext2, etc.).
/// `ino` is the inode number within that filesystem.
/// `operation` is a bitmask of `LOCK_SH`, `LOCK_EX`, `LOCK_UN`, and `LOCK_NB`.
/// `pid` is the current task's process ID.
///
/// Returns `Ok(())` on success, or `Err(negative_error_code)` on failure.
pub fn flock(fs_id: u64, ino: u64, operation: u64, pid: u64) -> Result<(), i64> {
    let op = operation & !LOCK_NB;
    let nonblocking = operation & LOCK_NB != 0;

    match op {
        LOCK_SH => acquire_shared(fs_id, ino, pid, nonblocking),
        LOCK_EX => acquire_exclusive(fs_id, ino, pid, nonblocking),
        LOCK_UN => release_lock(fs_id, ino, pid),
        _ => Err(-1), // InvalidArgument
    }
}

/// Acquire a shared (read) lock.
///
/// Succeeds if no exclusive lock is held. If a shared lock is already held
/// by this task, it is a no-op.
fn acquire_shared(fs_id: u64, ino: u64, pid: u64, nonblocking: bool) -> Result<(), i64> {
    let mut locks = FILE_LOCKS.lock();
    match locks.entry((fs_id, ino)) {
        Entry::Vacant(entry) => {
            entry.insert(InodeLockState {
                count: 1,
                lock_type: Some(LockType::Shared),
                holders: alloc::vec![LockRecord {
                    lock_type: LockType::Shared,
                    pid,
                }],
            });
            Ok(())
        }
        Entry::Occupied(mut entry) => {
            let state = entry.get_mut();

            // If this task already holds a lock, check compatibility.
            if let Some(record) = state.holders.iter().find(|r| r.pid == pid) {
                // Task already holds a lock — no-op (exclusive implies shared access).
                return Ok(());
            }

            // Check current lock type.
            match state.lock_type {
                None => {
                    // Lock was released by all holders but entry still exists.
                    state.count = 1;
                    state.lock_type = Some(LockType::Shared);
                    state.holders.push(LockRecord {
                        lock_type: LockType::Shared,
                        pid,
                    });
                    Ok(())
                }
                Some(LockType::Shared) => {
                    // Compatible — add another reader.
                    state.count += 1;
                    state.holders.push(LockRecord {
                        lock_type: LockType::Shared,
                        pid,
                    });
                    Ok(())
                }
                Some(LockType::Exclusive) => {
                    // An exclusive lock is held — must wait or fail.
                    if nonblocking {
                        Err(ERR_WOULD_BLOCK)
                    } else {
                        // Blocked mode — spin-wait. In a real implementation
                        // this would use a wait queue and scheduler blocking.
                        drop(locks);
                        spin_wait_shared(fs_id, ino, pid)
                    }
                }
            }
        }
    }
}

/// Acquire an exclusive (write) lock.
///
/// Succeeds only if no other task holds any lock on this inode.
fn acquire_exclusive(fs_id: u64, ino: u64, pid: u64, nonblocking: bool) -> Result<(), i64> {
    let mut locks = FILE_LOCKS.lock();
    match locks.entry((fs_id, ino)) {
        Entry::Vacant(entry) => {
            entry.insert(InodeLockState {
                count: 1,
                lock_type: Some(LockType::Exclusive),
                holders: alloc::vec![LockRecord {
                    lock_type: LockType::Exclusive,
                    pid,
                }],
            });
            Ok(())
        }
        Entry::Occupied(mut entry) => {
            let state = entry.get_mut();

            // If this task already holds the exclusive lock, it's a no-op.
            let existing_pos = state.holders.iter().position(|r| r.pid == pid);
            if let Some(pos) = existing_pos {
                match state.holders[pos].lock_type {
                    LockType::Exclusive => {
                        // Already holds exclusive — no-op.
                        return Ok(());
                    }
                    LockType::Shared => {
                        // Upgrade from shared to exclusive.
                        // This fails if other tasks also hold shared locks.
                        if state.count == 1 {
                            // Only this task holds the shared lock.
                            state.holders[pos].lock_type = LockType::Exclusive;
                            state.lock_type = Some(LockType::Exclusive);
                            return Ok(());
                        }
                        // Other tasks hold shared locks — must wait.
                        if nonblocking {
                            return Err(ERR_WOULD_BLOCK);
                        }
                        drop(locks);
                        return spin_wait_exclusive(fs_id, ino, pid);
                    }
                }
            }

            match state.lock_type {
                None => {
                    state.count = 1;
                    state.lock_type = Some(LockType::Exclusive);
                    state.holders.push(LockRecord {
                        lock_type: LockType::Exclusive,
                        pid,
                    });
                    Ok(())
                }
                Some(_) => {
                    // Some locks are held.
                    if nonblocking {
                        Err(ERR_WOULD_BLOCK)
                    } else {
                        drop(locks);
                        spin_wait_exclusive(fs_id, ino, pid)
                    }
                }
            }
        }
    }
}

/// Release the lock held by this task on the given inode.
#[allow(clippy::unnecessary_wraps)]
fn release_lock(fs_id: u64, ino: u64, pid: u64) -> Result<(), i64> {
    let mut locks = FILE_LOCKS.lock();
    let Some(state) = locks.get_mut(&(fs_id, ino)) else {
        // No lock exists — treat as success (matching BSD behavior).
        return Ok(());
    };

    // Find and remove this task's lock record.
    let before = state.holders.len();
    state.holders.retain(|r| r.pid != pid);
    let removed = before - state.holders.len();

    if removed == 0 {
        // This task did not hold a lock — BSD semantics: no-op.
        return Ok(());
    }

    debug_assert!(state.count >= removed as u32);
    state.count -= removed as u32;

    if state.count == 0 {
        state.lock_type = None;
        // Keep the entry in the map (it will be reused or cleaned up lazily).
    } else if state.holders.is_empty() {
        state.lock_type = None;
    } else {
        // Recompute lock type from remaining holders.
        let has_exclusive = state
            .holders
            .iter()
            .any(|r| r.lock_type == LockType::Exclusive);
        state.lock_type = if has_exclusive {
            Some(LockType::Exclusive)
        } else {
            Some(LockType::Shared)
        };
    }

    Ok(())
}

/// Spin-wait until a shared lock can be acquired.
///
/// In a full implementation, the task would block and yield the CPU.
/// For now, we spin-wait with HLT.
fn spin_wait_shared(fs_id: u64, ino: u64, pid: u64) -> Result<(), i64> {
    // SAFETY: HLT is safe when interrupts are enabled; the timer interrupt
    // will wake the CPU and let us re-check.
    loop {
        x86_64::instructions::hlt();
        let result = try_acquire_shared(fs_id, ino, pid);
        if result.is_ok() {
            return Ok(());
        }
    }
}

/// Spin-wait until an exclusive lock can be acquired.
fn spin_wait_exclusive(fs_id: u64, ino: u64, pid: u64) -> Result<(), i64> {
    loop {
        x86_64::instructions::hlt();
        let result = try_acquire_exclusive(fs_id, ino, pid);
        if result.is_ok() {
            return Ok(());
        }
    }
}

/// Attempt to acquire a shared lock without blocking (used in spin-wait loop).
fn try_acquire_shared(fs_id: u64, ino: u64, pid: u64) -> Result<(), i64> {
    let mut locks = FILE_LOCKS.lock();
    let Some(state) = locks.get_mut(&(fs_id, ino)) else {
        // No state yet — lock is free.
        locks.insert(
            (fs_id, ino),
            InodeLockState {
                count: 1,
                lock_type: Some(LockType::Shared),
                holders: alloc::vec![LockRecord {
                    lock_type: LockType::Shared,
                    pid,
                }],
            },
        );
        return Ok(());
    };

    // Check if this task already holds a lock.
    if state.holders.iter().any(|r| r.pid == pid) {
        return Ok(());
    }

    match state.lock_type {
        None | Some(LockType::Shared) => {
            state.count += 1;
            state.lock_type = Some(LockType::Shared);
            state.holders.push(LockRecord {
                lock_type: LockType::Shared,
                pid,
            });
            Ok(())
        }
        Some(LockType::Exclusive) => Err(ERR_WOULD_BLOCK),
    }
}

/// Attempt to acquire an exclusive lock without blocking (used in spin-wait loop).
fn try_acquire_exclusive(fs_id: u64, ino: u64, pid: u64) -> Result<(), i64> {
    let mut locks = FILE_LOCKS.lock();
    let Some(state) = locks.get_mut(&(fs_id, ino)) else {
        locks.insert(
            (fs_id, ino),
            InodeLockState {
                count: 1,
                lock_type: Some(LockType::Exclusive),
                holders: alloc::vec![LockRecord {
                    lock_type: LockType::Exclusive,
                    pid,
                }],
            },
        );
        return Ok(());
    };

    // If this task holds the exclusive lock, it's a no-op.
    let existing_pos = state.holders.iter().position(|r| r.pid == pid);
    if let Some(pos) = existing_pos {
        if state.holders[pos].lock_type == LockType::Exclusive {
            return Ok(());
        }
        // Shared -> exclusive upgrade: only if sole holder.
        if state.count == 1 {
            state.holders[pos].lock_type = LockType::Exclusive;
            state.lock_type = Some(LockType::Exclusive);
            return Ok(());
        }
    }

    if state.count == 0 {
        state.count = 1;
        state.lock_type = Some(LockType::Exclusive);
        state.holders.push(LockRecord {
            lock_type: LockType::Exclusive,
            pid,
        });
        return Ok(());
    }

    Err(ERR_WOULD_BLOCK)
}

/// Release all locks held by the given task (for cleanup on task exit).
///
/// Called by the kernel when a task terminates to ensure locks are not
/// leaked. This is an internal API, not exposed to user-space.
pub fn release_all_for_task(pid: u64) {
    let mut locks = FILE_LOCKS.lock();

    // Collect keys whose holders include this pid.
    let stale_keys: alloc::vec::Vec<(u64, u64)> = locks
        .iter()
        .filter(|(_, state)| state.holders.iter().any(|r| r.pid == pid))
        .map(|(key, _)| *key)
        .collect();

    for key in stale_keys {
        if let Some(state) = locks.get_mut(&key) {
            let before = state.holders.len();
            state.holders.retain(|r| r.pid != pid);
            let removed = before - state.holders.len();
            if removed > 0 {
                state.count = state.count.saturating_sub(removed as u32);
                if state.count == 0 {
                    state.lock_type = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reset the global lock table for testing.
    fn reset_locks() {
        *FILE_LOCKS.lock() = BTreeMap::new();
    }

    #[test]
    fn test_shared_lock_acquire() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
    }

    #[test]
    fn test_shared_lock_multiple_readers() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        assert!(flock(0, 1, LOCK_SH, 101).is_ok());
        assert!(flock(0, 1, LOCK_SH, 102).is_ok());
    }

    #[test]
    fn test_exclusive_lock_exclusive() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
    }

    #[test]
    fn test_exclusive_lock_conflict_shared() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        // Non-blocking exclusive should fail if shared lock is held.
        assert_eq!(flock(0, 1, LOCK_EX | LOCK_NB, 101), Err(ERR_WOULD_BLOCK));
    }

    #[test]
    fn test_exclusive_lock_conflict_exclusive() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
        assert_eq!(flock(0, 1, LOCK_EX | LOCK_NB, 101), Err(ERR_WOULD_BLOCK));
    }

    #[test]
    fn test_unlock_releases() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
        assert!(flock(0, 1, LOCK_UN, 100).is_ok());
        // After unlock, another task can acquire an exclusive lock.
        assert!(flock(0, 1, LOCK_EX, 101).is_ok());
    }

    #[test]
    fn test_unlock_shared_allows_exclusive() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        assert!(flock(0, 1, LOCK_SH, 101).is_ok());
        assert!(flock(0, 1, LOCK_UN, 100).is_ok());
        // Still one shared holder — exclusive should still be blocked.
        assert_eq!(flock(0, 1, LOCK_EX | LOCK_NB, 102), Err(ERR_WOULD_BLOCK));
        assert!(flock(0, 1, LOCK_UN, 101).is_ok());
        // All locks released — exclusive should work.
        assert!(flock(0, 1, LOCK_EX, 102).is_ok());
    }

    #[test]
    fn test_different_inodes_independent() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
        // Different inode — should succeed.
        assert!(flock(0, 2, LOCK_EX | LOCK_NB, 101).is_ok());
    }

    #[test]
    fn test_same_task_shared_then_shared_noop() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        // Second LOCK_SH by the same task is a no-op.
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        // After unlock once, lock is released (count goes from 1 to 0).
        assert!(flock(0, 1, LOCK_UN, 100).is_ok());
        // Lock is now free — another task can acquire exclusive.
        assert!(flock(0, 1, LOCK_EX | LOCK_NB, 101).is_ok());
    }

    #[test]
    fn test_same_task_exclusive_shared_noop_upgrade() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
        assert!(flock(0, 1, LOCK_SH, 100).is_ok()); // No-op
    }

    #[test]
    fn test_same_task_shared_exclusive_upgrade() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        // Same task: upgrade shared -> exclusive.
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
    }

    #[test]
    fn test_release_all_for_task() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
        assert!(flock(0, 2, LOCK_SH, 100).is_ok());
        release_all_for_task(100);
        // Other tasks can now lock both inodes.
        assert!(flock(0, 1, LOCK_EX, 200).is_ok());
        assert!(flock(0, 2, LOCK_EX, 200).is_ok());
    }

    #[test]
    fn test_invalid_operation() {
        reset_locks();
        assert_eq!(flock(0, 1, 99, 100), Err(-1)); // InvalidArgument
    }

    #[test]
    fn test_release_nonexistent_lock() {
        reset_locks();
        // Releasing a lock that was never acquired should succeed.
        assert!(flock(0, 999, LOCK_UN, 100).is_ok());
    }

    #[test]
    fn test_fs_id_isolation() {
        reset_locks();
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
        // Same inode number, different filesystem — should be independent.
        assert!(flock(1, 1, LOCK_EX | LOCK_NB, 101).is_ok());
    }

    #[test]
    fn test_upgrade_shared_to_exclusive_blocked() {
        reset_locks();
        assert!(flock(0, 1, LOCK_SH, 100).is_ok());
        assert!(flock(0, 1, LOCK_SH, 101).is_ok());
        // Task 100 cannot upgrade because task 101 also holds a shared lock.
        assert_eq!(flock(0, 1, LOCK_EX | LOCK_NB, 100), Err(ERR_WOULD_BLOCK));
        // Task 101 releases.
        assert!(flock(0, 1, LOCK_UN, 101).is_ok());
        // Now task 100 can upgrade (sole holder).
        assert!(flock(0, 1, LOCK_EX, 100).is_ok());
    }
}
