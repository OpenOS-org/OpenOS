//! Interrupt Descriptor Table (IDT) and 8259 PIC management.
//!
//! The IDT is the `x86_64` equivalent of a vector table: each entry maps an
//! interrupt vector number (0–255) to a handler function. We configure:
//!   - CPU exceptions (0–31): fault handlers with user/kernel discrimination
//!   - Hardware IRQs (32–47): timer (IRQ 0), keyboard (IRQ 1)
//!
//! ## User-Space Fault Tolerance
//!
//! When a user-space process (Ring 3) triggers a fatal exception, the kernel
//! must NOT panic — a buggy user program should never crash the kernel. Instead:
//!
//!   1. Detect Ring 3 origin by checking the CS selector's RPL bits
//!   2. Log the fault details to serial (always available)
//!   3. Halt the CPU gracefully (no kernel panic)
//!
//! Kernel-mode (Ring 0) faults still panic, as they indicate kernel bugs.
//! This design follows the microkernel principle: user-space failures are
//! contained, kernel failures are fatal.

use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::PrivilegeLevel;

use crate::{println, serial_println};

/// IRQ 0–7 from the master PIC are mapped to IDT vectors starting here.
/// We choose 32 (0x20) because vectors 0–31 are reserved for CPU exceptions.
pub const PIC_1_OFFSET: u8 = 32;

/// IRQ 8–15 from the slave PIC immediately follow the master's range.
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// Global PIC pair, mutex-protected because `notify_end_of_interrupt` mutates
/// the PIC's in-service register. The `unsafe` block in `Mutex::new` is sound
/// because `ChainedPics::new` only stores constants — no hardware access yet.
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Maps hardware IRQ numbers to IDT vector indices.
///
/// `#[repr(u8)]` ensures the discriminant is a single byte, matching the
/// `PIC_1_OFFSET`-based vector numbering the PIC uses.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    /// IRQ 0 — Programmable Interval Timer (channel 0, mode 3 square wave).
    Timer = PIC_1_OFFSET,
    /// IRQ 1 — PS/2 keyboard controller (8042).
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Check if an exception originated from Ring 3 (user mode).
///
/// The CPU stores the interrupted code segment in the interrupt stack frame.
/// The RPL (Requested Privilege Level) in the lower 2 bits tells us the
/// privilege level at the time of the fault.
fn is_user_fault(stack_frame: &InterruptStackFrame) -> bool {
    stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3
}

/// Halt the CPU gracefully after a user-space fault.
///
/// This is NOT a kernel panic — the kernel is fine, the user program crashed.
/// We log the fault to serial and halt. In a full microkernel with multiple
/// processes, this would terminate the faulting task and return to the scheduler.
fn user_fault_halt(fault_name: &str, details: &str, stack_frame: &InterruptStackFrame) -> ! {
    serial_println!("========================================");
    serial_println!("[USER FAULT] {fault_name}");
    serial_println!("{details}");
    serial_println!("Stack frame: {stack_frame:#?}");
    serial_println!("========================================");
    serial_println!("[KERNEL] User process terminated. Halting.");
    println!("[USER FAULT] {fault_name} — see serial output for details");

    loop {
        x86_64::instructions::hlt();
    }
}

lazy_static::lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // === User-fault-tolerant exception handlers ===

        // Division Error (#DE, vector 0).
        // Triggered by DIV/IDIV with zero divisor or overflow.
        idt.divide_error.set_handler_fn(division_error_handler);

        // Invalid Opcode (#UD, vector 6).
        // Triggered by executing an undefined or privileged instruction.
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);

        // Double Fault (#DF, vector 8).
        // Always fatal — the CPU faulted while calling another fault handler.
        // Uses IST[0] for a fresh stack (prevents stack overflow cascades).
        // SAFETY: IST index 0 is valid — points to the stack in gdt::TSS.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }

        // General Protection Fault (#GP, vector 13).
        // Triggered by privilege violations, invalid segment access, etc.
        idt.general_protection_fault.set_handler_fn(gp_fault_handler);

        // Page Fault (#PF, vector 14).
        // Triggered by unmapped or permission-violating memory access.
        idt.page_fault.set_handler_fn(page_fault_handler);

        // === Hardware IRQ handlers ===
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt
    };
}

/// Load the IDT into the CPU's IDTR register.
///
/// Must be called after GDT init (the IDT references code segments) and
/// before `interrupts::enable()`.
pub fn init_idt() {
    IDT.load();
}

// ═══════════════════════════════════════════════════════════════════════════
// Exception handlers
// ═══════════════════════════════════════════════════════════════════════════

/// Breakpoint exception (#BP, vector 3).
///
/// Triggered by the `int3` instruction. Useful for debuggers; we just log it.
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

/// Division Error (#DE, vector 0).
///
/// Triggered by `DIV`/`IDIV` with a zero divisor or when the quotient
/// doesn't fit in the destination register.
extern "x86-interrupt" fn division_error_handler(stack_frame: InterruptStackFrame) {
    if is_user_fault(&stack_frame) {
        user_fault_halt(
            "#DE Division Error",
            "Division by zero or overflow",
            &stack_frame,
        );
    }
    panic!("EXCEPTION: DIVISION ERROR\n{stack_frame:#?}");
}

/// Invalid Opcode (#UD, vector 6).
///
/// Triggered when the CPU encounters an undefined instruction, or a
/// privileged instruction executed from Ring 3 (e.g., `hlt`, `in`, `out`).
extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    if is_user_fault(&stack_frame) {
        user_fault_halt(
            "#UD Invalid Opcode",
            "Undefined instruction or privileged instruction from Ring 3",
            &stack_frame,
        );
    }
    panic!("EXCEPTION: INVALID OPCODE\n{stack_frame:#?}");
}

/// Double Fault (#DF, vector 8).
///
/// Fires when the CPU faults while calling another fault handler. This is
/// almost always caused by a kernel stack overflow. The IST mechanism gives
/// us a fresh stack so we can at least print a message before halting.
///
/// `-> !` because a double fault is not recoverable — the interrupted context
/// is corrupted, so there is no safe way to resume.
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    // Double faults are always fatal — even from user space. If a user
    // fault cascades into a double fault, the fault handler itself is broken.
    panic!("EXCEPTION: DOUBLE FAULT\n{stack_frame:#?}");
}

/// General Protection Fault (#GP, vector 13).
///
/// Triggered by:
///   - Accessing a privileged segment from Ring 3
///   - Writing to a read-only segment
///   - Executing privileged instructions (`in`, `out`, `hlt`, `lgdt`, etc.)
///   - Loading an invalid segment selector
extern "x86-interrupt" fn gp_fault_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    if is_user_fault(&stack_frame) {
        user_fault_halt(
            "#GP General Protection Fault",
            &alloc::format!(
                "Error code: {error_code:#x} — privilege violation or invalid access from Ring 3"
            ),
            &stack_frame,
        );
    }
    panic!("EXCEPTION: GENERAL PROTECTION FAULT\nError code: {error_code:#x}\n{stack_frame:#?}");
}

/// Page Fault (#PF, vector 14).
///
/// Fires when the CPU cannot translate a virtual address. The faulting
/// address is in CR2. The error code tells us why:
///   - bit 0: protection violation (vs. non-present page)
///   - bit 2: user-mode access (vs. supervisor)
///   - bit 4: instruction fetch
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let fault_addr = Cr2::read();

    if is_user_fault(&stack_frame) {
        let reason = if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
            "Protection violation (page present but access denied)"
        } else {
            "Non-present page (page not mapped)"
        };
        let access_type = if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) {
            "instruction fetch"
        } else if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
            "data write"
        } else {
            "data read"
        };
        user_fault_halt(
            "#PF Page Fault",
            &alloc::format!("Address: {fault_addr:?}, Access: {access_type}, Reason: {reason}"),
            &stack_frame,
        );
    }

    panic!(
        "EXCEPTION: PAGE FAULT\nAccessed address: {fault_addr:?}\nError code: {error_code:?}\n{stack_frame:#?}",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Hardware IRQ handlers
// ═══════════════════════════════════════════════════════════════════════════

/// Timer interrupt (IRQ 0). Fires at ~18.2 Hz by default (or configured rate).
///
/// Global tick counter. Incremented on each timer interrupt (~18.2 Hz).
/// Used by `sleep_ticks()` to implement timed delays.
pub static TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Preemption flag. Set by the timer handler when a task switch is needed.
/// Checked by the syscall exit path and the main loop.
pub static NEED_RESCHEDULE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Number of ticks between preemption checks.
const PREEMPT_INTERVAL: u64 = 18; // ~1 second at 18.2 Hz

/// Timer interrupt handler (IRQ 0). Fires at ~18.2 Hz.
///
/// Increments the global tick counter and sets the reschedule flag
/// at regular intervals for preemptive scheduling.
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let ticks = TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;

    // Set reschedule flag at regular intervals.
    if ticks % PREEMPT_INTERVAL == 0 {
        NEED_RESCHEDULE.store(true, core::sync::atomic::Ordering::Release);
    }

    // SAFETY: `notify_end_of_interrupt` writes to the PIC's command port.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

/// Keyboard interrupt (IRQ 1). Fires on every key press/release.
///
/// Reads the scancode from port 0x60 (the 8042 keyboard controller's data
/// port). The scancode must be read immediately — the 8042 holds it in a
/// one-deep buffer and will drop it (or assert IRQ again) if not consumed.
///
/// The raw scancode is passed to the keyboard driver for decoding and
/// buffering. Decoded ASCII characters are stored in the global input
/// buffer and can be read by user-space via `sys_read`.
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    // SAFETY: Port 0x60 is the keyboard data port. Reading it is safe as long
    // as the interrupt was actually from the keyboard (IRQ 1), which is
    // guaranteed by the PIC routing. The read also acknowledges the keystroke
    // to the 8042 controller.
    let scancode: u8 = unsafe { port.read() };

    // Decode the scancode and store the resulting character in the input buffer.
    crate::drivers::keyboard::process_scancode(scancode);

    // SAFETY: Same as timer — EOI is mandatory to unmask the IRQ line.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

use crate::arch::x86_64::gdt;
