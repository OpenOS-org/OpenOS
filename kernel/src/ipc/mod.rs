//! Channel-based IPC — the primary communication mechanism.
//!
//! A Channel is a bidirectional, synchronous message pipe between two
//! endpoints. Messages are byte sequences. The Channel has two ends;
//! each end can send and receive.
//!
//! ## Semantics
//!
//! - `send` blocks until the peer calls `receive` (rendezvous)
//! - `receive` blocks until a message arrives
//! - `call` = atomic send + block for reply (the RPC primitive)
//! - `reply` unblocks a waiting `call`

use alloc::vec::Vec;

/// Identifies which end of a channel is being operated on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndId {
    A,
    B,
}

/// State for one end of a Channel.
struct EndState {
    /// Message waiting to be received (set by the peer's `send`).
    pending_msg: Option<Vec<u8>>,
    /// Task blocked in `receive` on this end.
    blocked_receiver: Option<u64>,
    /// Task blocked in `call` on this end (waiting for peer to reply).
    blocked_caller: Option<u64>,
    /// Reply message waiting to be consumed by the caller.
    pending_reply: Option<Vec<u8>>,
}

/// A bidirectional message channel with two ends (A and B).
pub struct Channel {
    end_a: EndState,
    end_b: EndState,
}

/// Result of a `send` operation.
pub enum SendResult {
    /// Message delivered to a waiting receiver. Caller should unblock this task.
    Delivered(u64),
    /// Message stored, sender should block until receiver calls `receive`.
    Pending,
}

impl Channel {
    /// Create a new channel pair.
    pub fn new() -> Self {
        Self {
            end_a: EndState {
                pending_msg: None,
                blocked_receiver: None,
                blocked_caller: None,
                pending_reply: None,
            },
            end_b: EndState {
                pending_msg: None,
                blocked_receiver: None,
                blocked_caller: None,
                pending_reply: None,
            },
        }
    }

    /// Send a message from one end.
    pub fn send(&mut self, from: EndId, msg: Vec<u8>) -> SendResult {
        let (_src, dst) = self.ends_mut(from);
        if let Some(receiver_id) = dst.blocked_receiver.take() {
            dst.pending_msg = Some(msg);
            SendResult::Delivered(receiver_id)
        } else {
            _src.pending_msg = Some(msg);
            SendResult::Pending
        }
    }

    /// Receive a message on one end. Returns the message if available,
    /// or registers the caller as blocked and returns `None`.
    pub fn receive(&mut self, on: EndId, task_id: u64) -> Option<Vec<u8>> {
        let (src, _dst) = self.ends_mut(on);
        if let Some(msg) = src.pending_msg.take() {
            Some(msg)
        } else {
            src.blocked_receiver = Some(task_id);
            None
        }
    }

    /// Call = send + block for reply. Returns `Some(reply)` if already available.
    pub fn call(&mut self, from: EndId, msg: Vec<u8>, task_id: u64) -> Option<Vec<u8>> {
        let (src, dst) = self.ends_mut(from);
        if let Some(reply) = src.pending_reply.take() {
            return Some(reply);
        }
        let _receiver_id = dst.blocked_receiver.take();
        // Whether or not a receiver was waiting, store the message.
        // _receiver_id will be used later to unblock the receiver task.
        dst.pending_msg = Some(msg);
        src.blocked_caller = Some(task_id);
        None
    }

    /// Reply to a message. Unblocks the caller's `call`.
    pub fn reply(&mut self, on: EndId, reply: Vec<u8>) -> Option<u64> {
        let (_src, dst) = self.ends_mut(on);
        if let Some(caller_id) = dst.blocked_caller.take() {
            dst.pending_reply = Some(reply);
            Some(caller_id)
        } else {
            dst.pending_reply = Some(reply);
            None
        }
    }

    /// Get mutable references to (`my_end`, `peer_end`).
    fn ends_mut(&mut self, on: EndId) -> (&mut EndState, &mut EndState) {
        match on {
            EndId::A => (&mut self.end_a, &mut self.end_b),
            EndId::B => (&mut self.end_b, &mut self.end_a),
        }
    }
}

/// Initialize the IPC subsystem.
pub fn init() {
    crate::println!("[...] Initializing IPC subsystem");
    crate::serial_println!("[...] Initializing IPC subsystem");
    crate::println!("[OK] IPC subsystem initialized");
    crate::serial_println!("[OK] IPC subsystem initialized");
}
