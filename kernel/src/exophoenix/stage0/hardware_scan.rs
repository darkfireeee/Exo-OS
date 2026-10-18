//! # exophoenix/stage0/hardware_scan.rs — Énumération PCI et inspection des tables ACPI
//!
//! Gère l'énumération brute du bus PCI (périphériques et BARs), le calcul de taille
//! du pool Ring 3, et l'analyse/hachage cryptographique des tables ACPI (MADT/FADT/FACS/HPET).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use crate::arch::x86_64;
use crate::arch::x86_64::acpi::parser;
use crate::fs::exofs::core::types::BlobId;
use crate::security::crypto::blake3::blake3_hash;
use super::features::B_FEATURES;

pub const MAX_B_DEVICES: usize = 256;
pub const ACPI_MAX_TABLE_LEN: usize = 256 * 1024;

const PCI_CFG_ADDR: u16 = 0xCF8;
const PCI_CFG_DATA: u16 = 0xCFC;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct PciDeviceRecord {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub header_type: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub bar_bitmap: u8,
}

pub const EMPTY_PCI_DEVICE: PciDeviceRecord = PciDeviceRecord {
    bus: 0,
    device: 0,
    function: 0,
    header_type: 0,
    vendor_id: 0,
    device_id: 0,
    class_code: 0,
    subclass: 0,
    prog_if: 0,
    bar_bitmap: 0,
};

pub struct DeviceTableCell(UnsafeCell<[PciDeviceRecord; MAX_B_DEVICES]>);
unsafe impl Sync for DeviceTableCell {}

pub struct DriverBlobTableCell(UnsafeCell<[BlobId; MAX_B_DEVICES]>);
unsafe impl Sync for DriverBlobTableCell {}

pub static B_DEVICE_TABLE: DeviceTableCell =
    DeviceTableCell(UnsafeCell::new([EMPTY_PCI_DEVICE; MAX_B_DEVICES]));
pub static B_DRIVER_BLOB_TABLE: DriverBlobTableCell =
    DriverBlobTableCell(UnsafeCell::new([BlobId::ZERO; MAX_B_DEVICES]));
pub static B_DEVICE_COUNT: AtomicU16 = AtomicU16::new(0);
pub static B_DEVICE_BAR_COUNT: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug)]
pub struct Stage0AcpiSnapshot {
    pub madt_phys: u64,
    pub fadt_phys: u64,
    pub facs_phys: u64,
    pub hpet_phys: u64,
}

static MADT_HASH_QWORDS: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

#[inline(always)]
pub fn align_up_u64(value: u64, align: u64) -> u64 {
    if align == 0 {
        value
    } else {
        (value + align - 1) & !(align - 1)
    }
}

#[inline(always)]
pub fn pci_read_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    // SAFETY: accès CF8/CFC en ring0 pendant bootstrap Stage0.
    unsafe {
        x86_64::outl(PCI_CFG_ADDR, addr);
        x86_64::inl(PCI_CFG_DATA)
    }
}

#[inline(always)]
pub fn pci_vendor_id(bus: u8, device: u8, function: u8) -> u16 {
    (pci_read_dword(bus, device, function, 0x00) & 0xFFFF) as u16
}

/// Formule V4-M2 :
/// base fixe + contribution par device + contribution par BAR, puis alignement 2 MiB.
pub fn calc_pool_r3_size(device_count: usize, bar_count: usize) -> u64 {
    const BASE: u64 = 8 * 1024 * 1024;
    const PER_DEVICE: u64 = 64 * 1024;
    const PER_BAR: u64 = 16 * 1024;
    const ALIGN: u64 = 2 * 1024 * 1024;
    const MAX: u64 = 256 * 1024 * 1024;

    let raw = BASE
        .saturating_add((device_count as u64).saturating_mul(PER_DEVICE))
        .saturating_add((bar_count as u64).saturating_mul(PER_BAR));
    let aligned = align_up_u64(raw, ALIGN);
    aligned.min(MAX)
}

/// Étape 5b : enum PCI (devices + BARs) + taille pool R3.
pub fn enumerate_pci_devices() -> usize {
    B_DEVICE_COUNT.store(0, Ordering::Release);
    B_DEVICE_BAR_COUNT.store(0, Ordering::Release);

    let mut count = 0usize;
    let mut bar_count = 0usize;

    for bus in 0u16..=255u16 {
        let bus = bus as u8;
        for dev in 0u8..32u8 {
            if pci_vendor_id(bus, dev, 0) == 0xFFFF {
                continue;
            }

            let header0 = ((pci_read_dword(bus, dev, 0, 0x0C) >> 16) & 0xFF) as u8;
            let fn_count = if (header0 & 0x80) != 0 { 8 } else { 1 };

            for func in 0u8..fn_count {
                let id = pci_read_dword(bus, dev, func, 0x00);
                let vendor_id = (id & 0xFFFF) as u16;
                if vendor_id == 0xFFFF {
                    continue;
                }

                let device_id = (id >> 16) as u16;
                let class_reg = pci_read_dword(bus, dev, func, 0x08);
                let class_code = ((class_reg >> 24) & 0xFF) as u8;
                let subclass = ((class_reg >> 16) & 0xFF) as u8;
                let prog_if = ((class_reg >> 8) & 0xFF) as u8;
                let header_type = ((pci_read_dword(bus, dev, func, 0x0C) >> 16) & 0xFF) as u8;

                let bar_slots = if (header_type & 0x7F) == 0x01 { 2 } else { 6 };
                let mut bar_bitmap = 0u8;
                for bar_idx in 0..bar_slots {
                    let bar_offset = 0x10 + (bar_idx * 4) as u8;
                    let bar = pci_read_dword(bus, dev, func, bar_offset);
                    if bar != 0 && bar != 0xFFFF_FFFF {
                        bar_bitmap |= 1u8 << bar_idx;
                        bar_count += 1;
                    }
                }

                if count < MAX_B_DEVICES {
                    // SAFETY: bootstrap single-thread ; count borné < MAX_B_DEVICES.
                    unsafe {
                        (*B_DEVICE_TABLE.0.get())[count] = PciDeviceRecord {
                            bus,
                            device: dev,
                            function: func,
                            header_type,
                            vendor_id,
                            device_id,
                            class_code,
                            subclass,
                            prog_if,
                            bar_bitmap,
                        };
                        (*B_DRIVER_BLOB_TABLE.0.get())[count] = BlobId::ZERO;
                    }
                }

                count = count.saturating_add(1);
            }
        }
    }

    let clamped = count.min(MAX_B_DEVICES);
    B_DEVICE_COUNT.store(clamped as u16, Ordering::Release);
    B_DEVICE_BAR_COUNT.store(bar_count as u32, Ordering::Release);

    let pool_size = calc_pool_r3_size(clamped, bar_count);
    super::isolation::set_pool_r3_size_bytes(pool_size);

    clamped
}

pub fn b_device_count() -> usize {
    B_DEVICE_COUNT.load(Ordering::Acquire) as usize
}

pub fn b_device(index: usize) -> Option<PciDeviceRecord> {
    if index >= b_device_count() || index >= MAX_B_DEVICES {
        return None;
    }
    // SAFETY: table statique ; écritures uniquement au boot stage0.
    Some(unsafe { (*B_DEVICE_TABLE.0.get())[index] })
}

/// Retourne le BlobId driver associé à un BDF (clé 0x00BB_DDFF), si présent.
pub fn driver_blob_id(bdf_key: u32) -> Option<BlobId> {
    let count = b_device_count();
    let mut idx = 0usize;
    while idx < count {
        let dev = b_device(idx)?;
        let key = ((dev.bus as u32) << 16) | ((dev.device as u32) << 8) | dev.function as u32;
        if key == bdf_key {
            // SAFETY: écritures uniquement au boot stage0 ; lecture en RO ensuite.
            let blob = unsafe { (*B_DRIVER_BLOB_TABLE.0.get())[idx] };
            if blob == BlobId::ZERO {
                return None;
            }
            return Some(blob);
        }
        idx += 1;
    }
    None
}

pub fn parse_facs_from_fadt(fadt_phys: u64) -> u64 {
    if fadt_phys < 0x1000 || fadt_phys >= 0x4000_0000 {
        return 0;
    }

    // SdtHeader.length à offset +4.
    // SAFETY: zone ACPI identity-mappée et validée grossièrement ci-dessus.
    let fadt_len =
        unsafe { core::ptr::read_unaligned((fadt_phys as usize + 4) as *const u32) } as usize;

    if fadt_len < 40 {
        return 0;
    }

    // ACPI 1.0 FACS 32-bit (firmware_ctrl) à offset 36.
    // SAFETY: fadt_len vérifié >= 40.
    let facs32 =
        unsafe { core::ptr::read_unaligned((fadt_phys as usize + 36) as *const u32) } as u64;

    // ACPI 2.0+ X_FIRMWARE_CTRL à offset 132 (si table assez grande).
    let facs64 = if fadt_len >= 140 {
        // SAFETY: fadt_len vérifié >= 140.
        unsafe { core::ptr::read_unaligned((fadt_phys as usize + 132) as *const u64) }
    } else {
        0
    };

    if facs64 != 0 {
        facs64
    } else {
        facs32
    }
}

/// Étape 5 : parse ACPI MADT/FADT/FACS.
pub fn parse_stage0_acpi() -> Stage0AcpiSnapshot {
    let info = if parser::acpi_available() {
        Some(*parser::acpi_info())
    } else {
        parser::init_acpi()
    };

    let acpi = if let Some(info) = info {
        Stage0AcpiSnapshot {
            madt_phys: info.madt_phys,
            fadt_phys: info.fadt_phys,
            facs_phys: parse_facs_from_fadt(info.fadt_phys),
            hpet_phys: info.hpet_phys,
        }
    } else {
        Stage0AcpiSnapshot {
            madt_phys: 0,
            fadt_phys: 0,
            facs_phys: 0,
            hpet_phys: 0,
        }
    };

    B_FEATURES
        .hpet_available
        .store(acpi.hpet_phys != 0, Ordering::Release);

    acpi
}

pub fn acpi_table_len(table_phys: u64) -> Option<usize> {
    if table_phys < 0x1000 || table_phys >= 0x4000_0000 {
        return None;
    }
    // SAFETY: ACPI identity-map bootstrap ; lecture header SDT length @ +4.
    let len =
        unsafe { core::ptr::read_unaligned((table_phys as usize + 4) as *const u32) } as usize;
    if len < 36 || len > ACPI_MAX_TABLE_LEN {
        None
    } else {
        Some(len)
    }
}

pub fn store_madt_hash(hash: [u8; 32]) {
    for (idx, chunk) in hash.chunks_exact(8).enumerate() {
        let mut q = [0u8; 8];
        q.copy_from_slice(chunk);
        MADT_HASH_QWORDS[idx].store(u64::from_le_bytes(q), Ordering::Release);
    }
}

pub fn load_madt_hash() -> [u8; 32] {
    let mut out = [0u8; 32];
    for idx in 0..4 {
        let q = MADT_HASH_QWORDS[idx].load(Ordering::Acquire).to_le_bytes();
        out[idx * 8..idx * 8 + 8].copy_from_slice(&q);
    }
    out
}

/// Étape 10 : calcule et stocke le hash MADT (BLAKE3, 32 bytes).
pub fn hash_and_store_madt(madt_phys: u64) -> Option<[u8; 32]> {
    let len = acpi_table_len(madt_phys)?;
    // SAFETY: table ACPI identity-mappée ; longueur bornée et validée ci-dessus.
    let bytes = unsafe { core::slice::from_raw_parts(madt_phys as *const u8, len) };
    let hash = blake3_hash(bytes);
    store_madt_hash(hash);
    Some(hash)
}

pub fn madt_hash() -> [u8; 32] {
    load_madt_hash()
}
