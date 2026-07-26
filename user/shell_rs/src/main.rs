//! Interactive shell for OpenOS — Rust implementation.
//!
//! Supports built-in commands: help, exit, echo, ls, cat, run, clear.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use openos_sdk::{console, fs, process};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in shell!");
    process::exit(1);
}

/// Read a line from the console into `buf`. Returns the number of bytes read.
fn read_line(buf: &mut [u8]) -> usize {
    let mut pos = 0;
    loop {
        let mut byte = [0u8; 1];
        let Ok(n) = console::read(&mut byte, true) else {
            break;
        };
        if n == 0 {
            continue;
        }
        match byte[0] {
            b'\n' | b'\r' => {
                let _ = console::write("\n");
                break;
            }
            0x08 | 0x7F => {
                // Backspace
                if pos > 0 {
                    pos -= 1;
                    let _ = console::write("\x08 \x08");
                }
            }
            b if pos < buf.len() - 1 => {
                buf[pos] = b;
                pos += 1;
                let _ = console::write(core::str::from_utf8(&[b]).unwrap_or("?"));
            }
            _ => {}
        }
    }
    buf[pos] = 0;
    pos
}

/// Split input into command and arguments.
fn split_cmd(input: &[u8]) -> (&[u8], &[u8]) {
    let s = match core::str::from_utf8(input) {
        Ok(s) => s.trim(),
        Err(_) => return (b"", b""),
    };
    let trimmed = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    match trimmed.find(' ') {
        Some(i) => (trimmed[..i].as_bytes(), trimmed[i + 1..].as_bytes()),
        None => (trimmed.as_bytes(), b""),
    }
}

fn cmd_help() {
    let _ = console::writeln("Available commands:");
    let _ = console::writeln("  help          Show this help");
    let _ = console::writeln("  echo <msg>    Print a message");
    let _ = console::writeln("  ls            List files in ramfs");
    let _ = console::writeln("  cat <file>    Print file contents");
    let _ = console::writeln("  run <elf>     Run a program from initrd");
    let _ = console::writeln("  clear         Clear screen (serial)");
    let _ = console::writeln("  exit          Exit the shell");
}

fn cmd_echo(args: &[u8]) {
    if let Ok(s) = core::str::from_utf8(args) {
        let _ = console::writeln(s);
    }
}

fn cmd_ls() {
    match fs::open(".") {
        Ok(fd) => {
            let mut buf = [0u8; 512];
            if let Ok(n) = fs::read(fd, &mut buf) {
                if n == 0 {
                    let _ = console::writeln("(empty)");
                } else if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    for name in s.split('\n') {
                        if !name.is_empty() {
                            let _ = console::write("  ");
                            let _ = console::writeln(name);
                        }
                    }
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            let _ = console::writeln("ls: cannot list files");
        }
    }
}

fn cmd_cat(args: &[u8]) {
    let Ok(filename) = core::str::from_utf8(args) else {
        let _ = console::writeln("cat: invalid filename");
        return;
    };
    let filename = filename.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if filename.is_empty() {
        let _ = console::writeln("cat: missing filename");
        return;
    }

    match fs::open(filename) {
        Ok(fd) => {
            let mut buf = [0u8; 2048];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                            let _ = console::write(s);
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = console::writeln("");
            let _ = fs::close(fd);
        }
        Err(_) => {
            let _ = console::write("cat: file not found: ");
            let _ = console::writeln(filename);
        }
    }
}

fn cmd_run(args: &[u8]) {
    let Ok(elf_name) = core::str::from_utf8(args) else {
        let _ = console::writeln("run: invalid name");
        return;
    };
    let elf_name = elf_name.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if elf_name.is_empty() {
        let _ = console::writeln("run: missing ELF filename");
        return;
    }

    match process::create(elf_name) {
        Ok(task_id) => {
            let _ = console::write("Starting ");
            let _ = console::writeln(elf_name);
            if process::start(task_id, elf_name).is_err() {
                let _ = console::writeln("run: failed to start");
                return;
            }
            let _ = process::wait(task_id, 1000);
        }
        Err(_) => {
            let _ = console::writeln("run: failed to create process");
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("OpenOS Shell v0.2 (Rust)");
    let _ = console::writeln("Type 'help' for available commands.");
    let _ = console::writeln("");

    let mut input_buf = [0u8; 256];

    loop {
        let _ = console::write("openos> ");
        let len = read_line(&mut input_buf);
        if len == 0 {
            continue;
        }

        let (cmd, args) = split_cmd(&input_buf[..len]);

        match cmd {
            b"help" | b"?" => cmd_help(),
            b"exit" | b"quit" => {
                let _ = console::writeln("Goodbye!");
                process::exit(0);
            }
            b"echo" => cmd_echo(args),
            b"ls" => cmd_ls(),
            b"cat" => cmd_cat(args),
            b"run" => cmd_run(args),
            b"clear" => {
                // ANSI escape: clear screen + move cursor home
                let _ = console::write("\x1b[2J\x1b[H");
            }
            b"" => {}
            _ => {
                let _ = console::write("unknown command: ");
                if let Ok(s) = core::str::from_utf8(cmd) {
                    let _ = console::writeln(s);
                }
            }
        }
    }
}
