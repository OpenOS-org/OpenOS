//! `OpenOS` kernel library crate.
//!
//! This `lib.rs` enables `cargo test` on the host target. When compiling
//! normally (binary via `main.rs`), `#![no_std]` and `#![no_main]` are
//! set there. When compiling for tests, this crate is built with `std`
//! so the standard test harness works.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![feature(abi_x86_interrupt)]
#![cfg_attr(not(test), feature(alloc_error_handler))]
#![allow(unused_features)]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::module_inception,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    dead_code,
    unused_imports,
    unused_variables,
    clippy::missing_const_for_fn,
    clippy::used_underscore_items
)]

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod elf;
pub mod frame_alloc;
pub mod fs;
pub mod handle;
pub mod initrd;
pub mod ipc;
pub mod memory;
pub mod net;
pub mod sync;
pub mod syscall;
pub mod task;

/// Global ramdisk data. Set once during boot, read-only thereafter.
/// Used by `process_start` to load ELF binaries from the initrd.
pub static mut RAMDISK_DATA: Option<&'static [u8]> = None;
