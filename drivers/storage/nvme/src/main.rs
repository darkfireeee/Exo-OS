//! nvme_driver — Démon de stockage NVMe en espace utilisateur Ring 3
//!
//! Respecte les invariants formels TLA+ :
//! - S19/S20 : Revendication atomique BDF/MMIO (`SYS_PCI_CLAIM`).
//! - S24 : Topologie PCI avant IRQ (`SYS_PCI_SET_TOPOLOGY`).
//! - S31 : Acquittement synchrone FLUSH avant mise à jour de l'arbre VFS.
//! - SRV-02 : #![no_std] strict, arithmétique entière pure.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_nvme::NvmeDevice;
use exo_storage_common_hal::{BlockReplyWire, ExoUserspaceStorageHal};
use exo_syscall_abi as syscall;

pub const NVME_DRIVER_ENDPOINT: u64 = 31;
const SERVICE_NAME: &[u8] = b"nvme_driver";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut pci_info = syscall::PciDeviceInfo::default();
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_PCI_FIND_DEVICE,
            0, // Wildcard vendor
            0, // Wildcard device
            1, // Class Mass Storage
            8, // Subclass Non-Volatile Memory
            0, // Index 0
            &mut pci_info as *mut _ as u64,
        )
    };

    if rc >= 0 {
        let (bar_phys, bar_size) = (pci_info.bar0_phys, pci_info.bar0_size);
        let bdf = ((pci_info.bus as u32) << 8)
            | ((pci_info.device as u32) << 3)
            | (pci_info.function as u32);

        if bar_phys != 0 && bar_size != 0 {
            if let Ok(hal) = ExoUserspaceStorageHal::claim_and_map_pci(bar_phys, bar_size, bdf) {
                if let Ok(mut dev) = NvmeDevice::new(hal) {
                    exo_storage_common_hal::run_block_server(
                        NVME_DRIVER_ENDPOINT,
                        SERVICE_NAME,
                        &mut dev,
                    );
                }
            }
        }
    }

    // Fallback passif si aucun matériel détecté ou échec d'initialisation
    passive_loop()
}

fn passive_loop() -> ! {
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            SERVICE_NAME.as_ptr() as u64,
            SERVICE_NAME.len() as u64,
            NVME_DRIVER_ENDPOINT,
        )
    };

    let mut buf = [0u8; syscall::IPC_ENVELOPE_SIZE];
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                NVME_DRIVER_ENDPOINT,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                0,
            )
        };
        if rc >= syscall::IPC_HEADER_SIZE as i64 {
            let sender_pid = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let reply_wire = BlockReplyWire::error(syscall::ENODEV);
            let mut reply_packet = [0u8; syscall::IPC_ENVELOPE_SIZE];
            unsafe {
                core::ptr::write_unaligned(
                    reply_packet.as_mut_ptr().add(syscall::IPC_HEADER_SIZE) as *mut BlockReplyWire,
                    reply_wire,
                );
                let _ = syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    sender_pid as u64,
                    reply_packet.as_ptr() as u64,
                    reply_packet.len() as u64,
                    0,
                    0,
                    0,
                );
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
