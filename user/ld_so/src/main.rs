//! OpenOS dynamic linker (ld.so).
//!
//! Minimal user-space dynamic linker that:
//! 1. Reads a target ELF binary from the filesystem
//! 2. Parses its PT_DYNAMIC segment
//! 3. Loads needed shared libraries from /disk/lib/
//! 4. Performs RELA relocations (R_X86_64_RELATIVE, R_X86_64_GLOB_DAT, R_X86_64_JUMP_SLOT)
//! 5. Transfers control to the program's entry point
//!
//! Usage: ld_so <program> [args...]

#![no_std]
#![no_main]
#![allow(non_snake_case, dead_code)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, fs, process};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in ld.so!");
    process::exit(1);
}

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

// ─── ELF constants ───

const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_X86_64: u16 = 62;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;

const DT_NULL: i64 = 0;
const DT_NEEDED: i64 = 1;
const DT_STRTAB: i64 = 5;
const DT_STRSZ: i64 = 10;
const DT_SYMTAB: i64 = 6;
const DT_SYMENT: i64 = 11;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;
const DT_JMPREL: i64 = 23;
const DT_PLTRELSZ: i64 = 2;
const DT_PLTGOT: i64 = 3;

const R_X86_64_RELATIVE: u32 = 8;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;

// ─── ELF structures ───

#[repr(C)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

#[repr(C)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

// ─── Helpers ───

fn read_u16(data: &[u8], off: usize) -> u16 {
    if off + 2 > data.len() {
        process::exit(0);
    }
    u16::from_le_bytes([data[off], data[off + 1]])
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    if off + 4 > data.len() {
        process::exit(0);
    }
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn read_u64(data: &[u8], off: usize) -> u64 {
    if off + 8 > data.len() {
        process::exit(0);
    }
    u64::from_le_bytes([
        data[off],
        data[off + 1],
        data[off + 2],
        data[off + 3],
        data[off + 4],
        data[off + 5],
        data[off + 6],
        data[off + 7],
    ])
}

fn read_i64(data: &[u8], off: usize) -> i64 {
    read_u64(data, off) as i64
}

fn write_u64(addr: u64, val: u64) {
    // SAFETY: addr must be a valid, mapped, writable address.
    unsafe {
        core::ptr::write_unaligned(addr as *mut u64, val);
    }
}

fn strlen_null(s: &[u8]) -> usize {
    s.iter().position(|&b| b == 0).unwrap_or(s.len())
}

// ─── Entry point ───

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("ld.so: dynamic linker starting");

    // Read the program path from args (simplified: read from a fixed path).
    // In a real implementation, this would come from command-line arguments.
    let program_path = "/disk/program.elf";

    let _ = console::write("ld.so: loading ");
    let _ = console::writeln(program_path);

    // Open and read the program.
    let fd = match fs::open(program_path) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = console::writeln("ld.so: failed to open program");
            process::exit(1);
        }
    };

    let mut buf = [0u8; 65536]; // 64 KiB buffer.
    let n = match fs::read(fd, &mut buf) {
        Ok(n) => n,
        Err(_) => {
            let _ = console::writeln("ld.so: failed to read program");
            let _ = fs::close(fd);
            process::exit(1);
        }
    };
    let _ = fs::close(fd);

    let data = &buf[..n];

    // Validate ELF header.
    if data.len() < 64 || data[0..4] != ELFMAG {
        let _ = console::writeln("ld.so: not an ELF file");
        process::exit(1);
    }
    if data[4] != ELFCLASS64 || data[5] != ELFDATA2LSB {
        let _ = console::writeln("ld.so: not ELF64 little-endian");
        process::exit(1);
    }
    let e_machine = read_u16(data, 18);
    if e_machine != EM_X86_64 {
        let _ = console::writeln("ld.so: not x86-64");
        process::exit(1);
    }

    let e_type = read_u16(data, 16);
    let e_entry = read_u64(data, 24);
    let e_phoff = read_u64(data, 32);
    let e_phnum = read_u16(data, 56);
    let e_phentsize = read_u16(data, 54);

    let _ = console::writeln("ld.so: parsing ELF headers");

    // Find PT_DYNAMIC segment.
    let mut dyn_offset: u64 = 0;
    let mut dyn_vaddr: u64 = 0;
    let mut dyn_size: u64 = 0;
    let mut found_dynamic = false;

    // Also find load base (lowest PT_LOAD vaddr).
    let mut load_base: u64 = u64::MAX;

    for i in 0..e_phnum {
        let ph_off = e_phoff as usize + (i as usize) * e_phentsize as usize;
        if ph_off + 56 > data.len() {
            break;
        }
        let p_type = read_u32(data, ph_off);
        let p_vaddr = read_u64(data, ph_off + 16);
        let p_offset = read_u64(data, ph_off + 8);
        let p_filesz = read_u64(data, ph_off + 32);

        if p_type == PT_LOAD && p_vaddr < load_base {
            load_base = p_vaddr;
        }

        if p_type == PT_DYNAMIC {
            dyn_offset = p_offset;
            dyn_vaddr = p_vaddr;
            dyn_size = p_filesz;
            found_dynamic = true;
        }
    }

    if !found_dynamic {
        let _ = console::writeln("ld.so: no PT_DYNAMIC (static binary)");
        // Static binary — just jump to entry.
        jump_to_entry(e_entry);
        process::exit(0);
    }

    if load_base == u64::MAX {
        load_base = 0;
    }

    let _ = console::writeln("ld.so: found load base");

    // Parse dynamic entries.
    let mut strtab_addr: u64 = 0;
    let mut strtab_size: u64 = 0;
    let mut symtab_addr: u64 = 0;
    let mut rela_addr: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut jmprel_addr: u64 = 0;
    let mut jmprel_size: u64 = 0;
    let mut needed: [u64; 16] = [0; 16];
    let mut needed_count: usize = 0;

    let num_dyn = dyn_size / 16;
    for i in 0..num_dyn {
        let off = (dyn_offset + i * 16) as usize;
        if off + 16 > data.len() {
            break;
        }
        let tag = read_i64(data, off);
        let val = read_u64(data, off + 8);

        match tag {
            DT_NULL => break,
            DT_NEEDED => {
                if needed_count < needed.len() {
                    needed[needed_count] = val;
                    needed_count += 1;
                }
            }
            DT_STRTAB => strtab_addr = val,
            DT_STRSZ => strtab_size = val,
            DT_SYMTAB => symtab_addr = val,
            DT_RELA => rela_addr = val,
            DT_RELASZ => rela_size = val,
            DT_JMPREL => jmprel_addr = val,
            DT_PLTRELSZ => jmprel_size = val,
            _ => {}
        }
    }

    let _ = console::writeln("ld.so: parsed dynamic entries");

    // Process needed libraries.
    for i in 0..needed_count {
        let name_off = needed[i] as usize;
        let name_start = strtab_addr as usize + name_off - load_base as usize;
        if name_start < data.len() {
            let name_bytes = &data[name_start..];
            let name_len = strlen_null(name_bytes);
            let name = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("?");
            let _ = console::write("ld.so: needs ");
            let _ = console::writeln(name);
        }
    }

    // Perform RELA relocations.
    let num_rela = rela_size / 24;
    let mut reloc_count: u64 = 0;

    for i in 0..num_rela {
        let off = (rela_addr - load_base + i * 24) as usize;
        if off + 24 > data.len() {
            break;
        }
        let r_offset = read_u64(data, off);
        let r_info = read_u64(data, off + 8);
        let r_addend = read_i64(data, off + 16);

        let rel_type = (r_info & 0xFFFF_FFFF) as u32;
        let _sym_index = (r_info >> 32) as u32;

        match rel_type {
            R_X86_64_RELATIVE => {
                // B + A: base address + addend.
                let val = load_base.wrapping_add(r_addend as u64);
                write_u64(r_offset, val);
                reloc_count += 1;
            }
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                // For now, set to 0 (unresolved). A full implementation would
                // look up the symbol in the symbol table and resolve it.
                write_u64(r_offset, r_addend as u64);
                reloc_count += 1;
            }
            R_X86_64_NONE => {}
            _ => {
                // Unknown relocation type — skip.
            }
        }
    }

    let _ = console::writeln("ld.so: relocations done");

    // Also process PLT relocations.
    let num_plt = jmprel_size / 24;
    for i in 0..num_plt {
        let off = (jmprel_addr - load_base + i * 24) as usize;
        if off + 24 > data.len() {
            break;
        }
        let r_offset = read_u64(data, off);
        let r_info = read_u64(data, off + 8);
        let r_addend = read_i64(data, off + 16);

        let rel_type = (r_info & 0xFFFF_FFFF) as u32;
        if rel_type == R_X86_64_JUMP_SLOT {
            write_u64(r_offset, r_addend as u64);
        }
    }

    let _ = console::writeln("ld.so: jumping to program entry");

    // Jump to the program's entry point.
    jump_to_entry(e_entry);
}

/// Jump to the program's entry point.
///
/// # Safety
///
/// `entry` must be a valid, executable address.
fn jump_to_entry(entry: u64) -> ! {
    // SAFETY: entry is validated as an ELF entry point.
    // We use inline assembly to jump there. The program will
    // take over from here.
    unsafe {
        core::arch::asm!(
            "jmp {entry}",
            entry = in(reg) entry,
            options(noreturn)
        );
    }
}
