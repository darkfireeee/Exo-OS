#!/usr/bin/env python3
"""
tools/split_exo_shield.py
Découpe modulaire de servers/exo_shield/src/main.rs (1 611 lignes) en :
  servers/exo_shield/src/server/
    - protocol.rs     : Constantes IPC, structures ShieldRequest/ShieldReply, ticks, stats, helpers
    - pipeline.rs     : HookPipelineResult, analyse comportementale, classification ML, confinement, process_security_hooks
    - gate.rs         : Vérification des capabilities, extraction de tokens, audit IPC (authorize_request)
    - handlers.rs     : Handlers des requêtes IPC (scan, event, quarantine, threat, policy, heartbeat, pmc) et dispatch
    - feed.rs         : KShieldEvent, drain du feed de sécurité kernel (SYS_EXO_SHIELD_DRAIN)
    - maintenance.rs  : Maintenance périodique sur timeout IPC (scan scheduler, auto-containment)
    - mod.rs          : Façade réexportant les sous-modules
  servers/exo_shield/src/main.rs
    - Point d'entrée _start(), initialisation ordonnée des organes et boucle de réception IPC
"""

import os
import subprocess

SRC_PATH = os.path.join("servers", "exo_shield", "src", "main.rs")
SERVER_DIR = os.path.join("servers", "exo_shield", "src", "server")

def main():
    raw_text = subprocess.check_output(
        ['git', 'show', 'HEAD:servers/exo_shield/src/main.rs'],
        text=True,
        encoding='utf-8'
    )
    lines = raw_text.splitlines(keepends=True)

    os.makedirs(SERVER_DIR, exist_ok=True)

    def slice_lines(start, end):
        return "".join(lines[start - 1 : end])

    def make_pub(code):
        out = []
        in_struct = False
        for line in code.splitlines():
            stripped = line.strip()
            indent = line[: len(line) - len(line.lstrip())]
            if stripped.startswith("fn "):
                line = indent + "pub fn " + stripped[3:]
            elif stripped.startswith("const ") and not stripped.startswith("pub const ") and not stripped.startswith("const _"):
                line = indent + "pub const " + stripped[len("const ") :]
            elif stripped.startswith("static ") and not stripped.startswith("pub static "):
                line = indent + "pub static " + stripped[len("static ") :]
            elif stripped.startswith("struct ") and not stripped.startswith("pub struct "):
                line = indent + "pub struct " + stripped[len("struct ") :]
                if "{" in stripped:
                    in_struct = True
            elif in_struct:
                if stripped.startswith("}"):
                    in_struct = False
                elif ":" in stripped and not stripped.startswith("//") and not stripped.startswith("pub "):
                    line = indent + "pub " + stripped
            out.append(line)
        return "\n".join(out) + "\n"

    COMMON_HEADER = """#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use super::*;
"""

    # 1. PROTOCOL.RS : L39..L204
    # Constantes, structures, helpers read_u32_le / read_u64_le
    protocol_raw = slice_lines(39, 204)
    protocol_code = """#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{engine, ipc_gate};
use exo_syscall_abi as syscall;

""" + make_pub(protocol_raw)
    with open(os.path.join(SERVER_DIR, "protocol.rs"), "w", encoding="utf-8") as f:
        f.write(protocol_code)
    print("Wrote protocol.rs")

    # 2. PIPELINE.RS : L205..L608
    # HookPipelineResult .. process_security_hooks
    pipeline_raw = slice_lines(205, 608)
    pipeline_code = COMMON_HEADER + "\n" + make_pub(pipeline_raw)
    with open(os.path.join(SERVER_DIR, "pipeline.rs"), "w", encoding="utf-8") as f:
        f.write(pipeline_code)
    print("Wrote pipeline.rs")

    # 3. GATE.RS : L609..L676
    # extract_service_cap_token .. authorize_request
    gate_raw = slice_lines(609, 676)
    gate_code = COMMON_HEADER + "\n" + make_pub(gate_raw)
    with open(os.path.join(SERVER_DIR, "gate.rs"), "w", encoding="utf-8") as f:
        f.write(gate_code)
    print("Wrote gate.rs")

    # 4. HANDLERS.RS : L677..L1341
    # handle_scan_request .. handle_request
    handlers_raw = slice_lines(677, 1341)
    handlers_code = COMMON_HEADER + "\n" + make_pub(handlers_raw)
    with open(os.path.join(SERVER_DIR, "handlers.rs"), "w", encoding="utf-8") as f:
        f.write(handlers_code)
    print("Wrote handlers.rs")

    # 5. FEED.RS : L1342..L1441
    # KShieldEvent .. drain_shield_feed
    feed_raw = slice_lines(1342, 1441)
    feed_code = COMMON_HEADER + "\n" + make_pub(feed_raw)
    with open(os.path.join(SERVER_DIR, "feed.rs"), "w", encoding="utf-8") as f:
        f.write(feed_code)
    print("Wrote feed.rs")

    # 6. MAINTENANCE.RS : L1442..L1496
    # perform_maintenance
    maintenance_raw = slice_lines(1442, 1496)
    maintenance_code = COMMON_HEADER + "\n" + make_pub(maintenance_raw)
    with open(os.path.join(SERVER_DIR, "maintenance.rs"), "w", encoding="utf-8") as f:
        f.write(maintenance_code)
    print("Wrote maintenance.rs")

    # 7. MOD.RS : Réexport
    mod_code = """//! # exo_shield::server — Sous-système serveur IPC et orchestration NGAV
//!
//! Organisation modulaire ségréguée :
//! - `protocol`    : Constantes IPC, formats d'enveloppe, statistiques et compteurs globaux
//! - `pipeline`    : Pipeline de traitement des hooks de sécurité, confinement et inférence ML
//! - `gate`        : Porte d'autorisation IPC et vérification des capabilities
//! - `handlers`    : Gestionnaires individuels de commandes IPC et table de dispatch
//! - `feed`        : Dépilage du flux d'événements de sécurité du noyau (SYS_EXO_SHIELD_DRAIN)
//! - `maintenance` : Tâches de maintenance périodique et planificateur de scans

pub mod protocol;
pub mod pipeline;
pub mod gate;
pub mod handlers;
pub mod feed;
pub mod maintenance;

pub use protocol::*;
pub use pipeline::*;
pub use gate::*;
pub use handlers::*;
pub use feed::*;
pub use maintenance::*;
"""
    with open(os.path.join(SERVER_DIR, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(mod_code)
    print("Wrote mod.rs")

    # 8. MAIN.RS : Point d'entrée léger
    main_code = """#![no_std]
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
    boot_log(b"exo_shield: boot\\n");
    // PID runtime pour exo_cap_check (le kernel exige caller == target).
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid > 0 {
        RUNTIME_PID.store(pid as u32, Ordering::Relaxed);
    }
    boot_log(b"exo_shield: policy init\\n");
    ipc_gate::policy_init();
    boot_log(b"exo_shield: policy ready\\n");
    boot_log(b"exo_shield: ipc audit init\\n");
    ipc_gate::audit_init();
    boot_log(b"exo_shield: ipc audit ready\\n");
    boot_log(b"exo_shield: ipc gate ready\\n");
    engine::engine_init();
    boot_log(b"exo_shield: engine ready\\n");
    signatures::database::database_init();
    boot_log(b"exo_shield: signature database ready\\n");
    signatures::yara::yara_init();
    boot_log(b"exo_shield: yara ready\\n");
    signatures::update::update_init();
    boot_log(b"exo_shield: signature update ready\\n");
    boot_log(b"exo_shield: signatures ready\\n");
    behavioral::behavioral_init();
    boot_log(b"exo_shield: behavioral ready\\n");
    hooks::exec_hooks_init();
    hooks::net_hooks_init();
    hooks::mem_hooks_init();
    hooks::syscall_hooks_init();
    boot_log(b"exo_shield: hooks ready\\n");
    sandbox::sandbox_init();
    network::firewall_init();
    forensics::memory_dump_init();
    forensics::timeline_init();
    forensics::report_init();
    boot_log(b"exo_shield: containment ready\\n");
    ml::ensemble_init(0xE505_C117);
    boot_log(b"exo_shield: ml ensemble ready\\n");

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
        boot_log(b"exo_shield: register failed\\n");
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem));
            }
        }
    }
    boot_log(b"exo_shield: registered\\n");

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
"""
    with open(SRC_PATH, "w", encoding="utf-8") as f:
        f.write(main_code)
    print("Wrote main.rs")

if __name__ == "__main__":
    main()
