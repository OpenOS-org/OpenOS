//! Inter-Process Communication (IPC) subsystem.
//!
//! In a microkernel, user-space services communicate exclusively through IPC.
//! The kernel provides only the plumbing — message routing, port management,
//! and blocking/receiving. All protocol logic (request/response, capability
//! delegation, shared memory negotiation) lives in user space.
//!
//! Design: L4-inspired port-based message passing.
//!   - Each service owns a port (mailbox) identified by a `u64` ID.
//!   - Messages are copied into the receiver's inbox (no zero-copy yet).
//!   - Send is non-blocking; receive blocks until a message arrives.

use alloc::collections::VecDeque;

use spin::Mutex;

use crate::println;

/// An IPC message. The kernel copies this from the sender's address space
/// into the receiver's port inbox. Copy semantics (not reference) because
/// sender and receiver have separate address spaces in a full microkernel.
#[derive(Debug, Clone)]
pub struct Message {
    /// Task ID of the sender (set by the kernel, not by user-space).
    pub sender: u64,
    /// Port ID of the intended recipient.
    pub receiver: u64,
    /// The actual payload.
    pub data: MessageData,
}

/// Typed message payload. The `Request`/`Response` variants enable a
/// synchronous RPC pattern: the sender tags the request with a correlation
/// ID, and the responder echoes it back so the sender can match the reply.
#[derive(Debug, Clone)]
pub enum MessageData {
    /// Human-readable text (debugging, logging).
    Text(alloc::string::String),
    /// Opaque binary payload.
    Bytes(alloc::vec::Vec<u8>),
    /// Outgoing request — `id` correlates with the matching `Response`.
    Request {
        /// Caller-chosen correlation ID.
        id: u64,
        /// Serialized request body.
        data: alloc::vec::Vec<u8>,
    },
    /// Incoming response — `id` matches the original `Request`.
    Response {
        /// Echoed correlation ID from the request.
        id: u64,
        /// Serialized response body.
        data: alloc::vec::Vec<u8>,
    },
}

/// A message port (mailbox). Each task that wants to receive IPC messages
/// creates a port. Messages queue in `inbox` until the owner calls `receive`.
struct Port {
    id: u64,
    inbox: VecDeque<Message>,
}

lazy_static::lazy_static! {
    static ref IPC_MANAGER: Mutex<IpcManager> = Mutex::new(IpcManager::new());
}

/// Central IPC registry. Maps port IDs to port objects.
struct IpcManager {
    /// Port ID → Port. `BTreeMap` gives O(log n) lookup by port ID.
    ports: alloc::collections::BTreeMap<u64, Port>,
    /// Monotonically increasing port ID counter.
    next_port_id: u64,
}

impl IpcManager {
    fn new() -> Self {
        Self {
            ports: alloc::collections::BTreeMap::new(),
            // Start at 1 so port 0 can be reserved as "no port" / "kernel".
            next_port_id: 1,
        }
    }

    /// Allocate a new port and return its ID. The port starts with an empty inbox.
    fn create_port(&mut self) -> u64 {
        let id = self.next_port_id;
        self.next_port_id += 1;
        self.ports.insert(
            id,
            Port {
                id,
                inbox: VecDeque::new(),
            },
        );
        id
    }

    /// Enqueue a message in the receiver's port inbox.
    ///
    /// Returns `PortNotFound` if no port with the given ID exists — the sender
    /// is responsible for handling this (e.g., retry, or report error to user).
    fn send(&mut self, message: Message) -> Result<(), IpcError> {
        if let Some(port) = self.ports.get_mut(&message.receiver) {
            port.inbox.push_back(message);
            Ok(())
        } else {
            Err(IpcError::PortNotFound)
        }
    }

    /// Dequeue the oldest message from a port's inbox. Returns `None` if the
    /// inbox is empty (non-blocking). A blocking receive would park the task
    /// and wake it when a message arrives — not yet implemented.
    fn receive(&mut self, port_id: u64) -> Option<Message> {
        self.ports
            .get_mut(&port_id)
            .and_then(|p| p.inbox.pop_front())
    }
}

/// IPC error codes.
#[derive(Debug)]
pub enum IpcError {
    /// No port with the requested ID exists.
    PortNotFound,
    /// Message exceeds the port's capacity limit (not yet enforced).
    MessageTooLarge,
    /// Port inbox is at capacity (not yet enforced).
    QueueFull,
}

/// Create a new IPC port. Returns the port ID, which the owner uses to
/// receive messages and which other tasks use to send messages.
#[must_use]
pub fn create_port() -> u64 {
    IPC_MANAGER.lock().create_port()
}

/// Send a message. The message's `receiver` field identifies the target port.
pub fn send(message: Message) -> Result<(), IpcError> {
    IPC_MANAGER.lock().send(message)
}

/// Non-blocking receive. Returns the oldest pending message, or `None`.
#[must_use]
pub fn receive(port_id: u64) -> Option<Message> {
    IPC_MANAGER.lock().receive(port_id)
}

/// Initialize the IPC subsystem and create the kernel's well-known port.
///
/// The kernel port (ID 1) is used by system services to register themselves
/// and by user tasks to discover available services.
pub fn init() {
    println!("[...] Initializing IPC subsystem");
    let _kernel_port = create_port();
    println!("[OK] IPC subsystem initialized");
}
