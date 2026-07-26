//! `OpenOS` — A microkernel operating system written in Rust.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::module_inception,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    dead_code,
    unused_imports,
    unused_variables,
    clippy::missing_const_for_fn,
    clippy::used_underscore_items
)]

extern crate alloc;

use alloc::vec::Vec;
use core::panic::PanicInfo;

mod arch;
mod drivers;
mod elf;
mod frame_alloc;
mod fs;
mod handle;
mod initrd;
mod ipc;
mod memory;
mod net;
pub mod sync;
mod syscall;
mod task;

use bootloader_api::config::Mapping;
use bootloader_api::info::Optional;
use bootloader_api::{entry_point, BootloaderConfig};

/// Global ramdisk data. Set once during boot, read-only thereafter.
/// Used by `process_start` to load ELF binaries from the initrd.
static mut RAMDISK_DATA: Option<&'static [u8]> = None;

/// Bootloader configuration for `OpenOS`.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 80 * 1024;
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

#[allow(clippy::too_many_lines)]
fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    drivers::serial::SERIAL1.lock();

    if let Some(offset) = boot_info.physical_memory_offset.as_ref() {
        memory::set_physical_memory_offset(*offset);
    }

    let ramdisk_len = boot_info.ramdisk_len as usize;
    let ramdisk_phys: Option<u64> = match &boot_info.ramdisk_addr {
        Optional::Some(addr) => Some(*addr),
        Optional::None => None,
    };

    // Extract ramdisk info before VGA init consumes boot_info.
    // (Memory map extraction deferred to after heap init.)

    drivers::vga::init(boot_info);

    println!("=================================");
    println!("  OpenOS Microkernel v0.3.0");
    println!("=================================");
    println!();
    serial_println!("=================================");
    serial_println!("  OpenOS Microkernel v0.3.0");
    serial_println!("=================================");
    serial_println!();

    arch::x86_64::init();
    memory::init();
    fs::init();
    drivers::net::init();
    drivers::virtio_block::init();
    ipc::init();
    task::init();

    // Register ramfs as the root filesystem at "/".
    {
        use alloc::sync::Arc;
        let ramfs: Arc<dyn fs::vfs::FileSystem> = Arc::new(fs::ramfs::RamFsVfs);
        if fs::vfs::mount("/", 0, ramfs).is_err() {
            serial_println!("[WARN] Failed to mount ramfs at '/'");
        }
    }

    // If a VirtIO-Block device is available, try to mount ext2 at "/disk".
    {
        use alloc::sync::Arc;
        if drivers::block::get_device(0).is_some() {
            match fs::ext2::Ext2Fs::open(0) {
                Ok(ext2) => {
                    let disk_fs: Arc<dyn fs::vfs::FileSystem> = Arc::new(ext2);
                    if fs::vfs::mount("/disk", 0, disk_fs).is_err() {
                        serial_println!("[WARN] Failed to mount ext2 at '/disk'");
                    }
                }
                Err(()) => {
                    serial_println!("[SKIP] No ext2 filesystem on block device 0");
                }
            }
        }
    }

    // DHCP: negotiate an IP address from the network.
    {
        let mac = drivers::net::mac_address();
        let success =
            net::dhcp::dhcp_negotiate(mac, drivers::net::send_frame, drivers::net::receive_frame);
        if success {
            let state = net::dhcp::get_network_state();
            println!(
                "[OK] DHCP: {}.{}.{}.{}",
                state.ip[0], state.ip[1], state.ip[2], state.ip[3]
            );
            serial_println!(
                "[OK] DHCP: {}.{}.{}.{}",
                state.ip[0],
                state.ip[1],
                state.ip[2],
                state.ip[3]
            );
        } else {
            println!("[WARN] DHCP failed, using default IP");
            serial_println!("[WARN] DHCP failed, using default IP");
        }
    }

    // Wire up IRQ 1 (keyboard) through the IRQ forwarding mechanism.
    // This creates an IrqEvent and registers it so that when IRQ 1 fires,
    // the event is signaled, allowing user-space to wait on it via sys_irq_wait.
    {
        let irq_event = handle::create_irq_event(1);
        arch::x86_64::interrupts::register_irq_event(1, alloc::sync::Arc::clone(&irq_event));

        // Insert the IrqEvent into the idle task's handle table so it can be
        // transferred to user-space processes.
        let irq_handle = task::scheduler::with_current_task_mut(|task| {
            task.handle_table.insert(
                handle::KernelObject::IrqEvent(irq_event),
                handle::Rights::WAIT,
            )
        })
        .unwrap();

        serial_println!(
            "[...] IRQ forwarding: IRQ 1 (keyboard) -> handle {:#x}",
            irq_handle.as_u64()
        );
    }

    println!("[OK] Kernel initialization complete");
    serial_println!("[OK] Kernel initialization complete");

    // Extract ramdisk.
    // SAFETY: `virt_addr` is a virtual address provided by the bootloader (already
    // mapped). `ramdisk_len` is the size reported by the bootloader. The ramdisk
    // is read-only after boot.
    let ramdisk = ramdisk_phys.and_then(|virt_addr| {
        if ramdisk_len > 0 {
            Some(unsafe { core::slice::from_raw_parts(virt_addr as *const u8, ramdisk_len) })
        } else {
            None
        }
    });

    if let Some(rd) = ramdisk {
        // SAFETY: Storing the ramdisk reference in a global. This is safe because
        // the ramdisk is read-only after boot and the reference is 'static.
        unsafe {
            RAMDISK_DATA = Some(rd);
        }
        serial_println!("[...] Ramdisk loaded ({} bytes)", rd.len());

        // Create a channel for inter-process communication.
        let channel = alloc::sync::Arc::new(spin::Mutex::new(crate::ipc::Channel::new()));

        // Register end A in the idle task's handle table.
        let handle_a = crate::task::scheduler::with_current_task_mut(|task| {
            task.handle_table.insert(
                crate::handle::KernelObject::ChannelEndA(alloc::sync::Arc::clone(&channel)),
                crate::handle::Rights::ALL,
            )
        })
        .unwrap();

        // Register end B.
        let handle_b = crate::task::scheduler::with_current_task_mut(|task| {
            task.handle_table.insert(
                crate::handle::KernelObject::ChannelEndB(channel),
                crate::handle::Rights::ALL,
            )
        })
        .unwrap();

        serial_println!(
            "[...] Channel: handle_a={:#x}, handle_b={:#x}",
            handle_a.as_u64(),
            handle_b.as_u64()
        );

        // Step 1: Kernel sends a test message via end A.
        // The message is stored in the channel — no receiver needed yet.
        {
            let ch = crate::task::scheduler::with_current_task(|task| {
                match task.handle_table.get(handle_a) {
                    Some(crate::handle::KernelObject::ChannelEndA(ch)) => {
                        alloc::sync::Arc::clone(ch)
                    }
                    _ => unreachable!(),
                }
            })
            .unwrap();
            let msg = alloc::vec![
                b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r', b'o', b'm', b' ', b'u', b's', b'e',
                b'r', b'-', b's', b'p', b'a', b'c', b'e', b' ', b's', b'e', b'r', b'v', b'i', b'c',
                b'e', b'!', b'\n'
            ];
            ch.lock().send(crate::ipc::EndId::A, msg, 0);
            serial_println!("[...] Kernel sent message to channel");
        }

        // Step 2: Launch console service (end B).
        // The message is already in the channel — service receives it immediately,
        // prints to serial, replies "OK", and exits.
        // IRETQ to Ring 3 does not return.
        serial_println!("[...] Launching console service (user-space)");
        task::user::launch_from_initrd(rd, "console_svc.elf", handle_b.as_u64());
    } else {
        serial_println!("[SKIP] No ramdisk — cannot load user program");
        task::user::launch_first_process();
    }

    println!("[OK] Kernel halted");
    serial_println!("[OK] Kernel halted");
    loop {
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[PANIC] {info}");
    serial_println!("[PANIC] {info}");
    loop {
        x86_64::instructions::hlt();
    }
}

#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation error: {layout:?}");
}
