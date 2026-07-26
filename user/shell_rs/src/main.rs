//! Interactive shell for OpenOS -- Rust implementation.
//!
//! Supports built-in commands: help, exit, echo, ls, cat, run, clear,
//! cd, pwd, mkdir, rmdir, env, export, unset, ps, rm, cp, touch, stat.
//! Disk filesystem access via /disk mount point.
//! Environment variable expansion with $VAR syntax.
//! Output redirection with > and 2> operators, including 2>&1.
//! Pipe operator (|) for chaining commands.
//! Ctrl-C handling via SIGKILL to child processes.
//! Command history with arrow key navigation (ANSI escape sequences).
//! Exit code display in the prompt.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, env, fs, process};

/// Simple bump allocator for user-space (128 KiB heap).
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 131072]>,
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
        if off + size > 131072 {
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
    heap: core::cell::UnsafeCell::new([0u8; 131072]),
    offset: core::cell::Cell::new(0),
};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in shell!");
    process::exit(1);
}

/// Maximum number of commands in history.
const HISTORY_SIZE: usize = 10;
/// Maximum input line length.
const MAX_LINE: usize = 256;

/// Command history stored as a ring buffer of strings.
struct History {
    entries: VecDeque<String>,
}

impl History {
    const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn push(&mut self, line: &str) {
        // Don't store duplicate of the most recent entry.
        if self
            .entries
            .front()
            .is_some_and(|last| last.as_str() == line)
        {
            return;
        }
        if self.entries.len() >= HISTORY_SIZE {
            self.entries.pop_back();
        }
        self.entries.push_front(String::from(line));
    }

    fn display(&self) {
        for (i, entry) in self.entries.iter().enumerate() {
            let _ = console::write("  ");
            let mut buf = [0u8; 16];
            let num = format_u32(&mut buf, i as u32 + 1);
            let _ = console::write(num);
            let _ = console::write("  ");
            let _ = console::writeln(entry);
        }
    }
}

/// Format a u32 into a buffer, return as str.
fn format_u32(buf: &mut [u8], val: u32) -> &str {
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

/// Read a line from the console into `buf`, supporting arrow keys for history.
/// Returns the number of bytes read.
fn read_line(buf: &mut [u8], history: &mut History) -> usize {
    let mut pos = 0;
    let mut history_idx: Option<usize> = None;
    // Buffer for the line being edited before history recall.
    let mut saved_line = [0u8; MAX_LINE];
    let mut saved_len = 0;

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
            0x03 => {
                // Ctrl-C: print ^C and clear the line.
                let _ = console::write("^C\n");
                pos = 0;
                break;
            }
            0x08 | 0x7F => {
                // Backspace
                if pos > 0 {
                    pos -= 1;
                    let _ = console::write("\x08 \x08");
                }
            }
            0x1B => {
                // ANSI escape sequence start: ESC [ ...
                // Read the next bytes to determine the key.
                let mut seq = [0u8; 2];
                let Ok(n1) = console::read(&mut seq[0..1], true) else {
                    continue;
                };
                if n1 == 0 {
                    continue;
                }
                if seq[0] == b'[' {
                    let Ok(n2) = console::read(&mut seq[1..2], true) else {
                        continue;
                    };
                    if n2 == 0 {
                        continue;
                    }
                    match seq[1] {
                        b'A' => {
                            // Up arrow: previous command.
                            if history.entries.is_empty() {
                                continue;
                            }
                            let new_idx = match history_idx {
                                Some(i) => {
                                    if i + 1 < history.entries.len() {
                                        i + 1
                                    } else {
                                        i
                                    }
                                }
                                None => 0,
                            };
                            // Save current line on first history recall.
                            if history_idx.is_none() {
                                saved_line[..pos].copy_from_slice(&buf[..pos]);
                                saved_len = pos;
                            }
                            history_idx = Some(new_idx);
                            // Clear current line from display.
                            for _ in 0..pos {
                                let _ = console::write("\x08 \x08");
                            }
                            // Load history entry.
                            let entry = &history.entries[new_idx];
                            let bytes = entry.as_bytes();
                            let copy_len = bytes.len().min(buf.len() - 1);
                            buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
                            pos = copy_len;
                            // Echo the line.
                            if let Ok(s) = core::str::from_utf8(&buf[..pos]) {
                                let _ = console::write(s);
                            }
                        }
                        b'B' => {
                            // Down arrow: next command.
                            if history.entries.is_empty() {
                                continue;
                            }
                            match history_idx {
                                Some(0) => {
                                    // Restore saved line.
                                    history_idx = None;
                                    for _ in 0..pos {
                                        let _ = console::write("\x08 \x08");
                                    }
                                    buf[..saved_len].copy_from_slice(&saved_line[..saved_len]);
                                    pos = saved_len;
                                    if let Ok(s) = core::str::from_utf8(&buf[..pos]) {
                                        let _ = console::write(s);
                                    }
                                }
                                Some(i) => {
                                    let new_idx = i - 1;
                                    history_idx = Some(new_idx);
                                    for _ in 0..pos {
                                        let _ = console::write("\x08 \x08");
                                    }
                                    let entry = &history.entries[new_idx];
                                    let bytes = entry.as_bytes();
                                    let copy_len = bytes.len().min(buf.len() - 1);
                                    buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
                                    pos = copy_len;
                                    if let Ok(s) = core::str::from_utf8(&buf[..pos]) {
                                        let _ = console::write(s);
                                    }
                                }
                                None => {}
                            }
                        }
                        _ => {
                            // Ignore other escape sequences (left, right, etc.).
                        }
                    }
                }
                // If it's not ESC [, ignore the escape.
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
        Some(i) => (&trimmed.as_bytes()[..i], &trimmed.as_bytes()[i + 1..]),
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
                if let Ok(Some(v)) = env::get(name) {
                    result.push_str(&v);
                }
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Parsed command with redirections stripped out.
struct ParsedCommand<'a> {
    cmd_part: &'a str,
    stdout_redirect: Option<&'a str>,
    stderr_redirect: Option<&'a str>,
    stderr_to_stdout: bool,
}

/// Parse redirections from a command string.
///
/// Supports: `> file`, `2> file`, `2>&1`.
/// Multiple redirections can appear in any order: `cmd > f1 2> f2`, `cmd 2>&1 > file`, etc.
fn parse_redirections(input: &str) -> ParsedCommand<'_> {
    let mut stdout_redirect: Option<&str> = None;
    let mut stderr_redirect: Option<&str> = None;
    let mut stderr_to_stdout = false;

    // Work backwards: extract redirections from the end of the string.
    let mut working = input.trim();

    loop {
        let trimmed = working.trim_end();
        if trimmed.is_empty() {
            working = trimmed;
            break;
        }

        // Try to match `2>&1` at the end.
        if let Some(before) = trimmed.strip_suffix("2>&1") {
            // Ensure it's preceded by whitespace or is at the start.
            if before.is_empty()
                || before.ends_with(|c: char| c.is_whitespace())
                || before.ends_with('|')
            {
                stderr_to_stdout = true;
                working = before.trim_end();
                continue;
            }
        }

        // Try to match `2> filename` at the end.
        if let Some(pos) = trimmed.rfind("2>") {
            let after = &trimmed[pos + 2..].trim();
            // Make sure this `2>` is a redirection and not part of a filename.
            // The part after `2>` should be a single token (no spaces except trailing).
            if !after.is_empty()
                && !after.contains(|c: char| c.is_whitespace())
                && (pos == 0 || trimmed[..pos].ends_with(|c: char| c.is_whitespace() || c == '|'))
            {
                stderr_redirect = Some(after);
                working = trimmed[..pos].trim_end();
                continue;
            }
        }

        // Try to match `> filename` at the end.
        if let Some(pos) = trimmed.rfind('>') {
            // Skip if this is `>>` (append) or `2>`.
            if pos > 0 && trimmed.as_bytes().get(pos - 1) == Some(&b'2') {
                break;
            }
            if pos + 1 < trimmed.len() && trimmed.as_bytes().get(pos + 1) == Some(&b'>') {
                break; // append not supported
            }
            let after = trimmed[pos + 1..].trim();
            if !after.is_empty()
                && !after.contains(|c: char| c.is_whitespace())
                && (pos == 0 || trimmed[..pos].ends_with(|c: char| c.is_whitespace() || c == '|'))
            {
                stdout_redirect = Some(after);
                working = trimmed[..pos].trim_end();
                continue;
            }
        }

        break;
    }

    ParsedCommand {
        cmd_part: working,
        stdout_redirect,
        stderr_redirect,
        stderr_to_stdout,
    }
}

/// Write text to a file (creating it), used for output redirection.
fn write_to_file(filename: &str, text: &str) {
    match fs::create(filename) {
        Ok(fd) => {
            let _ = fs::write(fd, text.as_bytes());
            let _ = fs::close(fd);
        }
        Err(_) => {
            let _ = console::write("shell: cannot create: ");
            let _ = console::writeln(filename);
        }
    }
}

fn cmd_help() {
    let _ = console::writeln("OpenOS Shell v0.5 -- Available commands:");
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
    let _ = console::writeln("  Redirection and pipes:");
    let _ = console::writeln("    cmd > file         Redirect stdout to file");
    let _ = console::writeln("    cmd 2> file        Redirect stderr to file");
    let _ = console::writeln("    cmd 2>&1           Redirect stderr to stdout");
    let _ = console::writeln("    cmd1 | cmd2        Pipe stdout of cmd1 to cmd2");
    let _ = console::writeln("");
    let _ = console::writeln("  Other:");
    let _ = console::writeln("    history            Show command history");
    let _ = console::writeln("    clear              Clear screen");
    let _ = console::writeln("    help               Show this help");
    let _ = console::writeln("    exit               Exit shell");
    let _ = console::writeln("");
    let _ = console::writeln("  Ctrl-C kills the current child process.");
    let _ = console::writeln("  Up/Down arrows navigate command history.");
}

fn cmd_ls(args: &str) {
    let trimmed = args.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let path = if trimmed.is_empty() { "." } else { trimmed };
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

fn cmd_cat(args: &str) {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
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

fn cmd_run(args: &str) -> u64 {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("run: missing ELF filename");
        return 1;
    }

    match process::create(&expanded) {
        Ok(task_id) => {
            let _ = console::write("Starting ");
            let _ = console::writeln(&expanded);
            if process::start(task_id, &expanded).is_err() {
                let _ = console::writeln("run: failed to start");
                return 1;
            }
            process::wait(task_id, 5000).unwrap_or(1)
        }
        Err(_) => {
            let _ = console::writeln("run: failed to create process");
            1
        }
    }
}

fn cmd_cd(args: &str) -> u64 {
    let trimmed = args.trim_matches(|c: char| c == '\0' || c.is_whitespace());

    let target = if trimmed.is_empty() { "/" } else { trimmed };

    let expanded = expand_vars(target);

    match env::chdir(&expanded) {
        Ok(()) => {
            // Update PWD environment variable.
            if let Ok(cwd) = env::cwd() {
                let _ = env::set("PWD", &cwd);
            }
            0
        }
        Err(_) => {
            let _ = console::write("cd: no such directory: ");
            let _ = console::writeln(&expanded);
            1
        }
    }
}

fn cmd_pwd() -> u64 {
    match env::cwd() {
        Ok(cwd) => {
            let _ = console::writeln(&cwd);
            0
        }
        Err(_) => {
            let _ = console::writeln("/");
            0
        }
    }
}

fn cmd_mkdir(args: &str) -> u64 {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("mkdir: missing directory name");
        return 1;
    }

    match fs::mkdir(&expanded) {
        Ok(()) => 0,
        Err(_) => {
            let _ = console::write("mkdir: cannot create directory '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
            1
        }
    }
}

fn cmd_rmdir(args: &str) -> u64 {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("rmdir: missing directory name");
        return 1;
    }

    match fs::rmdir(&expanded) {
        Ok(()) => 0,
        Err(_) => {
            let _ = console::write("rmdir: failed to remove '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
            1
        }
    }
}

fn cmd_rm(args: &str) -> u64 {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("rm: missing filename");
        return 1;
    }

    match fs::unlink(&expanded) {
        Ok(()) => 0,
        Err(_) => {
            let _ = console::write("rm: cannot remove '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
            1
        }
    }
}

fn cmd_touch(args: &str) -> u64 {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("touch: missing filename");
        return 1;
    }

    match fs::create(&expanded) {
        Ok(fd) => {
            let _ = fs::close(fd);
            0
        }
        Err(_) => {
            let _ = console::write("touch: cannot create '");
            let _ = console::write(&expanded);
            let _ = console::writeln("'");
            1
        }
    }
}

fn cmd_cp(args: &str) -> u64 {
    let trimmed = args.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let (src, dst) = match trimmed.find(|c: char| c.is_whitespace()) {
        Some(i) => (trimmed[..i].trim(), trimmed[i..].trim()),
        None => {
            let _ = console::writeln("cp: usage: cp <source> <dest>");
            return 1;
        }
    };

    if src.is_empty() || dst.is_empty() {
        let _ = console::writeln("cp: usage: cp <source> <dest>");
        return 1;
    }

    let src_exp = expand_vars(src);
    let dst_exp = expand_vars(dst);

    let src_fd = match fs::open(&src_exp) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = console::write("cp: cannot open '");
            let _ = console::write(&src_exp);
            let _ = console::writeln("'");
            return 1;
        }
    };

    let dst_fd = match fs::create(&dst_exp) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = fs::close(src_fd);
            let _ = console::write("cp: cannot create '");
            let _ = console::write(&dst_exp);
            let _ = console::writeln("'");
            return 1;
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
    0
}

fn cmd_stat(args: &str) -> u64 {
    let expanded = expand_vars(args.trim_matches(|c: char| c == '\0' || c.is_whitespace()));
    if expanded.is_empty() {
        let _ = console::writeln("stat: missing filename");
        return 1;
    }

    match fs::file_size(&expanded) {
        Ok(size) => {
            let _ = console::write("  File: ");
            let _ = console::writeln(&expanded);
            let _ = console::write("  Size: ");
            let mut buf = [0u8; 32];
            let s = format_u64(&mut buf, size);
            let _ = console::write(s);
            let _ = console::writeln(" bytes");
            0
        }
        Err(_) => {
            let _ = console::write("stat: cannot stat '");
            let _ = console::write(&expanded);
            let _ = console::writeln("': No such file or directory");
            1
        }
    }
}

/// Format a u64 into a buffer, return as str.
fn format_u64(buf: &mut [u8], val: u64) -> &str {
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

fn cmd_env() -> u64 {
    let vars = ["HOME", "PATH", "PWD", "SHELL", "USER", "HOSTNAME"];
    for &key in &vars {
        if let Ok(Some(val)) = env::get(key) {
            let _ = console::write(key);
            let _ = console::write("=");
            let _ = console::writeln(&val);
        }
    }
    0
}

fn cmd_export(args: &str) -> u64 {
    let trimmed = args.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        return cmd_env();
    }

    // Parse KEY=VALUE
    match trimmed.find('=') {
        Some(pos) => {
            let key = trimmed[..pos].trim();
            let value = trimmed[pos + 1..].trim();
            if key.is_empty() {
                let _ = console::writeln("export: invalid syntax, use KEY=VALUE");
                return 1;
            }
            match env::set(key, value) {
                Ok(()) => 0,
                Err(_) => {
                    let _ = console::writeln("export: failed to set variable");
                    1
                }
            }
        }
        None => {
            let _ = console::writeln("export: invalid syntax, use KEY=VALUE");
            1
        }
    }
}

fn cmd_unset(args: &str) -> u64 {
    let trimmed = args.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        let _ = console::writeln("unset: missing variable name");
        return 1;
    }

    // Setting to empty string effectively unsets.
    let _ = env::set(trimmed, "");
    0
}

fn cmd_ps() -> u64 {
    let _ = console::writeln("  PID  STATE  NAME");
    let _ = console::writeln("  ---  -----  ----");

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
            0
        }
        Err(_) => {
            let _ = console::writeln("  (procfs not available)");
            0
        }
    }
}

fn cmd_history(history: &History) -> u64 {
    history.display();
    0
}

/// Spawn a single command (no pipes). Handles redirections.
/// Returns the exit code of the command.
fn run_single_command(cmd_str: &str, is_background: bool) -> u64 {
    let parsed = parse_redirections(cmd_str);
    let cmd_part = parsed.cmd_part.trim();
    if cmd_part.is_empty() {
        return 0;
    }

    // Expand variables in the command part.
    let expanded = expand_vars(cmd_part);
    let (cmd, args) = split_cmd(expanded.as_bytes());

    // Check if this is a builtin that should capture output for redirection.
    let has_redirect = parsed.stdout_redirect.is_some()
        || parsed.stderr_redirect.is_some()
        || parsed.stderr_to_stdout;

    match cmd {
        b"help" | b"?" => {
            cmd_help();
            0
        }
        b"exit" | b"quit" => {
            let _ = console::writeln("Goodbye!");
            process::exit(0);
        }
        b"echo" => {
            let args_str = core::str::from_utf8(args).unwrap_or("");
            let expanded_args = expand_vars(args_str.trim_matches(|c: char| c == '\0'));
            if has_redirect {
                if let Some(file) = parsed.stdout_redirect {
                    write_to_file(file, &expanded_args);
                } else {
                    let _ = console::writeln(&expanded_args);
                }
                // For echo, stderr is typically empty; create the file if redirected.
                if !parsed.stderr_to_stdout {
                    if let Some(file) = parsed.stderr_redirect {
                        let _ = fs::create(file);
                    }
                }
            } else {
                let _ = console::writeln(&expanded_args);
            }
            0
        }
        b"ls" => {
            // For simplicity, builtins print to console regardless of redirect.
            // A full implementation would capture output via pipe for builtins.
            cmd_ls(core::str::from_utf8(args).unwrap_or(""));
            0
        }
        b"cat" => {
            cmd_cat(core::str::from_utf8(args).unwrap_or(""));
            0
        }
        b"run" => cmd_run(core::str::from_utf8(args).unwrap_or("")),
        b"cd" => cmd_cd(core::str::from_utf8(args).unwrap_or("")),
        b"pwd" => cmd_pwd(),
        b"mkdir" => cmd_mkdir(core::str::from_utf8(args).unwrap_or("")),
        b"rmdir" => cmd_rmdir(core::str::from_utf8(args).unwrap_or("")),
        b"rm" => cmd_rm(core::str::from_utf8(args).unwrap_or("")),
        b"touch" => cmd_touch(core::str::from_utf8(args).unwrap_or("")),
        b"cp" => cmd_cp(core::str::from_utf8(args).unwrap_or("")),
        b"stat" => cmd_stat(core::str::from_utf8(args).unwrap_or("")),
        b"env" => cmd_env(),
        b"export" => cmd_export(core::str::from_utf8(args).unwrap_or("")),
        b"unset" => cmd_unset(core::str::from_utf8(args).unwrap_or("")),
        b"ps" => cmd_ps(),
        b"history" => cmd_history(&History::new()),
        b"clear" => {
            let _ = console::write("\x1b[2J\x1b[H");
            0
        }
        b"" => 0,
        _ => {
            // Run as an external program with optional redirections.
            let cmd_name = core::str::from_utf8(cmd).unwrap_or("");
            let expanded_cmd = expand_vars(cmd_name);

            match process::create(&expanded_cmd) {
                Ok(task_id) => {
                    // Set up stdout redirection via dup2 if requested.
                    if let Some(file) = parsed.stdout_redirect {
                        if let Ok(file_fd) = fs::create(file) {
                            let _ = fs::dup2(file_fd, 1); // stdout = file
                            let _ = fs::close(file_fd);
                        }
                    }
                    // Set up stderr redirection.
                    if parsed.stderr_to_stdout {
                        // stderr -> stdout: dup2(1, 2)
                        let _ = fs::dup2(1, 2);
                    } else if let Some(file) = parsed.stderr_redirect {
                        if let Ok(file_fd) = fs::create(file) {
                            let _ = fs::dup2(file_fd, 2); // stderr = file
                            let _ = fs::close(file_fd);
                        }
                    }

                    let _ = process::start(task_id, &expanded_cmd);

                    if is_background {
                        let _ = console::write("[bg] pid=");
                        let mut buf = [0u8; 16];
                        let s = format_u32(&mut buf, task_id as u32);
                        let _ = console::writeln(s);
                        0
                    } else {
                        process::wait(task_id, 5000).unwrap_or(1)
                    }
                }
                Err(_) => {
                    let _ = console::write("unknown command: ");
                    let _ = console::writeln(&expanded_cmd);
                    127
                }
            }
        }
    }
}

/// Dispatch a command line. Handles pipes (`|`).
/// Returns the exit code of the last command in the pipeline.
fn dispatch(line: &str, history: &mut History) -> u64 {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return 0;
    }

    // Add to history.
    history.push(trimmed);

    // Split on pipe operator.
    let pipe_segments: alloc::vec::Vec<&str> = trimmed.split('|').collect();

    if pipe_segments.len() == 1 {
        // No pipe -- run as a single command.
        return run_single_command(pipe_segments[0], false);
    }

    // Pipeline: connect commands with pipes.
    //
    // For a pipeline `cmd1 | cmd2 | cmd3`:
    //   - cmd1's stdout -> pipe0 write end
    //   - cmd2's stdin <- pipe0 read end, cmd2's stdout -> pipe1 write end
    //   - cmd3's stdin <- pipe1 read end
    //
    // Since OpenOS doesn't have fork(), we simulate pipelines by:
    //   1. Running cmd1 and capturing its stdout into a pipe buffer
    //   2. Running cmd2 with stdin from the pipe buffer
    //   3. etc.
    //
    // However, the kernel's pipe syscall creates a pipe that can be used
    // with dup2 to redirect stdin/stdout of child processes. We create
    // the pipe, fork the first command with stdout redirected to the pipe
    // write end, then fork the second command with stdin redirected from
    // the pipe read end.

    let num_cmds = pipe_segments.len();
    let mut last_exit_code: u64 = 0;

    // Create pipes between consecutive commands.
    // pipe_fds[i] = (read_fd, write_fd) for pipe between cmd i and cmd i+1.
    let mut pipe_fds: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    for _ in 0..num_cmds - 1 {
        match fs::pipe() {
            Ok((r, w)) => pipe_fds.push((r, w)),
            Err(_) => {
                let _ = console::writeln("pipe: failed to create pipe");
                return 1;
            }
        }
    }

    // Launch each command in the pipeline.
    for i in 0..num_cmds {
        let segment = pipe_segments[i].trim();
        if segment.is_empty() {
            continue;
        }

        let parsed = parse_redirections(segment);
        let cmd_part = parsed.cmd_part.trim();
        if cmd_part.is_empty() {
            continue;
        }

        let expanded = expand_vars(cmd_part);
        let (cmd, args) = split_cmd(expanded.as_bytes());
        let cmd_name = core::str::from_utf8(cmd).unwrap_or("");
        let expanded_cmd = expand_vars(cmd_name);
        let args_str = core::str::from_utf8(args).unwrap_or("");

        // For builtins in a pipeline, we just run them directly.
        // In a real shell, builtins would also get piped I/O, but
        // for simplicity we run them and print to the console.
        match cmd {
            b"echo" => {
                let expanded_args = expand_vars(args_str.trim_matches(|c: char| c == '\0'));
                if i < num_cmds - 1 {
                    // Not the last command: write to pipe write end.
                    let write_fd = pipe_fds[i].1;
                    let _ = fs::write(write_fd, expanded_args.as_bytes());
                    let _ = fs::write(write_fd, b"\n");
                } else {
                    let _ = console::writeln(&expanded_args);
                }
                last_exit_code = 0;
            }
            b"" => {}
            _ => {
                // External command.
                match process::create(&expanded_cmd) {
                    Ok(task_id) => {
                        // Redirect stdin from previous pipe read end.
                        if i > 0 {
                            let read_fd = pipe_fds[i - 1].0;
                            let _ = fs::dup2(read_fd, 0); // stdin = pipe read
                        }
                        // Redirect stdout to next pipe write end.
                        if i < num_cmds - 1 {
                            let write_fd = pipe_fds[i].1;
                            let _ = fs::dup2(write_fd, 1); // stdout = pipe write
                        }
                        // Handle explicit redirections in this segment.
                        if let Some(file) = parsed.stdout_redirect {
                            if let Ok(file_fd) = fs::create(file) {
                                let _ = fs::dup2(file_fd, 1);
                                let _ = fs::close(file_fd);
                            }
                        }
                        if parsed.stderr_to_stdout {
                            let _ = fs::dup2(1, 2);
                        } else if let Some(file) = parsed.stderr_redirect {
                            if let Ok(file_fd) = fs::create(file) {
                                let _ = fs::dup2(file_fd, 2);
                                let _ = fs::close(file_fd);
                            }
                        }

                        let _ = process::start(task_id, &expanded_cmd);

                        // Wait for this command to finish before launching the next,
                        // so the pipe data is flushed.
                        last_exit_code = process::wait(task_id, 5000).unwrap_or(1);
                    }
                    Err(_) => {
                        let _ = console::write("unknown command: ");
                        let _ = console::writeln(&expanded_cmd);
                        last_exit_code = 127;
                    }
                }
            }
        }
    }

    // Close all pipe file descriptors.
    for (r, w) in &pipe_fds {
        let _ = fs::close(*r);
        let _ = fs::close(*w);
    }

    last_exit_code
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

    let _ = console::writeln("OpenOS Shell v0.5 (Rust)");
    let _ = console::writeln("Type 'help' for available commands.");
    let _ = console::writeln("");

    let mut input_buf = [0u8; MAX_LINE];
    let mut history = History::new();
    let mut last_exit_code: u64 = 0;

    loop {
        // Show prompt with exit code and current directory.
        // Format: [exit_code] cwd $
        let _ = console::write("[");
        let mut code_buf = [0u8; 16];
        let code_str = format_u64(&mut code_buf, last_exit_code);
        let _ = console::write(code_str);
        let _ = console::write("] ");

        match env::cwd() {
            Ok(cwd) => {
                let _ = console::write(&cwd);
                let _ = console::write(" $ ");
            }
            Err(_) => {
                let _ = console::write("openos> ");
            }
        }

        let len = read_line(&mut input_buf, &mut history);
        if len == 0 {
            continue;
        }

        let Ok(line) = core::str::from_utf8(&input_buf[..len]) else {
            continue;
        };

        last_exit_code = dispatch(line, &mut history);
    }
}
