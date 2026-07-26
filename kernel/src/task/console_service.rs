//! Kernel console service — demonstrates multi-process Channel RPC.
//!
//! This service creates a Channel, registers the "server" end (B) as a
//! well-known handle in the idle task's handle table, and creates a
//! kernel task that loops receiving messages and printing them to serial.
//!
//! User-space sends a message via `channel_call` on end A; the kernel
//! service receives it on end B, prints it, and replies with an
//! acknowledgment. This demonstrates the full RPC cycle:
//!
//! ```text
//! User task (Ring 3)              Kernel console service
//!     │                                │
//!     │ channel_call(msg)              │
//!     │ ───────────────────────────→   │ channel_receive()
//!     │                                │ print to serial
//!     │                                │ channel_reply("ok")
//!     │   ←─────────────────────────── │
//!     │ (unblocked with reply)         │
//! ```

use alloc::sync::Arc;
use alloc::vec;

use spin::Mutex;

use crate::handle::{HandleTable, KernelObject, Rights};
use crate::ipc::{Channel, EndId};

/// The console service's kernel stack (8 KiB).
const CONSOLE_STACK_SIZE: usize = 4096 * 2;

/// Static kernel stack for the console service task.
static mut CONSOLE_STACK: [u8; CONSOLE_STACK_SIZE] = [0; CONSOLE_STACK_SIZE];

/// Entry point for the console service kernel task.
///
/// This function runs as a kernel task (Ring 0). It loops receiving
/// messages on the channel's end B and printing them to serial.
/// It never returns.
fn console_service_entry() -> ! {
    // The channel end B handle is stored in the task's handle table.
    // For Phase 3, we access it through a global since the task doesn't
    // have a clean way to get its own handle table yet.
    //
    // Instead, we use a simpler approach: the channel is stored in a global
    // and the service reads from it directly.
    crate::serial_println!("[CONSOLE] Service started");

    loop {
        // Try to receive a message from the channel.
        // In a full implementation, this would block and context-switch
        // back to the user task. For now, we spin-wait.
        if let Some(channel) = get_console_channel() {
            let mut ch = channel.lock();
            // Try to receive on end B (the server end).
            match ch.receive(EndId::B, 0) {
                crate::ipc::RecvResult::GotMessage(msg) => {
                    // Print the message to serial.
                    for &byte in &msg {
                        if (0x20..=0x7e).contains(&byte) || byte == b'\n' {
                            crate::serial_print!("{}", byte as char);
                        }
                    }
                    // Reply with acknowledgment.
                    drop(ch);
                    let reply = vec![b'O', b'K'];
                    if let Some(ch) = get_console_channel() {
                        ch.lock().reply(EndId::B, reply);
                    }
                }
                crate::ipc::RecvResult::Blocked => {
                    // No message yet — yield and try again.
                    drop(ch);
                    x86_64::instructions::hlt();
                }
            }
        } else {
            x86_64::instructions::hlt();
        }
    }
}

/// Global channel for the console service.
/// Set up during `init()`, accessed by both the service and syscall handlers.
static CONSOLE_CHANNEL: spin::Mutex<Option<Arc<Mutex<Channel>>>> = spin::Mutex::new(None);

/// Get the console service channel (if initialized).
fn get_console_channel() -> Option<Arc<Mutex<Channel>>> {
    CONSOLE_CHANNEL.lock().as_ref().map(Arc::clone)
}

/// Initialize the console service.
///
/// Creates a Channel, stores it globally, and creates a kernel task
/// that loops receiving and printing messages.
pub fn init() {
    use crate::task::scheduler;

    // Create the channel.
    let channel = Arc::new(Mutex::new(Channel::new()));

    // Store globally so the service and syscalls can access it.
    *CONSOLE_CHANNEL.lock() = Some(Arc::clone(&channel));

    // Register end A in the idle task's handle table so user-space
    // can discover it. The idle task is task ID 0.
    scheduler::with_current_task_mut(|task| {
        let handle_a = task
            .handle_table
            .insert(KernelObject::ChannelEndA(Arc::clone(&channel)), Rights::ALL);
        crate::serial_println!("[CONSOLE] handle_a={:#x} registered", handle_a.as_u64());
    });

    // Create the console service kernel task.
    // The task's entry point is `console_service_entry`.
    // Its stack is the static CONSOLE_STACK.
    let stack_top = (&raw const CONSOLE_STACK) as u64 + CONSOLE_STACK_SIZE as u64;
    let entry = console_service_entry as *const () as u64;

    // Create the task with a pre-set context.
    let mut task = crate::task::task::Task::new("console_svc", 1);
    task.context = Some(crate::task::task::SavedContext::kernel_mode(
        entry, stack_top,
    ));

    // Add to scheduler.
    scheduler::spawn_task_from(task);

    crate::serial_println!("[CONSOLE] Service task created");
}
