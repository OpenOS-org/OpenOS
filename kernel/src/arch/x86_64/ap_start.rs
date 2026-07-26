//! Application Processor (AP) startup for symmetric multiprocessing.
//!
//! Boots secondary CPUs from the BSP (Bootstrap Processor) using the
//! INIT-SIPI-SIPI sequence defined in the Intel SDM. Each AP executes
//! a real-mode trampoline at physical address 0x1000 that transitions
//! through protected mode to long mode, then calls `ap_main()` in Rust.
//!
//! ## Boot Sequence
//!
//! 1. BSP writes AP trampoline code to physical address 0x1000
//! 2. For each AP: send INIT IPI, wait 10ms, send SIPI twice
//! 3. AP starts in real mode at 0x1000, transitions to long mode
//! 4. AP calls `ap_main()`, sets up GDT/IDT, enables LAPIC
//! 5. AP signals ready via an atomic flag and enters idle loop
//!
//! ## References
//!
//! - Intel SDM Vol. 3, Section 8.4: Multiple-Processor (MP) Initialization
//! - Intel SDM Vol. 3, Section 10.6: ISS — Starting Application Processors

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::acpi::AcpiInfo;
use crate::memory::phys_to_virt;
use crate::serial_println;

// ============================================================================
// Constants
// ============================================================================

/// Physical address where the AP trampoline code is placed.
/// Must be below 1 MiB (real-mode addressable) and page-aligned.
const TRAMPOLINE_PHYS: u64 = 0x1000;

/// SIPI vector: trampoline address = vector * 0x1000.
/// With `TRAMPOLINE_PHYS = 0x1000`, the vector is 0x01.
const SIPI_VECTOR: u8 = 0x01;

/// Maximum number of APs the kernel supports.
const MAX_AP_COUNT: usize = 256;

/// AP readiness flags. Indexed by ACPI CPU index.
/// `AP_READY[i]` is set by AP `i` when it has completed initialization.
#[allow(clippy::declare_interior_mutable_const)]
const AP_READY_FALSE: AtomicBool = AtomicBool::new(false);
static AP_READY: [AtomicBool; MAX_AP_COUNT] = [AP_READY_FALSE; MAX_AP_COUNT];

/// Number of APs that have successfully started.
static AP_STARTED_COUNT: AtomicU32 = AtomicU32::new(0);

// ============================================================================
// Trampoline code (x86 real mode -> protected mode -> long mode)
// ============================================================================

/// Write a byte at `base + offset` and advance the offset.
///
/// # Safety
///
/// `base` must point to at least `offset + 1` writable bytes.
unsafe fn write_byte(base: *mut u8, offset: &mut isize, byte: u8) {
    unsafe {
        *base.add(usize::try_from(*offset).unwrap()) = byte;
    }
    *offset += 1;
}

/// Write a `u16` at `base + offset` (unaligned) and advance the offset by 2.
///
/// # Safety
///
/// `base` must point to at least `offset + 2` writable bytes.
unsafe fn write_u16(base: *mut u8, offset: &mut isize, value: u16) {
    unsafe {
        core::ptr::write_unaligned(
            base.add(usize::try_from(*offset).unwrap()).cast::<u16>(),
            value,
        );
    }
    *offset += 2;
}

/// Write a `u32` at `base + offset` (unaligned) and advance the offset by 4.
///
/// # Safety
///
/// `base` must point to at least `offset + 4` writable bytes.
unsafe fn write_u32(base: *mut u8, offset: &mut isize, value: u32) {
    unsafe {
        core::ptr::write_unaligned(
            base.add(usize::try_from(*offset).unwrap()).cast::<u32>(),
            value,
        );
    }
    *offset += 4;
}

/// Build the AP trampoline code at physical address 0x1000.
///
/// The trampoline is a small piece of 16-bit real-mode code that:
/// 1. Disables interrupts and sets up a real-mode stack
/// 2. Enables A20 line
/// 3. Loads a minimal GDT and switches to 32-bit protected mode
/// 4. Enables PAE and sets up a minimal page table for long mode
/// 5. Enables long mode and jumps to 64-bit code
/// 6. Calls the Rust entry point `ap_main()`
///
/// The code is written to physical memory via `phys_to_virt()`.
#[allow(clippy::too_many_lines)]
fn write_trampoline() {
    let virt = phys_to_virt(TRAMPOLINE_PHYS) as *mut u8;

    // GDT for the trampoline (placed at TRAMPOLINE_PHYS + 0x200).
    // Layout: null, code32, data32.
    let gdt_phys = TRAMPOLINE_PHYS + 0x200;
    let gdt_virt = phys_to_virt(gdt_phys) as *mut u8;

    // Page tables for the trampoline (placed at TRAMPOLINE_PHYS + 0x1000).
    // We identity-map the first 2 MiB using a single 2 MiB huge page.
    // PML4[0] -> PDPT, PDPT[0] -> PD (with PS bit for 2 MiB page).
    let pml4_phys = TRAMPOLINE_PHYS + 0x1000;
    let pdpt_phys = TRAMPOLINE_PHYS + 0x2000;
    let pd_phys = TRAMPOLINE_PHYS + 0x3000;

    let pml4_virt = phys_to_virt(pml4_phys) as *mut u64;
    let pdpt_virt = phys_to_virt(pdpt_phys) as *mut u64;
    let pd_virt = phys_to_virt(pd_phys) as *mut u64;

    // SAFETY: We are writing to physical memory that we own (the trampoline
    // region at 0x1000). The bootloader maps all physical memory. These
    // addresses are below 1 MiB and guaranteed to be available during AP boot.
    unsafe {
        // Zero out the trampoline region (code + GDT + page tables).
        core::ptr::write_bytes(virt, 0, 0x4000);

        // ── Write GDT ──
        // GDT is at gdt_phys (TRAMPOLINE_PHYS + 0x200).
        let gdt = gdt_virt;

        // Null descriptor (8 bytes of zeros).
        core::ptr::write_bytes(gdt, 0, 8);

        // Code32 descriptor (index 1): base=0, limit=4G, 32-bit, ring 0.
        // Access: present=1, DPL=0, type=code, readable, accessed.
        // Flags: granularity=1 (4K pages), 32-bit, limit high=0xF.
        let code32: u64 = 0x00CF_9A00_0000_FFFF;
        core::ptr::write_bytes(gdt.add(8), 0, 8);
        core::ptr::copy_nonoverlapping(core::ptr::addr_of!(code32).cast::<u8>(), gdt.add(8), 8);

        // Data32 descriptor (index 2): base=0, limit=4G, 32-bit, ring 0.
        // Access: present=1, DPL=0, type=data, writable, accessed.
        let data32: u64 = 0x00CF_9200_0000_FFFF;
        core::ptr::write_bytes(gdt.add(16), 0, 8);
        core::ptr::copy_nonoverlapping(core::ptr::addr_of!(data32).cast::<u8>(), gdt.add(16), 8);

        // ── Write page tables ──
        // PML4[0] -> PDPT (present, writable).
        core::ptr::write_volatile(pml4_virt, pdpt_phys | 0x03);
        // PDPT[0] -> PD (present, writable).
        core::ptr::write_volatile(pdpt_virt, pd_phys | 0x03);
        // PD[0] = 2 MiB huge page, identity-mapped (present, writable, PS).
        core::ptr::write_volatile(pd_virt, 0x0000_0083);

        // ── Write trampoline code ──
        // The code is emitted byte-by-byte into the trampoline buffer.
        let code = virt;
        let mut off: isize = 0;

        // ── 16-bit real mode entry ──
        write_byte(code, &mut off, 0xFA); // CLI
        write_byte(code, &mut off, 0x31); // XOR AX, AX
        write_byte(code, &mut off, 0xC0);
        write_byte(code, &mut off, 0x8E); // MOV DS, AX
        write_byte(code, &mut off, 0xD8);
        write_byte(code, &mut off, 0x8E); // MOV ES, AX
        write_byte(code, &mut off, 0xC0);
        write_byte(code, &mut off, 0x8E); // MOV SS, AX
        write_byte(code, &mut off, 0xD0);

        // MOV SP, 0x7C00 (real-mode stack below trampoline area)
        write_byte(code, &mut off, 0xBC);
        write_u16(code, &mut off, 0x7C00);

        // Enable A20 line via fast A20 gate (port 0x92).
        write_byte(code, &mut off, 0xE4); // IN AL, 0x92
        write_byte(code, &mut off, 0x92);
        write_byte(code, &mut off, 0x0C); // OR AL, 2
        write_byte(code, &mut off, 0x02);
        write_byte(code, &mut off, 0xE6); // OUT 0x92, AL
        write_byte(code, &mut off, 0x92);

        // Write GDT pointer at TRAMPOLINE_PHYS + 0x100.
        let gdt_ptr_phys = TRAMPOLINE_PHYS + 0x100;
        let gdt_ptr_virt = phys_to_virt(gdt_ptr_phys) as *mut u8;
        core::ptr::write_unaligned(gdt_ptr_virt.cast::<u16>(), 23u16);
        core::ptr::write_unaligned(gdt_ptr_virt.add(2).cast::<u32>(), gdt_phys as u32);

        // LGDT [0x100] (LGDT m16:32 = 0F 01 /1 mod=mem rm=disp16)
        write_byte(code, &mut off, 0x0F);
        write_byte(code, &mut off, 0x01);
        write_byte(code, &mut off, 0x16);
        write_u16(code, &mut off, 0x0100);

        // Enable protected mode: MOV EAX, CR0; OR EAX, 1; MOV CR0, EAX
        write_byte(code, &mut off, 0x0F); // MOV EAX, CR0
        write_byte(code, &mut off, 0x20);
        write_byte(code, &mut off, 0xC0);
        write_byte(code, &mut off, 0x66); // OR EAX, 1
        write_byte(code, &mut off, 0x83);
        write_byte(code, &mut off, 0xC8);
        write_byte(code, &mut off, 0x01);
        write_byte(code, &mut off, 0x0F); // MOV CR0, EAX
        write_byte(code, &mut off, 0x22);
        write_byte(code, &mut off, 0xC0);

        // Far jump to 32-bit code (CS = 0x08, code32 selector).
        // LJMP ptr16:32 = EA imm32 imm16
        let jmp_target = TRAMPOLINE_PHYS as u32 + (u32::try_from(off).unwrap() + 7);
        write_byte(code, &mut off, 0xEA);
        write_u32(code, &mut off, jmp_target);
        write_u16(code, &mut off, 0x08);

        // ── 32-bit protected mode code ──
        // MOV AX, 0x10 (data32 selector); MOV DS/ES/SS, AX
        write_byte(code, &mut off, 0x66);
        write_byte(code, &mut off, 0xB8);
        write_u16(code, &mut off, 0x10);
        write_byte(code, &mut off, 0x8E); // MOV DS, AX
        write_byte(code, &mut off, 0xD8);
        write_byte(code, &mut off, 0x8E); // MOV ES, AX
        write_byte(code, &mut off, 0xC0);
        write_byte(code, &mut off, 0x8E); // MOV SS, AX
        write_byte(code, &mut off, 0xD0);

        // Load PML4 into CR3: MOV EAX, pml4_phys; MOV CR3, EAX
        write_byte(code, &mut off, 0xB8);
        write_u32(code, &mut off, pml4_phys as u32);
        write_byte(code, &mut off, 0x0F); // MOV CR3, EAX
        write_byte(code, &mut off, 0x22);
        write_byte(code, &mut off, 0xD8);

        // Enable PAE in CR4: MOV EAX, CR4; OR EAX, 0x20; MOV CR4, EAX
        write_byte(code, &mut off, 0x0F); // MOV EAX, CR4
        write_byte(code, &mut off, 0x20);
        write_byte(code, &mut off, 0xE0);
        write_byte(code, &mut off, 0x66); // OR EAX, 0x20
        write_byte(code, &mut off, 0x83);
        write_byte(code, &mut off, 0xC8);
        write_byte(code, &mut off, 0x20);
        write_byte(code, &mut off, 0x0F); // MOV CR4, EAX
        write_byte(code, &mut off, 0x22);
        write_byte(code, &mut off, 0xE0);

        // Enable long mode in EFER MSR: MOV ECX, 0xC0000080; RDMSR;
        // OR EAX, 0x100 (LME); WRMSR
        write_byte(code, &mut off, 0xB9); // MOV ECX, 0xC0000080
        write_u32(code, &mut off, 0xC000_0080);
        write_byte(code, &mut off, 0x0F); // RDMSR
        write_byte(code, &mut off, 0x32);
        write_byte(code, &mut off, 0x66); // OR EAX, 0x100
        write_byte(code, &mut off, 0x0D);
        write_u32(code, &mut off, 0x0000_0100);
        write_byte(code, &mut off, 0x0F); // WRMSR
        write_byte(code, &mut off, 0x30);

        // Enable paging: MOV EAX, CR0; OR EAX, 0x80000000; MOV CR0, EAX
        write_byte(code, &mut off, 0x0F); // MOV EAX, CR0
        write_byte(code, &mut off, 0x20);
        write_byte(code, &mut off, 0xC0);
        write_byte(code, &mut off, 0x66); // OR EAX, 0x80000000
        write_byte(code, &mut off, 0x0D);
        write_u32(code, &mut off, 0x8000_0000);
        write_byte(code, &mut off, 0x0F); // MOV CR0, EAX
        write_byte(code, &mut off, 0x22);
        write_byte(code, &mut off, 0xC0);

        // Far jump to 64-bit code (CS = 0x08). Enters long mode.
        let jmp64_target = TRAMPOLINE_PHYS as u32 + (u32::try_from(off).unwrap() + 7);
        write_byte(code, &mut off, 0xEA);
        write_u32(code, &mut off, jmp64_target);
        write_u16(code, &mut off, 0x08);

        // ── 64-bit long mode code ──
        // Load ap_main address from TRAMPOLINE_PHYS + 0x0F0 (written by BSP).
        // MOV RAX, qword ptr ds:[0x0F0]  (REX.W 8B 04 25 F0 01 00 00)
        write_byte(code, &mut off, 0x48); // REX.W
        write_byte(code, &mut off, 0x8B); // MOV r64, r/m64
        write_byte(code, &mut off, 0x04); // ModRM: [SIB]
        write_byte(code, &mut off, 0x25); // SIB: scale=0, index=none, base=disp32
        write_u32(code, &mut off, 0x0000_01F0);

        // CALL RAX (FF D0)
        write_byte(code, &mut off, 0xFF);
        write_byte(code, &mut off, 0xD0);

        // HLT + JMP $-1 (infinite loop if ap_main returns)
        write_byte(code, &mut off, 0xF4);
        write_byte(code, &mut off, 0xEB);
        write_byte(code, &mut off, 0xFE);

        serial_println!(
            "[AP] Trampoline written to {:#x} ({} bytes of code)",
            TRAMPOLINE_PHYS,
            off
        );
    }
}

// ============================================================================
// BSP-side AP startup
// ============================================================================

/// Start all Application Processors discovered from ACPI.
///
/// For each AP (BSP excluded), this function:
/// 1. Writes the trampoline code to 0x1000
/// 2. Writes the `ap_main` function pointer at offset 0x0F0
/// 3. Sends INIT IPI and waits 10ms
/// 4. Sends SIPI twice (per Intel SDM recommendation)
/// 5. Waits for the AP to signal readiness via an atomic flag
///
/// # Arguments
///
/// * `acpi_info` -- Parsed ACPI information containing CPU LAPIC IDs.
pub fn start_aps(acpi_info: &AcpiInfo) {
    if acpi_info.cpu_count <= 1 {
        serial_println!("[AP] Single CPU system, no APs to start");
        return;
    }

    serial_println!(
        "[AP] Starting {} APs (BSP LAPIC ID={})",
        acpi_info.cpu_count - 1,
        acpi_info.cpu_lapic_ids[0]
    );

    // Write the trampoline code and page tables to physical memory.
    write_trampoline();

    // Write the ap_main function pointer at the fixed offset (0x0F0).
    let ap_main_ptr_virt = phys_to_virt(TRAMPOLINE_PHYS + 0x0F0) as *mut u64;
    // SAFETY: Writing to the trampoline region we just initialized.
    unsafe {
        core::ptr::write_volatile(ap_main_ptr_virt, ap_main_rust_entry as *const () as u64);
    }

    #[allow(clippy::needless_range_loop)]
    for i in 1..acpi_info.cpu_count as usize {
        let ap_lapic_id = acpi_info.cpu_lapic_ids[i];

        serial_println!("[AP] Starting AP {}: LAPIC ID {}", i, ap_lapic_id);

        // Reset the ready flag for this AP.
        AP_READY[i].store(false, Ordering::Release);

        // Step 1: Send INIT IPI.
        super::apic::send_init_ipi(ap_lapic_id);

        // Wait 10ms (INIT IPI requires a 10ms delay per the Intel SDM).
        busy_wait_ms(10);

        // Step 2: Send first SIPI.
        super::apic::send_sipi(ap_lapic_id, SIPI_VECTOR);

        // Wait 1ms after first SIPI.
        busy_wait_ms(1);

        // Step 3: Send second SIPI (per Intel SDM: send SIPI twice).
        super::apic::send_sipi(ap_lapic_id, SIPI_VECTOR);

        // Wait 1ms after second SIPI.
        busy_wait_ms(1);

        // Step 4: Wait for AP to signal ready (timeout ~1 second).
        let mut timeout = 1000;
        while !AP_READY[i].load(Ordering::Acquire) && timeout > 0 {
            busy_wait_ms(1);
            timeout -= 1;
        }

        if AP_READY[i].load(Ordering::Acquire) {
            serial_println!(
                "[AP] AP {} (LAPIC ID {}) started successfully",
                i,
                ap_lapic_id
            );
        } else {
            serial_println!(
                "[AP] WARNING: AP {} (LAPIC ID {}) did not signal ready (timeout)",
                i,
                ap_lapic_id
            );
        }
    }

    let started = AP_STARTED_COUNT.load(Ordering::Acquire);
    serial_println!(
        "[AP] {} of {} APs started",
        started,
        acpi_info.cpu_count - 1
    );
}

/// Busy-wait for approximately `ms` milliseconds.
///
/// This is a rough calibration -- the actual delay depends on CPU speed.
/// We use a conservative spin count that works on modern hardware.
fn busy_wait_ms(ms: u32) {
    // Conservative estimate: ~500k iterations per millisecond on a ~1 GHz CPU.
    const ITERS_PER_MS: u64 = 500_000;
    let total = u64::from(ms) * ITERS_PER_MS;
    for _ in 0..total {
        core::hint::spin_loop();
    }
}

// ============================================================================
// AP entry point (called from trampoline)
// ============================================================================

/// Rust entry point for Application Processors.
///
/// Called from the 64-bit trampoline code after the AP has transitioned
/// to long mode. This function:
/// 1. Sets up the GDT and IDT for this AP
/// 2. Enables the Local APIC
/// 3. Sets per-CPU data (`cpu_id`)
/// 4. Signals readiness to the BSP
/// 5. Enters an idle loop (HLT)
fn ap_main_rust_entry() -> ! {
    let ap_index = AP_STARTED_COUNT.fetch_add(1, Ordering::SeqCst) as usize + 1;

    serial_println!("[AP] AP {} entering ap_main", ap_index);

    // Set up the GDT for this AP. We load the same GDT the BSP uses.
    super::gdt::init();

    // Set up the IDT for this AP. The BSP's IDT is static, so we can
    // load it directly -- all APs share the same interrupt handlers.
    super::interrupts::init_idt();

    // Enable the Local APIC for this AP.
    super::apic::init(0xFEE0_0000);

    let lapic_id = super::apic::read_apic_id();
    serial_println!("[AP] AP {} initialized, LAPIC ID={}", ap_index, lapic_id);

    // Signal readiness to the BSP.
    if ap_index < MAX_AP_COUNT {
        AP_READY[ap_index].store(true, Ordering::Release);
    }

    serial_println!("[AP] AP {} ready, entering idle loop", ap_index);

    // Enable interrupts so this AP can receive IPIs and handle timers.
    x86_64::instructions::interrupts::enable();

    // Idle loop: HLT until interrupted, then repeat.
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trampoline_address_is_page_aligned() {
        assert_eq!(TRAMPOLINE_PHYS % 0x1000, 0);
    }

    #[test]
    fn test_trampoline_address_below_1mib() {
        // Real-mode trampoline must be below 1 MiB.
        assert!(TRAMPOLINE_PHYS < 0x0010_0000);
    }

    #[test]
    fn test_sipi_vector_matches_trampoline_address() {
        // SIPI vector * 0x1000 must equal the trampoline address.
        assert_eq!(u64::from(SIPI_VECTOR) * 0x1000, TRAMPOLINE_PHYS);
    }

    #[test]
    fn test_max_ap_count() {
        assert_eq!(MAX_AP_COUNT, 256);
    }

    #[test]
    fn test_ap_ready_array_initialized_to_false() {
        for i in 0..10 {
            assert!(!AP_READY[i].load(Ordering::Relaxed));
        }
    }
}
