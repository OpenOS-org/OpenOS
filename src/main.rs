//! OpenOS disk image builder.
//!
//! Creates a BIOS-bootable disk image from the compiled kernel ELF binary
//! and an optional initrd (ramdisk) archive.
//!
//! Usage: cargo run -- <kernel-elf> <output.img> [initrd.img]

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <kernel-elf> <output.img> [initrd.img]", args[0]);
        std::process::exit(1);
    }

    let kernel_path = Path::new(&args[1]);
    if !kernel_path.exists() {
        eprintln!("Error: kernel binary not found: {}", kernel_path.display());
        std::process::exit(1);
    }

    let out_path = Path::new(&args[2]);
    let initrd_path = if args.len() >= 4 {
        let p = Path::new(&args[3]);
        if !p.exists() {
            eprintln!("Error: initrd not found: {}", p.display());
            std::process::exit(1);
        }
        Some(p)
    } else {
        None
    };

    println!("Creating BIOS disk image...");
    println!("  Kernel: {}", kernel_path.display());
    println!("  Output: {}", out_path.display());
    if let Some(rd) = initrd_path {
        println!("  Ramdisk: {}", rd.display());
    }

    let mut builder = bootloader::BiosBoot::new(kernel_path);
    if let Some(rd) = initrd_path {
        builder.set_ramdisk(rd);
    }
    builder
        .create_disk_image(out_path)
        .expect("Failed to create BIOS disk image");

    println!("Done: {}", out_path.display());
}
