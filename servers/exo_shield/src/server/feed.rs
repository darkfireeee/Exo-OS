#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use super::*;

// ── Kernel Security Feed (TIER 3.1) ─────────────────────────────────────────

/// Miroir de `kernel::security::shield_feed::ShieldEvent` (40 octets, ABI stable
/// du syscall `SYS_EXO_SHIELD_DRAIN`). Le kernel pousse les événements de sécurité
/// (refus IPC/syscall, …) ici ; on les draine et on les injecte dans le moteur.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KShieldEvent {
    pub pid: u32,
    pub event_type: u8,
    pub severity: u8,
    pub _pad: [u8; 2],
    pub opcode: u32,
    pub _pad2: u32,
    pub arg0: u64,
    pub arg1: u64,
    pub seq: u64,
}

impl KShieldEvent {
    pub const fn zero() -> Self {
        Self {
            pid: 0,
            event_type: 0,
            severity: 0,
            _pad: [0; 2],
            opcode: 0,
            _pad2: 0,
            arg0: 0,
            arg1: 0,
            seq: 0,
        }
    }
}

pub const SHIELD_FEED_BATCH: usize = 64;

/// Mappe la catégorie d'événement du feed kernel vers `engine::EventType`
/// (valeurs différentes — cf. shield_feed::event_type côté kernel).
pub fn map_kernel_event_type(et: u8) -> engine::EventType {
    match et {
        0 => engine::EventType::Process,    // kernel PROCESS
        1 => engine::EventType::Syscall,    // kernel SYSCALL
        2 => engine::EventType::Memory,     // kernel MEMORY
        3 => engine::EventType::Network,    // kernel NETWORK
        4 => engine::EventType::Ipc,        // kernel IPC
        5 => engine::EventType::Capability, // kernel CAPABILITY
        other => engine::EventType::Custom(other),
    }
}

/// Draine le feed d'événements de sécurité kernel→shield et injecte chaque
/// événement dans le pipeline NGAV (hooks + moteur + confinement auto). C'est le
/// FEED réel qui rend le NGAV non-aveugle (TIER 3.1).
pub fn drain_shield_feed() {
    let mut buf = [KShieldEvent::zero(); SHIELD_FEED_BATCH];
    let n = unsafe {
        syscall::syscall3(
            syscall::SYS_EXO_SHIELD_DRAIN,
            buf.as_mut_ptr() as u64,
            SHIELD_FEED_BATCH as u64,
            0,
        )
    };
    if n <= 0 {
        return;
    }
    let count = (n as usize).min(SHIELD_FEED_BATCH);
    let tick = current_tick();
    let empty_payload = [0u8; syscall::IPC_INLINE_PAYLOAD_SIZE];

    for ev in &buf[..count] {
        if ev.pid == 0 {
            continue;
        }
        let event = engine::MonitoredEvent {
            pid: ev.pid,
            event_type: map_kernel_event_type(ev.event_type),
            opcode: ev.opcode,
            arg0: ev.arg0,
            arg1: ev.arg1,
            timestamp: tick,
            severity: engine::ThreatLevel::from_u8(ev.severity),
        };
        // Pipeline NGAV : hooks + ML ensemble + soumission moteur.
        let hook_result = process_security_hooks(&event, &empty_payload);
        let data = behaviour_data_for_event(&event, &hook_result);
        let fv = ml::FeatureExtractor::extract(&data);
        let et_raw = engine_event_to_markov_cat(event.event_type);
        let ml_result = ml::ensemble_classify(event.pid, et_raw, &fv);
        let _ = engine::submit_event(&event, tick);
        // Confinement auto sur menace critique OU ML malicious.
        if hook_result.level >= engine::ThreatLevel::Critical
            || ml_result.classification == ml::Classification::Malicious
        {
            apply_containment(event.pid, tick);
        }
    }
}

