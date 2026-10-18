//! block_ipc.rs — Serveur IPC universel de périphérique bloc pour Exo-OS Ring 3
//!
//! Conforme aux spécifications formelles TLA+ :
//! - S31 : Invariant ACK FLUSH synchrone avant confirmation de synchronisation VfsTree.
//! - S33 : Isolation ExoCordon sur les flux IPC de stockage.
//! - SRV-02 : #![no_std], arithmétique entière pure, aucun flottant ni SIMD.

use exo_syscall_abi as syscall;

/// Opcodes du protocole IPC de stockage Exo-OS
pub const BLOCK_OP_READ: u32 = 0x3001;
pub const BLOCK_OP_WRITE: u32 = 0x3002;
pub const BLOCK_OP_FLUSH: u32 = 0x3003;
pub const BLOCK_OP_GET_INFO: u32 = 0x3004;

pub const BLOCK_DEFAULT_SIZE: usize = 4096;

/// Commande de transfert de bloc (format fil encapsulé dans le payload IPC)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlockCmdWire {
    pub opcode: u32,
    pub flags: u32,
    pub lba: u64,
    pub count: u32,
    pub _pad: u32,
    pub dma_phys: u64,
    pub cap_token: [u8; 20],
    pub _reserved: [u8; 8],
}

impl BlockCmdWire {
    pub const fn zeroed() -> Self {
        Self {
            opcode: 0,
            flags: 0,
            lba: 0,
            count: 0,
            _pad: 0,
            dma_phys: 0,
            cap_token: [0u8; 20],
            _reserved: [0u8; 8],
        }
    }
}

/// Réponse du serveur de stockage (format fil encapsulé dans le payload IPC)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlockReplyWire {
    pub status: i64,
    pub lba: u64,
    pub count: u32,
    pub block_size: u32,
    pub total_blocks: u64,
    pub _reserved: [u8; 32],
}

impl BlockReplyWire {
    pub const fn ok(lba: u64, count: u32, block_size: u32, total_blocks: u64) -> Self {
        Self {
            status: 0,
            lba,
            count,
            block_size,
            total_blocks,
            _reserved: [0u8; 32],
        }
    }

    pub const fn error(status: i64) -> Self {
        Self {
            status,
            lba: 0,
            count: 0,
            block_size: 0,
            total_blocks: 0,
            _reserved: [0u8; 32],
        }
    }
}

/// Enveloppe complète de requête IPC compatible avec `syscall::IPC_ENVELOPE_SIZE`
#[repr(C)]
pub struct BlockIpcRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub cmd: BlockCmdWire,
    pub _pad: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE - core::mem::size_of::<BlockCmdWire>()],
}

const _: () = assert!(core::mem::size_of::<BlockIpcRequest>() == syscall::IPC_ENVELOPE_SIZE);

/// Enveloppe complète de réponse IPC compatible avec `syscall::IPC_ENVELOPE_SIZE`
#[repr(C)]
pub struct BlockIpcReply {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply: BlockReplyWire,
    pub _pad: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE - core::mem::size_of::<BlockReplyWire>()],
}

const _: () = assert!(core::mem::size_of::<BlockIpcReply>() == syscall::IPC_ENVELOPE_SIZE);

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

        let reply_wire = match cmd.opcode {
            BLOCK_OP_GET_INFO => {
                BlockReplyWire::ok(0, 0, dev.block_size(), dev.total_blocks())
            }
            BLOCK_OP_READ => {
                match dev.read_block(cmd.lba, &mut block_scratch) {
                    Ok(()) => {
                        // Si un tampon physique de destination DMA est fourni, copier
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
            BLOCK_OP_WRITE => {
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
