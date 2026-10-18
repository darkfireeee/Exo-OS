//! # cap_abi.rs — Format fil et définitions ABI des jetons de capacité (CapTokens)
//!
//! Conforme aux spécifications formelles TLA+ (`CapTokens.tla`) et au modèle
//! de sécurité Zero-Trust d'Exo-OS. Format compact non-forgeable de 20 octets.

pub const CAP_TOKEN_WIRE_SIZE: usize = 20;

/// Types d'objets protégés par les jetons de capacité
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapObjectType {
    Null = 0,
    MemoryRegion = 1,
    IpcEndpoint = 2,
    FileObject = 3,
    DeviceBar = 4,
    IrqLine = 5,
    NetworkSession = 6,
    PortalDevice = 7,
    SecurityDomain = 8,
}

/// Droits standardisés sous forme de bitmask u32
pub mod rights {
    pub const READ: u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const EXECUTE: u32 = 1 << 2;
    pub const GRANT: u32 = 1 << 3;
    pub const REVOKE: u32 = 1 << 4;
    pub const IPC_SEND: u32 = 1 << 5;
    pub const IPC_RECV: u32 = 1 << 6;
    pub const DMA_MAP: u32 = 1 << 7;
    pub const IRQ_BIND: u32 = 1 << 8;
    pub const NET_TX: u32 = 1 << 9;
    pub const NET_RX: u32 = 1 << 10;
    pub const NET_ADMIN: u32 = 1 << 11;
}

/// Jeton de capacité fil (exactement 20 octets)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapTokenWire {
    /// Identifiant unique d'objet (8 octets)
    pub object_id: u64,
    /// Masque de droits autorisés (4 octets)
    pub rights: u32,
    /// PID cible détenteur exclusif (4 octets)
    pub target_pid: u32,
    /// Nonce cryptographique anti-rejeu (4 octets)
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
