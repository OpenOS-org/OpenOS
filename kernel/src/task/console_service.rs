//! Kernel console service — inline message processing.
//!
//! When a user sends a message to the console channel, the syscall handler
//! processes it inline: prints to serial and replies with "OK".
//! This avoids the need for kernel task context switching.
//!
//! ```text
//! User task (Ring 3)              Kernel (inline)
//!     │                                │
//!     │ channel_send(msg)              │
//!     │ ────────────────────────────→  │ receive → print to serial
//!     │                                │ reply("OK")
//!     │   ←─────────────────────────── │
//!     │ (returns immediately)          │
//! ```

use alloc::sync::Arc;
use alloc::vec;

use spin::Mutex;

use crate::handle::{KernelObject, Rights};
use crate::ipc::{Channel, EndId};

/// Global channel for the console service.
static CONSOLE_CHANNEL: Mutex<Option<Arc<Mutex<Channel>>>> = Mutex::new(None);

/// The handle value for the console channel (end A).
/// User-space uses this to send messages to the console service.
/// Set during `init()`.
pub static CONSOLE_HANDLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Get the console channel (if initialized).
pub fn get_channel() -> Option<Arc<Mutex<Channel>>> {
    CONSOLE_CHANNEL.lock().as_ref().map(Arc::clone)
}

/// Process a pending message on a channel inline.
/// Called from the syscall handler after `channel_send` returns Pending.
/// Receives the message from the peer end, prints it to serial, and replies.
/// Returns `true` if a message was processed.
pub fn process_pending(channel: &Arc<Mutex<Channel>>, end: crate::ipc::EndId) -> bool {
    let mut ch = channel.lock();
    // Try to receive on the opposite end.
    let recv_end = match end {
        crate::ipc::EndId::A => crate::ipc::EndId::B,
        crate::ipc::EndId::B => crate::ipc::EndId::A,
    };
    match ch.receive(recv_end, 0) {
        crate::ipc::RecvResult::GotMessage(msg) => {
            // Print the message to serial.
            for &byte in &msg {
                if (0x20..=0x7e).contains(&byte) || byte == b'\n' {
                    crate::serial_print!("{}", byte as char);
                }
            }
            // Reply with acknowledgment.
            ch.reply(recv_end, vec![b'O', b'K']);
            true
        }
        crate::ipc::RecvResult::Blocked => false,
    }
}

/// Initialize the console service.
///
/// Creates a Channel and registers end A in the current task's handle table
/// so user-space can discover it.
pub fn init() {
    use crate::task::scheduler;

    let channel = Arc::new(Mutex::new(Channel::new()));

    // Store globally.
    *CONSOLE_CHANNEL.lock() = Some(Arc::clone(&channel));

    // Register end A in the idle task's handle table.
    scheduler::with_current_task_mut(|task| {
        let handle_a = task
            .handle_table
            .insert(KernelObject::ChannelEndA(Arc::clone(&channel)), Rights::ALL);
        CONSOLE_HANDLE.store(handle_a.as_u64(), core::sync::atomic::Ordering::Release);
        crate::serial_println!("[CONSOLE] handle_a={:#x} registered", handle_a.as_u64());
    });

    crate::serial_println!("[CONSOLE] Service initialized (inline mode)");
}
