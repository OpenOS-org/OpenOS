//! tar -- archive utility for OpenOS
//!
//! Supports three modes:
//!   -c  Create an archive from listed files
//!   -t  List contents of an archive
//!   -x  Extract files from an archive
//!
//! Usage:
//!   tar -c archive.tar file1 file2 ...
//!   tar -t archive.tar
//!   tar -x archive.tar
//!
//! Uses a simplified POSIX tar format with 512-byte headers.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::panic::PanicInfo;

use openos_sdk::{console, fs, process};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Size of a single tar header block.
const BLOCK_SIZE: usize = 512;

/// Maximum file name length in the header.
const NAME_MAX: usize = 100;

/// USTAR magic bytes at offset 257.
const USTAR_MAGIC: &[u8; 6] = b"ustar";

/// Regular file type flag.
const TYPE_REGULAR: u8 = b'0';

/// End-of-archive marker: two consecutive zero blocks.
const ZERO_BLOCK: [u8; BLOCK_SIZE] = [0u8; BLOCK_SIZE];

/// Read buffer size for copying file data.
const COPY_BUF_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in tar!");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write a message to stderr (console).
fn stderr(msg: &str) {
    let _ = console::write(msg);
}

/// Write a message followed by a newline to stderr.
fn stderrln(msg: &str) {
    let _ = console::writeln(msg);
}

/// Write a message to stdout (console).
fn stdout(msg: &str) {
    let _ = console::write(msg);
}

/// Write a message followed by a newline to stdout.
fn stdoutln(msg: &str) {
    let _ = console::writeln(msg);
}

/// Exit with an error message and non-zero status.
fn fatal(msg: &str) -> ! {
    stderr("tar: ");
    stderrln(msg);
    process::exit(1);
}

/// Format a u64 as a decimal string into a provided buffer, return the slice.
fn format_u64(val: u64, buf: &mut [u8; 20]) -> &str {
    if val == 0 {
        return "0";
    }
    let mut tmp = val;
    let mut pos = 19;
    while tmp > 0 {
        buf[pos] = b'0' + (tmp % 10) as u8;
        tmp /= 10;
        if pos == 0 {
            break;
        }
        pos -= 1;
    }
    // SAFETY: buffer contains only ASCII digits.
    unsafe { core::str::from_utf8_unchecked(&buf[pos..20]) }
}

/// Write exactly `len` zero bytes to a file descriptor.
fn write_zeros(fd: u64, len: usize) -> Result<(), openos_sdk::Error> {
    let zeros = [0u8; BLOCK_SIZE];
    let mut remaining = len;
    while remaining > 0 {
        let chunk = remaining.min(BLOCK_SIZE);
        fs::write(fd, &zeros[..chunk])?;
        remaining -= chunk;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tar header (512 bytes)
// ---------------------------------------------------------------------------

/// A simplified tar header.
///
/// Layout:
///   0..100    file name (NUL-terminated)
///   100..108  file mode (octal ASCII)
///   108..116  owner id (octal ASCII)
///   116..124  group id (octal ASCII)
///   124..136  file size (octal ASCII, 11 chars + NUL)
///   136..148  modification time (octal ASCII, 11 chars + NUL)
///   148..156  checksum (8 bytes: 6 octal digits + NUL + space)
///   156       type flag ('0' = regular file)
///   157..257  link name (unused, zeroed)
///   257..263  magic "ustar"
///   263..265  version "00"
///   265..500  remaining fields (zeroed)
///   500..512  padding (zeroed)
struct TarHeader {
    name: [u8; NAME_MAX],
    mode: [u8; 8],
    uid: [u8; 8],
    gid: [u8; 8],
    size: [u8; 12],
    mtime: [u8; 12],
    checksum: [u8; 8],
    typeflag: u8,
    linkname: [u8; 100],
    magic: [u8; 6],
    version: [u8; 2],
}

impl TarHeader {
    /// Create a new header for a regular file.
    fn new(name: &str, size: u64) -> Self {
        let mut hdr = Self {
            name: [0u8; NAME_MAX],
            mode: [0u8; 8],
            uid: [0u8; 8],
            gid: [0u8; 8],
            size: [0u8; 12],
            mtime: [0u8; 12],
            checksum: [0u8; 8],
            typeflag: TYPE_REGULAR,
            linkname: [0u8; 100],
            magic: [0u8; 5],
            version: *b"00",
        };

        // Copy file name, truncated to NAME_MAX - 1 (leave room for NUL).
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(NAME_MAX - 1);
        hdr.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

        // File mode: 0644 (octal).
        hdr.write_octal(&mut hdr.mode, 0o644);

        // Owner/group: 0.
        hdr.write_octal(&mut hdr.uid, 0);
        hdr.write_octal(&mut hdr.gid, 0);

        // File size.
        hdr.write_octal(&mut hdr.size, size);

        // Modification time: 0 (not tracked).
        hdr.write_octal(&mut hdr.mtime, 0);

        // USTAR magic.
        hdr.magic.copy_from_slice(USTAR_MAGIC);

        // Compute checksum: sum of all header bytes with checksum field
        // treated as spaces (0x20).
        hdr.checksum = [0u8; 8];
        let sum = hdr.compute_checksum();
        // Write checksum as 6 octal digits + NUL + space.
        hdr.write_checksum(sum);

        hdr
    }

    /// Parse a header from a raw 512-byte block. Returns None if the block
    /// is all zeros (end-of-archive marker).
    fn from_block(block: &[u8; BLOCK_SIZE]) -> Option<Self> {
        // Check for end-of-archive (all zeros).
        if block.iter().all(|&b| b == 0) {
            return None;
        }

        let mut hdr = Self {
            name: [0u8; NAME_MAX],
            mode: [0u8; 8],
            uid: [0u8; 8],
            gid: [0u8; 8],
            size: [0u8; 12],
            mtime: [0u8; 12],
            checksum: [0u8; 8],
            typeflag: block[156],
            linkname: [0u8; 100],
            magic: [0u8; 5],
            version: [0u8; 2],
        };

        hdr.name.copy_from_slice(&block[0..NAME_MAX]);
        hdr.mode.copy_from_slice(&block[100..108]);
        hdr.uid.copy_from_slice(&block[108..116]);
        hdr.gid.copy_from_slice(&block[116..124]);
        hdr.size.copy_from_slice(&block[124..136]);
        hdr.mtime.copy_from_slice(&block[136..148]);
        hdr.checksum.copy_from_slice(&block[148..156]);
        hdr.linkname.copy_from_slice(&block[157..257]);
        hdr.magic.copy_from_slice(&block[257..263]);
        hdr.version.copy_from_slice(&block[263..265]);

        Some(hdr)
    }

    /// Serialize this header into a 512-byte block.
    fn to_block(&self) -> [u8; BLOCK_SIZE] {
        let mut block = [0u8; BLOCK_SIZE];
        block[0..NAME_MAX].copy_from_slice(&self.name);
        block[100..108].copy_from_slice(&self.mode);
        block[108..116].copy_from_slice(&self.uid);
        block[116..124].copy_from_slice(&self.gid);
        block[124..136].copy_from_slice(&self.size);
        block[136..148].copy_from_slice(&self.mtime);
        block[148..156].copy_from_slice(&self.checksum);
        block[156] = self.typeflag;
        block[157..257].copy_from_slice(&self.linkname);
        block[257..263].copy_from_slice(&self.magic);
        block[263..265].copy_from_slice(&self.version);
        block
    }

    /// Get the file name as a string slice (trimmed of NUL bytes).
    fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
        // SAFETY: file names from the archive are treated as byte strings;
        /// lossy conversion is acceptable for display.
        unsafe {
            core::str::from_utf8_unchecked(&self.name[..end])
        }
    }

    /// Parse the size field as an octal number.
    fn size_value(&self) -> u64 {
        Self::parse_octal(&self.size)
    }

    /// Write a u64 value as an octal ASCII string into a byte slice.
    /// The slice is NUL-padded from the left.
    fn write_octal(&self, field: &mut [u8], val: u64) {
        // Zero-fill first.
        for b in field.iter_mut() {
            *b = 0;
        }
        if val == 0 {
            if !field.is_empty() {
                field[field.len() - 2] = b'0';
            }
            return;
        }
        let mut tmp = val;
        let mut pos = field.len();
        while tmp > 0 && pos > 0 {
            pos -= 1;
            field[pos] = b'0' + (tmp & 7) as u8;
            tmp >>= 3;
        }
    }

    /// Parse an octal ASCII field into a u64.
    fn parse_octal(field: &[u8]) -> u64 {
        let mut val: u64 = 0;
        for &b in field {
            if b == 0 || b == b' ' {
                if val > 0 {
                    break;
                }
                continue;
            }
            if b >= b'0' && b <= b'7' {
                val = val * 8 + (b - b'0') as u64;
            } else {
                break;
            }
        }
        val
    }

    /// Compute the checksum of the header block (checksum field treated as
    /// spaces).
    fn compute_checksum(&self) -> u64 {
        let block = self.to_block();
        let mut sum: u64 = 0;
        for (i, &b) in block.iter().enumerate() {
            if i >= 148 && i < 156 {
                // Checksum field: treat as space.
                sum += 0x20;
            } else {
                sum += b as u64;
            }
        }
        sum
    }

    /// Write the checksum value into the checksum field (6 octal digits + NUL + space).
    fn write_checksum(&mut self, val: u64) {
        let field = &mut self.checksum;
        for b in field.iter_mut() {
            *b = 0;
        }
        // Write 6 octal digits right-justified.
        let mut tmp = val;
        for i in (0..6).rev() {
            field[i] = b'0' + (tmp & 7) as u8;
            tmp >>= 3;
        }
        field[6] = 0; // NUL terminator.
        field[7] = b' '; // Traditional trailing space.
    }
}

// ---------------------------------------------------------------------------
// Create archive (-c)
// ---------------------------------------------------------------------------

fn cmd_create(archive: &str, files: &[&str]) {
    if files.is_empty() {
        fatal("no files specified");
    }

    let dst_fd = match fs::create(archive) {
        Ok(fd) => fd,
        Err(_) => fatal("cannot create archive"),
    };

    let mut total_files = 0u64;
    let mut total_bytes = 0u64;

    for &path in files {
        // Determine file size by reading the entire file into memory.
        // (OpenOS SDK does not provide stat-based size for arbitrary paths,
        // so we read to determine size, then write from the buffer.)
        let src_fd = match fs::open(path) {
            Ok(fd) => fd,
            Err(_) => {
                stderr("tar: ");
                stderr(path);
                stderrln(": cannot open, skipping");
                continue;
            }
        };

        // Read file contents into a buffer.
        let mut file_data = Vec::new();
        let mut read_buf = [0u8; COPY_BUF_SIZE];
        loop {
            match fs::read(src_fd, &mut read_buf) {
                Ok(0) => break,
                Ok(n) => {
                    file_data.extend_from_slice(&read_buf[..n]);
                }
                Err(_) => {
                    stderr("tar: ");
                    stderr(path);
                    stderrln(": read error, skipping");
                    let _ = fs::close(src_fd);
                    continue;
                }
            }
        }
        let _ = fs::close(src_fd);

        let size = file_data.len() as u64;

        // Build and write header.
        let header = TarHeader::new(path, size);
        let block = header.to_block();
        if fs::write(dst_fd, &block).is_err() {
            fatal("write error");
        }

        // Write file data.
        let mut offset = 0usize;
        while offset < file_data.len() {
            let chunk = (file_data.len() - offset).min(COPY_BUF_SIZE);
            if fs::write(dst_fd, &file_data[offset..offset + chunk]).is_err() {
                fatal("write error");
            }
            offset += chunk;
        }

        // Pad to 512-byte boundary.
        let remainder = size as usize % BLOCK_SIZE;
        if remainder != 0 {
            let pad = BLOCK_SIZE - remainder;
            if write_zeros(dst_fd, pad).is_err() {
                fatal("write error");
            }
        }

        total_files += 1;
        total_bytes += size;

        // Print each added file.
        stdout("a ");
        stdoutln(path);
    }

    // Write two zero blocks (end-of-archive marker).
    let _ = write_zeros(dst_fd, BLOCK_SIZE);
    let _ = write_zeros(dst_fd, BLOCK_SIZE);

    let _ = fs::close(dst_fd);

    // Summary.
    let mut buf = [0u8; 20];
    stdout(format_u64(total_files, &mut buf));
    stdout(" files, ");
    stdout(format_u64(total_bytes, &mut buf));
    stdoutln(" bytes");
}

// ---------------------------------------------------------------------------
// List archive (-t)
// ---------------------------------------------------------------------------

fn cmd_list(archive: &str) {
    let src_fd = match fs::open(archive) {
        Ok(fd) => fd,
        Err(_) => fatal("cannot open archive"),
    };

    let mut block = [0u8; BLOCK_SIZE];
    let mut count = 0u64;

    loop {
        // Read one 512-byte header block.
        match read_exact(src_fd, &mut block) {
            Ok(true) => {}
            Ok(false) => break, // EOF
            Err(_) => fatal("read error"),
        }

        // Parse header. None means end-of-archive.
        let header = match TarHeader::from_block(&block) {
            Some(h) => h,
            None => break,
        };

        let size = header.size_value();

        // Print entry: name (size bytes).
        stdout(header.name_str());
        stdout(" (");
        let mut buf = [0u8; 20];
        stdout(format_u64(size, &mut buf));
        stdoutln(" bytes)");

        count += 1;

        // Skip file data (padded to 512-byte boundary).
        let data_blocks = (size as usize + BLOCK_SIZE - 1) / BLOCK_SIZE;
        skip_bytes(src_fd, data_blocks * BLOCK_SIZE);
    }

    let _ = fs::close(src_fd);

    let mut buf = [0u8; 20];
    stdout(format_u64(count, &mut buf));
    stdoutln(" files");
}

// ---------------------------------------------------------------------------
// Extract archive (-x)
// ---------------------------------------------------------------------------

fn cmd_extract(archive: &str) {
    let src_fd = match fs::open(archive) {
        Ok(fd) => fd,
        Err(_) => fatal("cannot open archive"),
    };

    let mut block = [0u8; BLOCK_SIZE];
    let mut count = 0u64;

    loop {
        // Read header block.
        match read_exact(src_fd, &mut block) {
            Ok(true) => {}
            Ok(false) => break,
            Err(_) => fatal("read error"),
        }

        let header = match TarHeader::from_block(&block) {
            Some(h) => h,
            None => break,
        };

        let name = header.name_str();
        let size = header.size_value() as usize;

        // Only extract regular files.
        if header.typeflag == TYPE_REGULAR {
            // Create destination file.
            let dst_fd = match fs::create(name) {
                Ok(fd) => fd,
                Err(_) => {
                    stderr("tar: ");
                    stderr(name);
                    stderrln(": cannot create, skipping");
                    // Still need to skip the data.
                    let data_blocks = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;
                    skip_bytes(src_fd, data_blocks * BLOCK_SIZE);
                    continue;
                }
            };

            // Copy file data.
            let mut remaining = size;
            let mut read_buf = [0u8; COPY_BUF_SIZE];
            while remaining > 0 {
                let chunk = remaining.min(COPY_BUF_SIZE);
                match fs::read(src_fd, &mut read_buf[..chunk]) {
                    Ok(0) => break,
                    Ok(n) => {
                        if fs::write(dst_fd, &read_buf[..n]).is_err() {
                            stderr("tar: ");
                            stderr(name);
                            stderrln(": write error");
                            break;
                        }
                        remaining -= n;
                    }
                    Err(_) => {
                        stderr("tar: ");
                        stderr(name);
                        stderrln(": read error");
                        break;
                    }
                }
            }

            let _ = fs::close(dst_fd);

            // Skip padding.
            let remainder = size % BLOCK_SIZE;
            if remainder != 0 {
                skip_bytes(src_fd, BLOCK_SIZE - remainder);
            }

            count += 1;
            stdout("x ");
            stdoutln(name);
        } else {
            // Skip non-regular file data.
            let data_blocks = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;
            skip_bytes(src_fd, data_blocks * BLOCK_SIZE);
        }
    }

    let _ = fs::close(src_fd);

    let mut buf = [0u8; 20];
    stdout(format_u64(count, &mut buf));
    stdoutln(" files extracted");
}

// ---------------------------------------------------------------------------
// I/O helpers
// ---------------------------------------------------------------------------

/// Read exactly `BLOCK_SIZE` bytes from fd into buf. Returns Ok(true) on
/// success, Ok(false) on EOF (0 bytes read), Err on short read or error.
fn read_exact(fd: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<bool, openos_sdk::Error> {
    let mut offset = 0;
    while offset < BLOCK_SIZE {
        match fs::read(fd, &mut buf[offset..]) {
            Ok(0) => {
                if offset == 0 {
                    return Ok(false);
                }
                // Short read mid-block: treat as error.
                return Err(openos_sdk::Error::Unknown(-1));
            }
            Ok(n) => offset += n,
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Skip `count` bytes by reading and discarding.
fn skip_bytes(fd: u64, count: usize) {
    let mut remaining = count;
    let mut buf = [0u8; COPY_BUF_SIZE];
    while remaining > 0 {
        let chunk = remaining.min(COPY_BUF_SIZE);
        match fs::read(fd, &mut buf[..chunk]) {
            Ok(0) => break,
            Ok(n) => remaining -= n,
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

/// Parse arguments from the `__ARGS__` environment variable.
fn parse_args() -> (u8, String, Vec<String>) {
    let raw = openos_sdk::env::get("__ARGS__").ok().flatten();
    let data = match raw {
        Some(s) => s,
        None => fatal("usage: tar -c|-t|-x archive.tar [files...]"),
    };

    let bytes = data.as_bytes();
    let mut tokens: Vec<String> = Vec::new();
    let mut pos = 0;

    while pos < bytes.len() {
        // Skip whitespace.
        while pos < bytes.len() && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let start = pos;
        while pos < bytes.len() && bytes[pos] != b' ' {
            pos += 1;
        }
        let token = unsafe { core::str::from_utf8_unchecked(&bytes[start..pos]) };
        tokens.push(String::from(token));
    }

    if tokens.is_empty() {
        fatal("usage: tar -c|-t|-x archive.tar [files...]");
    }

    // First token: mode flag.
    let flag = tokens[0].as_bytes();
    let mode = if flag == b"-c" {
        b'c'
    } else if flag == b"-t" {
        b't'
    } else if flag == b"-x" {
        b'x'
    } else {
        fatal("usage: tar -c|-t|-x archive.tar [files...]");
    };

    if tokens.len() < 2 {
        fatal("usage: tar -c|-t|-x archive.tar [files...]");
    }

    let archive = String::from(tokens[1].as_str());

    (mode, archive, tokens[2..].to_vec())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let (mode, archive, files) = parse_args();

    match mode {
        b'c' => {
            let file_refs: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
            cmd_create(&archive, &file_refs);
        }
        b't' => {
            cmd_list(&archive);
        }
        b'x' => {
            cmd_extract(&archive);
        }
        _ => unreachable!(),
    }

    process::exit(0);
}
