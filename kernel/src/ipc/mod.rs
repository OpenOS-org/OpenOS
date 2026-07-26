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
//!
//! ## Storage
//!
//! Channels are stored in per-task handle tables as `KernelObject::ChannelEndA`
//! and `KernelObject::ChannelEndB`. Both ends reference the same `Channel`
//! via `Arc<Mutex<Channel>>`.

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
    /// Task ID blocked in `receive` on this end.
    blocked_receiver: Option<u64>,
    /// Task ID blocked in `send` on this end (waiting for peer to receive).
    blocked_sender: Option<u64>,
    /// Task ID blocked in `call` on this end (waiting for peer to reply).
    blocked_caller: Option<u64>,
    /// Reply message waiting to be consumed by the caller.
    pending_reply: Option<Vec<u8>>,
}

/// A bidirectional message channel with two ends (A and B).
pub struct Channel {
    end_a: EndState,
    end_b: EndState,
    /// Handles pending transfer (serialized as u64 values).
    /// When a handle is transferred via `handle_transfer`, it's stored here.
    /// When the next message is received, pending handles are prepended.
    pub pending_handles: Vec<u64>,
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
                blocked_sender: None,
                blocked_caller: None,
                pending_reply: None,
            },
            end_b: EndState {
                pending_msg: None,
                blocked_receiver: None,
                blocked_sender: None,
                blocked_caller: None,
                pending_reply: None,
            },
            pending_handles: Vec::new(),
        }
    }

    /// Send a message from one end to the other.
    pub fn send(&mut self, from: EndId, msg: Vec<u8>, sender_task_id: u64) -> SendResult {
        let (_src, dst) = self.ends_mut(from);
        if let Some(receiver_id) = dst.blocked_receiver.take() {
            // Peer is waiting — deliver immediately.
            dst.pending_msg = Some(msg);
            SendResult::Delivered(receiver_id)
        } else {
            // Store on destination end so the peer can receive it.
            dst.pending_msg = Some(msg);
            // Sender blocks until peer calls receive.
            _src.blocked_sender = Some(sender_task_id);
            SendResult::Pending
        }
    }

    /// Receive a message on one end.
    pub fn receive(&mut self, on: EndId, task_id: u64) -> RecvResult {
        let (src, _dst) = self.ends_mut(on);
        if let Some(msg) = src.pending_msg.take() {
            // Unblock the sender if it was waiting.
            src.blocked_sender = None;
            RecvResult::GotMessage(msg)
        } else {
            // No message — block the receiver.
            src.blocked_receiver = Some(task_id);
            RecvResult::Blocked
        }
    }

    /// Call = send + block for reply. The atomic RPC primitive.
    pub fn call(&mut self, from: EndId, msg: Vec<u8>, task_id: u64) -> CallResult {
        let (src, dst) = self.ends_mut(from);
        if let Some(reply) = src.pending_reply.take() {
            return CallResult::GotReply(reply);
        }
        let _receiver_id = dst.blocked_receiver.take();
        dst.pending_msg = Some(msg);
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

/// Initialize the IPC subsystem.
pub fn init() {
    crate::println!("[...] Initializing IPC subsystem");
    crate::serial_println!("[...] Initializing IPC subsystem");
    crate::println!("[OK] IPC subsystem initialized");
    crate::serial_println!("[OK] IPC subsystem initialized");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_send_receive() {
        let mut ch = Channel::new();
        let msg = vec![b'H', b'i'];

        // Send from A — should be pending (no receiver).
        let result = ch.send(EndId::A, msg.clone(), 1);
        assert!(matches!(result, SendResult::Pending));

        // Receive on B — should get the message.
        let result = ch.receive(EndId::B, 2);
        match result {
            RecvResult::GotMessage(data) => assert_eq!(data, msg),
            _ => panic!("Expected GotMessage"),
        }
    }

    #[test]
    fn test_channel_send_delivered() {
        let mut ch = Channel::new();

        // Register a blocked receiver on B first.
        ch.receive(EndId::B, 2);

        // Send from A — should deliver immediately.
        let msg = vec![1, 2, 3];
        let result = ch.send(EndId::A, msg, 1);
        match result {
            SendResult::Delivered(task_id) => assert_eq!(task_id, 2),
            _ => panic!("Expected Delivered"),
        }
    }

    #[test]
    fn test_channel_call_reply() {
        let mut ch = Channel::new();
        let msg = vec![b'q', b'u', b'e', b'r', b'y'];

        // Call from A — should be blocked.
        let result = ch.call(EndId::A, msg.clone(), 1);
        assert!(matches!(result, CallResult::Blocked));

        // The message should be available on B.
        let recv = ch.receive(EndId::B, 2);
        match recv {
            RecvResult::GotMessage(data) => assert_eq!(data, msg),
            _ => panic!("Expected GotMessage"),
        }

        // Reply from B — should unblock A.
        let reply = vec![b'o', b'k'];
        let result = ch.reply(EndId::B, reply.clone());
        match result {
            ReplyResult::Unblocked(task_id) => assert_eq!(task_id, 1),
            _ => panic!("Expected Unblocked"),
        }

        // A should get the reply on next call.
        let result = ch.call(EndId::A, vec![], 1);
        match result {
            CallResult::GotReply(data) => assert_eq!(data, reply),
            _ => panic!("Expected GotReply"),
        }
    }

    #[test]
    fn test_channel_receive_empty() {
        let mut ch = Channel::new();

        // Receive on empty channel — should block.
        let result = ch.receive(EndId::A, 1);
        assert!(matches!(result, RecvResult::Blocked));
    }

    #[test]
    fn test_channel_bidirectional() {
        let mut ch = Channel::new();

        // A sends to B.
        ch.send(EndId::A, vec![1], 1);
        // B sends to A.
        ch.send(EndId::B, vec![2], 2);

        // B receives from A.
        let r = ch.receive(EndId::B, 2);
        assert!(matches!(r, RecvResult::GotMessage(ref d) if d == &[1]));

        // A receives from B.
        let r = ch.receive(EndId::A, 1);
        assert!(matches!(r, RecvResult::GotMessage(ref d) if d == &[2]));
    }

    #[test]
    fn test_channel_reply_without_caller() {
        let mut ch = Channel::new();

        // Reply without a pending call — should store.
        let result = ch.reply(EndId::A, vec![42]);
        assert!(matches!(result, ReplyResult::Stored));
    }

    #[test]
    fn test_channel_pending_handles() {
        let mut ch = Channel::new();
        ch.pending_handles.push(0xDEAD);
        ch.pending_handles.push(0xBEEF);
        assert_eq!(ch.pending_handles.len(), 2);
        assert_eq!(ch.pending_handles[0], 0xDEAD);
        assert_eq!(ch.pending_handles[1], 0xBEEF);
    }
}
