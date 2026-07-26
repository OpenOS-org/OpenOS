//! Handle table and kernel object management.
//!
//! Implements the capability-based access model from INTERFACE.md:
//! - **Handle**: opaque, unforgeable token referencing a kernel object
//! - **Rights**: bitmask of allowed operations (monotonic reduction only)
//! - **`HandleTable`**: per-task container mapping slot IDs to kernel objects
//! - **`KernelObject`**: typed enum of all kernel object kinds
//!
//! Handles are the *only* way user-space interacts with the kernel.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use spin::Mutex;

use crate::ipc::Channel;

/// Rights bitmask. Each bit grants a specific permission.
/// Rights can only be narrowed (via `intersect`), never amplified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(u16);

#[allow(dead_code)]
impl Rights {
    pub const ALL: Self = Self(0x3FF);
    pub const BASIC: Self =
        Self(Self::TRANSFER.0 | Self::DUPLICATE.0 | Self::WAIT.0 | Self::DESTROY.0);
    pub const CONFIGURE: Self = Self(1 << 9);
    pub const DESTROY: Self = Self(1 << 7);
    pub const DUPLICATE: Self = Self(1 << 4);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const IO: Self = Self(Self::READ.0 | Self::WRITE.0);
    pub const MAP: Self = Self(1 << 8);
    pub const READ: Self = Self(1 << 0);
    pub const SIGNAL: Self = Self(1 << 5);
    pub const TRANSFER: Self = Self(1 << 3);
    pub const WAIT: Self = Self(1 << 6);
    pub const WRITE: Self = Self(1 << 1);

    pub fn from_raw(raw: u16) -> Self {
        Self(raw & 0x3FF)
    }

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Intersect (narrow) two rights sets — monotonic privilege reduction.
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub fn raw(self) -> u16 {
        self.0
    }
}

/// A Handle is a process-local token referencing a kernel object.
///
/// Packed into 64 bits:
///   bits [0:31]   `slot_id`    — index into the task's handle table
///   bits [32:47]  rights       — capability rights bitmask
///   bits [48:63]  generation   — prevents use-after-close
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(u64);

impl Handle {
    pub fn new(slot_id: u32, rights: Rights, generation: u16) -> Self {
        Self(slot_id as u64 | ((rights.raw() as u64) << 32) | ((generation as u64) << 48))
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn slot_id(self) -> u32 {
        self.0 as u32
    }

    pub fn rights(self) -> Rights {
        Rights::from_raw(((self.0 >> 32) & 0xFFFF) as u16)
    }

    pub fn generation(self) -> u16 {
        ((self.0 >> 48) & 0xFFFF) as u16
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Create a derived handle with narrowed rights.
    pub fn with_rights(self, new_rights: Rights) -> Self {
        Self::new(
            self.slot_id(),
            self.rights().intersect(new_rights),
            self.generation(),
        )
    }
}

/// A typed kernel object that a Handle can reference.
///
/// Channel endpoints are distinguished: `ChannelEndA` and `ChannelEndB`
/// both reference the same underlying `Channel` but represent opposite
/// ends. This lets the syscall layer know which end to operate on without
/// storing extra metadata in the Handle.
pub enum KernelObject {
    /// End A of a channel (typically the "client" end).
    ChannelEndA(Arc<Mutex<Channel>>),
    /// End B of a channel (typically the "server" end).
    ChannelEndB(Arc<Mutex<Channel>>),
}

/// Entry in a handle table.
struct HandleEntry {
    handle: Handle,
    object: KernelObject,
}

/// Per-task handle table.
pub struct HandleTable {
    slots: BTreeMap<u32, HandleEntry>,
    next_slot: u32,
    generation: u32,
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
            next_slot: 0,
            generation: 0,
        }
    }

    /// Insert a kernel object, returning a new Handle.
    pub fn insert(&mut self, object: KernelObject, rights: Rights) -> Handle {
        let slot_id = self.next_slot;
        self.next_slot += 1;
        let gen = (self.generation & 0xFFFF) as u16;
        self.generation += 1;
        let handle = Handle::new(slot_id, rights, gen);
        self.slots.insert(slot_id, HandleEntry { handle, object });
        handle
    }

    /// Get the kernel object behind a handle, validating generation.
    pub fn get(&self, handle: Handle) -> Option<&KernelObject> {
        self.slots.get(&handle.slot_id()).and_then(|entry| {
            if entry.handle.generation() == handle.generation() {
                Some(&entry.object)
            } else {
                None
            }
        })
    }

    /// Remove a handle from the table.
    pub fn close(&mut self, handle: Handle) -> bool {
        let gen = handle.generation();
        match self.slots.get(&handle.slot_id()) {
            Some(entry) if entry.handle.generation() == gen => {
                self.slots.remove(&handle.slot_id());
                true
            }
            _ => false,
        }
    }

    /// Duplicate a handle with optionally narrowed rights.
    pub fn duplicate(&mut self, handle: Handle, new_rights: Rights) -> Option<Handle> {
        if !handle.rights().contains(Rights::DUPLICATE) {
            return None;
        }
        let entry = self.slots.get(&handle.slot_id())?;
        if entry.handle.generation() != handle.generation() {
            return None;
        }
        // Clone the kernel object reference.
        let object = match &entry.object {
            KernelObject::ChannelEndA(ch) => KernelObject::ChannelEndA(Arc::clone(ch)),
            KernelObject::ChannelEndB(ch) => KernelObject::ChannelEndB(Arc::clone(ch)),
        };
        let slot_id = self.next_slot;
        self.next_slot += 1;
        let gen = (self.generation & 0xFFFF) as u16;
        self.generation += 1;
        let new_handle = Handle::new(slot_id, handle.rights().intersect(new_rights), gen);
        self.slots.insert(
            slot_id,
            HandleEntry {
                handle: new_handle,
                object,
            },
        );
        Some(new_handle)
    }
}
