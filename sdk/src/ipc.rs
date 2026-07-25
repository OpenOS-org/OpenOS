//! Inter-Process Communication (IPC) primitives.
//!
//! Provides wrappers for the kernel's port-based message passing syscalls.
//! Each task creates ports (mailboxes) and sends/receives messages through them.

use crate::abi::error::Result;
use crate::abi::{number, raw};

/// An IPC message.
///
/// This mirrors the kernel's `Message` struct layout. The kernel copies
/// the entire message from sender to receiver (no shared memory).
#[repr(C)]
#[derive(Debug, Clone)]
pub struct Message {
    /// Task ID of the sender (set by the kernel).
    pub sender: u64,
    /// Port ID of the intended recipient.
    pub receiver: u64,
    /// Message payload.
    pub data: MessageData,
}

/// Typed message payload.
///
/// The `Request`/`Response` variants enable a synchronous RPC pattern:
/// the sender tags the request with a correlation ID, and the responder
/// echoes it back so the sender can match the reply.
#[repr(C)]
#[derive(Debug, Clone)]
pub enum MessageData {
    /// Human-readable text (for debugging, logging).
    Text(u64, u64), // (ptr, len) — inline string would need alloc
    /// Opaque binary payload.
    Bytes(u64, u64), // (ptr, len)
    /// Outgoing request — `id` correlates with the matching `Response`.
    Request {
        /// Caller-chosen correlation ID.
        id: u64,
        /// Pointer to request data.
        ptr: u64,
        /// Length of request data in bytes.
        len: u64,
    },
    /// Incoming response — `id` matches the original `Request`.
    Response {
        /// Echoed correlation ID from the request.
        id: u64,
        /// Pointer to response data.
        ptr: u64,
        /// Length of response data in bytes.
        len: u64,
    },
}

/// Create a new IPC port. Returns the port ID.
///
/// The port starts with an empty inbox. Other tasks can send messages
/// to this port using the returned ID.
///
/// # Errors
/// Returns an error if the kernel cannot allocate a new port.
pub fn port_create() -> Result<u64> {
    let ret = unsafe { raw::syscall0(number::SYS_PORT_CREATE) };
    crate::abi::error::decode(ret)
}

/// Send a message to a port.
///
/// The kernel copies the message from the caller's address space into
/// the receiver's port inbox. This is non-blocking — the message is
/// queued even if the receiver hasn't called `receive` yet.
///
/// # Errors
/// Returns `Error::InvalidArgument` if the port ID is invalid.
pub fn send(port_id: u64, message: &Message) -> Result<()> {
    let ret = unsafe { raw::syscall2(number::SYS_SEND, port_id, message as *const Message as u64) };
    crate::abi::error::decode(ret).map(|_| ())
}

/// Receive a message from a port (blocking).
///
/// Dequeues the oldest pending message from the port's inbox. If no
/// message is available, this blocks until one arrives.
///
/// # Errors
/// Returns `Error::InvalidArgument` if the port ID is invalid.
pub fn receive(port_id: u64, buffer: &mut Message) -> Result<()> {
    let ret = unsafe { raw::syscall2(number::SYS_RECEIVE, port_id, buffer as *mut Message as u64) };
    crate::abi::error::decode(ret).map(|_| ())
}
