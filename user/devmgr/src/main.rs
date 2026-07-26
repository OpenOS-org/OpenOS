//! User-space device manager for OpenOS.
//!
//! Scans the PCI bus via SYS_PORT_IN/OUT (config space 0xCF8/0xCFC),
//! discovers VirtIO devices, registers as the "devmgr" service, and
//! responds to driver requests with device information.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use openos_sdk::{channel, console, device, process, service};

/// PCI configuration space ports.
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// VirtIO vendor ID.
const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

/// VirtIO device IDs.
const VIRTIO_NET_DEVICE_ID: u16 = 0x1000;
const VIRTIO_BLK_DEVICE_ID: u16 = 0x1001;

/// A discovered PCI device's essential configuration.
#[derive(Debug, Clone, Copy)]
struct PciDeviceInfo {
    bus: u8,
    device: u8,
    function: u8,
    vendor_id: u16,
    device_id: u16,
    class_code: u8,
    subclass: u8,
    prog_if: u8,
    bar0: u32,
    bar1: u32,
    bar2: u32,
    bar3: u32,
    bar4: u32,
    bar5: u32,
    interrupt_line: u8,
    interrupt_pin: u8,
}

/// Device manager request types.
const REQ_LIST_DEVICES: u8 = 0x01;
const REQ_GET_DEVICE: u8 = 0x02;

/// Maximum devices we track.
const MAX_DEVICES: usize = 32;

/// Discovered device table.
static mut DEVICES: [Option<PciDeviceInfo>; MAX_DEVICES] = [None; MAX_DEVICES];
static mut DEVICE_COUNT: usize = 0;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in devmgr!");
    process::exit(1);
}

/// Build a PCI CONFIG_ADDRESS value.
fn pci_address(bus: u8, device: u8, function: u8, register: u8) -> u32 {
    (1 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((register as u32) & 0xFC)
}

/// Read a 32-bit value from PCI configuration space.
fn pci_config_read(bus: u8, device: u8, function: u8, register: u8) -> u32 {
    let addr = pci_address(bus, device, function, register);
    // Write address to CONFIG_ADDRESS port.
    if device::port_out(PCI_CONFIG_ADDRESS, addr as u64, 4).is_err() {
        return 0xFFFF_FFFF;
    }
    // Read data from CONFIG_DATA port.
    match device::port_in(PCI_CONFIG_DATA, 4) {
        Ok(val) => val as u32,
        Err(_) => 0xFFFF_FFFF,
    }
}

/// Scan PCI bus 0 for devices and populate the device table.
fn scan_pci_bus() {
    let _ = console::writeln("[devmgr] Scanning PCI bus...");
    let mut count = 0usize;

    for dev in 0..32u8 {
        let vendor_device = pci_config_read(0, dev, 0, 0);
        let vendor_id = (vendor_device & 0xFFFF) as u16;
        let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

        if vendor_id == 0xFFFF || vendor_id == 0x0000 {
            continue;
        }

        let class_rev = pci_config_read(0, dev, 0, 0x08);
        let _revision_id = (class_rev & 0xFF) as u8;
        let prog_if = ((class_rev >> 8) & 0xFF) as u8;
        let subclass = ((class_rev >> 16) & 0xFF) as u8;
        let class_code = ((class_rev >> 24) & 0xFF) as u8;

        let bar0 = pci_config_read(0, dev, 0, 0x10);
        let bar1 = pci_config_read(0, dev, 0, 0x14);
        let bar2 = pci_config_read(0, dev, 0, 0x18);
        let bar3 = pci_config_read(0, dev, 0, 0x1C);
        let bar4 = pci_config_read(0, dev, 0, 0x20);
        let bar5 = pci_config_read(0, dev, 0, 0x24);

        let interrupt = pci_config_read(0, dev, 0, 0x3C);
        let interrupt_line = (interrupt & 0xFF) as u8;
        let interrupt_pin = ((interrupt >> 8) & 0xFF) as u8;

        let info = PciDeviceInfo {
            bus: 0,
            device: dev,
            function: 0,
            vendor_id,
            device_id,
            class_code,
            subclass,
            prog_if,
            bar0,
            bar1,
            bar2,
            bar3,
            bar4,
            bar5,
            interrupt_line,
            interrupt_pin,
        };

        // SAFETY: We are the sole writer during initialization.
        unsafe {
            if count < MAX_DEVICES {
                DEVICES[count] = Some(info);
                count += 1;
                DEVICE_COUNT = count;
            }
        }

        let _ = console::write("[devmgr]   ");
        log_device(&info);
    }

    let _ = console::write("[devmgr] Found ");
    log_number(count as u64);
    let _ = console::writeln(" PCI devices");
}

/// Log a discovered device to the console.
fn log_device(dev: &PciDeviceInfo) {
    let _ = console::write("PCI ");
    log_hex8(dev.bus);
    let _ = console::write(":");
    log_hex8(dev.device);
    let _ = console::write(".0  ");
    log_hex16(dev.vendor_id);
    let _ = console::write(":");
    log_hex16(dev.device_id);
    let _ = console::write("  class=");
    log_hex8(dev.class_code);
    let _ = console::write(" subclass=");
    log_hex8(dev.subclass);
    let _ = console::writeln("");
}

/// Format a byte as two hex digits.
fn log_hex8(val: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let _ = console::write(core::str::from_utf8(&[HEX[(val >> 4) as usize]]).unwrap_or("?"));
    let _ = console::write(core::str::from_utf8(&[HEX[(val & 0xF) as usize]]).unwrap_or("?"));
}

/// Format a u16 as four hex digits.
fn log_hex16(val: u16) {
    log_hex8((val >> 8) as u8);
    log_hex8((val & 0xFF) as u8);
}

/// Format a u32 as eight hex digits.
fn log_hex32(val: u32) {
    log_hex16((val >> 16) as u16);
    log_hex16((val & 0xFFFF) as u16);
}

/// Log a decimal number.
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

/// Encode a device info record into a byte buffer for channel messages.
///
/// Format: [vendor_id:2][device_id:2][class:1][subclass:1][prog_if:1]
///         [bar0:4][bar1:4][bar2:4][bar3:4][bar4:4][bar5:4]
///         [irq_line:1][irq_pin:1][bus:1][dev:1][func:1]
/// Total: 31 bytes
fn encode_device(dev: &PciDeviceInfo, buf: &mut [u8]) -> usize {
    buf[0] = (dev.vendor_id & 0xFF) as u8;
    buf[1] = (dev.vendor_id >> 8) as u8;
    buf[2] = (dev.device_id & 0xFF) as u8;
    buf[3] = (dev.device_id >> 8) as u8;
    buf[4] = dev.class_code;
    buf[5] = dev.subclass;
    buf[6] = dev.prog_if;

    // BARs (little-endian u32)
    let bars = [dev.bar0, dev.bar1, dev.bar2, dev.bar3, dev.bar4, dev.bar5];
    for (i, &bar) in bars.iter().enumerate() {
        let off = 7 + i * 4;
        buf[off] = (bar & 0xFF) as u8;
        buf[off + 1] = ((bar >> 8) & 0xFF) as u8;
        buf[off + 2] = ((bar >> 16) & 0xFF) as u8;
        buf[off + 3] = ((bar >> 24) & 0xFF) as u8;
    }

    buf[31] = dev.interrupt_line;
    buf[32] = dev.interrupt_pin;
    buf[33] = dev.bus;
    buf[34] = dev.device;
    buf[35] = dev.function;

    36
}

/// Identify VirtIO devices and log them.
fn identify_virtio_devices() {
    let _ = console::writeln("[devmgr] VirtIO devices:");
    // SAFETY: read-only after scan_pci_bus.
    unsafe {
        for i in 0..DEVICE_COUNT {
            if let Some(ref dev) = DEVICES[i] {
                if dev.vendor_id == VIRTIO_VENDOR_ID {
                    let _ = console::write("[devmgr]   VirtIO ");
                    match dev.device_id {
                        VIRTIO_NET_DEVICE_ID => {
                            let _ = console::write("Net");
                        }
                        VIRTIO_BLK_DEVICE_ID => {
                            let _ = console::write("Block");
                        }
                        _ => {
                            let _ = console::write("Unknown(");
                            log_hex16(dev.device_id);
                            let _ = console::write(")");
                        }
                    }
                    let _ = console::write(" at BAR0=");
                    log_hex32(dev.bar0);
                    let _ = console::write(" IRQ=");
                    log_number(dev.interrupt_line as u64);
                    let _ = console::writeln("");
                }
            }
        }
    }
}

/// Handle a device manager request from a client.
///
/// Request format: [opcode:1][index:1] (for GET_DEVICE)
/// Response format: encoded device info, or [0xFF] if not found.
fn handle_request(msg: &[u8], reply_buf: &mut [u8]) -> usize {
    if msg.is_empty() {
        reply_buf[0] = 0xFF;
        return 1;
    }

    match msg[0] {
        REQ_LIST_DEVICES => {
            // Return count of discovered devices.
            // SAFETY: read-only after init.
            let count = unsafe { DEVICE_COUNT };
            reply_buf[0] = count as u8;
            1
        }
        REQ_GET_DEVICE => {
            if msg.len() < 2 {
                reply_buf[0] = 0xFF;
                return 1;
            }
            let index = msg[1] as usize;
            // SAFETY: read-only after init.
            unsafe {
                if index < DEVICE_COUNT {
                    if let Some(ref dev) = DEVICES[index] {
                        return encode_device(dev, reply_buf);
                    }
                }
            }
            reply_buf[0] = 0xFF;
            1
        }
        _ => {
            reply_buf[0] = 0xFF;
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("[devmgr] Device manager starting...");

    // Step 1: Scan PCI bus for devices.
    scan_pci_bus();
    identify_virtio_devices();

    // Step 2: Create a channel for client requests.
    let (server_end, _client_end) = match channel::create() {
        Ok(ends) => ends,
        Err(_) => {
            let _ = console::writeln("[devmgr] Failed to create channel");
            process::exit(1);
        }
    };

    // Step 3: Register as "devmgr" service.
    if service::register("devmgr", server_end).is_err() {
        let _ = console::writeln("[devmgr] Failed to register service");
        process::exit(1);
    }
    let _ = console::writeln("[devmgr] Registered as 'devmgr' service");

    // Step 4: Main request loop — receive and reply.
    let _ = console::writeln("[devmgr] Waiting for driver requests...");
    let mut recv_buf = [0u8; 256];
    let mut reply_buf = [0u8; 64];

    loop {
        match channel::receive(server_end, &mut recv_buf) {
            Ok(len) => {
                let reply_len = handle_request(&recv_buf[..len], &mut reply_buf);
                let _ = channel::reply(server_end, &reply_buf[..reply_len]);
            }
            Err(_) => {
                // Channel error — brief pause and retry.
                let _ = console::writeln("[devmgr] receive error, retrying");
            }
        }
    }
}
