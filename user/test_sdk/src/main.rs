//! Integration test for the OpenOS SDK — exercises channel, handle, event, and fs APIs.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use openos_sdk::{channel, console, event, fs, handle, process};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in test_sdk!");
    process::exit(1);
}

/// Helper to assert a condition, printing pass/fail to the console.
fn check(name: &str, ok: bool) {
    if ok {
        let _ = console::write("  PASS: ");
    } else {
        let _ = console::write("  FAIL: ");
    }
    let _ = console::writeln(name);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("=== OpenOS SDK Integration Test ===");

    // --- Channel IPC ---
    let _ = console::writeln("[Channel]");
    let ch = channel::create();
    check("channel::create", ch.is_ok());

    if let Ok((a, b)) = ch {
        let send_res = channel::send(a, b"ping");
        check("channel::send", send_res.is_ok());

        let mut recv_buf = [0u8; 64];
        let recv_res = channel::receive(b, &mut recv_buf);
        check("channel::receive", recv_res.is_ok());
        if let Ok(len) = recv_res {
            check("message content", &recv_buf[..len] == b"ping");
        }

        // Test call/reply — call blocks until reply, so just verify API exists.
        let mut reply_buf = [0u8; 64];
        let _call_res = channel::call(a, b"rpc-request", &mut reply_buf);

        let _ = handle::close(a);
        let _ = handle::close(b);
    }

    // --- Event ---
    let _ = console::writeln("[Event]");
    let ev = event::create();
    check("event::create", ev.is_ok());

    if let Ok(ev_handle) = ev {
        check("event::wait (pre-signal)", event::wait(ev_handle).is_err());
        let _ = event::signal(ev_handle);
        check("event::wait (post-signal)", event::wait(ev_handle).is_ok());
        let _ = event::destroy(ev_handle);
    }

    // --- Filesystem ---
    let _ = console::writeln("[Filesystem]");
    let fd = fs::open("test.txt");
    check("fs::open (existing)", fd.is_ok());

    if let Ok(fd) = fd {
        let mut buf = [0u8; 64];
        let read_res = fs::read(fd, &mut buf);
        check("fs::read", read_res.is_ok());

        let write_res = fs::write(fd, b"hello from test");
        check("fs::write", write_res.is_ok());

        let _ = fs::close(fd);
    }

    // --- Console ---
    let _ = console::writeln("[Console]");
    let _ = console::writeln("console::writeln works");

    // --- Process ---
    let _ = console::writeln("[Process]");
    let proc = process::create("child");
    check("process::create", proc.is_ok());

    if let Ok(task_id) = proc {
        let start_res = process::start(task_id, "hello.elf");
        check("process::start", start_res.is_ok());

        let wait_res = process::wait(task_id, 100);
        check("process::wait", wait_res.is_ok());
    }

    let _ = console::writeln("=== Test Complete ===");
    process::exit(0);
}
