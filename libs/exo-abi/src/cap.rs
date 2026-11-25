//! Canonical Capability Token wire layouts and rights (`CapTokens.tla` / `CAP-01`).
//!
//! Unifies:
//! - [`CapTokenWire`] / [`ExoCapTokenWire`] (`20` bytes): inline IPC capability footer
//! - [`CapTokenUser`] (`48` bytes): initial process capability token passed via `AT_CAP_TOKEN` (`52`)
//! - [`CapObjectType`] & [`rights`]: canonical discriminants aligned with Ring 0 (`IpcEndpoint = 1`, `MemoryRegion = 2`)

pub const CAP_TOKEN_WIRE_SIZE: usize = 20;
pub const EXO_CAP_TOKEN_WIRE_SIZE: usize = CAP_TOKEN_WIRE_SIZE;
pub const CAP_TOKEN_USER_SIZE: usize = 48;

pub const EXO_CAP_TYPE_IPC_ENDPOINT: u32 = 1;

pub const EXO_CAP_RIGHT_IPC_CONNECT: u32 = 1 << 6;
pub const EXO_CAP_RIGHT_IPC_SEND: u32 = 1 << 7;
pub const EXO_CAP_RIGHT_IPC_RECV: u32 = 1 << 8;
pub const EXO_CAP_RIGHT_IPC_MANAGE: u32 = 1 << 9;

/// Protected object types for wire capability tokens (`IpcEndpoint = 1`, `MemoryRegion = 2`).
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapObjectType {
    Null = 0,
    IpcEndpoint = 1,
    MemoryRegion = 2,
    FileObject = 3,
    DeviceBar = 4,
    IrqLine = 5,
    NetworkSession = 6,
    PortalDevice = 7,
    SecurityDomain = 8,
    DmaBuffer = 9,
    CryptoKey = 10,
    BpfProgram = 11,
}

/// Standardized `u32` capability permission bitmask.
pub mod rights {
    pub const READ: u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const EXECUTE: u32 = 1 << 2;
    pub const GRANT: u32 = 1 << 3;
    pub const REVOKE: u32 = 1 << 4;
    pub const DELEGATE: u32 = 1 << 5;
    pub const IPC_CONNECT: u32 = 1 << 6;
    pub const IPC_SEND: u32 = 1 << 7;
    pub const IPC_RECV: u32 = 1 << 8;
    pub const IPC_MANAGE: u32 = 1 << 9;
    pub const MEM_MAP: u32 = 1 << 10;
    pub const MEM_PROTECT: u32 = 1 << 11;
    pub const MEM_UNMAP: u32 = 1 << 12;
    pub const DEV_MMIO: u32 = 1 << 13;
    pub const DMA_MAP: u32 = 1 << 14;
    pub const IRQ_BIND: u32 = 1 << 15;
    pub const NET_TX: u32 = 1 << 16;
    pub const NET_RX: u32 = 1 << 17;
    pub const NET_ADMIN: u32 = 1 << 18;
    pub const NET_OBSERVE: u32 = 1 << 19;
}

/// 20-byte raw IPC capability wire wrapper.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExoCapTokenWire {
    pub bytes: [u8; EXO_CAP_TOKEN_WIRE_SIZE],
}

const _: () = assert!(core::mem::size_of::<ExoCapTokenWire>() == 20);

impl ExoCapTokenWire {
    #[inline(always)]
    pub const fn empty() -> Self {
        Self {
            bytes: [0u8; EXO_CAP_TOKEN_WIRE_SIZE],
        }
    }

    #[inline(always)]
    pub fn object_id(self) -> u64 {
        u64::from_ne_bytes([
            self.bytes[0],
            self.bytes[1],
            self.bytes[2],
            self.bytes[3],
            self.bytes[4],
            self.bytes[5],
            self.bytes[6],
            self.bytes[7],
        ])
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.object_id() == 0
    }
}

/// 20-byte structured wire capability token (`CapTokens.tla`).
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapTokenWire {
    pub object_id: u64,
    pub rights: u32,
    pub target_pid: u32,
    pub nonce: u32,
}

const _: () = assert!(core::mem::size_of::<CapTokenWire>() == 20);

impl CapTokenWire {
    #[inline]
    pub const fn new(object_id: u64, rights: u32, target_pid: u32, nonce: u32) -> Self {
        Self {
            object_id,
            rights,
            target_pid,
            nonce,
        }
    }

    #[inline]
    pub fn to_bytes(&self) -> [u8; CAP_TOKEN_WIRE_SIZE] {
        let mut out = [0u8; CAP_TOKEN_WIRE_SIZE];
        out[0..8].copy_from_slice(&self.object_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.rights.to_le_bytes());
        out[12..16].copy_from_slice(&self.target_pid.to_le_bytes());
        out[16..20].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }

    #[inline]
    pub fn from_bytes(bytes: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Self {
        let object_id = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let rights = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let target_pid = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let nonce = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        Self {
            object_id,
            rights,
            target_pid,
            nonce,
        }
    }
}

/// 48-byte userspace initial capability token (`AT_CAP_TOKEN = 52`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapTokenUser {
    pub raw: [u8; CAP_TOKEN_USER_SIZE],
}

const _: () = assert!(core::mem::size_of::<CapTokenUser>() == 48);

impl Default for CapTokenUser {
    #[inline]
    fn default() -> Self {
        Self {
            raw: [0u8; CAP_TOKEN_USER_SIZE],
        }
    }
}

impl CapTokenUser {
    #[inline]
    pub fn generation(&self) -> u32 {
        u32::from_le_bytes([self.raw[32], self.raw[33], self.raw[34], self.raw[35]])
    }

    #[inline]
    pub fn rights(&self) -> u32 {
        u32::from_le_bytes([self.raw[36], self.raw[37], self.raw[38], self.raw[39]])
    }

    #[inline]
    pub fn owner_pid(&self) -> u32 {
        u32::from_le_bytes([self.raw[40], self.raw[41], self.raw[42], self.raw[43]])
    }
}
