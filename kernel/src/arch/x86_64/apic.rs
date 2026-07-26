//! Local APIC (Advanced Programmable Interrupt Controller) driver.
//!
//! The Local APIC is integrated into each CPU core and handles:
//! - Receiving interrupts from the I/O APIC and other cores (IPIs)
//! - Timer interrupts for preemption
//! - Inter-processor interrupts for multi-core coordination
//!
//! All register access is via MMIO at the physical address provided by
//! the ACPI MADT (typically `0xFEE0_0000`). We translate to virtual
//! using `phys_to_virt()`.
//!
//! ## References
//!
//! - Intel SDM Vol. 3, Chapter 10: Advanced Programmable Interrupt Controller
//! - APIC register offsets are architecturally defined (not implementation-specific)

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::phys_to_virt;
use crate::serial_println;

// ============================================================================
// LAPIC register offsets (architecturally defined in Intel SDM Vol. 3, §10.4)
// ============================================================================

/// Local APIC ID register (read-only). Identifies this core's APIC ID.
const LAPIC_ID: u64 = 0x020;

/// Local APIC Version register (read-only).
const LAPIC_VERSION: u64 = 0x030;

/// Task Priority Register (TPR). Setting to 0 accepts all interrupt priorities.
const LAPIC_TPR: u64 = 0x080;

/// Spurious Interrupt Vector Register. Bit 8 enables the APIC; bits 0-7 set
/// the spurious interrupt vector number.
const LAPIC_SVR: u64 = 0x0F0;

/// End-of-Interrupt register. Writing any value acknowledges the current interrupt.
const LAPIC_EOI: u64 = 0x0B0;

/// Error Status Register. Reports errors detected by the local APIC.
const LAPIC_ESR: u64 = 0x280;

/// Interrupt Command Register (ICR), low dword. Controls IPI delivery.
const LAPIC_ICR_LOW: u64 = 0x300;

/// Interrupt Command Register (ICR), high dword. Contains the destination APIC ID.
const LAPIC_ICR_HIGH: u64 = 0x310;

/// LVT Timer Register. Configures the local APIC timer.
const LAPIC_LVT_TIMER: u64 = 0x320;

/// LVT LINT0 Register. Local interrupt input 0.
const LAPIC_LVT_LINT0: u64 = 0x350;

/// LVT LINT1 Register. Local interrupt input 1.
const LAPIC_LVT_LINT1: u64 = 0x360;

/// LVT Error Register. Configures error interrupt delivery.
const LAPIC_LVT_ERROR: u64 = 0x370;

/// Timer Initial Count Register. The countdown starting value.
const LAPIC_TIMER_ICR: u64 = 0x380;

/// Timer Current Count Register (read-only). The current countdown value.
const LAPIC_TIMER_CCR: u64 = 0x390;

/// Timer Divide Configuration Register. Controls the timer frequency divider.
const LAPIC_TIMER_DCR: u64 = 0x3E0;

// ============================================================================
// ICR delivery modes (Intel SDM Vol. 3, Table 10-3)
// ============================================================================

/// Fixed delivery mode: deliver the interrupt to the destination.
const ICR_DELIVERY_FIXED: u32 = 0x000;

/// INIT delivery mode: sends an INIT IPI to reset the destination core.
const ICR_DELIVERY_INIT: u32 = 0x500;

/// Start-Up delivery mode: sends a SIPI to start execution at a vector address.
const ICR_DELIVERY_SIPI: u32 = 0x600;

// ============================================================================
// ICR destination shorthand (Intel SDM Vol. 3, Table 10-5)
// ============================================================================

/// No shorthand: use the destination field in the ICR high dword.
const ICR_DEST_NONE: u32 = 0x000;

// ============================================================================
// ICR status flags
// ============================================================================

/// ICR low bit 12: delivery status (1 = pending, 0 = idle).
const ICR_STATUS_PENDING: u32 = 1 << 12;

/// ICR low bit 14: level (1 = assert for INIT/SIPI).
const ICR_LEVEL_ASSERT: u32 = 1 << 14;

/// ICR low bit 15: trigger mode (0 = edge, 1 = level). SIPI requires edge.
const ICR_TRIGGER_LEVEL: u32 = 1 << 15;

// ============================================================================
// SVR flags
// ============================================================================

/// SVR bit 8: APIC software enable.
const SVR_ENABLE: u32 = 1 << 8;

/// Spurious interrupt vector number. Conventionally 0xFF.
const SPURIOUS_VECTOR: u32 = 0xFF;

// ============================================================================
// LVT timer modes
// ============================================================================

/// LVT timer: one-shot mode (bits 17:16 = 0b00).
const LVT_TIMER_ONESHOT: u32 = 0x00_0000;

/// LVT timer: periodic mode (bits 17:16 = 0b01).
const LVT_TIMER_PERIODIC: u32 = 0x02_0000;

/// LVT timer: TSC-deadline mode (bits 17:16 = 0b10).
const LVT_TIMER_TSC_DEADLINE: u32 = 0x04_0000;

/// LVT bit 16: masked (1 = interrupt delivery suppressed).
const LVT_MASKED: u32 = 1 << 16;

/// Timer divider: divide by 1.
const TIMER_DIV_1: u32 = 0x0B;

/// Timer divider: divide by 16.
const TIMER_DIV_16: u32 = 0x03;

/// Timer divider: divide by 128.
const TIMER_DIV_128: u32 = 0x0A;

// ============================================================================
// PIC disable constants (write 0xFF to mask all IRQs)
// ============================================================================

/// PIC data port offset from command port.
const PIC_DATA_OFFSET: u16 = 1;

/// Master PIC command port.
const PIC_MASTER_CMD: u16 = 0x20;

/// Slave PIC command port.
const PIC_SLAVE_CMD: u16 = 0xA0;

/// PIC mask value: all IRQs masked.
const PIC_MASK_ALL: u8 = 0xFF;

/// IPI delivery timeout: maximum spin iterations waiting for ICR delivery.
const ICR_TIMEOUT_SPINS: u32 = 1_000_000;

/// Calibrated LAPIC timer frequency. Set by `calibrate_timer()`, zero if
/// calibration has not been performed.
static LAPIC_TIMER_FREQ: AtomicU64 = AtomicU64::new(0);

/// Virtual address of the LAPIC MMIO registers.
static LAPIC_BASE_VIRT: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Register read/write
// ============================================================================

/// Read a 32-bit LAPIC register at the given offset.
///
/// # Safety
///
/// The LAPIC base must have been initialized via `init()` so the MMIO
/// address is valid. The caller must ensure the offset is a valid
/// LAPIC register offset (4-byte aligned).
fn reg_read(offset: u64) -> u32 {
    let base = LAPIC_BASE_VIRT.load(Ordering::Acquire);
    assert!(base != 0, "LAPIC not initialized — call init() first");

    // SAFETY: `base` was set from the validated physical address via
    // `phys_to_virt`, and the offset is a well-known LAPIC register offset.
    // The bootloader maps all physical memory, so the MMIO region is accessible.
    // Volatile reads prevent the compiler from eliding the hardware access.
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

/// Write a 32-bit LAPIC register at the given offset.
///
/// # Safety
///
/// Same as `reg_read`. The caller must ensure the offset is a valid
/// writable LAPIC register offset.
fn reg_write(offset: u64, value: u32) {
    let base = LAPIC_BASE_VIRT.load(Ordering::Acquire);
    assert!(base != 0, "LAPIC not initialized — call init() first");

    // SAFETY: Same as `reg_read` — valid MMIO address, aligned access,
    // bootloader guarantees the mapping.
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
}

// ============================================================================
// Public API
// ============================================================================

/// Initialize the Local APIC.
///
/// This function:
/// 1. Disables the legacy 8259 PIC by masking all IRQs
/// 2. Maps the LAPIC physical address to a virtual address
/// 3. Enables the LAPIC via the Spurious Interrupt Vector Register
/// 4. Sets the Task Priority Register to 0 (accept all interrupts)
/// 5. Masks LVT entries (LINT0, LINT1, Error, Timer) until configured
///
/// # Arguments
///
/// * `lapic_phys_addr` — Physical address of the LAPIC MMIO region,
///   typically from the ACPI MADT (default `0xFEE0_0000`).
///
/// # Panics
///
/// Panics if `phys_to_virt()` has not been initialized (i.e.,
/// `set_physical_memory_offset()` was not called during early boot).
pub fn init(lapic_phys_addr: u64) {
    serial_println!("[LAPIC] Initializing LAPIC at phys {:#x}", lapic_phys_addr);

    // Disable the legacy 8259 PIC. With the LAPIC active, the PIC is
    // unused and its IRQs would conflict with the LAPIC interrupt vectors.
    disable_pic();

    // Translate physical address to virtual and store globally.
    let base_virt = phys_to_virt(lapic_phys_addr);
    LAPIC_BASE_VIRT.store(base_virt, Ordering::Release);

    // Enable the LAPIC by setting the software enable bit in the SVR.
    // The spurious vector is 0xFF (convention). Bit 8 (SVR_ENABLE) turns
    // the APIC on; without this, it will not deliver any interrupts.
    let mut svr = SPURIOUS_VECTOR;
    svr |= SVR_ENABLE;
    reg_write(LAPIC_SVR, svr);

    // Set Task Priority Register to 0 so the LAPIC accepts all interrupt
    // priorities. A non-zero TPR would mask interrupts below that priority.
    reg_write(LAPIC_TPR, 0);

    // Mask all LVT entries until the caller explicitly configures them.
    // An unmasked LVT entry with a zero vector would fire spuriously.
    reg_write(LAPIC_LVT_TIMER, LVT_MASKED);
    reg_write(LAPIC_LVT_LINT0, LVT_MASKED);
    reg_write(LAPIC_LVT_LINT1, LVT_MASKED);
    reg_write(LAPIC_LVT_ERROR, LVT_MASKED);

    // Clear any pending errors.
    reg_write(LAPIC_ESR, 0);

    // Read back the APIC ID for logging.
    let apic_id = read_apic_id();
    let version = reg_read(LAPIC_VERSION) & 0xFF;

    serial_println!(
        "[LAPIC] ID={}, version={:#x}, SVR={:#x}",
        apic_id,
        version,
        reg_read(LAPIC_SVR)
    );
}

/// Read the Local APIC ID of the current core.
///
/// Returns the 8-bit APIC ID from the LAPIC ID register. This is the
/// ID assigned by hardware/firmware and matches the IDs discovered in
/// the ACPI MADT.
pub fn read_apic_id() -> u8 {
    // The APIC ID is in bits 24:31 of the LAPIC ID register.
    // We shift right by 24 to extract the 8-bit ID.
    ((reg_read(LAPIC_ID) >> 24) & 0xFF) as u8
}

/// Send End-of-Interrupt to the LAPIC.
///
/// Must be called at the end of every interrupt handler that is routed
/// through the LAPIC. Writing any value to the EOI register acknowledges
/// the current interrupt, allowing the LAPIC to deliver the next one.
/// Failure to send EOI will block all interrupts of equal or lower priority.
pub fn send_eoi() {
    reg_write(LAPIC_EOI, 0);
}

/// Send a generic IPI (Inter-Processor Interrupt) to a specific CPU.
///
/// Delivers the given interrupt vector to the CPU identified by `apic_id`.
/// Uses fixed delivery mode (the standard for software IPIs).
///
/// # Arguments
///
/// * `apic_id` — Local APIC ID of the target CPU.
/// * `vector` — Interrupt vector number to deliver (0x10..=0xFF).
pub fn send_ipi(apic_id: u8, vector: u8) {
    // Write the destination APIC ID to the ICR high dword.
    // Bits 24:31 contain the destination field for physical addressing.
    reg_write(LAPIC_ICR_HIGH, u32::from(apic_id) << 24);

    // Write the ICR low dword with fixed delivery, edge-triggered, assert.
    let icr_low = u32::from(vector) | ICR_DELIVERY_FIXED | ICR_LEVEL_ASSERT | ICR_DEST_NONE;
    reg_write(LAPIC_ICR_LOW, icr_low);

    // Wait for delivery to complete. The ICR's delivery status bit (bit 12)
    // is 1 while the IPI is pending and clears when the LAPIC sends it.
    wait_for_icr_delivery();
}

/// Send an INIT IPI to a target CPU.
///
/// The INIT IPI puts the target CPU into the wait-for-SIPI state, which
/// is the first step in the AP boot sequence. After INIT, the target CPU
/// waits for a Startup IPI (SIPI) to begin execution.
///
/// # Arguments
///
/// * `apic_id` — Local APIC ID of the target CPU.
pub fn send_init_ipi(apic_id: u8) {
    // Destination APIC ID in the high dword.
    reg_write(LAPIC_ICR_HIGH, u32::from(apic_id) << 24);

    // INIT delivery mode, level-triggered, assert.
    // The INIT IPI uses level trigger mode per the Intel SDM.
    let icr_low = ICR_DELIVERY_INIT | ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL | ICR_DEST_NONE;
    reg_write(LAPIC_ICR_LOW, icr_low);

    wait_for_icr_delivery();
}

/// Send a Startup IPI (SIPI) to a target CPU.
///
/// The SIPI causes the target CPU to begin executing at the real-mode
/// address `vector * 0x1000` (4 KiB boundary). This is the second step
/// in the AP boot sequence, after an INIT IPI.
///
/// # Arguments
///
/// * `apic_id` — Local APIC ID of the target CPU.
/// * `vector` — Startup vector (e.g., `0x08` for address `0x8000`).
pub fn send_sipi(apic_id: u8, vector: u8) {
    // Destination APIC ID in the high dword.
    reg_write(LAPIC_ICR_HIGH, u32::from(apic_id) << 24);

    // SIPI delivery mode, edge-triggered, assert.
    let icr_low = u32::from(vector) | ICR_DELIVERY_SIPI | ICR_LEVEL_ASSERT | ICR_DEST_NONE;
    reg_write(LAPIC_ICR_LOW, icr_low);

    wait_for_icr_delivery();
}

/// Start the LAPIC timer in periodic mode.
///
/// Configures the LAPIC timer to fire at a fixed interval. The timer
/// decrements from an initial count at the calibrated frequency. When
/// it reaches zero, it generates an interrupt and reloads the initial
/// count.
///
/// # Arguments
///
/// * `interval_us` — Timer interval in microseconds.
/// * `vector` — Interrupt vector number for the timer interrupt.
///
/// # Panics
///
/// Panics if `calibrate_timer()` has not been called (timer frequency is zero).
pub fn start_timer(interval_us: u32, vector: u8) {
    let freq = LAPIC_TIMER_FREQ.load(Ordering::Acquire);
    assert!(
        freq != 0,
        "LAPIC timer not calibrated — call calibrate_timer() first"
    );

    // Set the timer divider to 1 (divide-by-1) for maximum resolution.
    // The LAPIC timer counts at `freq` ticks per second with divider=1.
    reg_write(LAPIC_TIMER_DCR, TIMER_DIV_1);

    // Calculate the initial count from the interval and calibrated frequency.
    // count = freq_hz * interval_us / 1_000_000
    let count = u64::from(interval_us) * freq / 1_000_000;
    let count = count.max(1); // At least 1 tick to avoid immediate expiry.
    reg_write(LAPIC_TIMER_ICR, count as u32);

    // Configure the LVT timer entry: periodic mode, vector, not masked.
    let lvt = u32::from(vector) | LVT_TIMER_PERIODIC;
    reg_write(LAPIC_LVT_TIMER, lvt);

    serial_println!(
        "[LAPIC] Timer started: {} us interval, {} ticks, vector {:#x}",
        interval_us,
        count,
        vector
    );
}

/// Start the LAPIC timer in one-shot mode.
///
/// The timer counts down from `initial_count` and fires interrupt
/// `vector` once when it reaches zero.
///
/// # Arguments
///
/// * `initial_count` — Number of timer ticks before the interrupt fires.
/// * `vector` — Interrupt vector number for the timer interrupt.
pub fn start_timer_oneshot(initial_count: u32, vector: u8) {
    // Set divider to 1 for highest resolution.
    reg_write(LAPIC_TIMER_DCR, TIMER_DIV_1);

    // Configure LVT: one-shot, not masked, specified vector.
    let lvt = u32::from(vector) | LVT_TIMER_ONESHOT;
    reg_write(LAPIC_LVT_TIMER, lvt);

    // Set the initial count to start counting.
    reg_write(LAPIC_TIMER_ICR, initial_count);
}

/// Stop the LAPIC timer by masking the timer LVT entry.
pub fn stop_timer() {
    let current = reg_read(LAPIC_LVT_TIMER);
    reg_write(LAPIC_LVT_TIMER, current | LVT_MASKED);
    // Zero the initial count so it does not fire on unmask.
    reg_write(LAPIC_TIMER_ICR, 0);
}

/// Read the current timer count (remaining ticks).
pub fn timer_current_count() -> u32 {
    reg_read(LAPIC_TIMER_CCR)
}

/// Calibrate the LAPIC timer frequency using the PIT as a reference.
///
/// The PIT (Programmable Interval Timer) runs at a known frequency of
/// ~1.193182 MHz. We configure the PIT for a known delay, measure how
/// many LAPIC timer ticks elapse, and compute the LAPIC frequency.
///
/// This function must be called before `start_timer()`.
///
/// # Returns
///
/// The calibrated LAPIC timer frequency in Hz.
///
/// # Implementation Details
///
/// Uses PIT channel 2 in one-shot mode (hardware-dependent, port 0x42).
/// The PIT is not affected by the PIC mask since we read it directly.
pub fn calibrate_timer() -> u64 {
    use x86_64::instructions::port::Port;

    // PIT frequency constant (Intel 8253/8254 reference clock).
    const PIT_FREQ_HZ: u32 = 1_193_182;

    // Calibration interval: 10 ms (10,000 us).
    const CALIBRATION_US: u32 = 10_000;

    // PIT channel 2 ports.
    const PIT_CH2_DATA: u16 = 0x42;
    const PIT_CMD: u16 = 0x43;
    const PORT_B: u16 = 0x61;

    // PIT command: channel 2, access lo/hi, one-shot mode (mode 0).
    const PIT_CMD_CH2_ONESHOT: u8 = 0xB2;

    // Calculate the PIT count for the calibration interval.
    // count = freq_hz * interval_us / 1_000_000
    let pit_count = (u64::from(PIT_FREQ_HZ) * u64::from(CALIBRATION_US) / 1_000_000) as u16;

    // Configure LAPIC timer: one-shot, masked (we only read the count
    // register, we do not need interrupts), divider=1.
    reg_write(LAPIC_TIMER_DCR, TIMER_DIV_1);
    reg_write(LAPIC_LVT_TIMER, LVT_MASKED);

    // Set a large initial count so it does not expire during calibration.
    reg_write(LAPIC_TIMER_ICR, 0xFFFF_FFFF);

    // Read the starting LAPIC timer count before the PIT timing window.
    let lapic_start = reg_read(LAPIC_TIMER_CCR);

    // SAFETY: We are configuring PIT channel 2 and port B for calibration.
    // These are standard PC I/O ports. Port B bit 0 gates PIT channel 2.
    unsafe {
        let mut port_b: Port<u8> = Port::new(PORT_B);
        let mut pit_cmd: Port<u8> = Port::new(PIT_CMD);
        let mut pit_data: Port<u8> = Port::new(PIT_CH2_DATA);

        // Stop channel 2: clear bit 0 of port B.
        let pb = port_b.read();
        port_b.write(pb & 0xFC); // Clear bits 0 (gate) and 1 (speaker).

        // Program PIT channel 2 for one-shot mode.
        pit_cmd.write(PIT_CMD_CH2_ONESHOT);
        pit_data.write(pit_count as u8); // Low byte.
        pit_data.write((pit_count >> 8) as u8); // High byte.

        // Start PIT channel 2: set bit 0 of port B (gate high).
        port_b.write(pb | 0x01);

        // Spin until PIT channel 2 output (port B bit 5) goes high,
        // indicating the countdown is complete.
        while port_b.read() & 0x20 == 0 {
            // Wait for PIT to finish.
        }

        // Stop PIT channel 2 again.
        port_b.write(pb & 0xFC);
    }

    // Read the ending LAPIC timer count.
    let lapic_end = reg_read(LAPIC_TIMER_CCR);

    // The LAPIC counts down, so start > end.
    let elapsed_ticks = lapic_start.wrapping_sub(lapic_end);

    // Compute frequency: ticks / seconds = ticks / (calibration_us / 1_000_000).
    let freq = u64::from(elapsed_ticks) * 1_000_000 / u64::from(CALIBRATION_US);

    LAPIC_TIMER_FREQ.store(freq, Ordering::Release);

    serial_println!(
        "[LAPIC] Timer calibrated: {} Hz ({} ticks in {} us)",
        freq,
        elapsed_ticks,
        CALIBRATION_US
    );

    freq
}

/// Read the LAPIC error status register.
///
/// Returns the contents of the Error Status Register (ESR). The ESR
/// captures errors detected by the local APIC (e.g., send checksum
/// error, receive checksum error, redirectable IPI).
pub fn read_error_status() -> u32 {
    // The ESR must be written with 0 before reading to latch the
    // current error state (Intel SDM Vol. 3, §10.5.3).
    reg_write(LAPIC_ESR, 0);
    reg_read(LAPIC_ESR)
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Disable the legacy 8259 PIC by masking all IRQ lines.
///
/// With the LAPIC active, the PIC is unused. Leaving it enabled could
/// cause conflicting interrupt deliveries if the PIC and LAPIC routes
/// overlap.
fn disable_pic() {
    // SAFETY: Writing 0xFF to the PIC data ports masks all IRQs.
    // The PIC command/data ports (0x20/0x21 for master, 0xA0/0xA1 for slave)
    // are standard PC hardware. This is safe because we are only masking
    // interrupts, not reprogramming the PIC.
    unsafe {
        let mut master_data: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(PIC_MASTER_CMD + PIC_DATA_OFFSET);
        let mut slave_data: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(PIC_SLAVE_CMD + PIC_DATA_OFFSET);
        master_data.write(PIC_MASK_ALL);
        slave_data.write(PIC_MASK_ALL);
    }
    serial_println!("[LAPIC] Legacy 8259 PIC disabled");
}

/// Spin until the ICR delivery status bit clears.
///
/// The ICR's bit 12 (delivery status) is 1 while an IPI is pending
/// and 0 when the LAPIC has accepted it. We spin-check this bit to
/// avoid issuing a new IPI before the previous one has been sent.
///
/// Times out after `ICR_TIMEOUT_SPINS` iterations to prevent an
/// infinite hang on hardware bugs.
fn wait_for_icr_delivery() {
    let mut spins = 0u32;
    while reg_read(LAPIC_ICR_LOW) & ICR_STATUS_PENDING != 0 {
        spins += 1;
        if spins >= ICR_TIMEOUT_SPINS {
            serial_println!("[LAPIC] WARNING: ICR delivery timeout");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_offsets_are_4_byte_aligned() {
        // All LAPIC registers must be 4-byte aligned (32-bit MMIO).
        assert_eq!(LAPIC_ID % 4, 0);
        assert_eq!(LAPIC_VERSION % 4, 0);
        assert_eq!(LAPIC_TPR % 4, 0);
        assert_eq!(LAPIC_SVR % 4, 0);
        assert_eq!(LAPIC_EOI % 4, 0);
        assert_eq!(LAPIC_ESR % 4, 0);
        assert_eq!(LAPIC_ICR_LOW % 4, 0);
        assert_eq!(LAPIC_ICR_HIGH % 4, 0);
        assert_eq!(LAPIC_LVT_TIMER % 4, 0);
        assert_eq!(LAPIC_LVT_LINT0 % 4, 0);
        assert_eq!(LAPIC_LVT_LINT1 % 4, 0);
        assert_eq!(LAPIC_LVT_ERROR % 4, 0);
        assert_eq!(LAPIC_TIMER_ICR % 4, 0);
        assert_eq!(LAPIC_TIMER_CCR % 4, 0);
        assert_eq!(LAPIC_TIMER_DCR % 4, 0);
    }

    #[test]
    fn test_icr_delivery_constants_do_not_overlap() {
        // Ensure the three delivery modes have distinct bit patterns.
        assert_ne!(ICR_DELIVERY_FIXED, ICR_DELIVERY_INIT);
        assert_ne!(ICR_DELIVERY_FIXED, ICR_DELIVERY_SIPI);
        assert_ne!(ICR_DELIVERY_INIT, ICR_DELIVERY_SIPI);
    }

    #[test]
    fn test_spurious_vector() {
        // The spurious vector must be in the valid IDT range (0x10..=0xFF).
        // Convention is 0xFF.
        assert_eq!(SPURIOUS_VECTOR, 0xFF);
        assert!(SPURIOUS_VECTOR >= 0x10);
    }

    #[test]
    fn test_svr_enable_bit() {
        // SVR bit 8 is the APIC software enable flag.
        assert_eq!(SVR_ENABLE, 0x100);
    }

    #[test]
    fn test_pic_mask_all() {
        // Masking all 8 IRQ lines means all bits set.
        assert_eq!(PIC_MASK_ALL, 0xFF);
    }

    #[test]
    fn test_timer_modes_are_distinct() {
        assert_ne!(LVT_TIMER_ONESHOT, LVT_TIMER_PERIODIC);
        assert_ne!(LVT_TIMER_ONESHOT, LVT_TIMER_TSC_DEADLINE);
        assert_ne!(LVT_TIMER_PERIODIC, LVT_TIMER_TSC_DEADLINE);
    }

    #[test]
    fn test_lvt_masked_bit() {
        // Bit 16 of the LVT entry masks the interrupt.
        assert_eq!(LVT_MASKED, 1 << 16);
    }
}
