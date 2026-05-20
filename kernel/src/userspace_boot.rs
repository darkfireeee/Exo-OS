//! Bootstrap userspace: payloads embarques et creation de PID 1.
//!
//! Le VFS/ExoFS de boot expose les binaires via `BLOB_CACHE`; l'ELF loader
//! resout ensuite `/sbin/...` par le meme hash canonique que `execve()`.

extern crate alloc;

use alloc::vec::Vec;

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::syscall::path_resolve::resolve_path_to_blob;
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

#[cfg(all(debug_assertions, exo_kernel_trace))]
fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        debug_byte(byte);
    }
}

#[cfg(all(debug_assertions, exo_kernel_trace))]
fn debug_usize(mut value: usize) {
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        debug_byte(b'0');
        return;
    }
    while value != 0 && len < buf.len() {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len != 0 {
        len -= 1;
        debug_byte(buf[len]);
    }
}

struct EmbeddedPayload {
    path: &'static str,
    bytes: &'static [u8],
}

#[cfg(exo_boot_payloads)]
macro_rules! boot_payload {
    ($name:ident, $file:literal) => {
        #[link_section = ".boot_payloads"]
        #[used]
        static $name: [u8; include_bytes!(concat!(env!("EXO_BOOT_PAYLOAD_DIR"), "/", $file))
            .len()] = *include_bytes!(concat!(env!("EXO_BOOT_PAYLOAD_DIR"), "/", $file));
    };
}

#[cfg(exo_boot_payloads)]
boot_payload!(INIT_SERVER_BYTES, "exo-init-server");
#[cfg(exo_boot_payloads)]
boot_payload!(IPC_ROUTER_BYTES, "exo-ipc-router");
#[cfg(exo_boot_payloads)]
boot_payload!(MEMORY_SERVER_BYTES, "exo-memory-server");
#[cfg(exo_boot_payloads)]
boot_payload!(VFS_SERVER_BYTES, "exo-vfs-server");
#[cfg(exo_boot_payloads)]
boot_payload!(CRYPTO_SERVER_BYTES, "exo-crypto-server");
#[cfg(exo_boot_payloads)]
boot_payload!(DEVICE_SERVER_BYTES, "exo-device-server");
#[cfg(exo_boot_payloads)]
boot_payload!(VIRTIO_DRIVERS_BYTES, "exo-virtio-drivers");
#[cfg(exo_boot_payloads)]
boot_payload!(NETWORK_SERVER_BYTES, "exo-network-server");
#[cfg(exo_boot_payloads)]
boot_payload!(SCHEDULER_SERVER_BYTES, "exo-scheduler-server");
#[cfg(exo_boot_payloads)]
boot_payload!(INPUT_SERVER_BYTES, "exo-input-server");
#[cfg(exo_boot_payloads)]
boot_payload!(TTY_SERVER_BYTES, "exo-tty-server");
#[cfg(exo_boot_payloads)]
boot_payload!(EXOSH_BYTES, "exosh");
#[cfg(exo_boot_payloads)]
boot_payload!(EXO_SHIELD_BYTES, "exo-shield");
#[cfg(exo_boot_payloads)]
boot_payload!(EXO_LOADER_BYTES, "exo-loader");

#[cfg(exo_boot_payloads)]
static EMBEDDED_PAYLOADS: &[EmbeddedPayload] = &[
    EmbeddedPayload {
        path: "/sbin/exo-init-server",
        bytes: &INIT_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-ipc-router",
        bytes: &IPC_ROUTER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-memory-server",
        bytes: &MEMORY_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-vfs-server",
        bytes: &VFS_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-crypto-server",
        bytes: &CRYPTO_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-device-server",
        bytes: &DEVICE_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-virtio-drivers",
        bytes: &VIRTIO_DRIVERS_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-network-server",
        bytes: &NETWORK_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-scheduler-server",
        bytes: &SCHEDULER_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-input-server",
        bytes: &INPUT_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-tty-server",
        bytes: &TTY_SERVER_BYTES,
    },
    EmbeddedPayload {
        path: "/sbin/exo-shield",
        bytes: &EXO_SHIELD_BYTES,
    },
    EmbeddedPayload {
        path: "/bin/exosh",
        bytes: &EXOSH_BYTES,
    },
    EmbeddedPayload {
        path: "/lib/ld-exo.so",
        bytes: &EXO_LOADER_BYTES,
    },
];

#[cfg(not(exo_boot_payloads))]
static EMBEDDED_PAYLOADS: &[EmbeddedPayload] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootUserspaceStatus {
    Started {
        payloads_seeded: usize,
        init_pid: u32,
    },
    AlreadyRunning,
    NoEmbeddedPayloads,
    PayloadEmpty,
    PayloadPathInvalid,
    PayloadCacheFailed,
    PayloadOutOfMemory,
    InitLoadFailed(ExecError),
    InitCreateFailed(CreateError),
}

fn payload_blob_id(payload: &EmbeddedPayload) -> Option<BlobId> {
    resolve_path_to_blob(payload.path.as_bytes(), 0)
        .ok()
        .map(|resolved| BlobId(resolved.blob_id))
}

pub fn embedded_payload_by_blob(blob_id: BlobId) -> Option<&'static [u8]> {
    for payload in EMBEDDED_PAYLOADS {
        if payload_blob_id(payload) == Some(blob_id) {
            return Some(payload.bytes);
        }
    }
    None
}

fn seed_payload(payload: &EmbeddedPayload) -> Result<(), BootUserspaceStatus> {
    if payload.bytes.is_empty() {
        return Err(BootUserspaceStatus::PayloadEmpty);
    }

    #[cfg(all(debug_assertions, exo_kernel_trace))]
    {
        debug_write(b"boot_payload: ");
        debug_write(payload.path.as_bytes());
        debug_write(b" len=");
        debug_usize(payload.bytes.len());
        debug_write(b"\n");
    }

    let blob_id = payload_blob_id(payload).ok_or(BootUserspaceStatus::PayloadPathInvalid)?;
    let mut data = Vec::new();
    data.try_reserve_exact(payload.bytes.len())
        .map_err(|_| BootUserspaceStatus::PayloadOutOfMemory)?;
    data.extend_from_slice(payload.bytes);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(|_| BootUserspaceStatus::PayloadCacheFailed)
}

pub fn seed_embedded_payloads() -> Result<usize, BootUserspaceStatus> {
    if EMBEDDED_PAYLOADS.is_empty() {
        return Err(BootUserspaceStatus::NoEmbeddedPayloads);
    }

    let mut seeded = 0usize;
    for payload in EMBEDDED_PAYLOADS {
        seed_payload(payload)?;
        seeded = seeded.saturating_add(1);
    }
    Ok(seeded)
}

pub fn boot_userspace() -> BootUserspaceStatus {
    debug_byte(b'u');
    if crate::process::is_alive(Pid::INIT.0) {
        return BootUserspaceStatus::AlreadyRunning;
    }

    debug_byte(b's');
    let payloads_seeded = match seed_embedded_payloads() {
        Ok(count) => count,
        Err(err) => return err,
    };

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
                payloads_seeded,
                init_pid: handle.pid.0,
            }
        }
        Err(err) => BootUserspaceStatus::InitCreateFailed(err),
    }
}
