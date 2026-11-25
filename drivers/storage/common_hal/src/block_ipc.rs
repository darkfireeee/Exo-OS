//! block_ipc.rs — Serveur IPC universel de périphérique bloc pour Exo-OS Ring 3
//!
//! Conforme aux spécifications formelles TLA+ :
//! - S31 : Invariant ACK FLUSH synchrone avant confirmation de synchronisation VfsTree.
//! - S33 : Isolation ExoCordon sur les flux IPC de stockage.
//! - SRV-02 : #![no_std], arithmétique entière pure, aucun flottant ni SIMD.

use exo_syscall_abi as syscall;
pub use syscall::{
    BlockCmdWire, BlockIpcReply, BlockIpcRequest, BlockReplyWire, BLOCK_DEFAULT_SIZE,
    BLOCK_OP_FLUSH, BLOCK_OP_GET_INFO, BLOCK_OP_READ, BLOCK_OP_WRITE,
};

/// Trait unifié de périphérique bloc abstrait pour les drivers Ring 3
pub trait BlockDeviceOps {
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), i64>;
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), i64>;
    fn flush(&mut self) -> Result<(), i64>;
    fn total_blocks(&self) -> u64;
    fn block_size(&self) -> u32;
}

/// Boucle de service principale d'un démon de stockage
pub fn run_block_server<D: BlockDeviceOps>(
    endpoint_id: u64,
    service_name: &[u8],
    dev: &mut D,
) -> ! {
    // 1. Enregistrement du service auprès de l'IPC router
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            service_name.as_ptr() as u64,
            service_name.len() as u64,
            endpoint_id,
        )
    };

    let mut request_raw = [0u8; syscall::IPC_ENVELOPE_SIZE];
    let mut block_scratch = [0u8; BLOCK_DEFAULT_SIZE];

    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                endpoint_id,
                request_raw.as_mut_ptr() as u64,
                request_raw.len() as u64,
                0, // Bloquant
            )
        };

        if rc < syscall::IPC_HEADER_SIZE as i64 {
            continue;
        }

        let sender_pid = u32::from_ne_bytes([request_raw[0], request_raw[1], request_raw[2], request_raw[3]]);
        let _msg_type = u32::from_ne_bytes([request_raw[4], request_raw[5], request_raw[6], request_raw[7]]);

        let cmd: BlockCmdWire = unsafe {
            core::ptr::read_unaligned(
                request_raw.as_ptr().add(syscall::IPC_HEADER_SIZE) as *const BlockCmdWire
            )
        };

        // T12-001 : Validation stricte du tampon mémoire fourni pour le transfert
        const USER_SPACE_MAX: u64 = 0x0000_8000_0000_0000;
        let is_valid_dma_target = |addr: u64| -> bool {
            addr != 0
                && addr < USER_SPACE_MAX
                && addr.checked_add(BLOCK_DEFAULT_SIZE as u64).map_or(false, |end| end <= USER_SPACE_MAX)
        };

        let reply_wire = match cmd.opcode {
            BLOCK_OP_GET_INFO => {
                BlockReplyWire::ok(0, 0, dev.block_size(), dev.total_blocks())
            }
            BLOCK_OP_READ => {
                if cmd.dma_phys != 0 && !is_valid_dma_target(cmd.dma_phys) {
                    BlockReplyWire::error(syscall::EFAULT)
                } else {
                    match dev.read_block(cmd.lba, &mut block_scratch) {
                        Ok(()) => {
                            if cmd.dma_phys != 0 {
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        block_scratch.as_ptr(),
                                        cmd.dma_phys as *mut u8,
                                        BLOCK_DEFAULT_SIZE,
                                    );
                                }
                            }
                            BlockReplyWire::ok(cmd.lba, 1, dev.block_size(), dev.total_blocks())
                        }
                        Err(err) => BlockReplyWire::error(err),
                    }
                }
            }
            BLOCK_OP_WRITE => {
                if cmd.dma_phys != 0 && !is_valid_dma_target(cmd.dma_phys) {
                    BlockReplyWire::error(syscall::EFAULT)
                } else {
                    if cmd.dma_phys != 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                cmd.dma_phys as *const u8,
                                block_scratch.as_mut_ptr(),
                                BLOCK_DEFAULT_SIZE,
                            );
                        }
                    }
                    match dev.write_block(cmd.lba, &block_scratch) {
                        Ok(()) => BlockReplyWire::ok(cmd.lba, 1, dev.block_size(), dev.total_blocks()),
                        Err(err) => BlockReplyWire::error(err),
                    }
                }
            }
            BLOCK_OP_FLUSH => {
                // Invariant TLA+ S31 : flush synchrone garanti avant d'acquitter
                match dev.flush() {
                    Ok(()) => BlockReplyWire::ok(0, 0, dev.block_size(), dev.total_blocks()),
                    Err(err) => BlockReplyWire::error(err),
                }
            }
            _ => BlockReplyWire::error(syscall::EINVAL),
        };

        // Envoi de la réponse à l'émetteur
        let mut reply_packet = [0u8; syscall::IPC_ENVELOPE_SIZE];
        reply_packet[0..4].copy_from_slice(&0u32.to_ne_bytes());
        reply_packet[4..8].copy_from_slice(&cmd.opcode.to_ne_bytes());
        unsafe {
            core::ptr::write_unaligned(
                reply_packet.as_mut_ptr().add(syscall::IPC_HEADER_SIZE) as *mut BlockReplyWire,
                reply_wire,
            );
        }

        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                sender_pid as u64,
                reply_packet.as_ptr() as u64,
                reply_packet.len() as u64,
                0,
                0,
                0,
            )
        };
    }
}
