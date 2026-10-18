//! exo-tty — Démon pilote TTY / discipline de ligne en espace utilisateur
//!
//! Assure le pont IPC complet entre :
//! - `input_server` (événements clavier bruts)
//! - `tty_server` / descripteurs d'`exosh` (mode canonique / raw, bufferisation de ligne)
//! - `fb_server` / console VGA (écho visuel à l'écran)

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_tty::{LineDiscipline, LineEvent, Signal};

pub const TTY_DRIVER_ENDPOINT: u64 = 22;
const SERVICE_NAME: &[u8] = b"tty_driver";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 1. Enregistrement du service auprès d'ipc_router
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            SERVICE_NAME.as_ptr() as u64,
            SERVICE_NAME.len() as u64,
            TTY_DRIVER_ENDPOINT,
        )
    };

    let mut ldisc = LineDiscipline::new();
    let mut msg_buf = [0u8; syscall::IPC_ENVELOPE_SIZE];
    let mut line_buf = [0u8; syscall::TTY_LINE_MAX];

    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                TTY_DRIVER_ENDPOINT,
                msg_buf.as_mut_ptr() as u64,
                msg_buf.len() as u64,
                0, // Bloquant
            )
        };

        if rc < syscall::IPC_HEADER_SIZE as i64 {
            continue;
        }

        let sender_pid = u32::from_ne_bytes([msg_buf[0], msg_buf[1], msg_buf[2], msg_buf[3]]);
        let payload_len = (rc as usize - syscall::IPC_HEADER_SIZE).min(syscall::IPC_INLINE_PAYLOAD_SIZE);
        let payload = &msg_buf[syscall::IPC_HEADER_SIZE..syscall::IPC_HEADER_SIZE + payload_len];

        for &byte in payload {
            if let Some(event) = ldisc.input_byte(byte) {
                match event {
                    LineEvent::Echo(ch) => {
                        // Écho vers fb_server ou console VGA
                        send_echo(&[ch]);
                    }
                    LineEvent::Backspace => {
                        send_echo(b"\x08 \x08");
                    }
                    LineEvent::ClearScreen => {
                        send_echo(b"\x1b[2J\x1b[H");
                    }
                    LineEvent::LineReady { .. } => {
                        let line = ldisc.take_line(&mut line_buf);
                        send_line_to_client(sender_pid, line);
                    }
                    LineEvent::Signal(Signal::Interrupt) => {
                        // Signal SIGINT (2) envoyé au processus de premier plan
                        if sender_pid > 0 {
                            unsafe {
                                let _ = syscall::syscall2(syscall::SYS_KILL, sender_pid as u64, 2);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn send_echo(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    // Tenter l'envoi vers fb_server, fallback sur SYS_EXO_LOG / VGA
    let mut echo_packet = [0u8; syscall::IPC_ENVELOPE_SIZE];
    let len = bytes.len().min(syscall::IPC_INLINE_PAYLOAD_SIZE);
    echo_packet[syscall::IPC_HEADER_SIZE..syscall::IPC_HEADER_SIZE + len].copy_from_slice(&bytes[..len]);

    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::FB_SERVER_ENDPOINT,
            echo_packet.as_ptr() as u64,
            echo_packet.len() as u64,
            0,
            0,
            0,
        )
    };

    if rc < 0 {
        unsafe {
            let _ = syscall::syscall3(
                syscall::SYS_EXO_LOG,
                bytes.as_ptr() as u64,
                bytes.len() as u64,
                1,
            );
        }
    }
}

fn send_line_to_client(client_pid: u32, line: &[u8]) {
    if client_pid == 0 || line.is_empty() {
        return;
    }
    let mut reply_packet = [0u8; syscall::IPC_ENVELOPE_SIZE];
    let len = line.len().min(syscall::IPC_INLINE_PAYLOAD_SIZE);
    reply_packet[syscall::IPC_HEADER_SIZE..syscall::IPC_HEADER_SIZE + len].copy_from_slice(&line[..len]);

    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            client_pid as u64,
            reply_packet.as_ptr() as u64,
            reply_packet.len() as u64,
            0,
            0,
            0,
        )
    };
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
