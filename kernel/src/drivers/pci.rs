//! PCI bus enumeration for device discovery.
//!
//! Reads PCI configuration space to find devices by vendor/device ID.
//! Used by virtio-net to locate the network device.
//!
//! ## PCI Configuration Space Access (Type 1 - PCI-to-PCI Bridge)
//!
//! Port `0xCF8`: `CONFIG_ADDRESS` (32-bit)
//! Port `0xCFC`: `CONFIG_DATA` (32-bit)
//!
//! `CONFIG_ADDRESS` format:
//!   bit 31:    enable
//!   bits 23-16: bus number
//!   bits 15-11: device number
//!   bits 10-8:  function number
//!   bits 7-2:   register offset (in DWORDs)

use x86_64::instructions::port::Port;

/// PCI configuration space access via I/O ports.
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// A PCI device configuration space registers (`PciDevice`).
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision_id: u8,
    pub bar0: u32,
    pub bar1: u32,
    pub bar2: u32,
    pub bar3: u32,
    pub bar4: u32,
    pub bar5: u32,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

/// Build a `CONFIG_ADDRESS` value for a specific PCI register.
fn pci_address(bus: u8, device: u8, function: u8, register: u8) -> u32 {
    (1 << 31) // enable
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((register as u32) & 0xFC)
}

/// Read a 32-bit value from PCI configuration space.
///
/// # Safety
/// Accesses PCI I/O ports 0xCF8 and 0xCFC.
unsafe fn pci_config_read(bus: u8, device: u8, function: u8, register: u8) -> u32 {
    let addr = pci_address(bus, device, function, register);
    let mut address_port = Port::new(PCI_CONFIG_ADDRESS);
    let mut data_port = Port::new(PCI_CONFIG_DATA);
    // SAFETY: Writing to CONFIG_ADDRESS then reading CONFIG_DATA is the
    // standard PCI configuration space access mechanism.
    unsafe {
        address_port.write(addr);
        data_port.read()
    }
}

/// Write a 32-bit value to PCI configuration space.
///
/// # Safety
/// Accesses PCI I/O ports 0xCF8 and 0xCFC.
#[allow(dead_code)]
unsafe fn pci_config_write(bus: u8, device: u8, function: u8, register: u8, value: u32) {
    let addr = pci_address(bus, device, function, register);
    let mut address_port = Port::new(PCI_CONFIG_ADDRESS);
    let mut data_port = Port::new(PCI_CONFIG_DATA);
    // SAFETY: Standard PCI configuration space write.
    unsafe {
        address_port.write(addr);
        data_port.write(value);
    }
}

/// Read a PCI device's configuration space.
fn read_device(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    // SAFETY: PCI config reads are safe — they access well-known I/O ports.
    let vendor_device = unsafe { pci_config_read(bus, device, function, 0) };
    let vendor_id = (vendor_device & 0xFFFF) as u16;
    let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

    if vendor_id == 0xFFFF {
        return None; // No device
    }

    let class_rev = unsafe { pci_config_read(bus, device, function, 0x08) };
    let revision_id = (class_rev & 0xFF) as u8;
    let prog_if = ((class_rev >> 8) & 0xFF) as u8;
    let subclass = ((class_rev >> 16) & 0xFF) as u8;
    let class_code = ((class_rev >> 24) & 0xFF) as u8;

    let bar0 = unsafe { pci_config_read(bus, device, function, 0x10) };
    let bar1 = unsafe { pci_config_read(bus, device, function, 0x14) };
    let bar2 = unsafe { pci_config_read(bus, device, function, 0x18) };
    let bar3 = unsafe { pci_config_read(bus, device, function, 0x1C) };
    let bar4 = unsafe { pci_config_read(bus, device, function, 0x20) };
    let bar5 = unsafe { pci_config_read(bus, device, function, 0x24) };

    let interrupt = unsafe { pci_config_read(bus, device, function, 0x3C) };
    let interrupt_line = (interrupt & 0xFF) as u8;
    let interrupt_pin = ((interrupt >> 8) & 0xFF) as u8;

    Some(PciDevice {
        bus,
        device,
        function,
        vendor_id,
        device_id,
        class_code,
        subclass,
        prog_if,
        revision_id,
        bar0,
        bar1,
        bar2,
        bar3,
        bar4,
        bar5,
        interrupt_line,
        interrupt_pin,
    })
}

/// Scan the PCI bus for all devices.
pub fn scan_bus() -> alloc::vec::Vec<PciDevice> {
    let mut devices = alloc::vec::Vec::new();
    for bus in 0..=255u16 {
        for device in 0..32u8 {
            if let Some(dev) = read_device(bus as u8, device, 0) {
                devices.push(dev);
            }
        }
    }
    devices
}

/// Find a PCI device by vendor and device ID.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    for bus in 0..=255u16 {
        for device in 0..32u8 {
            if let Some(dev) = read_device(bus as u8, device, 0) {
                if dev.vendor_id == vendor_id && dev.device_id == device_id {
                    return Some(dev);
                }
            }
        }
    }
    None
}

/// `VIRTIO_VENDOR_ID` constant (0x1AF4).
pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

/// VirtIO-Net device ID.
pub const VIRTIO_NET_DEVICE_ID: u16 = 0x1000;
