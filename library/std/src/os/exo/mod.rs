//! `std::os::exo` — Exo-OS native extensions for `std` (§5.1 of the migration plan):
//! - [`CapToken`] & [`CapTokenExt`]: capability inspection and delegation (`SYS_EXO_CAP_DELEGATE = 520`)
//! - [`ExoFsObjectStat`] & [`ExoFsExt`]: native ExoFS metadata (`SYS_EXOFS_OBJECT_STAT = 510`, `generation`, `flags`, `content_hash`)
//! - [`PhoenixSafeExt`]: zero-loss Phoenix SSR checkpoint/quiesce hooks (`SYS_EXO_RESURRECT = 523`)
//! - [`ExoPledgeExt`]: dynamic capability and promise restriction (`SYS_EXO_PLEDGE`)

pub use exo_phoenix_ssr::{PhoenixError, PhoenixRecovery, PhoenixSafe};
pub use exo_syscall::exofs::{CapTokenUser, ExoFsFileStat as ExoFsObjectStat};
pub use exo_types::cap::{
    make_driver_pci_cap, verify_cap_token, CapToken, CapabilityType, Rights as CapPermissions,
};

use crate::sys::exo_os::fs::File;
use crate::sys::exo_os::process::Command;
use crate::sys::exo_os::stdio::{Error, Result};

/// Extension trait for [`CapTokenUser`] / [`CapToken`] delegation and inspection in Ring 1/3 (`std::os::exo::CapTokenExt`).
pub trait CapTokenExt {
    /// Returns the initial process `CapTokenUser` passed by the kernel via `AT_CAP_TOKEN` (`52`).
    fn current_process_token() -> CapTokenUser;

    /// Delegates this capability token to `target_pid` with attenuated `rights` (`SYS_EXO_CAP_DELEGATE = 520`).
    fn delegate_to(&self, target_pid: u32, rights: CapPermissions) -> Result<CapTokenUser>;
}

impl CapTokenExt for CapTokenUser {
    #[inline]
    fn current_process_token() -> CapTokenUser {
        let ptr = exo_rt::initial_cap_token_ptr();
        if ptr.is_null() {
            CapTokenUser::default()
        } else {
            unsafe { *ptr }
        }
    }

    fn delegate_to(&self, target_pid: u32, rights: CapPermissions) -> Result<CapTokenUser> {
        exo_syscall::exofs::exofs_delegate(self, rights.0, target_pid)
            .map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

/// Extension trait exposing native ExoFS object properties (`500..=520`) on [`File`].
pub trait ExoFsExt {
    /// Queries the native ExoFS object stat (`SYS_EXOFS_OBJECT_STAT = 510`).
    fn exofs_stat(&self) -> Result<ExoFsObjectStat>;

    /// Returns the CoW generation counter of the underlying ExoFS object.
    fn exofs_generation(&self) -> Result<u64>;
}

impl ExoFsExt for File {
    fn exofs_stat(&self) -> Result<ExoFsObjectStat> {
        exo_syscall::exofs::exofs_fstat(self.as_raw_fd())
            .map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    fn exofs_generation(&self) -> Result<u64> {
        Ok(self.exofs_stat()?.generation)
    }
}

/// Extension trait for Phoenix SSR state checkpointing (`std::os::exo::PhoenixSafeExt`).
pub trait PhoenixSafeExt: PhoenixSafe {
    /// Triggers or tests Phoenix SSR resurrection (`SYS_EXO_RESURRECT = 523`).
    fn trigger_resurrection(&self, mode: u64) -> Result<usize> {
        exo_syscall::phoenix::trigger_resurrection(mode)
            .map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl<T: PhoenixSafe + ?Sized> PhoenixSafeExt for T {}

/// Extension trait on [`Command`] for Exo-OS capability pledging (`std::os::exo::ExoPledgeExt`).
pub trait ExoPledgeExt {
    /// Restricts the child process to the given [`CapPermissions`] mask before `execve`.
    fn pledge_permissions(&mut self, perms: CapPermissions) -> &mut Self;
}

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
