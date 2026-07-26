//! PCI bus enumeration for device discovery.
//!
//! Reads PCI configuration space to find devices by vendor/device ID.
//! Supports multi-bus topologies, multi-function devices, and PCI
//! capability list traversal (MSI, MSI-X).
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

/// PCI capability ID: Message Signalled Interrupts.
pub const PCI_CAP_MSI: u8 = 0x05;

/// PCI capability ID: Extended Message Signalled Interrupts.
pub const PCI_CAP_MSIX: u8 = 0x11;

/// Header type register offset (byte).
const HEADER_TYPE_REG: u8 = 0x0E;

/// Multi-function bit in header type register (bit 7).
const HEADER_TYPE_MULTI_FN: u8 = 1 << 7;

/// Status register offset (high half of 0x04 DWORD — actually byte 6..7 of
/// the first DWORD pair; the register at offset 0x04 contains command (low
/// 16) and status (high 16)).
const STATUS_REG: u8 = 0x04;

/// Status register bit 4: capabilities list is available.
const STATUS_CAPABILITIES_LIST: u32 = 1 << 20;

/// Capabilities pointer register offset (byte 0x34 in config space).
const CAP_PTR_REG: u8 = 0x34;

/// Errors that can occur during PCI operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciError {
    /// The requested capability was not found in the device's capability list.
    CapabilityNotFound,
    /// The device has no capabilities list (status bit 4 not set).
    NoCapabilitiesList,
}

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
    /// Header type register (offset 0x0E). Bit 7 indicates multi-function.
    pub header_type: u8,
    /// Status register (upper 16 bits of offset 0x04). Bit 4 indicates
    /// capabilities list availability.
    pub status: u16,
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

    let status_cmd = unsafe { pci_config_read(bus, device, function, STATUS_REG) };
    let status = ((status_cmd >> 16) & 0xFFFF) as u16;

    let header_type_raw = unsafe { pci_config_read(bus, device, function, HEADER_TYPE_REG) };
    let header_type = ((header_type_raw >> 16) & 0xFF) as u8;

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
        header_type,
        status,
        interrupt_line,
        interrupt_pin,
    })
}

/// Scan the PCI bus for all devices (all 256 buses, including multi-function).
#[must_use]
pub fn scan_bus() -> alloc::vec::Vec<PciDevice> {
    let mut devices = alloc::vec::Vec::new();
    for bus in 0..=255u16 {
        for dev_num in 0..32u8 {
            if let Some(dev) = read_device(bus as u8, dev_num, 0) {
                let multi_fn = (dev.header_type & HEADER_TYPE_MULTI_FN) != 0;
                devices.push(dev);
                if multi_fn {
                    // Scan functions 1-7 for multi-function devices.
                    for fn_num in 1..8u8 {
                        if let Some(func_dev) = read_device(bus as u8, dev_num, fn_num) {
                            devices.push(func_dev);
                        }
                    }
                }
            }
        }
    }
    devices
}

/// Find a PCI device by vendor and device ID.
///
/// If `bus` is `None`, scans all 256 buses. If `Some(bus)`, scans only that
/// bus. In both cases, devices 0..32 and (for multi-function devices)
/// functions 0..7 are scanned.
///
/// Uses a quick vendor check first to avoid reading full config for empty slots.
#[must_use]
pub fn find_device(vendor_id: u16, device_id: u16, bus: Option<u16>) -> Option<PciDevice> {
    let bus_start = bus.unwrap_or(0);
    let bus_end = bus.unwrap_or(255);

    for bus_num in bus_start..=bus_end {
        for dev_num in 0..32u8 {
            // Quick check: read only vendor/device ID (register 0).
            let vendor_device = unsafe { pci_config_read(bus_num as u8, dev_num, 0, 0) };
            let vid = (vendor_device & 0xFFFF) as u16;
            if vid == 0xFFFF || vid == 0x0000 {
                continue; // No device
            }
            let did = ((vendor_device >> 16) & 0xFFFF) as u16;
            if vid == vendor_id && did == device_id {
                return read_device(bus_num as u8, dev_num, 0);
            }

            // Check if multi-function — if so, scan functions 1-7.
            let header_type_raw =
                unsafe { pci_config_read(bus_num as u8, dev_num, 0, HEADER_TYPE_REG) };
            let header_type = ((header_type_raw >> 16) & 0xFF) as u8;
            if (header_type & HEADER_TYPE_MULTI_FN) != 0 {
                for fn_num in 1..8u8 {
                    let fn_vd = unsafe { pci_config_read(bus_num as u8, dev_num, fn_num, 0) };
                    let fn_vid = (fn_vd & 0xFFFF) as u16;
                    if fn_vid == 0xFFFF {
                        continue;
                    }
                    let fn_did = ((fn_vd >> 16) & 0xFFFF) as u16;
                    if fn_vid == vendor_id && fn_did == device_id {
                        return read_device(bus_num as u8, dev_num, fn_num);
                    }
                }
            }
        }
    }
    None
}

/// Walk the PCI capability linked list to find a capability by ID.
///
/// Returns the config-space byte offset of the capability header, or an error
/// if the capability is not found or the device has no capabilities list.
///
/// The capability list is a linked list starting at the pointer stored in
/// byte 0x34 of config space. Each capability entry is at least 2 bytes:
///   byte 0: capability ID
///   byte 1: pointer to next capability (0 = end of list)
pub fn find_pci_capability(bus: u8, device: u8, function: u8, cap_id: u8) -> Result<u16, PciError> {
    // SAFETY: PCI config reads are safe — they access well-known I/O ports.
    let status_cmd = unsafe { pci_config_read(bus, device, function, STATUS_REG) };
    let status = (status_cmd >> 16) & 0xFFFF;
    if (status & STATUS_CAPABILITIES_LIST) == 0 {
        return Err(PciError::NoCapabilitiesList);
    }

    // Read the capabilities pointer (byte 0x34). It is in the low byte of
    // the DWORD at 0x34.
    let cap_ptr_dword = unsafe { pci_config_read(bus, device, function, CAP_PTR_REG) };
    let mut next_ptr = (cap_ptr_dword & 0xFF) as u8;

    // Walk the capability linked list. Limit iterations to prevent infinite
    // loops from corrupt config space (max 48 capabilities in 256-byte space).
    for _ in 0..48 {
        if next_ptr == 0 || next_ptr < 0x40 {
            break; // End of list or invalid pointer.
        }
        // Read the DWORD containing the capability ID (byte 0) and next
        // pointer (byte 1).
        let cap_dword = unsafe { pci_config_read(bus, device, function, next_ptr) };
        let this_cap_id = (cap_dword & 0xFF) as u8;
        if this_cap_id == cap_id {
            return Ok(u16::from(next_ptr));
        }
        let next = ((cap_dword >> 8) & 0xFF) as u8;
        if next == next_ptr {
            break; // Avoid infinite loop from corrupt pointer.
        }
        next_ptr = next;
    }

    Err(PciError::CapabilityNotFound)
}

/// Enable MSI for a PCI device by writing the given interrupt vector into
/// the MSI capability structure.
///
/// This function locates the MSI capability, sets the message address and
/// data registers with the local APIC delivery info for `vector`, and sets
/// the MSI enable bit. Only 32-bit MSI (no per-vector masking) is supported.
///
/// # Safety
///
/// The caller must ensure `vector` is a valid IDT entry that will handle
/// the interrupt.
pub unsafe fn enable_msi(device: &PciDevice, vector: u8) -> Result<(), PciError> {
    let cap_offset = find_pci_capability(device.bus, device.device, device.function, PCI_CAP_MSI)?;

    // SAFETY: Caller guarantees `vector` is valid. PCI config space access
    // via well-known I/O ports is safe.
    unsafe {
        // Read the MSI capability header DWORD (at cap_offset).
        let cap_header =
            pci_config_read(device.bus, device.device, device.function, cap_offset as u8);
        // Bits 16..17 of header: 64-bit capable flag.
        let is_64bit = ((cap_header >> 16) & 1) != 0;

        // Message address: deliver to this CPU's local APIC (ID 0).
        // Format: 0xFEE0_0000 | (dest ID << 12) | (rh << 3) | (dm << 2)
        // We use physical delivery mode, destination = 0.
        let msg_addr: u32 = 0xFEE0_0000;

        // Write message address (low 32 bits) at cap_offset + 4.
        pci_config_write(
            device.bus,
            device.device,
            device.function,
            (cap_offset + 4) as u8,
            msg_addr,
        );

        // If 64-bit capable, write high 32 bits of message address at +8.
        if is_64bit {
            pci_config_write(
                device.bus,
                device.device,
                device.function,
                (cap_offset + 8) as u8,
                0,
            );
        }

        // Message data register offset: 32-bit at +8, 64-bit at +12.
        let data_offset = if is_64bit {
            cap_offset + 12
        } else {
            cap_offset + 8
        };

        // Message data: vector in low 8 bits, trigger mode edge (bit 14 = 0,
        // bit 15 = 1 for edge).
        let msg_data: u32 = u32::from(vector) | (1 << 14);
        pci_config_write(
            device.bus,
            device.device,
            device.function,
            data_offset as u8,
            msg_data,
        );

        // Enable MSI: bit 16 of the capability control word.
        // The control word is in the upper 16 bits of the DWORD at cap_offset.
        let current = pci_config_read(device.bus, device.device, device.function, cap_offset as u8);
        pci_config_write(
            device.bus,
            device.device,
            device.function,
            cap_offset as u8,
            current | (1 << 16),
        );
    }

    Ok(())
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
            header_type: 0,
            status: 0,
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

    #[test]
    fn pci_capability_msi_constant() {
        assert_eq!(PCI_CAP_MSI, 0x05);
    }

    #[test]
    fn pci_capability_msix_constant() {
        assert_eq!(PCI_CAP_MSIX, 0x11);
    }

    #[test]
    fn pci_error_display_variants() {
        // Ensure PciError variants can be compared.
        assert_ne!(PciError::CapabilityNotFound, PciError::NoCapabilitiesList);
        assert_eq!(PciError::CapabilityNotFound, PciError::CapabilityNotFound);
    }

    #[test]
    fn header_type_multi_function_bit() {
        // Verify the multi-function bit constant is bit 7.
        assert_eq!(HEADER_TYPE_MULTI_FN, 0x80);
    }

    #[test]
    #[ignore] // Requires PCI I/O port access (bare-metal or QEMU only).
    fn find_device_none_bus_scans_all() {
        // find_device with None should scan all 256 buses.
        let result = find_device(0x1AF4, 0x1000, None);
        assert!(result.is_none());
    }

    #[test]
    #[ignore] // Requires PCI I/O port access (bare-metal or QEMU only).
    fn find_device_specific_bus() {
        // find_device with a specific bus should scan only that bus.
        let result = find_device(0x1AF4, 0x1000, Some(0));
        assert!(result.is_none());
    }

    #[test]
    fn status_capabilities_list_bit() {
        // Status bit 4 in the status register corresponds to bit 20 of the
        // status/command DWORD.
        assert_eq!(STATUS_CAPABILITIES_LIST, 1 << 20);
    }

    #[test]
    fn pci_device_header_type_field() {
        // Verify header_type and status are part of PciDevice.
        let dev = PciDevice {
            bus: 0,
            device: 0,
            function: 0,
            vendor_id: 0,
            device_id: 0,
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
            header_type: HEADER_TYPE_MULTI_FN,
            status: 0x0010, // capabilities list bit set
            interrupt_line: 0,
            interrupt_pin: 0,
        };
        assert_eq!(dev.header_type & HEADER_TYPE_MULTI_FN, HEADER_TYPE_MULTI_FN);
        assert_ne!(dev.status & (1 << 4), 0); // bit 4 of status word
    }
}
