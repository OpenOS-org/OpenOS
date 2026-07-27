//! readelf — display ELF file header information
//!
//! Usage: readelf FILE

#![no_std]
#![no_main]

mod common;

use common::{exit, format_hex, stderrln, stdout, stdoutln};
use openos_sdk::fs;

/// ELF magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF header size for 64-bit.
const ELF64_HDR_SIZE: usize = 64;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut args_iter = common::args();
    let path = match args_iter.next() {
        Some(p) => p,
        None => {
            stderrln("readelf: missing operand");
            exit(1);
        }
    };

    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("readelf: cannot open file");
            exit(1);
        }
    };

    // Read enough bytes for the ELF header.
    let mut buf = [0u8; ELF64_HDR_SIZE];
    let n = match fs::read(fd, &mut buf) {
        Ok(n) => n,
        Err(_) => {
            stderrln("readelf: read error");
            let _ = fs::close(fd);
            exit(1);
        }
    };
    let _ = fs::close(fd);

    if n < 16 || buf[..4] != ELF_MAGIC {
        stderrln("readelf: not an ELF file");
        exit(1);
    }

    // EI_CLASS (byte 4): 1 = 32-bit, 2 = 64-bit.
    let class = buf[4];
    // EI_DATA (byte 5): 1 = little-endian, 2 = big-endian.
    let data_enc = buf[5];

    stdout("ELF Header:\n");

    stdout("  Magic:   ");
    for i in 0..16.min(n) {
        let hi = (buf[i] >> 4) & 0xF;
        let lo = buf[i] & 0xF;
        let h = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
        let l = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
        let s = [h, l];
        let _ = core::str::from_utf8(&s).map(|c| {
            let _ = openos_sdk::console::write(c);
        });
        let _ = openos_sdk::console::write(" ");
    }
    stdout("\n");

    stdout("  Class:                             ");
    match class {
        1 => stdoutln("ELF32"),
        2 => stdoutln("ELF64"),
        _ => stdoutln("Invalid"),
    }

    stdout("  Data:                              ");
    match data_enc {
        1 => stdoutln("2's complement, little-endian"),
        2 => stdoutln("2's complement, big-endian"),
        _ => stdoutln("Invalid"),
    }

    // EI_OSABI (byte 7).
    stdout("  OS/ABI:                            ");
    match buf[7] {
        0 => stdoutln("UNIX - System V"),
        3 => stdoutln("UNIX - Linux"),
        other => {
            let mut num_buf = [0u8; 20];
            let num = common::format_u64(other as u64, &mut num_buf);
            let _ = core::str::from_utf8(num).map(|s| stdoutln(s));
            let _ = other;
        }
    }

    // For 64-bit ELF, parse the rest of the header.
    if class == 2 && n >= ELF64_HDR_SIZE {
        // e_type at offset 16 (u16 little-endian).
        let elf_type = u16::from_le_bytes([buf[16], buf[17]]);
        stdout("  Type:                              ");
        match elf_type {
            0 => stdoutln("NONE (No file type)"),
            1 => stdoutln("REL (Relocatable file)"),
            2 => stdoutln("EXEC (Executable file)"),
            3 => stdoutln("DYN (Shared object file)"),
            4 => stdoutln("CORE (Core file)"),
            _ => stdoutln("Unknown"),
        }

        // e_machine at offset 18 (u16).
        let machine = u16::from_le_bytes([buf[18], buf[19]]);
        stdout("  Machine:                           ");
        match machine {
            0x03 => stdoutln("x86"),
            0x3E => stdoutln("x86-64"),
            0x28 => stdoutln("ARM"),
            0xB7 => stdoutln("AArch64"),
            _ => stdoutln("Unknown"),
        }

        // e_version at offset 20 (u32).
        let version = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
        stdout("  Version:                           ");
        let mut ver_buf = [0u8; 20];
        let ver = common::format_u64(version as u64, &mut ver_buf);
        let _ = core::str::from_utf8(ver).map(|s| stdoutln(s));

        // e_entry at offset 24 (u64).
        let entry = u64::from_le_bytes([
            buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
        ]);
        stdout("  Entry point address:               ");
        let mut hex_buf = [0u8; 18];
        let hex = format_hex(entry, &mut hex_buf);
        let _ = core::str::from_utf8(hex).map(|s| stdoutln(s));

        // e_phoff at offset 32 (u64).
        let phoff = u64::from_le_bytes([
            buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
        ]);
        stdout("  Start of program headers:          ");
        let mut ph_buf = [0u8; 20];
        let ph = common::format_u64(phoff, &mut ph_buf);
        let _ = core::str::from_utf8(ph).map(|s| stdoutln(s));

        // e_shoff at offset 40 (u64).
        let shoff = u64::from_le_bytes([
            buf[40], buf[41], buf[42], buf[43], buf[44], buf[45], buf[46], buf[47],
        ]);
        stdout("  Start of section headers:          ");
        let mut sh_buf = [0u8; 20];
        let sh = common::format_u64(shoff, &mut sh_buf);
        let _ = core::str::from_utf8(sh).map(|s| stdoutln(s));

        // e_flags at offset 48 (u32).
        let flags = u32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]);
        stdout("  Flags:                             ");
        let mut fl_buf = [0u8; 20];
        let fl = common::format_u64(flags as u64, &mut fl_buf);
        let _ = core::str::from_utf8(fl).map(|s| stdoutln(s));

        // e_ehsize at offset 52 (u16).
        let ehsize = u16::from_le_bytes([buf[52], buf[53]]);
        stdout("  Size of this header:               ");
        let mut eh_buf = [0u8; 20];
        let eh = common::format_u64(ehsize as u64, &mut eh_buf);
        let _ = core::str::from_utf8(eh).map(|s| stdoutln(s));

        // e_phentsize at offset 54 (u16).
        let phentsize = u16::from_le_bytes([buf[54], buf[55]]);
        stdout("  Size of program headers:           ");
        let mut pe_buf = [0u8; 20];
        let pe = common::format_u64(phentsize as u64, &mut pe_buf);
        let _ = core::str::from_utf8(pe).map(|s| stdoutln(s));

        // e_phnum at offset 56 (u16).
        let phnum = u16::from_le_bytes([buf[56], buf[57]]);
        stdout("  Number of program headers:         ");
        let mut pn_buf = [0u8; 20];
        let pn = common::format_u64(phnum as u64, &mut pn_buf);
        let _ = core::str::from_utf8(pn).map(|s| stdoutln(s));

        // e_shentsize at offset 58 (u16).
        let shentsize = u16::from_le_bytes([buf[58], buf[59]]);
        stdout("  Size of section headers:           ");
        let mut se_buf = [0u8; 20];
        let se = common::format_u64(shentsize as u64, &mut se_buf);
        let _ = core::str::from_utf8(se).map(|s| stdoutln(s));

        // e_shnum at offset 60 (u16).
        let shnum = u16::from_le_bytes([buf[60], buf[61]]);
        stdout("  Number of section headers:         ");
        let mut sn_buf = [0u8; 20];
        let sn = common::format_u64(shnum as u64, &mut sn_buf);
        let _ = core::str::from_utf8(sn).map(|s| stdoutln(s));

        // e_shstrndx at offset 62 (u16).
        let shstrndx = u16::from_le_bytes([buf[62], buf[63]]);
        stdout("  Section header string table index: ");
        let mut sx_buf = [0u8; 20];
        let sx = common::format_u64(shstrndx as u64, &mut sx_buf);
        let _ = core::str::from_utf8(sx).map(|s| stdoutln(s));
    } else if class == 1 && n >= 52 {
        // 32-bit ELF: parse what we can.
        let elf_type = u16::from_le_bytes([buf[16], buf[17]]);
        stdout("  Type:                              ");
        match elf_type {
            0 => stdoutln("NONE (No file type)"),
            1 => stdoutln("REL (Relocatable file)"),
            2 => stdoutln("EXEC (Executable file)"),
            3 => stdoutln("DYN (Shared object file)"),
            4 => stdoutln("CORE (Core file)"),
            _ => stdoutln("Unknown"),
        }

        let machine = u16::from_le_bytes([buf[18], buf[19]]);
        stdout("  Machine:                           ");
        match machine {
            0x03 => stdoutln("x86"),
            0x3E => stdoutln("x86-64"),
            0x28 => stdoutln("ARM"),
            0xB7 => stdoutln("AArch64"),
            _ => stdoutln("Unknown"),
        }

        // e_entry at offset 24 (u32 for ELF32).
        let entry = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        stdout("  Entry point address:               ");
        let mut hex_buf = [0u8; 18];
        let hex = format_hex(entry as u64, &mut hex_buf);
        let _ = core::str::from_utf8(hex).map(|s| stdoutln(s));
    } else {
        stderrln("readelf: ELF header too short");
        exit(1);
    }

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
