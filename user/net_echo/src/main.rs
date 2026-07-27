//! Network echo test — sends a raw Ethernet frame via SYS_NET_SEND.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, net, process};

/// Simple bump allocator for user-space (64 KiB heap).
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 65536]>,
    offset: core::cell::Cell<usize>,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut off = self.offset.get();
        off = (off + align - 1) & !(align - 1);
        if off + size > 65536 {
            return core::ptr::null_mut();
        }
        let ptr = (*self.heap.get()).as_mut_ptr().add(off);
        self.offset.set(off + size);
        ptr
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: no-op dealloc.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0u8; 65536]),
    offset: core::cell::Cell::new(0),
};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in net_echo!");
    process::exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("net_echo: testing network send...");

    // Build a minimal Ethernet frame (broadcast)
    let mut frame = [0u8; 64];
    // Dst MAC: broadcast
    frame[0..6].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    // Src MAC: 52:54:00:12:34:56 (QEMU default)
    frame[6..12].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    // EtherType: 0x0800 (IPv4)
    frame[12] = 0x08;
    frame[13] = 0x00;

    match net::send_frame(&frame) {
        Ok(bytes) => {
            let _ = console::writeln("net_echo: frame sent!");
            let _ = console::write(&format_usize(bytes));
            let _ = console::writeln(" bytes");
        }
        Err(_) => {
            let _ = console::writeln("net_echo: send failed");
        }
    }

    process::exit(0);
}

fn format_usize(mut n: usize) -> &'static str {
    // Use a static buffer for simplicity in no_std.
    static mut BUF: [u8; 20] = [0u8; 20];
    let mut i = 19;
    if n == 0 {
        // SAFETY: Single-threaded user program.
        unsafe {
            BUF[19] = b'0';
            return core::str::from_utf8_unchecked(&BUF[19..20]);
        }
    }
    while n > 0 {
        // SAFETY: Single-threaded user program.
        unsafe {
            BUF[i] = b'0' + (n % 10) as u8;
        }
        n /= 10;
        if i > 0 {
            i -= 1;
        }
    }
    // SAFETY: We only write ASCII digits.
    unsafe { core::str::from_utf8_unchecked(&BUF[i + 1..20]) }
}
