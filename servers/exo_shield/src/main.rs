#![no_std]
#![no_main]

//! # exo_shield — AI/Process Containment Security Server (ExoShield Phase 3)
//!
//! Monitors processes, detects anomalies, contains threats, and provides forensics.
//! All other servers delegate security queries here.
//!
//! ## Architecture modulaire
//! - `server::protocol`    — Enveloppes IPC, constantes wire et télémétrie
//! - `server::pipeline`    — Pipeline NGAV, inférence ML, confinement automatique
//! - `server::gate`        — Contrôle d'accès et vérification de capability IPC
//! - `server::handlers`    — Dispatcher de commandes clientes
//! - `server::feed`        — Dépilage du flux de sécurité Ring 0
//! - `server::maintenance` — Planification de scans et maintenance périodique

mod server;

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;
use exo_shield::{
    behavioral, engine, forensics, hooks, ipc_gate, ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use server::*;

// ── Entry Point ─────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 0. Initialize all engine sub-modules ────────────────────────────────
    boot_log(b"exo_shield: boot\n");
    // PID runtime pour exo_cap_check (le kernel exige caller == target).
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid > 0 {
        RUNTIME_PID.store(pid as u32, Ordering::Relaxed);
    }
    boot_log(b"exo_shield: policy init\n");
    ipc_gate::policy_init();
    boot_log(b"exo_shield: policy ready\n");
    boot_log(b"exo_shield: ipc audit init\n");
    ipc_gate::audit_init();
    boot_log(b"exo_shield: ipc audit ready\n");
    boot_log(b"exo_shield: ipc gate ready\n");
    engine::engine_init();
    boot_log(b"exo_shield: engine ready\n");
    signatures::database::database_init();
    boot_log(b"exo_shield: signature database ready\n");
    signatures::yara::yara_init();
    boot_log(b"exo_shield: yara ready\n");
    signatures::update::update_init();
    boot_log(b"exo_shield: signature update ready\n");
    boot_log(b"exo_shield: signatures ready\n");
    behavioral::behavioral_init();
    boot_log(b"exo_shield: behavioral ready\n");
    hooks::exec_hooks_init();
    hooks::net_hooks_init();
    hooks::mem_hooks_init();
    hooks::syscall_hooks_init();
    boot_log(b"exo_shield: hooks ready\n");
    sandbox::sandbox_init();
    network::firewall_init();
    forensics::memory_dump_init();
    forensics::timeline_init();
    forensics::report_init();
    boot_log(b"exo_shield: containment ready\n");
    ml::ensemble_init(0xE505_C117);
    boot_log(b"exo_shield: ml ensemble ready\n");

    // ── 1. Register public exo_shield endpoint ─────────────────────────────
    let name = b"exo_shield";
    let register_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            EXO_SHIELD_ENDPOINT,
        )
    };
    if register_rc < 0 {
        boot_log(b"exo_shield: register failed\n");
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem));
            }
        }
    }
    boot_log(b"exo_shield: registered\n");

    // ── 2. Main IPC receive loop ────────────────────────────────────────────
    let mut req = ShieldRequest {
        sender_pid: 0,
        msg_type: 0,
        payload: [0u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };

    loop {
        let r = unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_RECV,
                EXO_SHIELD_ENDPOINT,
                &mut req as *mut ShieldRequest as u64,
                core::mem::size_of::<ShieldRequest>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            // Perform periodic maintenance on timeout
            perform_maintenance();
            continue;
        }
        if r < 0 {
            continue;
        }

        let reply = handle_request(&req);

        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                req.sender_pid as u64,
                &reply as *const ShieldReply as u64,
                core::mem::size_of::<ShieldReply>() as u64,
                0,
                0,
                0,
            )
        };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
