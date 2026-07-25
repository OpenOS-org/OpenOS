//! ABI interface layer — the contract between user-space and the kernel.
//!
//! This module provides:
//! - [`raw`] — Unsafe raw syscall invocation via inline assembly
//! - [`number`] — Syscall number constants (mirrors kernel definitions)
//! - [`error`] — Error types matching the kernel's return convention
//!
//! Higher-level SDK modules (`io`, `process`, `ipc`) build on these
//! primitives to provide safe, ergonomic APIs.

pub mod error;
pub mod number;
pub mod raw;
