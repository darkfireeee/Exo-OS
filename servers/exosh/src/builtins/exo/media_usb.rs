//! `exo usb`, `exo beep`/`beep`, and `exo chime`/`chime`.
#![allow(dead_code, unused_imports)]
use super::doctor_metrics::{ipc_service_online, path_exists_probe};
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

pub(super) fn handle_exo_usb() {
    write_all(b"=== USB Subsystem Status ===\n");
    if ipc_service_online(b"usb_server") {
        write_all(b"usb_server : online (device enumeration RPC not exposed yet)\n");
    } else {
        write_all(b"usb_server : offline\n");
    }
    if path_exists_probe(b"/dev/usb0\0") {
        write_all(b"/dev/usb0  : present\n");
    } else {
        write_all(b"/dev/usb0  : not present\n");
    }
}

pub fn cmd_beep(rest: &[u8]) {
    handle_exo_beep(rest);
}

pub fn cmd_chime(_rest: &[u8]) {
    handle_exo_chime();
}

pub(super) fn handle_exo_beep(args: &[u8]) {
    let (freq_str, dur_str) = first_token(args);
    let freq = parse_u64(freq_str).unwrap_or(800) as u32;
    let dur = parse_u64(dur_str).unwrap_or(200) as u32;
    let req = syscall::AudioRequest {
        sender_pid: 0,
        msg_type: syscall::AUDIO_MSG_BEEP,
        reply_endpoint: 0,
        a: freq,
        b: dur,
        _reserved: [0; 16],
    };
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::AUDIO_SERVER_ENDPOINT,
            &req as *const syscall::AudioRequest as u64,
            core::mem::size_of::<syscall::AudioRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if rc >= 0 {
        write_all(b"Audio: Beep sent (");
        write_u64(freq as u64);
        write_all(b" Hz, ");
        write_u64(dur as u64);
        write_all(b" ms)\n");
    } else {
        write_all(b"exo beep: audio_server unavailable (errno ");
        write_i64(rc);
        write_all(b")\n");
    }
}

pub(super) fn handle_exo_chime() {
    let req = syscall::AudioRequest {
        sender_pid: 0,
        msg_type: syscall::AUDIO_MSG_PLAY_SYSTEM_SOUND,
        reply_endpoint: 0,
        a: syscall::AUDIO_SOUND_BOOT_COMPLETE,
        b: 0,
        _reserved: [0; 16],
    };
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::AUDIO_SERVER_ENDPOINT,
            &req as *const syscall::AudioRequest as u64,
            core::mem::size_of::<syscall::AudioRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if rc >= 0 {
        write_all(b"Audio: Playing Exo-OS resurrection chime (Intel HDA / VirtIO Sound)...\n");
    } else {
        write_all(b"exo chime: audio_server unavailable (errno ");
        write_i64(rc);
        write_all(b")\n");
    }
}
