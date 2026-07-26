//! mkinitrd — Create an OpenOS initrd archive.
//!
//! Usage: mkinitrd output.initrd name1=path1 name2=path2 ...
//!
//! Archive format:
//!   Header (8 bytes):
//!     [0x00] magic:   u32 = 0x4F535244  ("OSRD")
//!     [0x04] count:   u32               number of files
//!
//!   File entries (264 bytes each):
//!     [0x00]   name:    [u8; 256]       null-terminated filename
//!     [0x100]  offset:  u32             byte offset from start of archive
//!     [0x104]  size:    u32             file size in bytes
//!
//!   Data section:
//!     Raw file contents at the specified offsets.

use std::io::Write;
use std::{env, fs, process};

const MAGIC: u32 = 0x4F535244; // "OSRD" in little-endian
const ENTRY_SIZE: usize = 264; // 256 + 4 + 4
const HEADER_SIZE: usize = 8; // magic + count

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

fn create_initrd(files: &[(String, String)], output_path: &str) -> std::io::Result<()> {
    let count = files.len();

    // Calculate data offsets.
    let table_size = HEADER_SIZE + count * ENTRY_SIZE;
    let data_offset = align_up(table_size, 4096); // Page-align data section

    // Read file contents and calculate offsets.
    let mut file_data: Vec<(String, Vec<u8>, usize)> = Vec::new();
    let mut current_offset = data_offset;
    for (name, path) in files {
        let data = fs::read(path)?;
        file_data.push((name.clone(), data, current_offset));
        current_offset += file_data.last().unwrap().1.len();
    }

    // Write archive.
    let mut out = fs::File::create(output_path)?;

    // Header.
    out.write_all(&MAGIC.to_le_bytes())?;
    out.write_all(&(count as u32).to_le_bytes())?;

    // File entries.
    for (name, data, offset) in &file_data {
        let name_bytes = name.as_bytes();
        let mut name_buf = [0u8; 256];
        let copy_len = name_bytes.len().min(255);
        name_buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        out.write_all(&name_buf)?;
        out.write_all(&(*offset as u32).to_le_bytes())?;
        out.write_all(&(data.len() as u32).to_le_bytes())?;
    }

    // Pad to data section start.
    let current_pos = HEADER_SIZE + count * ENTRY_SIZE;
    if data_offset > current_pos {
        out.write_all(&vec![0u8; data_offset - current_pos])?;
    }

    // File data.
    for (_, data, _) in &file_data {
        out.write_all(data)?;
    }

    println!("Created {output_path}: {count} files, {current_offset} bytes");
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} output.initrd name1=path1 [name2=path2 ...]",
            args[0]
        );
        process::exit(1);
    }

    let output = &args[1];
    let mut files: Vec<(String, String)> = Vec::new();

    for arg in &args[2..] {
        let Some((name, path)) = arg.split_once('=') else {
            eprintln!("Error: expected name=path, got '{arg}'");
            process::exit(1);
        };
        if !std::path::Path::new(path).exists() {
            eprintln!("Error: file not found: {path}");
            process::exit(1);
        }
        files.push((name.to_string(), path.to_string()));
    }

    if let Err(e) = create_initrd(&files, output) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
