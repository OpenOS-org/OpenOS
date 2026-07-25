//! # `OpenOS` User-Space SDK
//!
//! A zero-dependency, `#![no_std]` library for building user-space programs
//! on the `OpenOS` microkernel. Provides:
//!
//! - **ABI layer** ([`abi`]) — raw syscall invocation and error types
//! - **I/O** ([`io`]) — `write`/`read` wrappers
//! - **Process** ([`process`]) — `exit`/`yield_`
//! - **IPC** ([`ipc`]) — port-based message passing
//! - **Print macros** — `print!`/`println!` via `SYS_WRITE`
//! - **Allocator** — bump allocator for `alloc` types
//! - **Panic handler** — prints and exits on panic
//!
//! ## Quick Start
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//!
//! extern crate alloc;
//!
//! #[no_mangle]
//! extern "C" fn _start() -> ! {
//!     openos_sdk::println!("Hello from user-space!");
//!     openos_sdk::process::exit(0);
//! }
//! ```

#![no_std]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions
)]

extern crate alloc;

pub mod abi;
pub mod alloc_impl;
pub mod io;
pub mod ipc;
pub mod panic;
pub mod print;
pub mod process;
