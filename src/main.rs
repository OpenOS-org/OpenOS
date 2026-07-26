//! OpenOS disk image builder.
//!
//! Creates bootable disk images from the compiled kernel ELF binary
//! and an optional initrd (ramdisk) archive.
//!
//! Usage:
//!   cargo run -- <kernel-elf> <output.img> [initrd.img] [--uefi]
//!
//! The output format depends on the file extension and flags:
//!   - `.img` without `--uefi`: BIOS boot (MBR disk image)
//!   - `.img` with `--uefi`: UEFI boot (FAT EFI system partition)

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <kernel-elf> <output.img> [initrd.img] [--uefi]",
            args[0]
        );
        std::process::exit(1);
    }

    let kernel_path = Path::new(&args[1]);
    if !kernel_path.exists() {
        eprintln!("Error: kernel binary not found: {}", kernel_path.display());
        std::process::exit(1);
    }

    // Parse arguments: positional args + optional --uefi flag.
    let mut out_path = None;
    let mut initrd_path = None;
    let mut uefi = false;

    for arg in &args[2..] {
        if arg == "--uefi" {
            uefi = true;
        } else if out_path.is_none() {
            out_path = Some(Path::new(arg.as_str()));
        } else if initrd_path.is_none() {
            let p = Path::new(arg.as_str());
            if !p.exists() {
                eprintln!("Error: initrd not found: {}", p.display());
                std::process::exit(1);
            }
            initrd_path = Some(p);
        }
    }

    let out_path = out_path.unwrap_or_else(|| {
        eprintln!("Error: no output path specified");
        std::process::exit(1);
    });

    if uefi {
        println!("Creating UEFI disk image...");
    } else {
        println!("Creating BIOS disk image...");
    }
    println!("  Kernel: {}", kernel_path.display());
    println!("  Output: {}", out_path.display());
    if let Some(rd) = initrd_path {
        println!("  Ramdisk: {}", rd.display());
    }

    if uefi {
        let mut builder = bootloader::UefiBoot::new(kernel_path);
        if let Some(rd) = initrd_path {
            builder.set_ramdisk(rd);
        }
        builder
            .create_disk_image(out_path)
            .expect("Failed to create UEFI disk image");
    } else {
        let mut builder = bootloader::BiosBoot::new(kernel_path);
        if let Some(rd) = initrd_path {
            builder.set_ramdisk(rd);
        }
        builder
            .create_disk_image(out_path)
            .expect("Failed to create BIOS disk image");
    }

    println!("Done: {}", out_path.display());
}
