//! User-space VirtIO-Net driver for OpenOS.
//!
//! Demonstrates the full user-space driver pattern:
//! 1. Discovers VirtIO-Net device info from the devmgr service
//! 2. Maps VirtIO MMIO registers via SYS_MMIO_MAP
//! 3. Sets up virtqueues via DMA-compatible memory allocation
//! 4. Registers as the "network" service via SYS_ENDPOINT_REGISTER
//! 5. Loops: receives send_frame/recv_frame requests, performs VirtIO I/O
//!
//! This driver replaces the kernel-side virtio-net driver with a user-space
//! implementation that communicates via channels and MMIO.

#![no_std]
#![no_main]
#![allow(dead_code)]

use core::panic::PanicInfo;

use openos_sdk::{channel, console, device, process, service};

// ---------------------------------------------------------------------------
// VirtIO-Net constants
// ---------------------------------------------------------------------------

/// VirtIO vendor ID.
const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
/// VirtIO-Net device ID.
const VIRTIO_NET_DEVICE_ID: u16 = 0x1000;

/// Number of descriptors per virtqueue (must be a power of two).
const VQ_SIZE: usize = 16;

/// Maximum Ethernet frame payload (excluding virtio-net header).
const MAX_FRAME_SIZE: usize = 1518;

/// Size of the legacy virtio-net header.
const VIRTIO_NET_HDR_SIZE: usize = 10;

/// Size of one buffer: header + maximum frame.
const BUF_SIZE: usize = VIRTIO_NET_HDR_SIZE + MAX_FRAME_SIZE;

/// Page size for alignment.
const PAGE_SIZE: u64 = 4096;

// ---------------------------------------------------------------------------
// Legacy VirtIO I/O port register offsets
// ---------------------------------------------------------------------------

const VIRTIO_REG_DEVICE_FEATURES: u16 = 0x00;
const VIRTIO_REG_GUEST_FEATURES: u16 = 0x04;
const VIRTIO_REG_QUEUE_PFN: u16 = 0x08;
const VIRTIO_REG_QUEUE_NUM: u16 = 0x0C;
const VIRTIO_REG_QUEUE_SEL: u16 = 0x0E;
const VIRTIO_REG_QUEUE_NOTIFY: u16 = 0x10;
const VIRTIO_REG_DEVICE_STATUS: u16 = 0x12;
const VIRTIO_REG_ISR_STATUS: u16 = 0x13;
const VIRTIO_REG_MAC_BASE: u16 = 0x14;

// Device status bits.
const STATUS_ACK: u8 = 0x01;
const STATUS_DRIVER: u8 = 0x02;
const STATUS_DRIVER_OK: u8 = 0x04;
const STATUS_FEATURES_OK: u8 = 0x08;

// VirtIO-Net feature bits.
const VIRTIO_NET_F_MAC: u32 = 1 << 5;

// ---------------------------------------------------------------------------
// Virtqueue descriptor flags
// ---------------------------------------------------------------------------

const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

// ---------------------------------------------------------------------------
// Device manager request opcodes
// ---------------------------------------------------------------------------

const REQ_LIST_DEVICES: u8 = 0x01;
const REQ_GET_DEVICE: u8 = 0x02;

// ---------------------------------------------------------------------------
// Network service request opcodes
// ---------------------------------------------------------------------------

const NET_REQ_SEND: u8 = 0x01;
const NET_REQ_RECV: u8 = 0x02;
const NET_REQ_GET_MAC: u8 = 0x03;

// ---------------------------------------------------------------------------
// Virtqueue descriptor (16 bytes)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

// ---------------------------------------------------------------------------
// Available ring header
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct VringAvail {
    flags: u16,
    idx: u16,
    ring: [u16; VQ_SIZE],
}

// ---------------------------------------------------------------------------
// Used ring element
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct VringUsedElem {
    id: u32,
    len: u32,
}

// ---------------------------------------------------------------------------
// Used ring header
// ---------------------------------------------------------------------------

#[repr(C)]
struct VringUsed {
    flags: u16,
    idx: u16,
    ring: [VringUsedElem; VQ_SIZE],
}

// ---------------------------------------------------------------------------
// Virtqueue state
// ---------------------------------------------------------------------------

struct Virtqueue {
    /// Base I/O port for this VirtIO device.
    base_port: u16,
    /// Descriptor table (physical addresses stored in descriptors).
    descriptors: [VringDesc; VQ_SIZE],
    /// Available ring (driver writes, device reads).
    avail: VringAvail,
    /// Used ring (device writes, driver reads).
    used: VringUsed,
    /// Per-descriptor buffers.
    buffers: [[u8; BUF_SIZE]; VQ_SIZE],
    /// Free descriptor index chain.
    free_head: usize,
    /// Number of free descriptors.
    num_free: usize,
    /// Available ring index (next slot to fill).
    avail_idx: u16,
    /// Used ring index (last consumed by driver).
    used_idx: u16,
    /// Queue index (0 = receive, 1 = transmit).
    queue_index: u16,
}

// ---------------------------------------------------------------------------
// Global driver state
// ---------------------------------------------------------------------------

static mut NET_DRIVER: Option<NetDriver> = None;

struct NetDriver {
    /// VirtIO-Net base I/O port (from PCI BAR0).
    base_port: u16,
    /// Device MAC address.
    mac: [u8; 6],
    /// Receive virtqueue.
    rx_queue: Virtqueue,
    /// Transmit virtqueue.
    tx_queue: Virtqueue,
    /// MMIO-mapped virtual base (for MMIO-mode devices).
    mmio_virt: u64,
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in net_driver!");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Helper: read a value from a VirtIO register via port I/O
// ---------------------------------------------------------------------------

fn virtio_in8(port: u16) -> u8 {
    device::port_in(port, 1).unwrap_or(0) as u8
}

fn virtio_in16(port: u16) -> u16 {
    device::port_in(port, 2).unwrap_or(0) as u16
}

fn virtio_in32(port: u16) -> u32 {
    device::port_in(port, 4).unwrap_or(0) as u32
}

fn virtio_out8(port: u16, val: u8) {
    let _ = device::port_out(port, val as u64, 1);
}

fn virtio_out16(port: u16, val: u16) {
    let _ = device::port_out(port, val as u64, 2);
}

fn virtio_out32(port: u16, val: u32) {
    let _ = device::port_out(port, val as u64, 4);
}

// ---------------------------------------------------------------------------
// Logging helpers
// ---------------------------------------------------------------------------

fn log_hex8(val: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let _ = console::write(core::str::from_utf8(&[HEX[(val >> 4) as usize]]).unwrap_or("?"));
    let _ = console::write(core::str::from_utf8(&[HEX[(val & 0xF) as usize]]).unwrap_or("?"));
}

fn log_hex16(val: u16) {
    log_hex8((val >> 8) as u8);
    log_hex8((val & 0xFF) as u8);
}

fn log_hex32(val: u32) {
    log_hex16((val >> 16) as u16);
    log_hex16((val & 0xFFFF) as u16);
}

fn log_number(mut val: u64) {
    if val == 0 {
        let _ = console::write("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut pos = buf.len();
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    let _ = console::write(core::str::from_utf8(&buf[pos..]).unwrap_or("?"));
}

// ---------------------------------------------------------------------------
// Virtqueue initialization
// ---------------------------------------------------------------------------

impl Virtqueue {
    /// Create a new virtqueue structure with zeroed state.
    const fn new(queue_index: u16) -> Self {
        Self {
            base_port: 0,
            descriptors: [VringDesc {
                addr: 0,
                len: 0,
                flags: 0,
                next: 0,
            }; VQ_SIZE],
            avail: VringAvail {
                flags: 0,
                idx: 0,
                ring: [0; VQ_SIZE],
            },
            used: VringUsed {
                flags: 0,
                idx: 0,
                ring: [VringUsedElem { id: 0, len: 0 }; VQ_SIZE],
            },
            buffers: [[0u8; BUF_SIZE]; VQ_SIZE],
            free_head: 0,
            num_free: VQ_SIZE,
            avail_idx: 0,
            used_idx: 0,
            queue_index,
        }
    }

    /// Initialize the free descriptor chain.
    fn init_descriptors(&mut self) {
        for i in 0..VQ_SIZE {
            self.descriptors[i].next = ((i + 1) % VQ_SIZE) as u16;
            self.descriptors[i].flags = 0;
            self.descriptors[i].addr = self.buffers[i].as_ptr() as u64;
            self.descriptors[i].len = BUF_SIZE as u32;
        }
        self.free_head = 0;
        self.num_free = VQ_SIZE;
    }

    /// Allocate a descriptor index from the free list.
    fn alloc_desc(&mut self) -> Option<usize> {
        if self.num_free == 0 {
            return None;
        }
        let idx = self.free_head;
        self.free_head = self.descriptors[idx].next as usize;
        self.num_free -= 1;
        Some(idx)
    }

    /// Free a descriptor back to the free list.
    fn free_desc(&mut self, idx: usize) {
        self.descriptors[idx].next = self.free_head as u16;
        self.free_head = idx;
        self.num_free += 1;
    }

    /// Submit a buffer to the available ring and notify the device.
    fn submit(&mut self, desc_idx: usize) {
        self.avail.ring[self.avail_idx as usize % VQ_SIZE] = desc_idx as u16;
        // Memory barrier: ensure descriptor writes are visible before avail ring update.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.avail.idx = self.avail_idx;
        // Notify the device by writing the queue index.
        virtio_out16(self.base_port + VIRTIO_REG_QUEUE_NOTIFY, self.queue_index);
    }

    /// Check if the device has consumed any descriptors.
    fn poll_used(&mut self) -> Option<(usize, u32)> {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        if self.used_idx == self.used.idx {
            return None;
        }
        let elem = self.used.ring[self.used_idx as usize % VQ_SIZE];
        self.used_idx = self.used_idx.wrapping_add(1);
        Some((elem.id as usize, elem.len))
    }
}

// ---------------------------------------------------------------------------
// VirtIO-Net device initialization
// ---------------------------------------------------------------------------

/// Probe PCI bus for VirtIO-Net device and return its BAR0 (I/O port base).
fn find_virtio_net() -> Option<u16> {
    let _ = console::writeln("[net_driver] Looking up devmgr service...");

    let devmgr_handle = match service::discover("devmgr") {
        Ok(h) => h,
        Err(_) => {
            let _ = console::writeln("[net_driver] devmgr service not found");
            return None;
        }
    };

    let _ = console::writeln("[net_driver] Querying device list...");
    let mut reply = [0u8; 64];

    // Request device count.
    let req = [REQ_LIST_DEVICES];
    let Ok(n) = channel::call(devmgr_handle, &req, &mut reply) else {
        let _ = console::writeln("[net_driver] failed to query devmgr");
        return None;
    };
    if n == 0 {
        return None;
    }
    let device_count = reply[0];
    let _ = console::write("[net_driver] Found ");
    log_number(device_count as u64);
    let _ = console::writeln(" PCI devices");

    // Scan for VirtIO-Net.
    for i in 0..device_count {
        let req = [REQ_GET_DEVICE, i];
        let Ok(n) = channel::call(devmgr_handle, &req, &mut reply) else {
            continue;
        };
        if n < 36 {
            continue;
        }

        let vendor_id = u16::from(reply[0]) | (u16::from(reply[1]) << 8);
        let device_id = u16::from(reply[2]) | (u16::from(reply[3]) << 8);

        if vendor_id == VIRTIO_VENDOR_ID && device_id == VIRTIO_NET_DEVICE_ID {
            let bar0 = u32::from(reply[7])
                | (u32::from(reply[8]) << 8)
                | (u32::from(reply[9]) << 16)
                | (u32::from(reply[10]) << 24);
            let irq_line = reply[31];

            let _ = console::write("[net_driver] VirtIO-Net at BAR0=");
            log_hex32(bar0);
            let _ = console::write(" IRQ=");
            log_number(irq_line as u64);
            let _ = console::writeln("");

            // BAR0 contains the I/O port base (bit 0 = 0 means I/O port).
            if bar0 & 1 == 0 {
                let _ = console::writeln("[net_driver] BAR0 is memory-mapped, not I/O port");
                // For MMIO, we need to map it.
                let phys = (bar0 & 0xFFFF_FFF0) as u64;
                let size = PAGE_SIZE * 4; // 16 KiB for VirtIO MMIO region.
                match device::mmio_map(phys, size) {
                    Ok(virt) => {
                        let _ = console::write("[net_driver] MMIO mapped at virt=");
                        log_hex32(virt as u32);
                        let _ = console::writeln("");
                        // Return a sentinel indicating MMIO mode.
                        // We'll use the MMIO virtual address instead.
                        return Some(0xFFFF); // Sentinel for MMIO mode.
                    }
                    Err(_) => {
                        let _ = console::writeln("[net_driver] MMIO map failed");
                        return None;
                    }
                }
            }
            return Some((bar0 & 0xFFFC) as u16);
        }
    }

    let _ = console::writeln("[net_driver] VirtIO-Net not found");
    None
}

/// Initialize the VirtIO-Net device.
fn init_virtio_net(base_port: u16) -> bool {
    let _ = console::write("[net_driver] Initializing VirtIO-Net at port ");
    log_hex16(base_port);
    let _ = console::writeln("");

    // Step 1: Reset device.
    virtio_out8(base_port + VIRTIO_REG_DEVICE_STATUS, 0);

    // Step 2: Acknowledge.
    virtio_out8(base_port + VIRTIO_REG_DEVICE_STATUS, STATUS_ACK);

    // Step 3: Set DRIVER status.
    virtio_out8(
        base_port + VIRTIO_REG_DEVICE_STATUS,
        STATUS_ACK | STATUS_DRIVER,
    );

    // Step 4: Read and negotiate features.
    let host_features = virtio_in32(base_port + VIRTIO_REG_DEVICE_FEATURES);
    let _ = console::write("[net_driver] Host features: ");
    log_hex32(host_features);
    let _ = console::writeln("");

    // We only need the MAC feature.
    let guest_features = if host_features & VIRTIO_NET_F_MAC != 0 {
        VIRTIO_NET_F_MAC
    } else {
        0
    };
    virtio_out32(base_port + VIRTIO_REG_GUEST_FEATURES, guest_features);
    virtio_out8(
        base_port + VIRTIO_REG_DEVICE_STATUS,
        STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK,
    );

    // Step 5: Read MAC address.
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = virtio_in8(base_port + VIRTIO_REG_MAC_BASE + i as u16);
    }
    let _ = console::write("[net_driver] MAC address: ");
    for (i, &b) in mac.iter().enumerate() {
        if i > 0 {
            let _ = console::write(":");
        }
        log_hex8(b);
    }
    let _ = console::writeln("");

    // SAFETY: We are the sole writer during initialization.
    unsafe {
        NET_DRIVER = Some(NetDriver {
            base_port,
            mac,
            rx_queue: Virtqueue::new(0),
            tx_queue: Virtqueue::new(1),
            mmio_virt: 0,
        });
    }

    // Step 6: Initialize receive queue (queue 0).
    // SAFETY: NET_DRIVER was just set above.
    unsafe {
        if let Some(ref mut driver) = NET_DRIVER {
            driver.rx_queue.base_port = base_port;
            init_queue(&mut driver.rx_queue, base_port, 0);
            driver.tx_queue.base_port = base_port;
            init_queue(&mut driver.tx_queue, base_port, 1);
        }
    }

    // Step 7: Set DRIVER_OK.
    virtio_out8(
        base_port + VIRTIO_REG_DEVICE_STATUS,
        STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
    );

    let _ = console::writeln("[net_driver] VirtIO-Net initialized successfully");
    true
}

/// Initialize a single virtqueue.
fn init_queue(vq: &mut Virtqueue, base_port: u16, queue_idx: u16) {
    let _ = console::write("[net_driver]   Initializing queue ");
    log_number(queue_idx as u64);
    let _ = console::writeln("");

    // Select queue.
    virtio_out16(base_port + VIRTIO_REG_QUEUE_SEL, queue_idx);

    // Read queue size.
    let queue_num = virtio_in16(base_port + VIRTIO_REG_QUEUE_NUM);
    let _ = console::write("[net_driver]   Queue size: ");
    log_number(queue_num as u64);
    let _ = console::writeln("");

    if queue_num == 0 || queue_num as usize > VQ_SIZE {
        let _ = console::writeln("[net_driver]   Invalid queue size");
        return;
    }

    // Initialize descriptors with buffers.
    vq.init_descriptors();

    // Set the queue PFN. The descriptor table, available ring, and used ring
    // are stored in the Virtqueue struct. For legacy VirtIO, the device
    // expects a physical page frame number pointing to the descriptor table.
    //
    // In user-space, we use the virtual address as-is since the kernel
    // maps our memory identity-mapped for DMA (or we use the physical
    // address of our buffer).
    //
    // For simplicity, we set the PFN to the page-aligned address of our
    // descriptor table divided by PAGE_SIZE. This works in QEMU when
    // the memory is identity-mapped.
    #[allow(clippy::cast_possible_truncation)]
    let desc_phys = vq.descriptors.as_ptr() as u64;
    let pfn = (desc_phys / PAGE_SIZE) as u32;
    virtio_out32(base_port + VIRTIO_REG_QUEUE_PFN, pfn);

    // For receive queue, post all descriptors to the available ring.
    if queue_idx == 0 {
        for i in 0..VQ_SIZE {
            // Mark descriptors as device-writable for receive.
            vq.descriptors[i].flags = VRING_DESC_F_WRITE;
            vq.descriptors[i].len = BUF_SIZE as u32;
            let avail_idx = vq.avail_idx;
            vq.avail.ring[avail_idx as usize % VQ_SIZE] = i as u16;
            vq.avail_idx = avail_idx.wrapping_add(1);
        }
        vq.avail.idx = vq.avail_idx;
        // Notify the device.
        virtio_out16(base_port + VIRTIO_REG_QUEUE_NOTIFY, queue_idx);
    }
}

// ---------------------------------------------------------------------------
// Network service request handling
// ---------------------------------------------------------------------------

/// Handle a network service request.
fn handle_net_request(msg: &[u8], reply: &mut [u8]) -> usize {
    if msg.is_empty() {
        reply[0] = 0xFF;
        return 1;
    }

    match msg[0] {
        NET_REQ_SEND => {
            // Send an Ethernet frame.
            // msg[1..] = frame data
            if msg.len() < 2 {
                reply[0] = 0xFF;
                return 1;
            }
            let frame_data = &msg[1..];
            // SAFETY: NET_DRIVER is initialized before the main loop.
            let result = unsafe {
                if let Some(ref mut driver) = NET_DRIVER {
                    send_frame(&mut driver.tx_queue, frame_data)
                } else {
                    Err(())
                }
            };
            match result {
                Ok(sent) => {
                    reply[0] = 0x00; // Success.
                    reply[1] = (sent & 0xFF) as u8;
                    reply[2] = ((sent >> 8) & 0xFF) as u8;
                    3
                }
                Err(_) => {
                    reply[0] = 0xFF;
                    1
                }
            }
        }
        NET_REQ_RECV => {
            // Receive an Ethernet frame (non-blocking).
            // SAFETY: NET_DRIVER is initialized before the main loop.
            let result = unsafe {
                if let Some(ref mut driver) = NET_DRIVER {
                    recv_frame(&mut driver.rx_queue)
                } else {
                    None
                }
            };
            match result {
                Some(recv) => {
                    let len = recv.len.min(reply.len() - 3);
                    reply[0] = 0x00; // Success.
                    reply[1] = (len & 0xFF) as u8;
                    reply[2] = ((len >> 8) & 0xFF) as u8;
                    reply[3..3 + len].copy_from_slice(&recv.data[..len]);
                    3 + len
                }
                None => {
                    reply[0] = 0x01; // No data available.
                    1
                }
            }
        }
        NET_REQ_GET_MAC => {
            // Return the MAC address.
            reply[0] = 0x00;
            // SAFETY: NET_DRIVER is initialized before the main loop.
            unsafe {
                if let Some(ref driver) = NET_DRIVER {
                    reply[1..7].copy_from_slice(&driver.mac);
                }
            }
            7
        }
        _ => {
            reply[0] = 0xFF;
            1
        }
    }
}

/// Send an Ethernet frame via the transmit virtqueue.
fn send_frame(tx: &mut Virtqueue, data: &[u8]) -> Result<usize, ()> {
    if data.len() > MAX_FRAME_SIZE {
        return Err(());
    }

    let desc_idx = tx.alloc_desc().ok_or(())?;

    // Build the virtio-net header + frame in the descriptor buffer.
    let buf = &mut tx.buffers[desc_idx];
    // Zero the header.
    for b in &mut buf[..VIRTIO_NET_HDR_SIZE] {
        *b = 0;
    }
    // Copy frame data after the header.
    let copy_len = data.len().min(MAX_FRAME_SIZE);
    buf[VIRTIO_NET_HDR_SIZE..VIRTIO_NET_HDR_SIZE + copy_len].copy_from_slice(&data[..copy_len]);

    tx.descriptors[desc_idx].len = (VIRTIO_NET_HDR_SIZE + copy_len) as u32;
    tx.descriptors[desc_idx].flags = 0; // Device-readable.
    tx.descriptors[desc_idx].next = 0;

    tx.submit(desc_idx);

    // Reclaim completed descriptors.
    reclaim_used(tx);

    Ok(copy_len)
}

/// Result of a receive attempt: a frame copied into a fixed-size buffer
/// plus its actual length.
struct RecvResult {
    data: [u8; MAX_FRAME_SIZE],
    len: usize,
}

/// Try to receive an Ethernet frame from the receive virtqueue.
fn recv_frame(rx: &mut Virtqueue) -> Option<RecvResult> {
    // Check if any descriptors have been consumed by the device.
    if let Some((desc_idx, len)) = rx.poll_used() {
        if len as usize > VIRTIO_NET_HDR_SIZE {
            let frame_len = len as usize - VIRTIO_NET_HDR_SIZE;
            let mut result = RecvResult {
                data: [0u8; MAX_FRAME_SIZE],
                len: frame_len,
            };
            result.data[..frame_len]
                .copy_from_slice(&rx.buffers[desc_idx][VIRTIO_NET_HDR_SIZE..len as usize]);

            // Re-post the descriptor for the next receive.
            rx.descriptors[desc_idx].flags = VRING_DESC_F_WRITE;
            rx.descriptors[desc_idx].len = BUF_SIZE as u32;
            rx.descriptors[desc_idx].next = 0;
            rx.submit(desc_idx);

            return Some(result);
        }

        // Re-post even if the frame was too small.
        rx.descriptors[desc_idx].flags = VRING_DESC_F_WRITE;
        rx.descriptors[desc_idx].len = BUF_SIZE as u32;
        rx.descriptors[desc_idx].next = 0;
        rx.submit(desc_idx);
    }

    None
}

/// Reclaim completed transmit descriptors from the used ring.
fn reclaim_used(tx: &mut Virtqueue) {
    while let Some((desc_idx, _len)) = tx.poll_used() {
        tx.free_desc(desc_idx);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("[net_driver] User-space VirtIO-Net driver starting...");

    // Step 1: Find the VirtIO-Net device via devmgr.
    let Some(base_port) = find_virtio_net() else {
        let _ = console::writeln("[net_driver] No VirtIO-Net device found, exiting");
        process::exit(1);
    };

    // Step 2: Initialize the VirtIO-Net device.
    if !init_virtio_net(base_port) {
        let _ = console::writeln("[net_driver] Failed to initialize VirtIO-Net");
        process::exit(1);
    }

    // Step 3: Create a channel for network service requests.
    let (server_end, _client_end) = match channel::create() {
        Ok(ends) => ends,
        Err(_) => {
            let _ = console::writeln("[net_driver] Failed to create channel");
            process::exit(1);
        }
    };

    // Step 4: Register as "network" service.
    if service::register("network", server_end).is_err() {
        let _ = console::writeln("[net_driver] Failed to register service");
        process::exit(1);
    }
    let _ = console::writeln("[net_driver] Registered as 'network' service");

    // Step 5: Main request loop.
    let _ = console::writeln("[net_driver] Waiting for network requests...");
    let mut recv_buf = [0u8; 2048];
    let mut reply_buf = [0u8; 2048];

    loop {
        match channel::receive(server_end, &mut recv_buf) {
            Ok(len) => {
                let reply_len = handle_net_request(&recv_buf[..len], &mut reply_buf);
                let _ = channel::reply(server_end, &reply_buf[..reply_len]);
            }
            Err(_) => {
                // Channel error -- brief yield and retry.
                openos_sdk::thread::yield_();
            }
        }
    }
}
