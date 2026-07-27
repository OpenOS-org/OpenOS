//! Shared `VirtIO` types and helpers.
//!
//! Contains the common split-virtqueue structures, I/O port helpers, register
//! offsets, status flags, and the `VirtQueue` implementation used by both the
//! VirtIO-Block and VirtIO-Net drivers.
//!
//! ## Split Virtqueue Layout
//!
//! The "split virtqueue" layout (`VirtIO` 1.0, Section 2.6) consists of three
//! parts placed in physically-contiguous memory:
//!
//! 1. **Descriptor Table** -- array of `(addr, len, flags, next)` entries that
//!    describe buffers the device can read from or write to.
//! 2. **Available Ring** -- driver writes descriptor indices here to hand
//!    buffers to the device.
//! 3. **Used Ring** -- device writes completed descriptor indices here to
//!    return buffers to the driver.
//!
//! ## Legacy I/O Port Interface
//!
//! Both drivers use the legacy (transitional) virtio I/O port interface
//! because QEMU's `virtio-*-pci` devices default to legacy mode when the
//! transport is PCI. All register offsets come from `VirtIO` 1.0, Appendix B.

use core::sync::atomic::{fence, Ordering};

// ---------------------------------------------------------------------------
// Virtqueue geometry
// ---------------------------------------------------------------------------

/// Number of descriptors per virtqueue (must be a power of two).
pub const VQ_SIZE: usize = 16;

/// Page size used by legacy virtio for queue alignment.
pub const PAGE_SIZE: u64 = 4096;

// ---------------------------------------------------------------------------
// Legacy virtio I/O port register offsets
//
// From the VirtIO 1.0 spec, Appendix B (Legacy Interface):
// ---------------------------------------------------------------------------

/// Device feature bits (read, 32-bit).
pub const VIRTIO_REG_DEVICE_FEATURES: u16 = 0x00;
/// Guest (driver) feature bits (write, 32-bit).
pub const VIRTIO_REG_GUEST_FEATURES: u16 = 0x04;
/// Queue PFN -- page frame number of the virtqueue (write, 32-bit).
pub const VIRTIO_REG_QUEUE_PFN: u16 = 0x08;
/// Queue size -- number of elements (read, 16-bit).
pub const VIRTIO_REG_QUEUE_NUM: u16 = 0x0C;
/// Queue select -- which queue to configure (write, 16-bit).
pub const VIRTIO_REG_QUEUE_SEL: u16 = 0x0E;
/// Queue notify -- write queue index to kick the device (write, 16-bit).
pub const VIRTIO_REG_QUEUE_NOTIFY: u16 = 0x10;
/// Device status (write/read, 8-bit).
pub const VIRTIO_REG_DEVICE_STATUS: u16 = 0x12;
/// ISR status (read, 8-bit) -- reading acknowledges the interrupt.
pub const VIRTIO_REG_ISR_STATUS: u16 = 0x13;

// ---------------------------------------------------------------------------
// VirtIO device status flags
// ---------------------------------------------------------------------------

/// Indicates that the guest has found the device.
pub const STATUS_ACKNOWLEDGE: u8 = 1;
/// Indicates that the guest can drive the device.
pub const STATUS_DRIVER: u8 = 2;
/// Indicates that the driver is set up and ready.
pub const STATUS_DRIVER_OK: u8 = 4;
/// Indicates that the driver has finished feature negotiation.
pub const STATUS_FEATURES_OK: u8 = 8;
/// Indicates a fatal error.
pub const STATUS_FAILED: u8 = 128;

// ---------------------------------------------------------------------------
// Virtqueue descriptor flags
// ---------------------------------------------------------------------------

/// Descriptor continues via `next` field.
pub const DESC_F_NEXT: u16 = 1;
/// Buffer is device-writable (otherwise device-readable).
pub const DESC_F_WRITE: u16 = 2;

// ---------------------------------------------------------------------------
// Split Virtqueue structures (placed in memory the device can DMA)
// ---------------------------------------------------------------------------

/// A single descriptor in the virtqueue descriptor table.
///
/// The device reads `addr` as a physical address. `len` is the buffer
/// length. `flags` control chaining and read/write direction. `next`
/// is the index of the next descriptor in a chain.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqDesc {
    /// Physical address of the buffer.
    pub addr: u64,
    /// Length of the buffer in bytes.
    pub len: u32,
    /// Flags (`DESC_F_NEXT`, `DESC_F_WRITE`).
    pub flags: u16,
    /// Index of the next descriptor in a chain (or `0xFFFF` for end).
    pub next: u16,
}

/// Available ring -- driver writes, device reads.
///
/// `idx` is the next slot the driver will write. The device reads
/// descriptors from `ring[last_seen_idx .. idx]`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqAvail {
    /// Flags (e.g., `VRING_AVAIL_F_NO_INTERRUPT`).
    pub flags: u16,
    /// Next index the driver will write.
    pub idx: u16,
    /// Descriptor indices the device should process.
    pub ring: [u16; VQ_SIZE],
}

/// A single element in the used ring.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqUsedElem {
    /// Index of the descriptor that was used.
    pub id: u32,
    /// Total bytes written by the device.
    pub len: u32,
}

/// Used ring -- device writes, driver reads.
///
/// `idx` is the next slot the device will write. The driver reads
/// descriptors from `ring[last_consumed_idx .. idx]`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqUsed {
    /// Flags (e.g., `VRING_USED_F_NO_NOTIFY`).
    pub flags: u16,
    /// Next index the device will write.
    pub idx: u16,
    /// Completed descriptor entries.
    pub ring: [VirtqUsedElem; VQ_SIZE],
}

// ---------------------------------------------------------------------------
// VirtQueue -- runtime state for one virtqueue
// ---------------------------------------------------------------------------

/// Per-queue runtime state for a split virtqueue.
///
/// The `descriptors`, `avail`, and `used` arrays live at physical
/// addresses whose PFN was written to the device via `QUEUE_PFN`.
/// The device DMAs directly into these structures.
///
/// Buffer management is left to each driver: the block driver uses a
/// single shared buffer frame, while the net driver allocates one frame
/// per descriptor. The `VirtQueue` manages only the descriptor table,
/// available ring, used ring, and the free-descriptor list.
pub struct VirtQueue {
    /// Descriptor table -- device reads these for buffer addresses.
    pub descriptors: &'static mut [VirtqDesc],
    /// Available ring -- driver writes descriptor indices here.
    pub avail: &'static mut VirtqAvail,
    /// Used ring -- device writes completed descriptor indices here.
    pub used: &'static mut VirtqUsed,

    /// Physical address of the descriptor table.
    pub desc_phys: u64,
    /// Physical address of the available ring.
    pub avail_phys: u64,
    /// Physical address of the used ring.
    pub used_phys: u64,

    /// Free descriptor list -- index of the first free descriptor.
    pub free_head: u16,
    /// Number of free descriptors.
    pub num_free: u16,

    /// Last used ring index we consumed. When `used.idx != next_used`,
    /// the device has completed one or more buffers.
    pub next_used: u16,
}

// ---------------------------------------------------------------------------
// Port I/O helpers
// ---------------------------------------------------------------------------

/// Read a 32-bit value from a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
#[must_use]
pub unsafe fn io_read32(base: u16, offset: u16) -> u32 {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.read()
    }
}

/// Write a 32-bit value to a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
pub unsafe fn io_write32(base: u16, offset: u16, value: u32) {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.write(value);
    }
}

/// Read a 16-bit value from a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
#[must_use]
pub unsafe fn io_read16(base: u16, offset: u16) -> u16 {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.read()
    }
}

/// Write a 16-bit value to a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
pub unsafe fn io_write16(base: u16, offset: u16, value: u16) {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.write(value);
    }
}

/// Read an 8-bit value from a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
#[must_use]
pub unsafe fn io_read8(base: u16, offset: u16) -> u8 {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.read()
    }
}

/// Write an 8-bit value to a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
pub unsafe fn io_write8(base: u16, offset: u16, value: u8) {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.write(value);
    }
}

/// Read a 64-bit value from two consecutive 32-bit I/O port registers.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// Reads `offset` (low 32 bits) and `offset + 4` (high 32 bits).
#[must_use]
pub unsafe fn io_read64(base: u16, offset: u16) -> u64 {
    let lo = unsafe { io_read32(base, offset) };
    let hi = unsafe { io_read32(base, offset + 4) };
    u64::from(lo) | (u64::from(hi) << 32)
}

// ---------------------------------------------------------------------------
// Physical address helpers
// ---------------------------------------------------------------------------

/// Convert a virtual address to a physical address.
///
/// Uses the bootloader's `physical_memory_offset` which maps
/// `physical -> virtual` as `virtual = physical + offset`.
/// Therefore `physical = virtual - offset`.
///
/// # Panics
/// Panics if `physical_memory_offset` has not been set (called before boot).
#[must_use]
pub fn virt_to_phys(virt: u64) -> u64 {
    let offset = crate::memory::physical_memory_offset();
    assert!(
        offset != 0,
        "physical_memory_offset not set -- virt_to_phys called too early"
    );
    virt.checked_sub(offset)
        .expect("virt_to_phys underflow: virtual address is below physical_memory_offset")
}

// ---------------------------------------------------------------------------
// VirtQueue implementation
// ---------------------------------------------------------------------------

impl VirtQueue {
    /// Create and initialize a new virtqueue.
    ///
    /// Allocates the descriptor table, available ring, and used ring from
    /// physical frames so the device can DMA into them. The free descriptor
    /// list is initialized as a chain through all entries.
    ///
    /// Buffer management (allocating frames for data) is left to the caller.
    #[must_use]
    pub fn new() -> Self {
        // Allocate descriptor table (16 entries x 16 bytes = 256 bytes, fits in 1 frame).
        let desc_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtqueue descriptors");
        let desc_virt = crate::memory::phys_to_virt(desc_phys);
        // SAFETY: Frame is exclusively allocated.
        let descriptors = unsafe {
            core::ptr::write_bytes(desc_virt as *mut u8, 0, 4096);
            core::slice::from_raw_parts_mut(desc_virt as *mut VirtqDesc, VQ_SIZE)
        };
        // Set up free descriptor chain.
        for (i, desc) in descriptors.iter_mut().enumerate().take(VQ_SIZE - 1) {
            desc.next = (i + 1) as u16;
        }
        descriptors[VQ_SIZE - 1].next = 0xFFFF;

        // Allocate available ring (struct on physical frame).
        let avail_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtqueue avail ring");
        let avail_virt = crate::memory::phys_to_virt(avail_phys);
        // SAFETY: Frame is exclusively allocated.
        let avail: &'static mut VirtqAvail = unsafe {
            core::ptr::write_bytes(avail_virt as *mut u8, 0, 4096);
            &mut *(avail_virt as *mut VirtqAvail)
        };

        // Allocate used ring (struct on physical frame).
        let used_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtqueue used ring");
        let used_virt = crate::memory::phys_to_virt(used_phys);
        // SAFETY: Frame is exclusively allocated.
        let used: &'static mut VirtqUsed = unsafe {
            core::ptr::write_bytes(used_virt as *mut u8, 0, 4096);
            &mut *(used_virt as *mut VirtqUsed)
        };

        Self {
            descriptors,
            avail,
            used,
            desc_phys,
            avail_phys,
            used_phys,
            free_head: 0,
            num_free: VQ_SIZE as u16,
            next_used: 0,
        }
    }
}

impl Default for VirtQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtQueue {
    /// Configure this virtqueue on the device via legacy I/O ports.
    ///
    /// Writes the queue's page frame number to `QUEUE_PFN`, which tells
    /// the device where the descriptor table, available ring, and used
    /// ring live in physical memory.
    ///
    /// # Safety
    /// `io_base` must be a valid virtio I/O port base.
    pub unsafe fn enable_on_device(&self, io_base: u16, queue_index: u16) {
        // SAFETY: Writing to legacy virtio I/O port registers.
        // 1. Select the queue.
        unsafe {
            io_write16(io_base, VIRTIO_REG_QUEUE_SEL, queue_index);
        }
        // 2. Read back the queue size the device reports.
        let device_queue_size = unsafe { io_read16(io_base, VIRTIO_REG_QUEUE_NUM) };
        crate::serial_println!(
            "  Queue {}: device reports size {}",
            queue_index,
            device_queue_size
        );

        // 3. Write the page frame number of the descriptor table.
        let pfn = (self.desc_phys / PAGE_SIZE) as u32;
        // SAFETY: Writing queue PFN to valid legacy virtio register.
        unsafe {
            io_write32(io_base, VIRTIO_REG_QUEUE_PFN, pfn);
        }

        crate::serial_println!(
            "  Queue {}: PFN={:#x} (desc_phys={:#x})",
            queue_index,
            pfn,
            self.desc_phys
        );
    }

    /// Allocate a free descriptor index.
    ///
    /// Returns `None` if all descriptors are in use (the device has not
    /// yet consumed enough buffers).
    pub fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }
        let idx = self.free_head;
        // Advance the free head to the next free descriptor.
        self.free_head = self.descriptors[idx as usize].next;
        self.num_free -= 1;
        // Mark this descriptor as end-of-chain (no next).
        self.descriptors[idx as usize].next = 0xFFFF;
        Some(idx)
    }

    /// Free a descriptor, returning it to the free list.
    ///
    /// # Safety
    /// `idx` must be a valid descriptor index that was previously
    /// allocated and not yet freed.
    pub fn free_desc(&mut self, idx: u16) {
        self.descriptors[idx as usize].next = self.free_head;
        self.free_head = idx;
        self.num_free += 1;
    }

    /// Submit a descriptor to the available ring and notify the device.
    ///
    /// This is the final step of a virtqueue operation: the descriptor
    /// chain has been filled in, now we tell the device about it.
    ///
    /// # Safety
    /// `io_base` must be a valid virtio I/O port base.
    pub unsafe fn submit_and_notify(&mut self, desc_idx: u16, io_base: u16, queue_index: u16) {
        // Write descriptor index into the available ring at position `avail.idx`.
        let slot = self.avail.idx as usize % VQ_SIZE;
        self.avail.ring[slot] = desc_idx;
        // Memory barrier: ensure the descriptor and ring writes are visible
        // before we update the index.
        fence(Ordering::Release);
        self.avail.idx = self.avail.idx.wrapping_add(1);

        // Notify the device by writing the queue index to QUEUE_NOTIFY.
        // SAFETY: Writing to valid legacy virtio notify register.
        unsafe {
            io_write16(io_base, VIRTIO_REG_QUEUE_NOTIFY, queue_index);
        }
    }

    /// Check the used ring for completed descriptors.
    ///
    /// Returns `Some((desc_id, len))` if the device has completed a
    /// buffer, or `None` if nothing new is available.
    pub fn poll_used(&mut self) -> Option<(u16, u32)> {
        // Memory barrier: ensure we read the device's latest write to used.idx.
        fence(Ordering::Acquire);
        if self.next_used == self.used.idx {
            return None;
        }
        let slot = self.next_used as usize % VQ_SIZE;
        let elem = self.used.ring[slot];
        self.next_used = self.next_used.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }
}

// ---------------------------------------------------------------------------
// Feature negotiation helper
// ---------------------------------------------------------------------------

/// Negotiate `VirtIO` features with the device.
///
/// Reads the device feature bits, masks them against `requested`, and
/// writes the result back as the guest features. Returns the negotiated
/// feature set.
///
/// # Safety
/// `io_base` must be a valid virtio I/O port base.
#[must_use]
pub unsafe fn negotiate_features(io_base: u16, requested: u64) -> u64 {
    let device_features = unsafe { io_read32(io_base, VIRTIO_REG_DEVICE_FEATURES) };
    let negotiated = device_features as u64 & requested;
    // SAFETY: Writing to valid virtio feature register.
    unsafe {
        io_write32(io_base, VIRTIO_REG_GUEST_FEATURES, negotiated as u32);
    }
    negotiated
}

/// Perform the standard `VirtIO` device initialization sequence.
///
/// Follows the legacy virtio initialization sequence (spec Section 3.1.1):
///   1. Reset device (status = 0)
///   2. Set ACKNOWLEDGE
///   3. Set DRIVER
///
/// Returns `true` if the initial steps succeeded. The caller must continue
/// with feature negotiation, virtqueue setup, and `DRIVER_OK`.
///
/// # Safety
/// `io_base` must be a valid virtio I/O port base.
pub unsafe fn init_device(io_base: u16) {
    // Step 1: Reset -- write 0 to device status.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, 0);
    }

    // Step 2: Acknowledge the device.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE);
    }

    // Step 3: Indicate we have a driver for this device.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(
            io_base,
            VIRTIO_REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER,
        );
    }
}

/// Set the `FEATURES_OK` status bit and verify the device accepted the features.
///
/// Returns `true` if the device accepted the negotiated features, `false`
/// if the device rejected them (and FAILED status is written).
///
/// # Safety
/// `io_base` must be a valid virtio I/O port base.
#[must_use]
pub unsafe fn set_features_ok(io_base: u16) -> bool {
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(
            io_base,
            VIRTIO_REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
    }
    // Verify the device accepted our features.
    // SAFETY: Reading from valid virtio device status register.
    let status = unsafe { io_read8(io_base, VIRTIO_REG_DEVICE_STATUS) };
    if status & STATUS_FEATURES_OK == 0 {
        // Set FAILED status.
        // SAFETY: Writing to valid virtio device status register.
        unsafe {
            io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, STATUS_FAILED);
        }
        return false;
    }
    true
}

/// Set the `DRIVER_OK` status bit, marking the device as live.
///
/// # Safety
/// `io_base` must be a valid virtio I/O port base.
pub unsafe fn set_driver_ok(io_base: u16) {
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(
            io_base,
            VIRTIO_REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────── Geometry tests ───────────────────

    #[test]
    fn vq_size_is_power_of_two() {
        assert!(VQ_SIZE.is_power_of_two());
    }

    #[test]
    fn vq_size_value() {
        assert_eq!(VQ_SIZE, 16);
    }

    #[test]
    fn page_size_value() {
        assert_eq!(PAGE_SIZE, 4096);
    }

    // ─────────────────── Register offset tests ───────────────────

    #[test]
    fn virtio_reg_device_features_offset() {
        assert_eq!(VIRTIO_REG_DEVICE_FEATURES, 0x00);
    }

    #[test]
    fn virtio_reg_guest_features_offset() {
        assert_eq!(VIRTIO_REG_GUEST_FEATURES, 0x04);
    }

    #[test]
    fn virtio_reg_queue_pfn_offset() {
        assert_eq!(VIRTIO_REG_QUEUE_PFN, 0x08);
    }

    #[test]
    fn virtio_reg_queue_num_offset() {
        assert_eq!(VIRTIO_REG_QUEUE_NUM, 0x0C);
    }

    #[test]
    fn virtio_reg_queue_sel_offset() {
        assert_eq!(VIRTIO_REG_QUEUE_SEL, 0x0E);
    }

    #[test]
    fn virtio_reg_queue_notify_offset() {
        assert_eq!(VIRTIO_REG_QUEUE_NOTIFY, 0x10);
    }

    #[test]
    fn virtio_reg_device_status_offset() {
        assert_eq!(VIRTIO_REG_DEVICE_STATUS, 0x12);
    }

    #[test]
    fn virtio_reg_isr_status_offset() {
        assert_eq!(VIRTIO_REG_ISR_STATUS, 0x13);
    }

    // ─────────────────── Status flags tests ───────────────────

    #[test]
    fn status_acknowledge_value() {
        assert_eq!(STATUS_ACKNOWLEDGE, 1);
    }

    #[test]
    fn status_driver_value() {
        assert_eq!(STATUS_DRIVER, 2);
    }

    #[test]
    fn status_driver_ok_value() {
        assert_eq!(STATUS_DRIVER_OK, 4);
    }

    #[test]
    fn status_features_ok_value() {
        assert_eq!(STATUS_FEATURES_OK, 8);
    }

    #[test]
    fn status_failed_value() {
        assert_eq!(STATUS_FAILED, 128);
    }

    #[test]
    fn status_driver_ok_combined() {
        let ok = STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK;
        assert_eq!(ok, 0x0F);
    }

    #[test]
    fn status_flags_are_unique() {
        let flags = [
            STATUS_ACKNOWLEDGE,
            STATUS_DRIVER,
            STATUS_DRIVER_OK,
            STATUS_FEATURES_OK,
            STATUS_FAILED,
        ];
        for &f in &flags {
            assert!(
                f.is_power_of_two() || f == 0,
                "status flag {f} should be a power of 2"
            );
        }
    }

    // ─────────────────── Descriptor flag tests ───────────────────

    #[test]
    fn desc_f_next_value() {
        assert_eq!(DESC_F_NEXT, 1);
    }

    #[test]
    fn desc_f_write_value() {
        assert_eq!(DESC_F_WRITE, 2);
    }

    #[test]
    fn desc_flags_are_independent() {
        assert_eq!(DESC_F_NEXT | DESC_F_WRITE, 3);
        assert_eq!(DESC_F_NEXT & DESC_F_WRITE, 0);
    }

    // ─────────────────── VirtqDesc layout tests ───────────────────

    #[test]
    fn virtq_desc_sizeof() {
        // #[repr(C)]: u64 + u32 + u16 + u16 = 16 bytes.
        assert_eq!(core::mem::size_of::<VirtqDesc>(), 16);
    }
}
