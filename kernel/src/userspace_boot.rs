//! Bootstrap userspace depuis le disque ExoFS racine.
//!
//! Les binaires Ring1 ne sont pas embarques dans le noyau. Le chargeur ELF
//! resout `/sbin/...` via ExoFS, lit les blobs depuis le block device global,
//! puis transfere l'execution a `/lib/ld-exo.so` pour le handoff dynamique.

use crate::process::core::pid::Pid;
use crate::process::lifecycle::{
    create_init_process_from_elf, load_elf_for_boot, CreateError, ExecError,
};

pub const INIT_PATH: &str = "/sbin/exo-init-server";

#[inline(always)]
fn debug_byte(byte: u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = byte;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootUserspaceStatus {
    Started { init_pid: u32 },
    AlreadyRunning,
    InitLoadFailed(ExecError),
    InitCreateFailed(CreateError),
}

pub fn boot_userspace() -> BootUserspaceStatus {
    debug_byte(b'u');
    if crate::process::is_alive(Pid::INIT.0) {
        return BootUserspaceStatus::AlreadyRunning;
    }

    debug_byte(b'L');
    let elf = match load_elf_for_boot(INIT_PATH, &[INIT_PATH], &[]) {
        Ok(elf) => elf,
        Err(err) => return BootUserspaceStatus::InitLoadFailed(err),
    };

    debug_byte(b'C');
    match create_init_process_from_elf(elf) {
        Ok(handle) => {
            crate::exophoenix::set_state(crate::exophoenix::PhoenixState::Normal);
            debug_byte(b'D');
            BootUserspaceStatus::Started {
                init_pid: handle.pid.0,
            }
        }
        Err(err) => BootUserspaceStatus::InitCreateFailed(err),
    }
}
