//! Global Descriptor Table (GDT) and Task State Segment (TSS).
//!
//! On `x86_64`, the GDT is vestigial in long mode — segmentation is disabled and
//! all segments cover the full flat address space. We still need it for:
//!   - A kernel code segment (CS must point to a valid 64-bit code descriptor
//!     for `syscall`/`sysret` and privilege-level transitions to work).
//!   - A TSS descriptor, because the CPU reads the TSS to locate the interrupt
//!     stack table (IST) entries used by the double-fault handler.

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// IST index 0 is reserved for the double-fault handler.
///
/// A double fault occurs when the CPU faults while calling a fault handler —
/// typically caused by a stack overflow that corrupts the exception frame. The
/// IST mechanism gives the double-fault handler a fresh, pre-allocated stack
/// so it can execute even when the current stack is corrupted.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static::lazy_static! {
    /// Task State Segment. On x86_64 the TSS is used only for stack switching:
    /// IST entries for exception stacks, and ring-0 stack for privilege transitions.
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            // 20 KiB stack — generous for a fault handler that may need to
            // print a backtrace and halt. Must be static so the address is
            // valid for the lifetime of the kernel.
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            // `&raw const` avoids creating a shared reference to a `static mut`,
            // which is UB under Rust's aliasing rules. We only need the address.
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            // TSS expects the stack *top* (highest address), since x86 stacks grow down.
            stack_start + STACK_SIZE as u64
        };
        tss
    };

    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
        (
            gdt,
            Selectors { code_selector, tss_selector },
        )
    };
}

/// Cached segment selectors from the GDT. We need these to reload CS and TR
/// after loading the new GDT — the CPU doesn't update them automatically.
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

/// Load the GDT, reload CS, and load the TSS.
///
/// # Safety contract
/// This function must be called exactly once, before any interrupt or
/// privilege-level transition occurs. Calling it twice with different GDTs
/// would leave stale segment selectors in use.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();

    // SAFETY: We reload CS to point to our kernel code segment descriptor in
    // the new GDT. The `load` above only updates the GDTR register — the CPU
    // still uses the old CS value until we do a far jump or `CS::set_reg`.
    // load_tss updates the TR register so the CPU can find the TSS for IST.
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
    }
}
