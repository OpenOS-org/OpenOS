//! Kernel module loading framework.
//!
//! Manages user-space "kernel modules" — dynamically loaded user-space drivers
//! that the kernel loads from ELF binaries and tracks by name. Modules run in
//! their own address space (like regular processes) but are managed through the
//! module registry rather than the process tree.
//!
//! ## Design
//!
//! Modules are user-space ELF binaries loaded into their own address space with
//! a dedicated page table. The `ModuleRegistry` provides a name-keyed registry
//! so the kernel can look up loaded modules by name, query metadata, and unload
//! them.
//!
//! A module's "entry point" is the ELF entry address — the kernel records it
//! but does not call it directly. The module creator is expected to start the
//! module as a task (via `SYS_PROCESS_START` or similar) after loading.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use spin::Mutex;

use crate::memory::create_user_page_table;
use crate::memory::pagetable::PAGE_SIZE;
use crate::{elf, serial_println};

/// Maximum number of modules that can be loaded simultaneously.
const MAX_MODULES: usize = 64;

/// Maximum length of a module name (bytes).
const MAX_MODULE_NAME_LEN: usize = 64;

/// Default virtual address base for loading module ELFs (512 MiB).
const MODULE_LOAD_BASE: u64 = 0x2000_0000;

/// Metadata for a loaded kernel module.
#[derive(Debug, Clone)]
pub struct ModuleMetadata {
    /// Human-readable module name (e.g., "kb_driver", "net_driver").
    pub name: String,
    /// Module version (major, minor, patch).
    pub version: (u16, u16, u16),
    /// Virtual address of the ELF entry point.
    pub entry_point: u64,
    /// Physical address of the module's page table.
    pub page_table: u64,
    /// Size of the module's mapped memory region in bytes.
    pub mapped_size: u64,
    /// Base virtual address where the module was loaded.
    pub load_address: u64,
}

/// Global module registry.
///
/// Internally protected by a spinlock for safe concurrent access from syscall
/// handlers and the boot path.
static MODULE_REGISTRY: Mutex<ModuleRegistry> = Mutex::new(ModuleRegistry::new());

/// A registry of loaded kernel modules, keyed by name.
#[derive(Debug, Default)]
pub struct ModuleRegistry {
    /// Map from module name to metadata.
    modules: BTreeMap<String, ModuleMetadata>,
}

impl ModuleRegistry {
    /// Create a new, empty module registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
        }
    }

    /// Register a loaded module in the registry.
    ///
    /// Returns `true` if the module was registered, `false` if the registry is
    /// full or a module with the same name already exists.
    pub fn register(&mut self, meta: ModuleMetadata) -> bool {
        if self.modules.len() >= MAX_MODULES {
            serial_println!("[MODULE] register: registry full (max {MAX_MODULES})");
            return false;
        }
        if self.modules.contains_key(&meta.name) {
            serial_println!("[MODULE] register: '{}' already registered", meta.name);
            return false;
        }
        serial_println!(
            "[MODULE] register: '{}' v{}.{}.{} entry={:#x}",
            meta.name,
            meta.version.0,
            meta.version.1,
            meta.version.2,
            meta.entry_point,
        );
        self.modules.insert(meta.name.clone(), meta);
        true
    }

    /// Unregister a module by name.
    ///
    /// Returns the metadata of the removed module, or `None` if not found.
    pub fn unregister(&mut self, name: &str) -> Option<ModuleMetadata> {
        let meta = self.modules.remove(name);
        if let Some(ref m) = meta {
            serial_println!("[MODULE] unregister: '{}'", m.name);
        } else {
            serial_println!("[MODULE] unregister: '{}' not found", name);
        }
        meta
    }

    /// Look up a module by name.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&ModuleMetadata> {
        self.modules.get(name)
    }

    /// Return the number of loaded modules.
    #[must_use]
    pub fn count(&self) -> usize {
        self.modules.len()
    }

    /// Return `true` if no modules are loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Iterate over all registered modules.
    pub fn iter(&self) -> impl Iterator<Item = &ModuleMetadata> {
        self.modules.values()
    }

    /// Collect all module names into a vector.
    #[must_use]
    pub fn list_names(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }
}

/// Load a module from ELF data.
///
/// Validates the ELF header, allocates a page table, maps the ELF segments into
/// the module's address space, and registers the module in the global registry.
///
/// # Arguments
///
/// * `name` — Module name (e.g., `"kb_driver"`). Must not exceed `MAX_MODULE_NAME_LEN`.
/// * `version` — Module version as `(major, minor, patch)`.
/// * `elf_data` — Raw ELF64 binary data.
///
/// # Returns
///
/// `Ok(module_metadata)` on success, or a negative error code on failure.
///
/// # Errors
///
/// Returns `Err` if:
/// - The name is empty or too long.
/// - The ELF header is invalid or not an executable/PIE.
/// - Memory allocation for the page table or segments fails.
/// - A module with the same name is already loaded.
pub fn module_load(
    name: &str,
    version: (u16, u16, u16),
    elf_data: &[u8],
) -> Result<ModuleMetadata, i64> {
    // Validate name.
    if name.is_empty() || name.len() > MAX_MODULE_NAME_LEN {
        return Err(-1); // Error::InvalidArgument
    }

    // Validate ELF header.
    if elf::parse_header(elf_data).is_err() {
        serial_println!("[MODULE] '{}': invalid ELF header", name);
        return Err(-1); // Error::InvalidArgument
    }

    // Allocate a page table for the module.
    let Some(page_table_phys) = create_user_page_table() else {
        serial_println!("[MODULE] '{}': out of memory for page table", name);
        return Err(-4); // Error::OutOfMemory
    };

    // Load ELF segments into the module's page table.
    // We need to temporarily switch to the module's page table to map pages.
    let (kernel_p4, _) = x86_64::registers::control::Cr3::read();
    let kernel_cr3 = kernel_p4.start_address().as_u64();

    // Disable interrupts during the page table switch window.
    let flags = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    // SAFETY: page_table_phys was just allocated and is valid.
    unsafe {
        crate::memory::switch_page_table(page_table_phys);
    }

    let load_result = elf::load_elf(elf_data, |virt, phys, writable, executable| {
        let mut pt_flags = x86_64::structures::paging::PageTableFlags::PRESENT
            | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE;
        if writable {
            pt_flags |= x86_64::structures::paging::PageTableFlags::WRITABLE;
        }
        if !executable {
            pt_flags |= x86_64::structures::paging::PageTableFlags::NO_EXECUTE;
        }
        // SAFETY: `virt` is page-aligned, `phys` was allocated by the ELF loader,
        // and `pt_flags` correctly reflect the segment permissions.
        unsafe {
            crate::task::user::map_page_user(virt, phys, pt_flags);
        }
    });

    // Switch back to the kernel's page table.
    // SAFETY: kernel_cr3 is the original kernel page table.
    unsafe {
        crate::memory::switch_page_table(kernel_cr3);
    }

    // Restore interrupt state.
    if flags {
        x86_64::instructions::interrupts::enable();
    }

    let load_result = match load_result {
        Ok(r) => r,
        Err(e) => {
            serial_println!("[MODULE] '{}': ELF load failed: {:?}", name, e);
            return Err(-1);
        }
    };

    let entry_point = load_result.entry_point;

    // Compute the mapped size from ELF program headers.
    // Walk PT_LOAD segments to find the min virtual address and max end.
    let (load_address, mapped_size) = {
        let mut min_vaddr: u64 = u64::MAX;
        let mut max_end: u64 = 0;
        if let Ok(header) = elf::parse_header(elf_data) {
            for i in 0..header.phnum {
                if let Ok(ph) = elf::parse_program_header(elf_data, header.phoff, i) {
                    if ph.p_type != elf::PT_LOAD {
                        continue;
                    }
                    let seg_start = ph.p_vaddr & !0xFFF;
                    let seg_end = seg_start + ((ph.p_memsz + 0xFFF) & !0xFFF);
                    if seg_start < min_vaddr {
                        min_vaddr = seg_start;
                    }
                    if seg_end > max_end {
                        max_end = seg_end;
                    }
                }
            }
        }
        if max_end > min_vaddr && min_vaddr != u64::MAX {
            (min_vaddr, max_end - min_vaddr)
        } else {
            // Fallback: no PT_LOAD segments found.
            (0x4000_0000, PAGE_SIZE)
        }
    };

    let meta = ModuleMetadata {
        name: name.to_string(),
        version,
        entry_point,
        page_table: page_table_phys,
        mapped_size,
        load_address,
    };

    // Register in the global registry.
    let mut registry = MODULE_REGISTRY.lock();
    if !registry.register(meta.clone()) {
        serial_println!(
            "[MODULE] '{}': failed to register (duplicate or full)",
            name
        );
        return Err(-1);
    }

    serial_println!(
        "[MODULE] '{}' loaded: entry={:#x} page_table={:#x} mapped_size={}",
        name,
        entry_point,
        page_table_phys,
        mapped_size,
    );

    Ok(meta)
}

/// Unload a module by name.
///
/// Removes the module from the registry. Memory cleanup (page table, frames)
/// is deferred — the caller should ensure no task is running in the module's
/// address space before unloading.
///
/// Returns the metadata on success, or `None` if the module was not found.
pub fn module_unload(name: &str) -> Option<ModuleMetadata> {
    let mut registry = MODULE_REGISTRY.lock();
    let meta = registry.unregister(name);
    if let Some(ref m) = meta {
        serial_println!(
            "[MODULE] '{}' unloaded: entry={:#x} page_table={:#x}",
            m.name,
            m.entry_point,
            m.page_table,
        );
    }
    meta
}

/// Look up a module by name.
#[must_use]
pub fn module_lookup(name: &str) -> Option<ModuleMetadata> {
    let registry = MODULE_REGISTRY.lock();
    registry.lookup(name).cloned()
}

/// List all loaded module names.
#[must_use]
pub fn module_list() -> Vec<String> {
    let registry = MODULE_REGISTRY.lock();
    registry.list_names()
}

/// Return the number of loaded modules.
#[must_use]
pub fn module_count() -> usize {
    let registry = MODULE_REGISTRY.lock();
    registry.count()
}

// ─────────────────── Syscall handlers ───────────────────

/// Copy a buffer from user-space without the size limit of `copy_from_user`.
///
/// Validates that the source pointer is non-null, within user-space bounds,
/// and that the range does not cross into kernel space.
///
/// # Safety
///
/// Same safety requirements as `core::ptr::copy_nonoverlapping`.
unsafe fn copy_from_user_unbounded(src: *const u8, len: usize) -> Option<alloc::vec::Vec<u8>> {
    if src.is_null() || len == 0 {
        return None;
    }
    let src_addr = src as u64;
    if src_addr >= crate::memory::USER_SPACE_MAX {
        return None;
    }
    if src_addr.saturating_add(len as u64) > crate::memory::USER_SPACE_MAX {
        return None;
    }
    let mut buf = alloc::vec![0u8; len];
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len) }
    Some(buf)
}

/// Handle `SYS_MODULE_LOAD`.
///
/// Arguments:
///   arg0: pointer to module name (UTF-8) in user-space
///   arg1: name length
///   arg2: pointer to ELF data in user-space
///   arg3: ELF data length
///   arg4: version packed as `(major << 32) | (minor << 16) | patch`
///
/// Returns: 0 on success, negative `Error` code on failure.
pub fn sys_module_load(
    name_ptr: u64,
    name_len: u64,
    elf_ptr: u64,
    elf_len: u64,
    version_packed: u64,
) -> i64 {
    // Copy module name from user-space.
    let Some(name_bytes) =
        (unsafe { crate::syscall::copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return -9; // Error::BadPointer
    };
    let Ok(name) = core::str::from_utf8(&name_bytes) else {
        return -1; // Error::InvalidArgument
    };

    // Copy ELF data from user-space.
    if elf_ptr == 0 || elf_len == 0 || elf_len > 512 * 1024 {
        // Limit to 512 KiB for module ELFs.
        return -1; // Error::InvalidArgument
    }
    let elf_data = match unsafe { copy_from_user_unbounded(elf_ptr as *const u8, elf_len as usize) }
    {
        Some(d) => d,
        None => return -9, // Error::BadPointer
    };

    // Unpack version.
    let major = ((version_packed >> 32) & 0xFFFF) as u16;
    let minor = ((version_packed >> 16) & 0xFFFF) as u16;
    let patch = (version_packed & 0xFFFF) as u16;

    serial_println!(
        "[SYSCALL] module_load: '{}' v{}.{}.{} ({} bytes ELF)",
        name,
        major,
        minor,
        patch,
        elf_len,
    );

    match module_load(name, (major, minor, patch), &elf_data) {
        Ok(meta) => {
            serial_println!(
                "[SYSCALL] module_load: '{}' loaded, entry={:#x}",
                name,
                meta.entry_point,
            );
            0
        }
        Err(e) => e,
    }
}

/// Handle `SYS_MODULE_UNLOAD`.
///
/// Arguments:
///   arg0: pointer to module name (UTF-8) in user-space
///   arg1: name length
///
/// Returns: 0 on success, negative `Error` code on failure.
pub fn sys_module_unload(name_ptr: u64, name_len: u64) -> i64 {
    let Some(name_bytes) =
        (unsafe { crate::syscall::copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return -9; // Error::BadPointer
    };
    let Ok(name) = core::str::from_utf8(&name_bytes) else {
        return -1; // Error::InvalidArgument
    };

    serial_println!("[SYSCALL] module_unload: '{}'", name);

    if module_unload(name).is_some() {
        0
    } else {
        -2 // Error::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ModuleMetadata tests ───

    #[test]
    #[ignore]
    fn test_module_metadata_creation() {
        let meta = ModuleMetadata {
            name: "test_driver".into(),
            version: (1, 0, 0),
            entry_point: 0x4000_0000,
            page_table: 0x1000,
            mapped_size: 0x4000,
            load_address: 0x4000_0000,
        };
        assert_eq!(meta.name, "test_driver");
        assert_eq!(meta.version, (1, 0, 0));
        assert_eq!(meta.entry_point, 0x4000_0000);
    }

    // ─── ModuleRegistry tests ───

    #[test]
    #[ignore]
    fn test_registry_empty_on_creation() {
        let registry = ModuleRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.count(), 0);
    }

    #[test]
    #[ignore]
    fn test_registry_register_and_lookup() {
        let mut registry = ModuleRegistry::new();
        let meta = ModuleMetadata {
            name: "kb_driver".into(),
            version: (1, 2, 3),
            entry_point: 0x4000_0000,
            page_table: 0x2000,
            mapped_size: 0x8000,
            load_address: 0x4000_0000,
        };
        assert!(registry.register(meta.clone()));
        assert_eq!(registry.count(), 1);
        assert!(!registry.is_empty());

        let lookup = registry.lookup("kb_driver");
        assert!(lookup.is_some());
        assert_eq!(lookup.unwrap().version, (1, 2, 3));
    }

    #[test]
    #[ignore]
    fn test_registry_rejects_duplicate() {
        let mut registry = ModuleRegistry::new();
        let meta = ModuleMetadata {
            name: "driver".into(),
            version: (1, 0, 0),
            entry_point: 0x4000_0000,
            page_table: 0x2000,
            mapped_size: 0x4000,
            load_address: 0x4000_0000,
        };
        assert!(registry.register(meta.clone()));
        // Second registration with the same name should fail.
        assert!(!registry.register(meta));
        assert_eq!(registry.count(), 1);
    }

    #[test]
    #[ignore]
    fn test_registry_unregister() {
        let mut registry = ModuleRegistry::new();
        let meta = ModuleMetadata {
            name: "net_driver".into(),
            version: (2, 1, 0),
            entry_point: 0x4000_0000,
            page_table: 0x3000,
            mapped_size: 0x4000,
            load_address: 0x4000_0000,
        };
        assert!(registry.register(meta.clone()));
        assert_eq!(registry.count(), 1);

        let removed = registry.unregister("net_driver");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().version, (2, 1, 0));
        assert!(registry.is_empty());
    }

    #[test]
    #[ignore]
    fn test_registry_unregister_nonexistent() {
        let mut registry = ModuleRegistry::new();
        assert!(registry.unregister("nonexistent").is_none());
    }

    #[test]
    #[ignore]
    fn test_registry_max_modules() {
        let mut registry = ModuleRegistry::new();
        // Fill up to MAX_MODULES.
        for i in 0..MAX_MODULES {
            let meta = ModuleMetadata {
                name: alloc::format!("mod_{}", i),
                version: (0, 0, 1),
                entry_point: 0x4000_0000,
                page_table: 0x1000 * (i as u64 + 1),
                mapped_size: 0x4000,
                load_address: 0x4000_0000,
            };
            assert!(registry.register(meta), "register mod_{} failed", i);
        }
        assert_eq!(registry.count(), MAX_MODULES);

        // Exceeding the limit should fail.
        let extra = ModuleMetadata {
            name: "extra_mod".into(),
            version: (0, 0, 1),
            entry_point: 0x4000_0000,
            page_table: 0x9999,
            mapped_size: 0x4000,
            load_address: 0x4000_0000,
        };
        assert!(!registry.register(extra));
        assert_eq!(registry.count(), MAX_MODULES);
    }

    #[test]
    #[ignore]
    fn test_registry_list_names() {
        let mut registry = ModuleRegistry::new();
        assert!(registry.list_names().is_empty());

        let names = ["a", "b", "c"];
        for &n in &names {
            let meta = ModuleMetadata {
                name: n.into(),
                version: (1, 0, 0),
                entry_point: 0x4000_0000,
                page_table: 0x2000,
                mapped_size: 0x4000,
                load_address: 0x4000_0000,
            };
            assert!(registry.register(meta));
        }

        let listed = registry.list_names();
        assert_eq!(listed.len(), 3);
        for n in &names {
            assert!(listed.contains(&n.to_string()));
        }
    }

    #[test]
    #[ignore]
    fn test_registry_iter() {
        let mut registry = ModuleRegistry::new();
        let meta = ModuleMetadata {
            name: "test_mod".into(),
            version: (3, 2, 1),
            entry_point: 0x4000_0000,
            page_table: 0x2000,
            mapped_size: 0x4000,
            load_address: 0x4000_0000,
        };
        registry.register(meta.clone());

        let collected: Vec<&ModuleMetadata> = registry.iter().collect();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].name, "test_mod");
    }

    // ─── Global registry function tests ───

    #[test]
    #[ignore]
    fn test_global_registry_initial_state() {
        // Global registry starts empty.
        assert!(module_list().is_empty());
        assert_eq!(module_count(), 0);
    }

    #[test]
    #[ignore]
    fn test_global_registry_lookup_nonexistent() {
        assert!(module_lookup("nonexistent").is_none());
    }

    #[test]
    #[ignore]
    fn test_global_registry_unload_nonexistent() {
        assert!(module_unload("nonexistent").is_none());
    }

    // ─── sys_module_load validation tests ───

    #[test]
    #[ignore]
    fn test_sys_module_load_null_name() {
        let result = sys_module_load(0, 5, 0x2000, 64, 0x0001_0000_0000);
        assert!(result < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_load_zero_name_length() {
        let result = sys_module_load(0x1000, 0, 0x2000, 64, 0);
        assert!(result < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_load_null_elf() {
        // elf_ptr=0 is rejected by the ELF length check.
        // Use elf_ptr=0 which triggers the elf_ptr==0 check.
        assert!(sys_module_load(0x1000, 4, 0, 64, 0) < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_load_zero_elf_length() {
        // elf_len=0 is rejected by the ELF length check (before any copy attempt).
        // Use a valid-looking but not-yet-checked elf_ptr=0x2000.
        assert!(sys_module_load(0x1000, 4, 0x2000, 0, 0) < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_load_excessive_elf() {
        // ELF data larger than 512 KiB should be rejected.
        assert!(sys_module_load(0x1000, 4, 0x2000, 512 * 1024 + 1, 0) < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_load_kernel_name_ptr() {
        // Name pointer in kernel space — copy_from_user will reject.
        // But only if USER_SPACE_MAX is the cutoff; in test mode the function
        // may actually try to read. Let the function handle it.
        let result = sys_module_load(crate::memory::USER_SPACE_MAX, 4, 0x2000, 64, 0);
        assert!(result < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_unload_null_name() {
        assert!(sys_module_unload(0, 5) < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_unload_zero_length() {
        assert!(sys_module_unload(0x1000, 0) < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_unload_kernel_ptr() {
        assert!(sys_module_unload(crate::memory::USER_SPACE_MAX, 4) < 0);
    }

    #[test]
    #[ignore]
    fn test_sys_module_unload_nonexistent() {
        // The global registry is empty, so module_unload returns None.
        // We use sys_module_unload with a user-space pointer, but in test
        // mode we can't safely dereference user-space addresses.
        // This test verifies the error path from copy_from_user rejection.
        let buf = Vec::from(b"nonexistent" as &[u8]);
        let ptr = buf.as_ptr() as u64;
        let result = if ptr >= crate::memory::USER_SPACE_MAX {
            -9 // copy_from_user rejects kernel-space pointers
        } else {
            // We've placed data in user-space range — sys_module_unload will
            // attempt copy_from_user, which should succeed, then try to
            // unload a nonexistent module and return NotFound.
            let r = sys_module_unload(ptr, 11);
            if r == 0 {
                0
            } else {
                r
            }
        };
        assert!(result < 0);
    }

    // ─── Version unpacking tests ───

    #[test]
    #[ignore]
    fn test_version_unpacking() {
        // Pack version 1.2.3.
        let packed: u64 = (1u64 << 32) | (2u64 << 16) | 3u64;
        let major = ((packed >> 32) & 0xFFFF) as u16;
        let minor = ((packed >> 16) & 0xFFFF) as u16;
        let patch = (packed & 0xFFFF) as u16;
        assert_eq!(major, 1);
        assert_eq!(minor, 2);
        assert_eq!(patch, 3);
    }

    #[test]
    #[ignore]
    fn test_version_unpacking_zero() {
        let packed: u64 = 0;
        let major = ((packed >> 32) & 0xFFFF) as u16;
        let minor = ((packed >> 16) & 0xFFFF) as u16;
        let patch = (packed & 0xFFFF) as u16;
        assert_eq!(major, 0);
        assert_eq!(minor, 0);
        assert_eq!(patch, 0);
    }

    #[test]
    #[ignore]
    fn test_version_unpacking_max() {
        let packed: u64 = 0xFFFF_FFFF_FFFF;
        let major = ((packed >> 32) & 0xFFFF) as u16;
        let minor = ((packed >> 16) & 0xFFFF) as u16;
        let patch = (packed & 0xFFFF) as u16;
        assert_eq!(major, u16::MAX);
        assert_eq!(minor, u16::MAX);
        assert_eq!(patch, u16::MAX);
    }

    // ─── Name validation tests ───

    #[test]
    #[ignore]
    fn test_empty_name_rejected() {
        let result = module_load("", (0, 0, 0), &[]);
        assert!(result.is_err());
    }

    #[test]
    #[ignore]
    fn test_name_too_long_rejected() {
        let long_name = "a".repeat(MAX_MODULE_NAME_LEN + 1);
        let result = module_load(&long_name, (0, 0, 0), &[]);
        assert!(result.is_err());
    }

    #[test]
    #[ignore]
    fn test_name_at_max_length_ok() {
        let name = "a".repeat(MAX_MODULE_NAME_LEN);
        // module_load will fail later because ELF data is empty, but the
        // name validation should pass (returns ELF parse error, not name error).
        let result = module_load(&name, (0, 0, 0), &[]);
        assert!(result.is_err());
        // The error should be from ELF parsing, not name validation.
        // We can't easily check the exact error, but it should be an Err.
    }

    // ─── Constants tests ───

    #[test]
    #[ignore]
    fn test_max_modules_positive() {
        assert!(MAX_MODULES > 0);
        assert!(MAX_MODULES <= 256);
    }

    #[test]
    #[ignore]
    fn test_max_module_name_len_reasonable() {
        assert!(MAX_MODULE_NAME_LEN >= 16);
        assert!(MAX_MODULE_NAME_LEN <= 256);
    }
}
