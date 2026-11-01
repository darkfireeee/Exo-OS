//! `std::os::exo` — Exo-OS native extensions for `std`:
//! - [`CapToken`], [`CapTokenUser`], [`CapPermissions`], [`CapabilityType`], [`CapTokenExt`] (`SYS_EXO_CAP_DELEGATE = 321`)
//! - [`ExoFsObjectStat`] & [`ExoFsExt`] (`SYS_EXOFS_OBJECT_STAT = 506`, `generation`, `flags`, `content_hash`)
//! - [`PhoenixSafe`], [`PhoenixError`], [`PhoenixRecovery`], [`PhoenixSafeExt`] (`SYS_EXO_RESURRECT = 523`)
//! - [`ExoPledgeExt`] (`SYS_EXO_PLEDGE = 324`)

#![stable(feature = "rust1", since = "1.0.0")]
#![deny(unsafe_op_in_unsafe_fn)]

use crate::fs::File;
use crate::os::fd::AsRawFd;
use crate::process::Command;
use crate::sys::{map_exo_err, runtime, syscall};

pub mod ffi;
pub mod io;
pub mod unix;

/// 48-byte user capability token (`CapTokenUser`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[stable(feature = "rust1", since = "1.0.0")]
pub struct CapTokenUser {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub raw: [u8; 48],
}

#[stable(feature = "rust1", since = "1.0.0")]
impl Default for CapTokenUser {
    fn default() -> Self {
        Self { raw: [0u8; 48] }
    }
}

impl CapTokenUser {
    #[inline]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn generation(&self) -> u32 {
        u32::from_le_bytes([self.raw[32], self.raw[33], self.raw[34], self.raw[35]])
    }

    #[inline]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn rights(&self) -> u32 {
        u32::from_le_bytes([self.raw[36], self.raw[37], self.raw[38], self.raw[39]])
    }

    #[inline]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn owner_pid(&self) -> u32 {
        u32::from_le_bytes([self.raw[40], self.raw[41], self.raw[42], self.raw[43]])
    }
}

/// ExoFS object metadata (104 bytes — `EXOFS_STAT_LEN`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[stable(feature = "rust1", since = "1.0.0")]
pub struct ExoFsObjectStat {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub object_id: [u8; 32],
    #[stable(feature = "rust1", since = "1.0.0")]
    pub content_hash: [u8; 32],
    #[stable(feature = "rust1", since = "1.0.0")]
    pub size: u64,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub mode: u32,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub flags: u32,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub ctime_ns: u64,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub mtime_ns: u64,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub generation: u64,
}

#[stable(feature = "rust1", since = "1.0.0")]
impl Default for ExoFsObjectStat {
    fn default() -> Self {
        Self {
            object_id: [0u8; 32],
            content_hash: [0u8; 32],
            size: 0,
            mode: 0,
            flags: 0,
            ctime_ns: 0,
            mtime_ns: 0,
            generation: 0,
        }
    }
}

/// Signal action slot for 0-syscall `SignalTcb` (`SigActionSlot`).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[stable(feature = "rust1", since = "1.0.0")]
pub struct SigActionSlot {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub handler_vaddr: u64,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub flags: u32,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub _pad: u32,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub mask: u64,
    #[stable(feature = "rust1", since = "1.0.0")]
    pub restorer: u64,
}

/// 0-syscall `sigaction` via `SignalTcb`.
#[inline]
#[stable(feature = "rust1", since = "1.0.0")]
pub fn sigaction_0syscall(
    sig: u8,
    new_act: Option<SigActionSlot>,
    old_act: Option<&mut SigActionSlot>,
) -> Result<(), i32> {
    let new_internal = new_act.map(|a| runtime::SigActionSlot {
        handler_vaddr: a.handler_vaddr,
        flags: a.flags,
        _pad: a._pad,
        mask: a.mask,
        restorer: a.restorer,
    });
    let mut old_internal = runtime::SigActionSlot::default();
    let res = runtime::sigaction_0syscall(
        sig,
        new_internal,
        if old_act.is_some() { Some(&mut old_internal) } else { None },
    );
    if let (Ok(()), Some(out)) = (res, old_act) {
        *out = SigActionSlot {
            handler_vaddr: old_internal.handler_vaddr,
            flags: old_internal.flags,
            _pad: old_internal._pad,
            mask: old_internal.mask,
            restorer: old_internal.restorer,
        };
    }
    res
}

/// 0-syscall `sigprocmask` via `SignalTcb::blocked`.
#[inline]
#[stable(feature = "rust1", since = "1.0.0")]
pub fn sigprocmask_0syscall(
    how: i32,
    set: Option<u64>,
    oldset: Option<&mut u64>,
) -> Result<(), i32> {
    runtime::sigprocmask_0syscall(how, set, oldset)
}

/// Native `fork()` via `SYS_CLONE (56)` with `SignalTcb` pending signal reset in the child.
#[inline]
#[stable(feature = "rust1", since = "1.0.0")]
pub fn fork() -> Result<u32, i32> {
    runtime::fork()
}

/// Capability type tag (`CapabilityType`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[stable(feature = "rust1", since = "1.0.0")]
pub enum CapabilityType {
    #[stable(feature = "rust1", since = "1.0.0")]
    Invalid = 0,
    #[stable(feature = "rust1", since = "1.0.0")]
    MemoryPage = 1,
    #[stable(feature = "rust1", since = "1.0.0")]
    IpcEndpoint = 2,
    #[stable(feature = "rust1", since = "1.0.0")]
    FileObject = 3,
    #[stable(feature = "rust1", since = "1.0.0")]
    DeviceMmio = 4,
    #[stable(feature = "rust1", since = "1.0.0")]
    IrqLine = 5,
    #[stable(feature = "rust1", since = "1.0.0")]
    ProcessControl = 6,
    #[stable(feature = "rust1", since = "1.0.0")]
    IoPort = 7,
    #[stable(feature = "rust1", since = "1.0.0")]
    DmaBuffer = 8,
    #[stable(feature = "rust1", since = "1.0.0")]
    CryptoKey = 9,
    #[stable(feature = "rust1", since = "1.0.0")]
    BpfProgram = 10,
}

/// Bitmask of capability permissions (`CapPermissions`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[stable(feature = "rust1", since = "1.0.0")]
pub struct CapPermissions(#[stable(feature = "rust1", since = "1.0.0")] pub u32);

impl CapPermissions {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub const NONE: Self = Self(0);
    #[stable(feature = "rust1", since = "1.0.0")]
    pub const READ: Self = Self(1 << 0);
    #[stable(feature = "rust1", since = "1.0.0")]
    pub const WRITE: Self = Self(1 << 1);
    #[stable(feature = "rust1", since = "1.0.0")]
    pub const EXECUTE: Self = Self(1 << 2);
    #[stable(feature = "rust1", since = "1.0.0")]
    pub const GRANT: Self = Self(1 << 3);
    #[stable(feature = "rust1", since = "1.0.0")]
    pub const REVOKE: Self = Self(1 << 4);
}

/// 48-byte hardware/kernel capability token (`CapToken`).
#[stable(feature = "rust1", since = "1.0.0")]
pub type CapToken = CapTokenUser;

/// Extension trait for [`CapTokenUser`] / [`CapToken`] delegation and inspection (`std::os::exo::CapTokenExt`).
#[stable(feature = "rust1", since = "1.0.0")]
pub trait CapTokenExt {
    /// Returns the initial process `CapTokenUser` passed by the kernel via `AT_CAP_TOKEN` (`52`).
    #[stable(feature = "rust1", since = "1.0.0")]
    fn current_process_token() -> CapTokenUser;

    /// Delegates this capability token to `target_pid` with attenuated `rights` (`SYS_EXO_CAP_DELEGATE = 321`).
    #[stable(feature = "rust1", since = "1.0.0")]
    fn delegate_to(&self, target_pid: u32, rights: CapPermissions) -> crate::io::Result<CapTokenUser>;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl CapTokenExt for CapTokenUser {
    #[inline]
    fn current_process_token() -> CapTokenUser {
        let ptr = runtime::initial_cap_token_ptr();
        if ptr.is_null() {
            CapTokenUser::default()
        } else {
            let sys_tok = unsafe { *ptr };
            CapTokenUser { raw: sys_tok.raw }
        }
    }

    fn delegate_to(&self, target_pid: u32, rights: CapPermissions) -> crate::io::Result<CapTokenUser> {
        let sys_in = syscall::CapTokenUser { raw: self.raw };
        let sys_out = syscall::exofs_delegate(&sys_in, rights.0, target_pid).map_err(map_exo_err)?;
        Ok(CapTokenUser { raw: sys_out.raw })
    }
}

/// Extension trait exposing native ExoFS object properties (`500..=520`) on [`File`].
#[stable(feature = "rust1", since = "1.0.0")]
pub trait ExoFsExt {
    /// Queries the native ExoFS object stat (`SYS_EXOFS_OBJECT_STAT = 506`).
    #[stable(feature = "rust1", since = "1.0.0")]
    fn exofs_stat(&self) -> crate::io::Result<ExoFsObjectStat>;

    /// Returns the CoW generation counter of the underlying ExoFS object.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn exofs_generation(&self) -> crate::io::Result<u64>;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl ExoFsExt for File {
    fn exofs_stat(&self) -> crate::io::Result<ExoFsObjectStat> {
        let st = syscall::exofs_fstat(self.as_raw_fd()).map_err(map_exo_err)?;
        Ok(ExoFsObjectStat {
            object_id: st.object_id,
            content_hash: st.content_hash,
            size: st.size,
            mode: st.mode,
            flags: st.flags,
            ctime_ns: st.ctime_ns,
            mtime_ns: st.mtime_ns,
            generation: st.generation,
        })
    }

    fn exofs_generation(&self) -> crate::io::Result<u64> {
        Ok(self.exofs_stat()?.generation)
    }
}

/// Recovery strategy for ExoPhoenix SSR (`PhoenixRecovery`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[stable(feature = "rust1", since = "1.0.0")]
pub enum PhoenixRecovery {
    #[stable(feature = "rust1", since = "1.0.0")]
    Stateless,
    #[stable(feature = "rust1", since = "1.0.0")]
    Checkpoint,
    #[stable(feature = "rust1", since = "1.0.0")]
    Transactional,
}

/// Error returned by ExoPhoenix SSR hooks (`PhoenixError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[stable(feature = "rust1", since = "1.0.0")]
pub enum PhoenixError {
    #[stable(feature = "rust1", since = "1.0.0")]
    BufferTooSmall,
    #[stable(feature = "rust1", since = "1.0.0")]
    CorruptedState,
    #[stable(feature = "rust1", since = "1.0.0")]
    UnsupportedVersion,
    #[stable(feature = "rust1", since = "1.0.0")]
    HardwareNotQuiescent,
    #[stable(feature = "rust1", since = "1.0.0")]
    InvalidCapability,
}

/// Formal trait for structures surviving ExoPhoenix SSR (`PhoenixSafe`).
#[stable(feature = "rust1", since = "1.0.0")]
pub trait PhoenixSafe {
    #[stable(feature = "rust1", since = "1.0.0")]
    const RECOVERY: PhoenixRecovery;

    #[stable(feature = "rust1", since = "1.0.0")]
    fn is_stateless(&self) -> bool {
        matches!(Self::RECOVERY, PhoenixRecovery::Stateless)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    fn on_pre_switch(&self) -> Result<(), PhoenixError> {
        Ok(())
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        Ok(())
    }
}

/// Extension trait for Phoenix SSR state checkpointing (`std::os::exo::PhoenixSafeExt`).
#[stable(feature = "rust1", since = "1.0.0")]
pub trait PhoenixSafeExt: PhoenixSafe {
    /// Triggers or tests Phoenix SSR resurrection (`SYS_EXO_RESURRECT = 523`).
    #[stable(feature = "rust1", since = "1.0.0")]
    fn trigger_resurrection(&self, mode: u64) -> crate::io::Result<usize> {
        syscall::trigger_resurrection(mode).map_err(map_exo_err)
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl<T: PhoenixSafe + ?Sized> PhoenixSafeExt for T {}

/// Extension trait on [`Command`] for Exo-OS capability pledging (`std::os::exo::ExoPledgeExt`).
#[stable(feature = "rust1", since = "1.0.0")]
pub trait ExoPledgeExt {
    /// Restricts the child process to the given [`CapPermissions`] mask before `execve`.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn pledge_permissions(&mut self, perms: CapPermissions) -> &mut Self;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl ExoPledgeExt for Command {
    fn pledge_permissions(&mut self, perms: CapPermissions) -> &mut Self {
        let mut buf = [0u8; 20];
        let mut n = perms.0;
        let mut idx = buf.len();
        if n == 0 {
            idx -= 1;
            buf[idx] = b'0';
        } else {
            while n > 0 {
                idx -= 1;
                buf[idx] = b'0' + (n % 10) as u8;
                n /= 10;
            }
        }
        let val = core::str::from_utf8(&buf[idx..]).unwrap_or("0");
        self.env("EXO_PLEDGE_PERMS", val)
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
pub mod prelude {
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::io::*;
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::ffi::{OsStrExt, OsStringExt};
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::{CapTokenExt, ExoFsExt, ExoPledgeExt, PhoenixSafeExt};
}
