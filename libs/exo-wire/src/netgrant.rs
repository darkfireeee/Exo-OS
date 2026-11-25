//! Canonical ExoNet V5 NetGrant wire ABI (`S39`, `S40`, `PciDoExit.tla`).

pub const NETGRANT_ABI_VERSION_V0: u16 = 0;
pub const NETGRANT_SLOT_SIZE: usize = 2048;
pub const NETGRANT_MAX_SLOTS: usize = 256;

pub const NET_GRANT_OP_BEGIN_APP_TX: u16 = 1;
pub const NET_GRANT_OP_BEGIN_APP_RX: u16 = 2;
pub const NET_GRANT_OP_PORTAL_ALLOC_POOL: u16 = 3;
pub const NET_GRANT_OP_ACQUIRE_PORTAL_TX: u16 = 4;
pub const NET_GRANT_OP_SEAL_PORTAL_TX: u16 = 5;
pub const NET_GRANT_OP_PUBLISH_PORTAL_TX: u16 = 6;
pub const NET_GRANT_OP_ACQUIRE_PORTAL_RX: u16 = 7;
pub const NET_GRANT_OP_RETURN_PORTAL_RX: u16 = 8;
pub const NET_GRANT_OP_ACK_RETURN_PORTAL_RX: u16 = 9;
pub const NET_GRANT_OP_COMPLETE_APP: u16 = 10;
pub const NET_GRANT_OP_CANCEL: u16 = 11;
pub const NET_GRANT_OP_PORTAL_RESET: u16 = 12;

pub const NET_CTRL_OP_REGISTER_PORTAL: u32 = 1;
pub const NET_CTRL_OP_PUBLISH_POLICY: u32 = 2;
pub const NET_CTRL_OP_ISSUE_LEASE: u32 = 3;
pub const NET_CTRL_OP_MATERIALIZE_BRIN: u32 = 4;
pub const NET_CTRL_OP_REVOKE_PACT: u32 = 5;
pub const NET_CTRL_OP_MAP_AUTHORITY_VIEW: u32 = 6;
pub const NET_CTRL_OP_ACK_RECOVERY: u32 = 7;

pub const NETGRANT_F_READ: u32 = 1 << 0;
pub const NETGRANT_F_WRITE: u32 = 1 << 1;
pub const NETGRANT_F_DMA_LOCKED: u32 = 1 << 2;
pub const NETGRANT_F_TX: u32 = 1 << 3;
pub const NETGRANT_F_RX: u32 = 1 << 4;
pub const NETGRANT_F_COMPLETED: u32 = 1 << 5;
pub const NETGRANT_F_ERROR: u32 = 1 << 6;

pub const NETGRANT_STATE_FREE: u16 = 0;
pub const NETGRANT_STATE_MAPPED_ENGINE_TX: u16 = 1;
pub const NETGRANT_STATE_READY_TX: u16 = 2;
pub const NETGRANT_STATE_PUBLISHED_TX: u16 = 3;
pub const NETGRANT_STATE_PUBLISHED_RX: u16 = 4;
pub const NETGRANT_STATE_MAPPED_ENGINE_RX: u16 = 5;
pub const NETGRANT_STATE_RETURN_PENDING: u16 = 6;
pub const NETGRANT_STATE_QUARANTINE: u16 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetGrantRequestV0 {
    pub abi_version: u16,
    pub op: u16,
    pub flags: u32,
    pub session_or_portal: u32,
    pub peer_instance: u32,
    pub address: u64,
    pub length: u32,
    pub _reserved: u32,
}

const _: () = assert!(core::mem::size_of::<NetGrantRequestV0>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetGrantReplyV0 {
    pub grant_id: u32,
    pub generation: u32,
    pub mapped_addr: u64,
    pub mapped_len: u32,
    pub state: u16,
    pub abi_version: u16,
    pub authority_epoch: u64,
}

const _: () = assert!(core::mem::size_of::<NetGrantReplyV0>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetGrantDesc {
    pub slot_id: u32,
    pub generation_id: u32,
    pub offset: u16,
    pub len: u16,
    pub flags: u32,
    pub _reserved: u32,
}

const _: () = assert!(core::mem::size_of::<NetGrantDesc>() == 20);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetGrantSlot {
    pub slot_id: u32,
    pub generation: u32,
    pub phys_addr: u64,
    pub virt_addr: u64,
    pub flags: u32,
    pub _pad: u32,
}

const _: () = assert!(core::mem::size_of::<NetGrantSlot>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetAuthorityView {
    pub abi_version: u16,
    pub authority_state: u16,
    pub policy_sequence: u32,
    pub authority_epoch: u64,
    pub portal_epoch: u64,
    pub root_digest_hint: u64,
}

const _: () = assert!(core::mem::size_of::<NetAuthorityView>() == 32);
