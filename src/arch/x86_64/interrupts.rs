//! Interrupt Descriptor Table (IDT) and 8259 PIC management.
//!
//! The IDT is the `x86_64` equivalent of a vector table: each entry maps an
//! interrupt vector number (0–255) to a handler function. We configure:
//!   - CPU exceptions (0–31): breakpoint (#BP), double fault (#DF)
//!   - Hardware IRQs (32–47): timer (IRQ 0), keyboard (IRQ 1)
//!
//! The 8259 PIC is remapped so its IRQs don't collide with CPU exceptions
//! (which occupy vectors 0–31 by architectural convention).

use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::println;

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

lazy_static::lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // Breakpoint (INT 3) is a software interrupt used by debuggers.
        // We handle it here so the kernel doesn't triple-fault on `int3`.
        idt.breakpoint.set_handler_fn(breakpoint_handler);

        // SAFETY: set_stack_index is unsafe because an invalid IST index
        // would cause the CPU to load a garbage stack pointer on #DF.
        // DOUBLE_FAULT_IST_INDEX (0) is valid — it points to the stack
        // we allocated in gdt::TSS.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }

        // Hardware IRQ handlers — indexed by (IRQ number + PIC_1_OFFSET).
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

/// Breakpoint exception (#BP, vector 3).
///
/// Triggered by the `int3` instruction. Useful for debuggers; we just log it.
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

/// Double fault exception (#DF, vector 8).
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
    panic!("EXCEPTION: DOUBLE FAULT\n{stack_frame:#?}");
}

/// Timer interrupt (IRQ 0). Fires at ~18.2 Hz by default (or configured rate).
///
/// Currently a no-op beyond EOI. Will be used for preemptive scheduling:
/// the handler will call into the scheduler to context-switch tasks.
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // SAFETY: `notify_end_of_interrupt` writes to the PIC's command port.
    // Must be called or the PIC will hold the IRQ line low and no further
    // interrupts on that line (or lower-priority lines) will fire.
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
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    // SAFETY: Port 0x60 is the keyboard data port. Reading it is safe as long
    // as the interrupt was actually from the keyboard (IRQ 1), which is
    // guaranteed by the PIC routing. The read also acknowledges the keystroke
    // to the 8042 controller.
    let scancode: u8 = unsafe { port.read() };

    // TODO: Decode scancode via pc-keyboard crate (scancode set 1/2).
    println!("Keyboard scancode: {scancode}");

    // SAFETY: Same as timer — EOI is mandatory to unmask the IRQ line.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

use crate::arch::x86_64::gdt;
