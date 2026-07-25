//! OpenOS disk image builder.
//!
//! Creates a BIOS-bootable disk image from the compiled kernel ELF binary.
//! Usage: cargo run -- <kernel-elf-path> [output-path]

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <kernel-elf> [output.img]", args[0]);
        std::process::exit(1);
    }

    let kernel_path = Path::new(&args[1]);
    if !kernel_path.exists() {
        eprintln!("Error: kernel binary not found: {}", kernel_path.display());
        std::process::exit(1);
    }

    let out_path = if args.len() >= 3 {
        std::path::PathBuf::from(&args[2])
    } else {
        let mut p = kernel_path.parent().unwrap().to_path_buf();
        p.push("bios.img");
        p
    };

    println!("Creating BIOS disk image...");
    println!("  Kernel: {}", kernel_path.display());
    println!("  Output: {}", out_path.display());

    bootloader::BiosBoot::new(kernel_path)
        .create_disk_image(&out_path)
        .expect("Failed to create BIOS disk image");

    println!("Done: {}", out_path.display());
}
