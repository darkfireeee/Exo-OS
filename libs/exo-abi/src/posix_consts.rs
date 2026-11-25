//! Canonical POSIX, ExoFS, IPC, and ExoPhoenix ABI flag constants and wire metadata structs.

pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_RDWR: u64 = 2;
pub const O_CREAT: u64 = 0x0040;
pub const O_EXCL: u64 = 0x0080;
pub const O_TRUNC: u64 = 0x0200;
pub const O_APPEND: u64 = 0x0400;
pub const O_NONBLOCK: u64 = 0x0800;
pub const O_CLOEXEC: u64 = 0x0008_0000;

pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;
pub const AT_FDCWD: i64 = -100;
pub const AT_REMOVEDIR: u64 = 0x200;

pub const SYNC_FILE_RANGE_WRITE: u32 = 1;
pub const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 2;
pub const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 4;

pub const FALLOC_FL_KEEP_SIZE: u32 = 0x01;
pub const FALLOC_FL_PUNCH_HOLE: u32 = 0x02;
pub const FALLOC_FL_COLLAPSE_RANGE: u32 = 0x08;
pub const FALLOC_FL_ZERO_RANGE: u32 = 0x10;
pub const FALLOC_FL_INSERT_RANGE: u32 = 0x20;
pub const FALLOC_FL_UNSHARE_RANGE: u32 = 0x40;

pub const RENAME_NOREPLACE: u32 = 1;
pub const RENAME_EXCHANGE: u32 = 2;
pub const RENAME_WHITEOUT: u32 = 4;

pub const STATX_TYPE: u32 = 0x0000_0001;
pub const STATX_MODE: u32 = 0x0000_0002;
pub const STATX_NLINK: u32 = 0x0000_0004;
pub const STATX_UID: u32 = 0x0000_0008;
pub const STATX_GID: u32 = 0x0000_0010;
pub const STATX_ATIME: u32 = 0x0000_0020;
pub const STATX_MTIME: u32 = 0x0000_0040;
pub const STATX_CTIME: u32 = 0x0000_0080;
pub const STATX_INO: u32 = 0x0000_0100;
pub const STATX_SIZE: u32 = 0x0000_0200;
pub const STATX_BLOCKS: u32 = 0x0000_0400;
pub const STATX_BASIC_STATS: u32 = 0x0000_07ff;
pub const STATX_BTIME: u32 = 0x0000_0800;
pub const STATX_ATTR_IMMUTABLE: u64 = 0x0000_0010;
pub const STATX_ATTR_VERITY: u64 = 0x0010_0000;

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;
pub const EPOLL_CLOEXEC: i32 = 0x0008_0000;

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;
pub const PROT_PHOENIX_SAFE: u64 = 0x10;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;

pub const MFD_CLOEXEC: u64 = 0x0001;
pub const MFD_ALLOW_SEALING: u64 = 0x0002;
pub const F_ADD_SEALS: u64 = 1033;
pub const F_GET_SEALS: u64 = 1034;
pub const F_SEAL_SEAL: u64 = 0x0001;
pub const F_SEAL_SHRINK: u64 = 0x0002;
pub const F_SEAL_GROW: u64 = 0x0004;
pub const F_SEAL_WRITE: u64 = 0x0008;

/// Canonical socket address family type (`sa_family_t = u16`).
pub type SaFamily = u16;
pub const AF_UNIX: SaFamily = 1;
pub const AF_INET: SaFamily = 2;
pub const AF_INET6: SaFamily = 10;

pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOL_SOCKET: i32 = 1;
pub const SCM_RIGHTS: i32 = 1;

pub const ARCH_SET_GS: u64 = 0x1001;
pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;
pub const ARCH_GET_GS: u64 = 0x1004;

pub const IPC_FLAG_TIMEOUT: u64 = 0x0001;
pub const IPC_FLAG_INJECT_SRC_PID: u64 = 0x0002;
pub const WNOHANG: u64 = 1;
pub const SA_RESTART: u64 = 0x1000_0000;
pub const SA_RESTORER: u64 = 0x0400_0000;

pub const EXO_PLEDGE_QUERY_FLAG: u64 = 1;
pub const EXO_RESTRICTION_NO_DLOPEN: u64 = 1 << 9;

// ExoFS capability rights bitmask
pub const EXOFS_RIGHT_READ: u32 = 1 << 0;
pub const EXOFS_RIGHT_WRITE: u32 = 1 << 1;
pub const EXOFS_RIGHT_CREATE: u32 = 1 << 2;
pub const EXOFS_RIGHT_DELETE: u32 = 1 << 3;
pub const EXOFS_RIGHT_STAT: u32 = 1 << 4;
pub const EXOFS_RIGHT_SETMETA: u32 = 1 << 5;
pub const EXOFS_RIGHT_LIST: u32 = 1 << 6;
pub const EXOFS_RIGHT_EXEC: u32 = 1 << 7;
pub const EXOFS_RIGHT_CHOWN: u32 = 1 << 8;
pub const EXOFS_RIGHT_CHMOD: u32 = 1 << 9;
pub const EXOFS_RIGHT_INSPECT_CONTENT: u32 = 1 << 10;
pub const EXOFS_RIGHT_SNAPSHOT_CREATE: u32 = 1 << 11;
pub const EXOFS_RIGHT_RELATION_CREATE: u32 = 1 << 12;
pub const EXOFS_RIGHT_GC_TRIGGER: u32 = 1 << 13;
pub const EXOFS_RIGHT_EXPORT: u32 = 1 << 14;
pub const EXOFS_RIGHT_IMPORT: u32 = 1 << 15;
pub const EXOFS_RIGHT_ADMIN: u32 = 1 << 16;
pub const EXOFS_RIGHT_ALL: u32 = 0x0000_FFFF;
pub const EXOFS_RIGHT_READ_ONLY: u32 = EXOFS_RIGHT_READ | EXOFS_RIGHT_STAT | EXOFS_RIGHT_LIST;
pub const EXOFS_RIGHT_READ_WRITE: u32 = EXOFS_RIGHT_READ
    | EXOFS_RIGHT_WRITE
    | EXOFS_RIGHT_CREATE
    | EXOFS_RIGHT_DELETE
    | EXOFS_RIGHT_STAT
    | EXOFS_RIGHT_SETMETA
    | EXOFS_RIGHT_LIST;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoPhoenixStateWire {
    Normal = 1,
    NetworkDraining = 9,
    NetworkSerialized = 10,
}

impl ExoPhoenixStateWire {
    #[inline(always)]
    pub const fn as_syscall_arg(self) -> u64 {
        self as u8 as u64
    }
}

pub const EXO_PHOENIX_STATE_NORMAL: u64 = ExoPhoenixStateWire::Normal.as_syscall_arg();
pub const EXO_PHOENIX_STATE_NETWORK_DRAINING: u64 =
    ExoPhoenixStateWire::NetworkDraining.as_syscall_arg();
pub const EXO_PHOENIX_STATE_NETWORK_SERIALIZED: u64 =
    ExoPhoenixStateWire::NetworkSerialized.as_syscall_arg();

pub const PHOENIX_FLAG_DEGRADED_FREEZE_TIMEOUT: u32 = 1 << 0;
pub const PHOENIX_FLAG_COLD_DISK_FALLBACK: u32 = 1 << 1;
pub const PHOENIX_FLAG_TEXT_HEALED: u32 = 1 << 2;
pub const PHOENIX_FLAG_COLD_RESPAWN: u32 = 1 << 3;
pub const PHOENIX_FLAG_CRASH_NMI: u32 = 1 << 4;
pub const PHOENIX_FLAG_KVM_ACCEL: u32 = 1 << 5;
pub const PHOENIX_METRICS_MAGIC: u32 = 0x504D_4554;

pub const EXO_PROCESS_NAME_LEN: usize = 16;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExoProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub state: u32,
    pub threads: u32,
    pub name: [u8; EXO_PROCESS_NAME_LEN],
    pub utime_ns: u64,
    pub stime_ns: u64,
}

impl ExoProcessInfo {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            pid: 0,
            ppid: 0,
            state: 0,
            threads: 0,
            name: [0u8; EXO_PROCESS_NAME_LEN],
            utime_ns: 0,
            stime_ns: 0,
        }
    }
}

impl Default for ExoProcessInfo {
    #[inline(always)]
    fn default() -> Self {
        Self::zeroed()
    }
}

const _: () = assert!(core::mem::size_of::<ExoProcessInfo>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ExofsPathResolveResult {
    pub blob_id: [u8; 32],
    pub object_id: [u8; 32],
    pub object_kind: u8,
    pub _pad: [u8; 7],
    pub size_bytes: u64,
    pub epoch_id: u64,
    pub link_count: u32,
    pub flags: u32,
    pub _reserved: [u8; 8],
}

impl ExofsPathResolveResult {
    #[inline(always)]
    pub fn blob_id_low64(&self) -> u64 {
        u64::from_le_bytes([
            self.blob_id[0],
            self.blob_id[1],
            self.blob_id[2],
            self.blob_id[3],
            self.blob_id[4],
            self.blob_id[5],
            self.blob_id[6],
            self.blob_id[7],
        ])
    }
}

const _: () = assert!(core::mem::size_of::<ExofsPathResolveResult>() == 104);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ExofsOpenArgs {
    pub flags: u32,
    pub mode: u32,
    pub epoch_id: u64,
    pub owner_uid: u64,
    pub size_hint: u64,
    pub _reserved: [u64; 2],
}

const _: () = assert!(core::mem::size_of::<ExofsOpenArgs>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PciBarInfo {
    pub kind: u8,
    pub _pad: [u8; 7],
    pub phys: u64,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PciDeviceInfo {
    pub vendor_id: u16,
    pub device_id: u16,
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub irq_line: u8,
    pub irq_pin: u8,
    pub bar0_kind: u8,
    pub _pad0: u8,
    pub bar0_phys: u64,
    pub bar0_size: u64,
    pub bars: [PciBarInfo; 6],
}
