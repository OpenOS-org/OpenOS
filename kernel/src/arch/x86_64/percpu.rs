//! Per-CPU data via the `GSBASE` MSR.
//!
//! Each CPU core stores a pointer to its own `PerCpuData` struct in the
//! `IA32_GS_BASE` MSR. This allows O(1) access to per-CPU state (current
//! task, idle flag, LAPIC ID) without locking.
//!
//! ## Initialization
//!
//! The BSP calls `init_cpu(0)` during early boot. Each AP calls
//! `init_cpu(n)` after transitioning to long mode. The CPU ID is a
//! sequential index assigned by the ACPI MADT enumeration.
//!
//! ## Access
//!
//! - `current_cpu_id()` reads the CPU ID from GSBASE (fast path).
//! - `current_per_cpu()` returns a mutable reference to the full struct.
//!
//! ## References
//!
//! - Intel SDM Vol. 3, Section 10.12.3: `IA32_GS_BASE` MSR (0xC0000101)
//! - Linux kernel `percpu` implementation (similar GSBASE technique)

use core::arch::asm;
use core::cell::UnsafeCell;

/// Wrapper around `UnsafeCell` that implements `Sync`.
///
/// Each CPU accesses only its own slot (identified by GSBASE), so there
/// is no data race. This is the standard pattern for per-CPU static data.
struct SyncUnsafeCell<T>(UnsafeCell<T>);

// SAFETY: Each CPU accesses only its own slot. No two CPUs ever access
// the same `SyncUnsafeCell` simultaneously. This is the canonical
// per-CPU data pattern used by the Linux kernel and Rust's std.
unsafe impl<T> Sync for SyncUnsafeCell<T> {}

impl<T> SyncUnsafeCell<T> {
    const fn new(val: T) -> Self {
        Self(UnsafeCell::new(val))
    }

    fn get(&self) -> *mut T {
        self.0.get()
    }
}

/// Maximum number of CPUs the kernel supports.
///
/// Matches `MAX_AP_COUNT` in `ap_start.rs` but is smaller for practical
/// use — the per-CPU array is in BSS and 8 CPUs is the target platform.
pub const MAX_CPUS: usize = 8;

/// MSR number for the kernel GS base register.
///
/// On `x86_64`, `IA32_GS_BASE` (MSR 0xC0000101) is used by the `SWAPGS`
/// instruction and can be read/written with `RDMSR`/`WRMSR`. Unlike
/// `FSBASE`, `GSBASE` is the conventional choice for per-CPU data in
/// kernel code.
const IA32_GS_BASE: u32 = 0xC000_0101;

/// Per-CPU data structure.
///
/// One instance exists for each logical CPU. Accessed via GSBASE so that
/// code running on any core can read its own state without locking.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PerCpuData {
    /// Sequential CPU index (0 = BSP, 1..N = APs).
    pub cpu_id: u32,
    /// Local APIC ID from the ACPI MADT (matches hardware).
    pub lapic_id: u32,
    /// ID of the task currently running on this CPU, or `None` if idle.
    pub current_task_id: Option<u64>,
    /// Whether this CPU is in the idle loop (no runnable tasks).
    pub idle: bool,
}

/// Per-CPU storage array.
///
/// Each slot is wrapped in `SyncUnsafeCell` to allow mutable access
/// through a shared reference. Each CPU only touches its own slot
/// (identified by GSBASE), so there is no aliasing at runtime.
static PERCPU: [SyncUnsafeCell<PerCpuData>; MAX_CPUS] = [
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
    SyncUnsafeCell::new(PerCpuData::new()),
];

impl PerCpuData {
    /// Create a zeroed per-CPU data structure.
    const fn new() -> Self {
        Self {
            cpu_id: 0,
            lapic_id: 0,
            current_task_id: None,
            idle: true,
        }
    }
}

/// Initialize per-CPU data for the given CPU.
///
/// Sets `IA32_GS_BASE` to point at `PERCPU[cpu_id]` and populates the
/// `cpu_id` field. Must be called once per CPU:
/// - BSP calls `init_cpu(0)` during early boot
/// - Each AP calls `init_cpu(n)` after entering long mode
///
/// # Panics
///
/// Panics if `cpu_id >= MAX_CPUS`.
///
/// # Safety
///
/// This function writes the `IA32_GS_BASE` MSR, which is a privileged
/// operation. It is safe to call from kernel code at any privilege level.
pub fn init_cpu(cpu_id: u32) {
    let idx = cpu_id as usize;
    assert!(
        idx < MAX_CPUS,
        "cpu_id exceeds MAX_CPUS — increase MAX_CPUS or fix MADT enumeration"
    );

    let ptr = PERCPU[idx].get();

    // SAFETY: We are writing to the per-CPU slot for `cpu_id`. This is
    // called exactly once per CPU during initialization, so there is no
    // concurrent access. The pointer points into the static PERCPU array
    // which lives for the entire program.
    unsafe {
        (*ptr).cpu_id = cpu_id;
    }

    // Write GSBASE MSR so that `current_cpu_id()` and `current_per_cpu()`
    // return data for this CPU.
    //
    // SAFETY: `IA32_GS_BASE` is a model-specific register that is always
    // present on `x86_64` CPUs. Writing it is a privileged operation that
    // is only done from kernel code during CPU initialization.
    unsafe {
        write_gs_base(ptr as u64);
    }
}

/// Read the current CPU's sequential ID from GSBASE.
///
/// This is the fast path for identifying which CPU the caller is running
/// on. Reads the `cpu_id` field at offset 0 of the `PerCpuData` struct
/// pointed to by `IA32_GS_BASE`.
///
/// # Returns
///
/// The sequential CPU index (0 = BSP, 1..N = APs).
///
/// When testing on the host, GSBASE is not initialized, so this
/// always returns 0 (the BSP).
#[must_use]
pub fn current_cpu_id() -> u32 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        // SAFETY: GSBASE was set by `init_cpu()` to point at a valid
        // `PerCpuData` in the PERCPU array. The `cpu_id` field is at offset 0
        // (guaranteed by `#[repr(C)]`). Reading from GSBASE-relative memory is
        // the standard per-CPU access pattern and is safe on any CPU.
        unsafe {
            let value: u32;
            asm!(
                "mov {:e}, dword ptr gs:[0]",
                out(reg) value,
                options(nomem, nostack, preserves_flags),
            );
            value
        }
    }
}

/// Get a mutable reference to the current CPU's per-CPU data.
///
/// Returns a `&'static mut PerCpuData` for the CPU currently executing
/// this code. The reference is valid for the lifetime of the program
/// because the PERCPU array is static.
///
/// # Safety contract
///
/// The caller must ensure that no other code on the same CPU holds a
/// concurrent reference to the same `PerCpuData`. Since each CPU has its
/// own slot and CPUs do not migrate between slots, this is inherently
/// safe in a single-threaded-per-core model.
#[must_use]
pub fn current_per_cpu() -> &'static mut PerCpuData {
    // SAFETY: GSBASE points at a valid `PerCpuData` in the PERCPU array.
    // Each CPU has its own slot, so no aliasing occurs. The array is
    // static, so the reference is 'static. Mutability is safe because each
    // CPU only accesses its own slot (no cross-CPU mutation).
    unsafe {
        let base = read_gs_base();
        &mut *(base as *mut PerCpuData)
    }
}

/// Write `IA32_GS_BASE` MSR.
///
/// # Safety
///
/// `value` must be a valid pointer to a `PerCpuData` in the PERCPU array.
/// The MSR is a privileged register; calling from user mode will fault.
unsafe fn write_gs_base(value: u64) {
    // SAFETY: The caller guarantees `value` is a valid pointer. WRMSR is
    // always available on x86_64. The low 32 bits go in EAX, high 32 in EDX.
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") IA32_GS_BASE,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Read `IA32_GS_BASE` MSR.
///
/// Returns the 64-bit value currently stored in the GS base register.
fn read_gs_base() -> u64 {
    // SAFETY: RDMSR for IA32_GS_BASE is always safe from kernel code.
    // The CPU always has this MSR. EAX gets low 32 bits, EDX gets high 32.
    unsafe {
        let low: u32;
        let high: u32;
        asm!(
            "rdmsr",
            in("ecx") IA32_GS_BASE,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
        u64::from(high) << 32 | u64::from(low)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_cpus_is_8() {
        assert_eq!(MAX_CPUS, 8);
    }

    #[test]
    fn test_percpu_data_size() {
        // PerCpuData should be small: 4+4+8+1 = 17 bytes, padded to 24.
        let size = core::mem::size_of::<PerCpuData>();
        assert!(size <= 32, "PerCpuData is too large: {} bytes", size);
    }

    #[test]
    fn test_percpu_data_default_values() {
        let data = PerCpuData::new();
        assert_eq!(data.cpu_id, 0);
        assert_eq!(data.lapic_id, 0);
        assert_eq!(data.current_task_id, None);
        assert!(data.idle);
    }

    #[test]
    fn test_ia32_gs_base_msr_value() {
        // IA32_GS_BASE is MSR 0xC0000101.
        assert_eq!(IA32_GS_BASE, 0xC000_0101);
    }

    #[test]
    fn test_percpu_array_has_max_cpus_slots() {
        assert_eq!(PERCPU.len(), MAX_CPUS);
    }

    /// `PerCpuData` field offsets must match `#[repr(C)]` layout.
    /// The assembly code reads `cpu_id` at offset 0 via `gs:[0]`.
    #[test]
    fn test_percpu_data_field_offsets() {
        use core::mem::offset_of;
        assert_eq!(offset_of!(PerCpuData, cpu_id), 0);
        assert_eq!(offset_of!(PerCpuData, lapic_id), 4);
    }

    /// `PerCpuData` is `Copy` (can be trivially duplicated).
    #[test]
    fn test_percpu_data_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<PerCpuData>();
    }

    /// `PerCpuData` is `Clone`.
    #[test]
    fn test_percpu_data_is_clone() {
        let data = PerCpuData::new();
        let cloned = data.clone();
        assert_eq!(cloned.cpu_id, data.cpu_id);
        assert_eq!(cloned.lapic_id, data.lapic_id);
    }

    /// `PerCpuData` can be modified through mutable access.
    #[test]
    fn test_percpu_data_modification() {
        let mut data = PerCpuData::new();
        data.cpu_id = 42;
        data.lapic_id = 7;
        data.current_task_id = Some(100);
        data.idle = false;

        assert_eq!(data.cpu_id, 42);
        assert_eq!(data.lapic_id, 7);
        assert_eq!(data.current_task_id, Some(100));
        assert!(!data.idle);
    }

    /// `PerCpuData` size must be at least 16 bytes (4+4+8 fields).
    #[test]
    fn test_percpu_data_minimum_size() {
        let size = core::mem::size_of::<PerCpuData>();
        assert!(size >= 16, "PerCpuData too small: {} bytes", size);
    }

    /// `PerCpuData` alignment should be at least 4 (for u32 fields).
    #[test]
    fn test_percpu_data_alignment() {
        let align = core::mem::align_of::<PerCpuData>();
        assert!(align >= 4, "PerCpuData alignment too small: {}", align);
    }

    /// `MAX_CPUS` must be a power of two (for efficient modulo in scheduler).
    #[test]
    fn test_max_cpus_power_of_two() {
        assert!(MAX_CPUS.is_power_of_two());
    }

    /// `IA32_GS_BASE` MSR address must be in the model-specific range.
    #[test]
    fn test_ia32_gs_base_in_msr_range() {
        // MSRs are in the range 0x0000_0000..=0xFFFF_FFFF.
        // IA32_GS_BASE = 0xC000_0101 is in the expected range.
        assert!(IA32_GS_BASE >= 0xC000_0000);
        assert!(IA32_GS_BASE <= 0xC000_1000);
    }

    /// `SyncUnsafeCell` correctly wraps and unwraps values.
    #[test]
    fn test_sync_unsafe_cell_get() {
        let cell = SyncUnsafeCell::new(42u32);
        let ptr = cell.get();
        // SAFETY: We have exclusive access to this cell in the test.
        unsafe {
            assert_eq!(*ptr, 42);
            *ptr = 100;
            assert_eq!(*ptr, 100);
        }
    }

    /// `current_cpu_id` returns 0 in test mode (no GSBASE set).
    #[test]
    fn test_current_cpu_id_in_test_mode() {
        assert_eq!(current_cpu_id(), 0);
    }
}
