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

use spin::Mutex;

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
    /// Task ID blocked in `receive` on this end.
    blocked_receiver: Option<u64>,
    /// Task ID blocked in `call` on this end (waiting for peer to reply).
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
    /// Message delivered to a waiting receiver. Contains the receiver's task ID.
    Delivered(u64),
    /// Message stored, sender should block until receiver calls `receive`.
    Pending,
}

/// Result of a `receive` operation.
pub enum RecvResult {
    /// Message was available. Contains the message bytes.
    GotMessage(Vec<u8>),
    /// No message available. Caller is now registered as blocked.
    /// The caller's task ID has been stored; the sender will unblock it.
    Blocked,
}

/// Result of a `call` operation.
pub enum CallResult {
    /// Reply was already available (fast path).
    GotReply(Vec<u8>),
    /// No reply yet. Caller is now registered as blocked.
    Blocked,
}

/// Result of a `reply` operation.
pub enum ReplyResult {
    /// Caller was waiting. Contains the caller's task ID to unblock.
    Unblocked(u64),
    /// No caller was waiting. Reply stored for later consumption.
    Stored,
}

#[allow(dead_code)]
impl Channel {
    /// Create a new channel.
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
    pub fn send(&mut self, from: EndId, msg: Vec<u8>, sender_task_id: u64) -> SendResult {
        let (src, dst) = self.ends_mut(from);
        if let Some(receiver_id) = dst.blocked_receiver.take() {
            // Peer is already waiting in receive — deliver immediately.
            dst.pending_msg = Some(msg);
            SendResult::Delivered(receiver_id)
        } else {
            // No receiver waiting — store the message, sender blocks.
            src.pending_msg = Some(msg);
            src.blocked_receiver = Some(sender_task_id); // sender waits for someone to receive
            SendResult::Pending
        }
    }

    /// Receive a message on one end.
    pub fn receive(&mut self, on: EndId, task_id: u64) -> RecvResult {
        let (src, _dst) = self.ends_mut(on);
        if let Some(msg) = src.pending_msg.take() {
            // A message is already waiting — deliver it.
            // If a sender was blocked, we should unblock it.
            // (For rendezvous: the sender blocked until we received.)
            src.blocked_receiver = None; // sender is no longer blocked
            RecvResult::GotMessage(msg)
        } else {
            // No message — block the caller.
            src.blocked_receiver = Some(task_id);
            RecvResult::Blocked
        }
    }

    /// Call = send + block for reply. The atomic RPC primitive.
    pub fn call(&mut self, from: EndId, msg: Vec<u8>, task_id: u64) -> CallResult {
        let (src, dst) = self.ends_mut(from);

        // Check if there's already a pending reply from a previous call.
        if let Some(reply) = src.pending_reply.take() {
            return CallResult::GotReply(reply);
        }

        // Deliver the message to the peer.
        let _receiver_id = dst.blocked_receiver.take();
        dst.pending_msg = Some(msg);

        // Caller now blocks waiting for reply.
        src.blocked_caller = Some(task_id);
        CallResult::Blocked
    }

    /// Reply to a message received on one end. Unblocks the caller's `call`.
    pub fn reply(&mut self, on: EndId, reply: Vec<u8>) -> ReplyResult {
        let (_src, dst) = self.ends_mut(on);
        if let Some(caller_id) = dst.blocked_caller.take() {
            dst.pending_reply = Some(reply);
            ReplyResult::Unblocked(caller_id)
        } else {
            dst.pending_reply = Some(reply);
            ReplyResult::Stored
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

/// Global channel registry. Maps channel IDs to Channel objects.
/// In a full implementation, channels would be kernel objects accessed
/// through the handle table. This registry is a bridge for Phase 2.
struct ChannelRegistry {
    channels: alloc::collections::BTreeMap<u64, Channel>,
    next_id: u64,
}

impl ChannelRegistry {
    fn new() -> Self {
        Self {
            channels: alloc::collections::BTreeMap::new(),
            next_id: 1,
        }
    }

    fn create(&mut self) -> (u64, EndId, u64, EndId) {
        let id = self.next_id;
        self.next_id += 1;
        let channel = Channel::new();
        self.channels.insert(id, channel);
        // Both handles reference the same channel but different ends.
        // handle_a → EndId::A, handle_b → EndId::B
        (id, EndId::A, id, EndId::B)
    }

    fn get(&self, id: u64) -> Option<&Channel> {
        self.channels.get(&id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Channel> {
        self.channels.get_mut(&id)
    }
}

lazy_static::lazy_static! {
    static ref REGISTRY: Mutex<ChannelRegistry> = Mutex::new(ChannelRegistry::new());
}

/// Create a new channel. Returns (`channel_id`, `end_a`, `end_b`).
pub fn create_channel() -> (u64, EndId, EndId) {
    let mut reg = REGISTRY.lock();
    let (id, end_a, _id_b, end_b) = reg.create();
    (id, end_a, end_b)
}

/// Send a message on a channel.
pub fn channel_send(channel_id: u64, from: EndId, msg: Vec<u8>, task_id: u64) -> SendResult {
    let mut reg = REGISTRY.lock();
    reg.get_mut(channel_id)
        .map_or(SendResult::Pending, |ch| ch.send(from, msg, task_id))
}

/// Receive a message on a channel.
pub fn channel_receive(channel_id: u64, on: EndId, task_id: u64) -> RecvResult {
    let mut reg = REGISTRY.lock();
    reg.get_mut(channel_id)
        .map_or(RecvResult::Blocked, |ch| ch.receive(on, task_id))
}

/// Call = send + block for reply.
pub fn channel_call(channel_id: u64, from: EndId, msg: Vec<u8>, task_id: u64) -> CallResult {
    let mut reg = REGISTRY.lock();
    reg.get_mut(channel_id)
        .map_or(CallResult::Blocked, |ch| ch.call(from, msg, task_id))
}

/// Reply to a message on a channel.
pub fn channel_reply(channel_id: u64, on: EndId, reply: Vec<u8>) -> ReplyResult {
    let mut reg = REGISTRY.lock();
    reg.get_mut(channel_id)
        .map_or(ReplyResult::Stored, |ch| ch.reply(on, reply))
}

/// Initialize the IPC subsystem.
pub fn init() {
    crate::println!("[...] Initializing IPC subsystem");
    crate::serial_println!("[...] Initializing IPC subsystem");
    crate::println!("[OK] IPC subsystem initialized");
    crate::serial_println!("[OK] IPC subsystem initialized");
}
