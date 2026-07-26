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
///   bits [0:27]   `slot_id`    — index into the task's handle table (28 bits, 268M slots)
///   bits [28:37]  `rights`     — capability rights bitmask (10 bits)
///   bits [38:63]  `generation` — prevents use-after-close (26 bits, 67M generations)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(u64);

impl Handle {
    pub fn new(slot_id: u32, rights: Rights, generation: u32) -> Self {
        Self(
            (slot_id as u64 & 0x0FFF_FFFF)
                | ((rights.raw() as u64 & 0x3FF) << 28)
                | ((generation as u64 & 0x03FF_FFFF) << 38),
        )
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn slot_id(self) -> u32 {
        (self.0 & 0x0FFF_FFFF) as u32
    }

    pub fn rights(self) -> Rights {
        Rights::from_raw(((self.0 >> 28) & 0x3FF) as u16)
    }

    pub fn generation(self) -> u32 {
        ((self.0 >> 38) & 0x03FF_FFFF) as u32
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
    /// A one-shot or level-triggered event signal.
    Event(Arc<Mutex<Event>>),
    /// An IRQ-backed event, signaled by a hardware interrupt handler.
    IrqEvent(Arc<Mutex<IrqEvent>>),
    /// An open file in the VFS.
    File(Arc<Mutex<FileHandle>>),
}

/// An open file reference, tracking position and flags.
///
/// Stored inside `KernelObject::File` and shared via `Arc<Mutex<>>` so
/// duplicated handles share the same underlying offset (like POSIX `dup`).
pub struct FileHandle {
    /// Filename in the ramfs (flat namespace for now).
    pub name: alloc::string::String,
    /// Current read/write offset.
    pub offset: u64,
    /// Open flags (reserved for future use).
    pub flags: u32,
}

impl FileHandle {
    pub fn new(name: alloc::string::String, flags: u32) -> Self {
        Self {
            name,
            offset: 0,
            flags,
        }
    }
}

/// A simple event signaling primitive.
///
/// When signaled, wakes any task waiting on it.
pub struct Event {
    /// Whether the event has been signaled.
    pub signaled: bool,
}

/// An IRQ-backed event that is signaled when a hardware interrupt fires.
///
/// Created via `create_irq_event(irq)` and registered with the IRQ handler
/// table. When the corresponding IRQ fires, the handler sets `signaled = true`.
/// User-space can then wait on it via the `sys_irq_wait` syscall.
pub struct IrqEvent {
    /// The hardware IRQ number this event is bound to.
    pub irq: u8,
    /// Whether the IRQ has fired since the last clear.
    pub signaled: bool,
    /// Raw scancode (or device-specific byte) captured by the IRQ handler.
    /// Written by the interrupt handler, read by `sys_irq_wait` so user-space
    /// drivers can consume the data without needing direct port 0x60 access.
    pub last_data: u8,
}

impl IrqEvent {
    /// Create a new unsignaled IRQ event for the given IRQ number.
    pub fn new(irq: u8) -> Self {
        Self {
            irq,
            signaled: false,
            last_data: 0,
        }
    }

    /// Signal the event with optional device data (called from the IRQ handler).
    pub fn signal(&mut self) {
        self.signaled = true;
    }

    /// Signal the event and store the associated device data byte.
    pub fn signal_with_data(&mut self, data: u8) {
        self.last_data = data;
        self.signaled = true;
    }

    /// Check if the event is signaled.
    pub fn is_signaled(&self) -> bool {
        self.signaled
    }

    /// Clear the signal (called after user-space consumes the event).
    pub fn clear(&mut self) {
        self.signaled = false;
    }
}

impl Event {
    /// Create a new unsignaled event.
    pub fn new() -> Self {
        Self { signaled: false }
    }

    /// Signal the event.
    pub fn signal(&mut self) {
        self.signaled = true;
    }

    /// Check if the event is signaled.
    pub fn is_signaled(&self) -> bool {
        self.signaled
    }

    /// Clear the signal (for level-triggered events).
    pub fn clear(&mut self) {
        self.signaled = false;
    }
}

/// Create an `IrqEvent` handle for the given IRQ number.
///
/// Returns the `Arc<Mutex<IrqEvent>>` so the caller can register it with the
/// IRQ handler table. The handle itself must be inserted into a task's handle
/// table by the caller.
pub fn create_irq_event(irq: u8) -> Arc<Mutex<IrqEvent>> {
    Arc::new(Mutex::new(IrqEvent::new(irq)))
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
        let gen = self.generation & 0x03FF_FFFF;
        self.generation = self.generation.wrapping_add(1);
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

    /// Close all handles in the table, dropping all kernel objects.
    pub fn close_all(&mut self) {
        self.slots.clear();
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
            KernelObject::Event(ev) => KernelObject::Event(Arc::clone(ev)),
            KernelObject::IrqEvent(ev) => KernelObject::IrqEvent(Arc::clone(ev)),
            KernelObject::File(fh) => KernelObject::File(Arc::clone(fh)),
        };
        let slot_id = self.next_slot;
        self.next_slot += 1;
        let gen = self.generation & 0x03FF_FFFF;
        self.generation = self.generation.wrapping_add(1);
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

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use spin::Mutex;

    use super::*;

    fn make_channel() -> Arc<Mutex<crate::ipc::Channel>> {
        Arc::new(Mutex::new(crate::ipc::Channel::new()))
    }

    #[test]
    fn test_handle_packing() {
        let rights = Rights::ALL; // 0x3FF
        let h = Handle::new(42, rights, 7);
        assert_eq!(h.slot_id(), 42);
        assert_eq!(h.rights(), Rights::ALL);
        assert_eq!(h.generation(), 7);
    }

    #[test]
    fn test_handle_roundtrip() {
        let h = Handle::new(100, Rights::READ, 0);
        let raw = h.as_u64();
        let h2 = Handle::from_raw(raw);
        assert_eq!(h, h2);
    }

    #[test]
    fn test_rights_contains() {
        assert!(Rights::ALL.contains(Rights::READ));
        assert!(Rights::ALL.contains(Rights::WRITE));
        assert!(Rights::ALL.contains(Rights::TRANSFER));
        assert!(!Rights::READ.contains(Rights::WRITE));
    }

    #[test]
    fn test_rights_intersect() {
        let r = Rights::ALL.intersect(Rights::IO);
        assert_eq!(r, Rights::IO);
        let r = Rights::READ.intersect(Rights::WRITE);
        assert_eq!(r.raw(), 0); // no overlap
    }

    #[test]
    fn test_rights_monotonic_reduction() {
        let original = Rights::ALL;
        let narrowed = original.intersect(Rights::READ);
        assert!(narrowed.contains(Rights::READ));
        assert!(!narrowed.contains(Rights::WRITE));
    }

    #[test]
    fn test_handle_table_insert_and_get() {
        let mut table = HandleTable::new();
        let ch = make_channel();
        let handle = table.insert(KernelObject::ChannelEndA(ch), Rights::ALL);

        assert!(table.get(handle).is_some());
    }

    #[test]
    fn test_handle_table_get_wrong_generation() {
        let mut table = HandleTable::new();
        let ch = make_channel();
        let handle = table.insert(KernelObject::ChannelEndA(ch), Rights::ALL);

        // Create a handle with wrong generation.
        let bad = Handle::new(handle.slot_id(), handle.rights(), handle.generation() + 1);
        assert!(table.get(bad).is_none());
    }

    #[test]
    fn test_handle_table_close() {
        let mut table = HandleTable::new();
        let ch = make_channel();
        let handle = table.insert(KernelObject::ChannelEndA(ch), Rights::ALL);

        assert!(table.close(handle));
        assert!(table.get(handle).is_none());
    }

    #[test]
    fn test_handle_table_close_nonexistent() {
        let mut table = HandleTable::new();
        let bad = Handle::new(999, Rights::ALL, 0);
        assert!(!table.close(bad));
    }

    #[test]
    fn test_handle_table_duplicate() {
        let mut table = HandleTable::new();
        let ch = make_channel();
        let handle = table.insert(KernelObject::ChannelEndA(ch), Rights::ALL);

        let dup = table.duplicate(handle, Rights::READ);
        assert!(dup.is_some());
        let dup = dup.unwrap();
        assert_ne!(dup.slot_id(), handle.slot_id());
        assert!(dup.rights().contains(Rights::READ));
        assert!(!dup.rights().contains(Rights::WRITE)); // narrowed
    }

    #[test]
    fn test_handle_table_duplicate_no_permission() {
        let mut table = HandleTable::new();
        let ch = make_channel();
        // Insert with rights that don't include DUPLICATE.
        let handle = table.insert(KernelObject::ChannelEndA(ch), Rights::READ);

        let dup = table.duplicate(handle, Rights::READ);
        assert!(dup.is_none()); // no DUPLICATE right
    }

    #[test]
    fn test_handle_table_multiple_inserts() {
        let mut table = HandleTable::new();
        let ch1 = make_channel();
        let ch2 = make_channel();

        let h1 = table.insert(KernelObject::ChannelEndA(ch1), Rights::ALL);
        let h2 = table.insert(KernelObject::ChannelEndB(ch2), Rights::READ);

        assert_ne!(h1.slot_id(), h2.slot_id());
        assert!(table.get(h1).is_some());
        assert!(table.get(h2).is_some());
    }

    #[test]
    fn test_handle_with_rights() {
        let h = Handle::new(0, Rights::ALL, 0);
        let narrowed = h.with_rights(Rights::READ);
        assert!(narrowed.rights().contains(Rights::READ));
        assert!(!narrowed.rights().contains(Rights::WRITE));
    }

    // ─── Event tests ───

    #[test]
    fn test_event_new() {
        let ev = Event::new();
        assert!(!ev.is_signaled());
    }

    #[test]
    fn test_event_signal() {
        let mut ev = Event::new();
        ev.signal();
        assert!(ev.is_signaled());
    }

    #[test]
    fn test_event_clear() {
        let mut ev = Event::new();
        ev.signal();
        assert!(ev.is_signaled());
        ev.clear();
        assert!(!ev.is_signaled());
    }

    #[test]
    fn test_event_in_handle_table() {
        let mut table = HandleTable::new();
        let ev = Event::new();
        let handle = table.insert(KernelObject::Event(Arc::new(Mutex::new(ev))), Rights::ALL);
        assert!(table.get(handle).is_some());
    }

    #[test]
    fn test_event_signal_via_handle() {
        let mut table = HandleTable::new();
        let ev = Event::new();
        let handle = table.insert(KernelObject::Event(Arc::new(Mutex::new(ev))), Rights::ALL);

        // Signal the event via the handle.
        if let Some(KernelObject::Event(ev)) = table.get(handle) {
            ev.lock().signal();
            assert!(ev.lock().is_signaled());
        } else {
            panic!("Expected Event");
        }
    }

    #[test]
    fn test_event_duplicate() {
        let mut table = HandleTable::new();
        let ev = Event::new();
        let handle = table.insert(KernelObject::Event(Arc::new(Mutex::new(ev))), Rights::ALL);

        let dup = table.duplicate(handle, Rights::SIGNAL);
        assert!(dup.is_some());
    }

    // ─── File handle tests ───

    #[test]
    fn test_file_handle_new() {
        let fh = FileHandle::new(alloc::string::String::from("test.txt"), 0);
        assert_eq!(fh.name, "test.txt");
        assert_eq!(fh.offset, 0);
        assert_eq!(fh.flags, 0);
    }

    #[test]
    fn test_file_handle_in_table() {
        let mut table = HandleTable::new();
        let fh = FileHandle::new(alloc::string::String::from("hello.txt"), 0);
        let handle = table.insert(KernelObject::File(Arc::new(Mutex::new(fh))), Rights::IO);
        assert!(table.get(handle).is_some());
    }

    #[test]
    fn test_file_handle_duplicate() {
        let mut table = HandleTable::new();
        let fh = FileHandle::new(alloc::string::String::from("data.bin"), 0);
        let handle = table.insert(KernelObject::File(Arc::new(Mutex::new(fh))), Rights::ALL);

        let dup = table.duplicate(handle, Rights::READ);
        assert!(dup.is_some());
        let dup = dup.unwrap();
        assert!(dup.rights().contains(Rights::READ));
        assert!(!dup.rights().contains(Rights::WRITE));
    }

    #[test]
    fn test_file_handle_shared_offset() {
        let fh = Arc::new(Mutex::new(FileHandle::new(
            alloc::string::String::from("shared.txt"),
            0,
        )));
        let mut table = HandleTable::new();
        let h1 = table.insert(KernelObject::File(Arc::clone(&fh)), Rights::ALL);
        let h2 = table.duplicate(h1, Rights::ALL).unwrap();

        // Both handles share the same FileHandle via Arc.
        if let Some(KernelObject::File(f)) = table.get(h1) {
            f.lock().offset = 42;
        }
        if let Some(KernelObject::File(f)) = table.get(h2) {
            assert_eq!(f.lock().offset, 42); // shared state
        }
    }

    // ─── IrqEvent tests ───

    #[test]
    fn test_irq_event_new() {
        let ev = IrqEvent::new(1);
        assert_eq!(ev.irq, 1);
        assert!(!ev.is_signaled());
    }

    #[test]
    fn test_irq_event_signal() {
        let mut ev = IrqEvent::new(1);
        ev.signal();
        assert!(ev.is_signaled());
    }

    #[test]
    fn test_irq_event_clear() {
        let mut ev = IrqEvent::new(1);
        ev.signal();
        assert!(ev.is_signaled());
        ev.clear();
        assert!(!ev.is_signaled());
    }

    #[test]
    fn test_irq_event_in_handle_table() {
        let mut table = HandleTable::new();
        let ev = create_irq_event(1);
        let handle = table.insert(KernelObject::IrqEvent(Arc::clone(&ev)), Rights::ALL);
        assert!(table.get(handle).is_some());
    }

    #[test]
    fn test_irq_event_signal_via_handle() {
        let mut table = HandleTable::new();
        let ev = create_irq_event(1);
        let handle = table.insert(KernelObject::IrqEvent(Arc::clone(&ev)), Rights::ALL);

        if let Some(KernelObject::IrqEvent(ev)) = table.get(handle) {
            ev.lock().signal();
            assert!(ev.lock().is_signaled());
        } else {
            panic!("Expected IrqEvent");
        }
    }

    #[test]
    fn test_irq_event_duplicate() {
        let mut table = HandleTable::new();
        let ev = create_irq_event(1);
        let handle = table.insert(KernelObject::IrqEvent(Arc::clone(&ev)), Rights::ALL);

        let dup = table.duplicate(handle, Rights::WAIT);
        assert!(dup.is_some());
    }
}
