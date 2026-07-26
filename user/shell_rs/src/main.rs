//! Interactive shell for OpenOS -- Rust implementation.
//!
//! Supports built-in commands: help, exit, echo, ls, cat, run, clear,
//! cd, pwd, mkdir, rmdir, env, export, unset, ps, rm, cp, touch, stat.
//! Disk filesystem access via /disk mount point.
//! Environment variable expansion with $VAR syntax.
//! Output redirection with > operator.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, env, fs, process};

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
        // Align
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
    let _ = console::writeln("PANIC in shell!");
    process::exit(1);
}

/// Maximum number of commands in history.
const HISTORY_SIZE: usize = 32;
/// Maximum input line length.
const MAX_LINE: usize = 256;
/// Simple command history ring buffer.
struct History {
    entries: [[u8; MAX_LINE]; HISTORY_SIZE],
    lengths: [usize; HISTORY_SIZE],
    head: usize,
    count: usize,
}

impl History {
    const fn new() -> Self {
        Self {
            entries: [[0u8; MAX_LINE]; HISTORY_SIZE],
            lengths: [0usize; HISTORY_SIZE],
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, line: &[u8]) {
        let len = line.len().min(MAX_LINE - 1);
        self.entries[self.head][..len].copy_from_slice(&line[..len]);
        self.entries[self.head][len] = 0;
        self.lengths[self.head] = len;
        self.head = (self.head + 1) % HISTORY_SIZE;
        if self.count < HISTORY_SIZE {
            self.count += 1;
        }
    }

    fn display(&self) {
        let start = if self.count < HISTORY_SIZE {
            0
        } else {
            self.head
        };
        for i in 0..self.count {
            let idx = (start + i) % HISTORY_SIZE;
            let _ = console::write("  ");
            // Print history number
            let mut buf = [0u8; 16];
            let num = format_u32(&mut buf, i as u32 + 1);
            let _ = console::write(num);
            let _ = console::write("  ");
            if let Ok(s) = core::str::from_utf8(&self.entries[idx][..self.lengths[idx]]) {
                let _ = console::writeln(s);
            }
        }
    }
}

/// Format a u32 into a buffer, return as str.
fn format_u32<'a>(buf: &'a mut [u8], val: u32) -> &'a str {
    if val == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut n = val;
    let mut pos = 0;
    let mut digits = [0u8; 10];
    let mut dcount = 0;
    while n > 0 {
        digits[dcount] = b'0' + (n % 10) as u8;
        n /= 10;
        dcount += 1;
    }
    for i in (0..dcount).rev() {
        if pos < buf.len() {
            buf[pos] = digits[i];
            pos += 1;
        }
    }
    core::str::from_utf8(&buf[..pos]).unwrap_or("?")
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

/// Expand environment variables in a string ($VAR or ${VAR}).
fn expand_vars(input: &str) -> String {
    let mut result = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            i += 1;
            let (name, consumed) = if bytes[i] == b'{' {
                // ${VAR} syntax
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'}' {
                    i += 1;
                }
                let name = &input[start..i];
                if i < bytes.len() {
                    i += 1; // skip }
                }
                (name, true)
            } else {
                // $VAR syntax
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                (&input[start..i], true)
            };
            if consumed {
                if let Ok(val) = env::get(name) {
                    if let Some(v) = val {
                        result.push_str(&v);
                    }
                }
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Find the position of `>` redirect operator, skipping `>>`.
fn find_redirect(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                return None; // append not supported
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Write output to a file if redirect is present, otherwise write to console.
fn output_line(text: &str, redirect_file: Option<&str>) {
    match redirect_file {
        Some(filename) => match fs::create(filename) {
            Ok(fd) => {
                let _ = fs::write(fd, text.as_bytes());
                let _ = fs::write(fd, b"\n");
                let _ = fs::close(fd);
            }
            Err(_) => {
                let _ = console::write("shell: cannot create: ");
                let _ = console::writeln(filename);
            }
        },
        None => {
            let _ = console::writeln(text);
        }
    }
}

fn cmd_help() {
    let _ = console::writeln("OpenOS Shell v0.4 — Available commands:");
    let _ = console::writeln("");
    let _ = console::writeln("  File operations:");
    let _ = console::writeln("    ls [path]          List files");
    let _ = console::writeln("    cat <file>         Print file contents");
    let _ = console::writeln("    cp <src> <dst>     Copy a file");
    let _ = console::writeln("    rm <file>          Delete a file");
    let _ = console::writeln("    touch <file>       Create empty file");
    let _ = console::writeln("    stat <file>        Show file metadata");
    let _ = console::writeln("    mkdir <dir>        Create directory");
    let _ = console::writeln("    rmdir <dir>        Remove empty directory");
    let _ = console::writeln("");
    let _ = console::writeln("  Navigation:");
    let _ = console::writeln("    cd [dir]           Change directory");
    let _ = console::writeln("    pwd                Print working directory");
    let _ = console::writeln("");
    let _ = console::writeln("  Environment:");
    let _ = console::writeln("    env                Show all variables");
    let _ = console::writeln("    export K=V         Set variable");
    let _ = console::writeln("    unset <key>        Remove variable");
    let _ = console::writeln("    echo $VAR          Expand variable");
    let _ = console::writeln("");
    let _ = console::writeln("  Process:");
    let _ = console::writeln("    run <elf>          Run a program");
    let _ = console::writeln("    ps                 List processes");
    let _ = console::writeln("");
    let _ = console::writeln("  Other:");
    let _ = console::writeln("    history            Show command history");
    let _ = console::writeln("    echo <msg> > FILE  Redirect output to file");
    let _ = console::writeln("    clear              Clear screen");
    let _ = console::writeln("    help               Show this help");
    let _ = console::writeln("    exit               Exit shell");
}

fn cmd_echo(args: &[u8]) {
    let Ok(s) = core::str::from_utf8(args) else {
        return;
    };
    let expanded = expand_vars(s.trim_matches(|c: char| c == '\0'));

    if let Some(redirect_pos) = find_redirect(&expanded) {
        let data = expanded[..redirect_pos].trim();
        let filename = expanded[redirect_pos + 1..].trim();
        if filename.is_empty() {
            let _ = console::writeln("echo: missing filename after >");
            return;
        }
        output_line(data, Some(filename));
    } else {
        let _ = console::writeln(&expanded);
    }
}

fn cmd_ls(args: &[u8]) {
    let path = match core::str::from_utf8(args) {
        Ok(s) => {
            let trimmed = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
            if trimmed.is_empty() {
                "."
            } else {
                trimmed
            }
        }
        Err(_) => ".",
    };

    let expanded = expand_vars(path);

    match fs::open(&expanded) {
        Ok(fd) => {
            let mut buf = [0u8; 1024];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                            for name in s.split('\n') {
                                if !name.is_empty() {
                                    let _ = console::write("  ");
                                    let _ = console::writeln(name);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            let _ = console::write("ls: cannot access '");
            let _ = console::write(&expanded);
            let _ = console::writeln("': No such file or directory");
        }
    }
}

fn cmd_cat(args: &[u8]) {
    let Ok(filename) = core::str::from_utf8(args) else {
        let _ = console::writeln("cat: invalid filename");
        return;
    };
    let expanded = expand_vars(filename.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("cat: missing filename");
        return;
    }

    match fs::open(&expanded) {
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
            let _ = console::writeln(&expanded);
        }
    }
}

fn cmd_run(args: &[u8]) {
    let Ok(elf_name) = core::str::from_utf8(args) else {
        let _ = console::writeln("run: invalid name");
        return;
    };
    let expanded = expand_vars(elf_name.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("run: missing ELF filename");
        return;
    }

    match process::create(&expanded) {
        Ok(task_id) => {
            let _ = console::write("Starting ");
            let _ = console::writeln(&expanded);
            if process::start(task_id, &expanded).is_err() {
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

fn cmd_cd(args: &[u8]) {
    let Ok(path) = core::str::from_utf8(args) else {
        return;
    };
    let trimmed = path.trim_matches(|c: char| c == '\0' || c.is_whitespace());

    let target = if trimmed.is_empty() {
        // cd with no args goes to root
        "/"
    } else {
        trimmed
    };

    let expanded = expand_vars(target);

    match env::chdir(&expanded) {
        Ok(()) => {
            // Update PS1 prompt with new cwd
            if let Ok(cwd) = env::cwd() {
                let _ = env::set("PWD", &cwd);
            }
        }
        Err(_) => {
            let _ = console::write("cd: no such directory: ");
            let _ = console::writeln(&expanded);
        }
    }
}

fn cmd_pwd() {
    match env::cwd() {
        Ok(cwd) => {
            let _ = console::writeln(&cwd);
        }
        Err(_) => {
            let _ = console::writeln("/");
        }
    }
}

fn cmd_mkdir(args: &[u8]) {
    let Ok(name) = core::str::from_utf8(args) else {
        return;
    };
    let expanded = expand_vars(name.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("mkdir: missing directory name");
        return;
    }

    match fs::mkdir(&expanded) {
        Ok(()) => {}
        Err(_) => {
            let _ = console::write("mkdir: cannot create directory '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
        }
    }
}

fn cmd_rmdir(args: &[u8]) {
    let Ok(name) = core::str::from_utf8(args) else {
        return;
    };
    let expanded = expand_vars(name.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("rmdir: missing directory name");
        return;
    }

    match fs::rmdir(&expanded) {
        Ok(()) => {}
        Err(_) => {
            let _ = console::write("rmdir: failed to remove '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
        }
    }
}

fn cmd_rm(args: &[u8]) {
    let Ok(name) = core::str::from_utf8(args) else {
        return;
    };
    let expanded = expand_vars(name.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("rm: missing filename");
        return;
    }

    match fs::unlink(&expanded) {
        Ok(()) => {}
        Err(_) => {
            let _ = console::write("rm: cannot remove '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
        }
    }
}

fn cmd_touch(args: &[u8]) {
    let Ok(name) = core::str::from_utf8(args) else {
        return;
    };
    let expanded = expand_vars(name.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("touch: missing filename");
        return;
    }

    // Create the file (or open if exists — just updates timestamp conceptually).
    match fs::create(&expanded) {
        Ok(fd) => {
            let _ = fs::close(fd);
        }
        Err(_) => {
            let _ = console::write("touch: cannot create '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
        }
    }
}

fn cmd_cp(args: &[u8]) {
    let Ok(names) = core::str::from_utf8(args) else {
        return;
    };
    let trimmed = names.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let (src, dst) = match trimmed.find(|c: char| c.is_whitespace()) {
        Some(i) => (trimmed[..i].trim(), trimmed[i..].trim()),
        None => {
            let _ = console::writeln("cp: usage: cp <source> <dest>");
            return;
        }
    };

    if src.is_empty() || dst.is_empty() {
        let _ = console::writeln("cp: usage: cp <source> <dest>");
        return;
    }

    let src_exp = expand_vars(src);
    let dst_exp = expand_vars(dst);

    let src_fd = match fs::open(&src_exp) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = console::write("cp: cannot open '");
            let _ = console::write(&src_exp);
            let _ = console::writeln("'");
            return;
        }
    };

    let dst_fd = match fs::create(&dst_exp) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = fs::close(src_fd);
            let _ = console::write("cp: cannot create '");
            let _ = console::write(&dst_exp);
            let _ = console::writeln("'");
            return;
        }
    };

    let mut buf = [0u8; 2048];
    loop {
        match fs::read(src_fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if fs::write(dst_fd, &buf[..n]).is_err() {
                    let _ = console::writeln("cp: write error");
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = fs::close(src_fd);
    let _ = fs::close(dst_fd);
}

fn cmd_stat(args: &[u8]) {
    let Ok(name) = core::str::from_utf8(args) else {
        return;
    };
    let expanded = expand_vars(name.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("stat: missing filename");
        return;
    }

    // Try to get file size as a basic stat.
    match fs::file_size(&expanded) {
        Ok(size) => {
            let _ = console::write("  File: ");
            let _ = console::writeln(&expanded);
            let _ = console::write("  Size: ");
            let mut buf = [0u8; 32];
            let s = format_u64(&mut buf, size);
            let _ = console::write(s);
            let _ = console::writeln(" bytes");
        }
        Err(_) => {
            let _ = console::write("stat: cannot stat '");
            let _ = console::write(&expanded);
            let _ = console::writeln("': No such file or directory");
        }
    }
}

/// Format a u64 into a buffer, return as str.
fn format_u64<'a>(buf: &'a mut [u8], val: u64) -> &'a str {
    if val == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut n = val;
    let mut digits = [0u8; 20];
    let mut dcount = 0;
    while n > 0 {
        digits[dcount] = b'0' + (n % 10) as u8;
        n /= 10;
        dcount += 1;
    }
    let mut pos = 0;
    for i in (0..dcount).rev() {
        if pos < buf.len() {
            buf[pos] = digits[i];
            pos += 1;
        }
    }
    core::str::from_utf8(&buf[..pos]).unwrap_or("?")
}

fn cmd_env() {
    // Display common environment variables.
    let vars = ["HOME", "PATH", "PWD", "SHELL", "USER", "HOSTNAME"];
    for &key in &vars {
        if let Ok(Some(val)) = env::get(key) {
            let _ = console::write(key);
            let _ = console::write("=");
            let _ = console::writeln(&val);
        }
    }
}

fn cmd_export(args: &[u8]) {
    let Ok(s) = core::str::from_utf8(args) else {
        return;
    };
    let trimmed = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        // Show all exported variables.
        cmd_env();
        return;
    }

    // Parse KEY=VALUE
    match trimmed.find('=') {
        Some(pos) => {
            let key = trimmed[..pos].trim();
            let value = trimmed[pos + 1..].trim();
            if key.is_empty() {
                let _ = console::writeln("export: invalid syntax, use KEY=VALUE");
                return;
            }
            match env::set(key, value) {
                Ok(()) => {}
                Err(_) => {
                    let _ = console::writeln("export: failed to set variable");
                }
            }
        }
        None => {
            let _ = console::writeln("export: invalid syntax, use KEY=VALUE");
        }
    }
}

fn cmd_unset(args: &[u8]) {
    let Ok(key) = core::str::from_utf8(args) else {
        return;
    };
    let trimmed = key.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        let _ = console::writeln("unset: missing variable name");
        return;
    }

    // Setting to empty string effectively unsets.
    let _ = env::set(trimmed, "");
}

fn cmd_ps() {
    // Read from /proc filesystem if available.
    let _ = console::writeln("  PID  STATE  NAME");
    let _ = console::writeln("  ───  ─────  ────");

    // Try listing /proc/pid/ entries.
    match fs::open("/proc/pid") {
        Ok(fd) => {
            let mut buf = [0u8; 512];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                            for line in s.split('\n') {
                                if !line.is_empty() {
                                    let _ = console::write("  ");
                                    let _ = console::writeln(line);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            let _ = console::writeln("  (procfs not available)");
        }
    }
}

fn cmd_history(history: &History) {
    history.display();
}

/// Dispatch a command, handling redirects if present.
fn dispatch(line: &str, history: &mut History) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    // Add to history.
    history.push(trimmed.as_bytes());

    // Expand variables in the entire line.
    let expanded = expand_vars(trimmed);

    // Check for output redirection.
    let (cmd_part, redirect_file) = match find_redirect(&expanded) {
        Some(pos) => {
            let cmd = expanded[..pos].trim();
            let file = expanded[pos + 1..].trim();
            if file.is_empty() {
                (cmd, None)
            } else {
                (cmd, Some(file))
            }
        }
        None => (expanded.as_str(), None),
    };

    let (cmd, args) = split_cmd(cmd_part.as_bytes());

    match cmd {
        b"help" | b"?" => cmd_help(),
        b"exit" | b"quit" => {
            let _ = console::writeln("Goodbye!");
            process::exit(0);
        }
        b"echo" => {
            // For echo with redirect, handle specially.
            if redirect_file.is_some() {
                let args_str = core::str::from_utf8(args).unwrap_or("");
                let expanded_args = expand_vars(args_str.trim_matches(|c: char| c == '\0'));
                output_line(&expanded_args, redirect_file);
            } else {
                cmd_echo(args);
            }
        }
        b"ls" => cmd_ls(args),
        b"cat" => cmd_cat(args),
        b"run" => cmd_run(args),
        b"cd" => cmd_cd(args),
        b"pwd" => cmd_pwd(),
        b"mkdir" => cmd_mkdir(args),
        b"rmdir" => cmd_rmdir(args),
        b"rm" => cmd_rm(args),
        b"touch" => cmd_touch(args),
        b"cp" => cmd_cp(args),
        b"stat" => cmd_stat(args),
        b"env" => cmd_env(),
        b"export" => cmd_export(args),
        b"unset" => cmd_unset(args),
        b"ps" => cmd_ps(),
        b"history" => cmd_history(history),
        b"clear" => {
            let _ = console::write("\x1b[2J\x1b[H");
        }
        b"" => {}
        _ => {
            // Try to run as an external program.
            let cmd_str = core::str::from_utf8(cmd).unwrap_or("");
            let expanded_cmd = expand_vars(cmd_str);
            match process::create(&expanded_cmd) {
                Ok(task_id) => {
                    let _ = process::start(task_id, &expanded_cmd);
                    let _ = process::wait(task_id, 1000);
                }
                Err(_) => {
                    let _ = console::write("unknown command: ");
                    let _ = console::writeln(&expanded_cmd);
                }
            }
        }
    }
}

/// Initialize default environment variables.
fn init_env() {
    let _ = env::set("SHELL", "/ram/shell_rs");
    let _ = env::set("HOME", "/");
    let _ = env::set("PATH", "/ram:/disk");
    let _ = env::set("USER", "root");
    let _ = env::set("HOSTNAME", "openos");
    let _ = env::set("PWD", "/");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    init_env();

    let _ = console::writeln("OpenOS Shell v0.4 (Rust)");
    let _ = console::writeln("Type 'help' for available commands.");
    let _ = console::writeln("");

    let mut input_buf = [0u8; MAX_LINE];
    let mut history = History::new();

    loop {
        // Show prompt with current directory.
        match env::cwd() {
            Ok(cwd) => {
                let _ = console::write(&cwd);
                let _ = console::write(" $ ");
            }
            Err(_) => {
                let _ = console::write("openos> ");
            }
        }

        let len = read_line(&mut input_buf);
        if len == 0 {
            continue;
        }

        let Ok(line) = core::str::from_utf8(&input_buf[..len]) else {
            continue;
        };

        dispatch(line, &mut history);
    }
}
