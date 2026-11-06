//! Userspace bootstrap from the root ExoFS disk.
//!
//! Ring 1 binaries are not embedded into the kernel. The ELF loader
//! resolves `/sbin/...` via ExoFS, reads blobs from the global block device,
//! then transfers execution to `/lib/ld-exo.so` for the dynamic handoff.

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
