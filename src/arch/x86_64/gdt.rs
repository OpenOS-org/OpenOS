//! Global Descriptor Table (GDT) and Task State Segment (TSS).
//!
//! The GDT layout for user-mode support:
//!
//!   Index 0: null descriptor (required by CPU)
//!   Index 1: kernel code segment (DPL 0, 64-bit)
//!   Index 2: kernel data segment (DPL 0, unused in long mode but required for SYSCALL)
//!   Index 3: user data segment (DPL 3, required for SYSCALL/SYSRET)
//!   Index 4: user code segment (DPL 3, 64-bit)
//!   Index 5: TSS descriptor (occupies two slots in 64-bit mode)
//!
//! SYSCALL/SYSRET segment convention (STAR MSR):
//!   STAR[32:47]  = kernel CS (selector for index 1)
//!   STAR[48:63]  = user CS (selector for index 4, SYSRET loads CS+16 and SS+8)

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// IST index 0: double-fault handler stack.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Kernel stack size for SYSCALL entry (RSP0 in TSS).
const KERNEL_STACK_SIZE: usize = 4096 * 8;

/// IST stack size for double-fault handler.
const IST_STACK_SIZE: usize = 4096 * 5;

lazy_static::lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();

        // RSP0: the stack pointer the CPU loads when transitioning from Ring 3
        // to Ring 0 (via SYSCALL, interrupt, or exception). This is the kernel
        // stack the syscall handler runs on.
        static mut KERNEL_STACK: [u8; KERNEL_STACK_SIZE] = [0; KERNEL_STACK_SIZE];
        let kernel_stack_top = VirtAddr::from_ptr(&raw const KERNEL_STACK) + KERNEL_STACK_SIZE as u64;
        tss.privilege_stack_table[0] = kernel_stack_top;

        // IST[0]: double-fault stack. Separate from RSP0 so a stack overflow
        // in the kernel stack doesn't corrupt the double-fault handler.
        static mut IST_STACK: [u8; IST_STACK_SIZE] = [0; IST_STACK_SIZE];
        let ist_stack_top = VirtAddr::from_ptr(&raw const IST_STACK) + IST_STACK_SIZE as u64;
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = ist_stack_top;

        tss
    };

    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();

        // Kernel segments (Ring 0)
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());

        // User segments (Ring 3). Order matters for SYSCALL/SYSRET:
        // user_data must be at selector (user_code - 8) because SYSRET
        // sets SS = CS + 8 (which is user_data).
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());

        // TSS (occupies two GDT slots in 64-bit mode)
        let tss_sel = gdt.append(Descriptor::tss_segment(&TSS));

        (
            gdt,
            Selectors {
                kernel_code,
                kernel_data,
                user_code,
                user_data,
                tss: tss_sel,
            },
        )
    };
}

/// Segment selectors cached for use in SYSCALL/SYSRET configuration and
/// privilege-level transitions.
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub user_data: SegmentSelector,
    pub tss: SegmentSelector,
}

/// Get the raw selector values for SYSCALL MSR configuration.
pub fn selectors() -> &'static Selectors {
    &GDT.1
}

/// Load the GDT, reload segment registers, and load the TSS.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();

    // SAFETY: Reloading CS with the kernel code selector. The CPU continues
    // using the old CS until a far jump or CS::set_reg. DS/ES/SS are set to
    // the kernel data segment — required for the CPU to accept memory accesses.
    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        DS::set_reg(GDT.1.kernel_data);
        ES::set_reg(GDT.1.kernel_data);
        SS::set_reg(GDT.1.kernel_data);
        load_tss(GDT.1.tss);
    }
}
