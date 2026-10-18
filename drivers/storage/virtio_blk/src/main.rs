//! virtio_blk_driver — Démon de stockage VirtIO-Block en espace utilisateur Ring 3
//!
//! Respecte les invariants formels TLA+ :
//! - S31 : Invariant ACK FLUSH synchrone avant confirmation de synchronisation VfsTree.
//! - S19/S20 : Revendication atomique BDF/MMIO (`SYS_PCI_CLAIM`).
//! - S24 : Topologie PCI avant IRQ (`SYS_PCI_SET_TOPOLOGY`).
//! - SRV-02 : #![no_std] strict, arithmétique entière pure.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_storage_common_hal::BlockReplyWire;
use exo_syscall_abi as syscall;
use exo_virtio_blk::ExoVirtioBlkDevice;

pub const VIRTIO_BLK_DRIVER_ENDPOINT: u64 = 32;
const SERVICE_NAME: &[u8] = b"virtio_blk_driver";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut pci_info = syscall::PciDeviceInfo::default();
    // Recherche périphérique VirtIO-Block Legacy (0x1AF4, 0x1001) ou Modern (0x1042)
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_PCI_FIND_DEVICE,
            0x1AF4, // Red Hat VirtIO vendor
            0x1001, // VirtIO Block legacy device ID
            0xFFFF, // Wildcard class
            0xFFFF, // Wildcard subclass
            0,      // Index 0
            &mut pci_info as *mut _ as u64,
        )
    };

    if rc >= 0 {
        // En VirtIO Legacy, BAR0 est typiquement une région I/O port
        let io_base = if pci_info.bar0_phys != 0 {
            pci_info.bar0_phys as u16
        } else {
            0xC000 // Port I/O standard VirtIO QEMU fallback
        };

        if let Ok(mut dev) = ExoVirtioBlkDevice::new_legacy_pci(io_base) {
            exo_storage_common_hal::run_block_server(
                VIRTIO_BLK_DRIVER_ENDPOINT,
                SERVICE_NAME,
                &mut dev,
            );
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
            VIRTIO_BLK_DRIVER_ENDPOINT,
        )
    };

    let mut buf = [0u8; syscall::IPC_ENVELOPE_SIZE];
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                VIRTIO_BLK_DRIVER_ENDPOINT,
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
