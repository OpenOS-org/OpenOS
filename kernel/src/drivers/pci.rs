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
    /// PCI bus number.
    pub bus: u8,
    /// PCI device number.
    pub device: u8,
    /// PCI function number.
    pub function: u8,
    /// PCI vendor ID.
    pub vendor_id: u16,
    /// PCI device ID.
    pub device_id: u16,
    /// PCI class code.
    pub class_code: u8,
    /// PCI subclass.
    pub subclass: u8,
    /// Programming interface.
    pub prog_if: u8,
    /// Revision ID.
    pub revision_id: u8,
    /// Base address register 0.
    pub bar0: u32,
    /// Base address register 1.
    pub bar1: u32,
    /// Base address register 2.
    pub bar2: u32,
    /// Base address register 3.
    pub bar3: u32,
    /// Base address register 4.
    pub bar4: u32,
    /// Base address register 5.
    pub bar5: u32,
    /// Interrupt line.
    pub interrupt_line: u8,
    /// Interrupt pin.
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
///
/// Accesses PCI I/O ports 0xCF8 and 0xCFC.
#[must_use]
pub unsafe fn pci_config_read(bus: u8, device: u8, function: u8, register: u8) -> u32 {
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
///
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
#[must_use]
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
///
/// Scans bus 0, devices 0..32 (standard PCI).
/// Uses a quick vendor check first to avoid reading full config for empty slots.
#[must_use]
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    for device in 0..32u8 {
        // Quick check: read only vendor/device ID (register 0).
        let vendor_device = unsafe { crate::drivers::pci::pci_config_read(0, device, 0, 0) };
        let vid = (vendor_device & 0xFFFF) as u16;
        if vid == 0xFFFF || vid == 0x0000 {
            continue; // No device
        }
        let did = ((vendor_device >> 16) & 0xFFFF) as u16;
        if vid == vendor_id && did == device_id {
            return read_device(0, device, 0);
        }
    }
    None
}

/// `VIRTIO_VENDOR_ID` constant (0x1AF4).
pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

/// VirtIO-Net device ID.
pub const VIRTIO_NET_DEVICE_ID: u16 = 0x1000;

/// VirtIO-Block device ID.
pub const VIRTIO_BLK_DEVICE_ID: u16 = 0x1001;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_config_address_ports() {
        // Verify the well-known I/O port constants for PCI configuration.
        assert_eq!(PCI_CONFIG_ADDRESS, 0xCF8);
        assert_eq!(PCI_CONFIG_DATA, 0xCFC);
    }

    #[test]
    fn pci_address_enable_bit() {
        // The enable bit (bit 31) must be set in every CONFIG_ADDRESS value.
        let addr = pci_address(0, 0, 0, 0);
        assert_eq!(addr & (1 << 31), 1 << 31, "enable bit must be set");
    }

    #[test]
    fn pci_address_bus_field() {
        // Bus number occupies bits 23..16.
        let addr = pci_address(0xAB, 0, 0, 0);
        assert_eq!((addr >> 16) & 0xFF, 0xAB);
    }

    #[test]
    fn pci_address_device_field() {
        // Device number occupies bits 15..11.
        let addr = pci_address(0, 17, 0, 0);
        assert_eq!((addr >> 11) & 0x1F, 17);
    }

    #[test]
    fn pci_address_function_field() {
        // Function number occupies bits 10..8.
        let addr = pci_address(0, 0, 3, 0);
        assert_eq!((addr >> 8) & 0x07, 3);
    }

    #[test]
    fn pci_address_register_field() {
        // Register offset occupies bits 7..2 (lower 2 bits masked off).
        let addr = pci_address(0, 0, 0, 0x3C);
        assert_eq!((addr >> 2) & 0x3F, 0x3C >> 2);
    }

    #[test]
    fn pci_address_register_alignment() {
        // The register field masks off the lower 2 bits (DWORD alignment).
        // 0x10 and 0x11 map to the same DWORD (0x10), so their register
        // fields should be equal. 0x10 and 0x14 are different DWORDs.
        let addr_a = pci_address(0, 0, 0, 0x10);
        let addr_b = pci_address(0, 0, 0, 0x11);
        let addr_c = pci_address(0, 0, 0, 0x14);
        // 0x10 & 0xFC == 0x10, 0x11 & 0xFC == 0x10 (same DWORD).
        assert_eq!(
            (addr_a >> 2) & 0x3F,
            (addr_b >> 2) & 0x3F,
            "registers in the same DWORD should have the same field"
        );
        // 0x10 and 0x14 are different DWORDs.
        assert_ne!(
            (addr_a >> 2) & 0x3F,
            (addr_c >> 2) & 0x3F,
            "different DWORD offsets should produce different register fields"
        );
    }

    #[test]
    fn pci_address_full_device() {
        // A fully specified address: bus=1, device=2, function=3, register=0x10.
        let addr = pci_address(1, 2, 3, 0x10);
        assert_eq!(addr & (1 << 31), 1 << 31); // enable
        assert_eq!((addr >> 16) & 0xFF, 1); // bus
        assert_eq!((addr >> 11) & 0x1F, 2); // device
        assert_eq!((addr >> 8) & 0x07, 3); // function
        assert_eq!((addr >> 2) & 0x3F, 0x10 >> 2); // register
    }

    #[test]
    fn virtio_vendor_id() {
        assert_eq!(VIRTIO_VENDOR_ID, 0x1AF4);
    }

    #[test]
    fn virtio_net_device_id() {
        assert_eq!(VIRTIO_NET_DEVICE_ID, 0x1000);
    }

    #[test]
    fn virtio_blk_device_id() {
        assert_eq!(VIRTIO_BLK_DEVICE_ID, 0x1001);
    }

    #[test]
    fn pci_device_struct_defaults() {
        // Verify PciDevice can be constructed with expected field types.
        let dev = PciDevice {
            bus: 0,
            device: 0,
            function: 0,
            vendor_id: 0xFFFF,
            device_id: 0xFFFF,
            class_code: 0,
            subclass: 0,
            prog_if: 0,
            revision_id: 0,
            bar0: 0,
            bar1: 0,
            bar2: 0,
            bar3: 0,
            bar4: 0,
            bar5: 0,
            interrupt_line: 0,
            interrupt_pin: 0,
        };
        assert_eq!(dev.vendor_id, 0xFFFF);
        assert_eq!(dev.bus, 0);
    }

    #[test]
    fn pci_address_no_device_overlap() {
        // Different bus/device/function combinations must produce different addresses.
        let a = pci_address(0, 0, 0, 0);
        let b = pci_address(0, 1, 0, 0);
        let c = pci_address(1, 0, 0, 0);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }
}
