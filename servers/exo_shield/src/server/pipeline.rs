#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use super::*;

#[derive(Clone, Copy)]
pub struct HookPipelineResult {
    pub blocked: bool,
    pub level: engine::ThreatLevel,
    pub category: engine::ThreatCategory,
    pub timeline_type: forensics::TimelineEventType,
    pub detail1: u32,
    pub detail2: u32,
    pub alert_desc: &'static [u8],
}

impl HookPipelineResult {
    pub const fn pass() -> Self {
        Self {
            blocked: false,
            level: engine::ThreatLevel::Low,
            category: engine::ThreatCategory::None,
            timeline_type: forensics::TimelineEventType::ThreatDetected,
            detail1: 0,
            detail2: 0,
            alert_desc: b"",
        }
    }

    pub fn raise(
        &mut self,
        level: engine::ThreatLevel,
        category: engine::ThreatCategory,
        timeline_type: forensics::TimelineEventType,
        detail1: u32,
        detail2: u32,
        desc: &'static [u8],
    ) {
        if level >= self.level {
            self.level = level;
            self.category = category;
            self.timeline_type = timeline_type;
            self.detail1 = detail1;
            self.detail2 = detail2;
            self.alert_desc = desc;
        }
    }
}

pub fn trim_payload_tail(payload: &[u8], offset: usize) -> &[u8] {
    if offset >= payload.len() {
        return &[];
    }
    let tail = &payload[offset..];
    let mut len = tail.len();
    while len > 0 && tail[len - 1] == 0 {
        len -= 1;
    }
    &tail[..len]
}

pub fn u64_to_i32_saturating(value: u64) -> i32 {
    if value > i32::MAX as u64 {
        i32::MAX
    } else {
        value as i32
    }
}

pub fn threat_level_from_hook(threat: u8) -> engine::ThreatLevel {
    match threat {
        3 => engine::ThreatLevel::Critical,
        2 => engine::ThreatLevel::High,
        1 => engine::ThreatLevel::Medium,
        _ => engine::ThreatLevel::Low,
    }
}

pub fn decode_network_tuple(event: &engine::MonitoredEvent) -> (u32, u32, u16, u16, u8, u32) {
    let src_ip = (event.arg0 >> 32) as u32;
    let dst_ip = event.arg0 as u32;
    let src_port = (event.arg1 >> 48) as u16;
    let dst_port = (event.arg1 >> 32) as u16;
    let protocol = ((event.opcode & 0xFF) as u8).max(1);
    let byte_count = event.arg1 as u32;
    (src_ip, dst_ip, src_port, dst_port, protocol, byte_count)
}

pub fn behaviour_data_for_event(
    event: &engine::MonitoredEvent,
    hook_result: &HookPipelineResult,
) -> ml::features::ProcessBehaviourData {
    let mut data = ml::features::ProcessBehaviourData::zero();
    let severity = event.severity.as_u8() as i32;

    match event.event_type {
        engine::EventType::Syscall => {
            let syscall_stats = hooks::get_syscall_stats();
            data.syscall_rate = 1 + u64_to_i32_saturating(syscall_stats.total_syscalls.min(99));
            data.denied_syscall_count =
                u64_to_i32_saturating(syscall_stats.blocked_syscalls.min(99));
            data.syscall_diversity = (event.opcode & 0x7F) as i32;
            data.priv_escalation_attempts =
                u64_to_i32_saturating(syscall_stats.dangerous_syscall_count.min(99));
        }
        engine::EventType::Network => {
            let net_stats = hooks::get_net_stats();
            data.net_connect_rate = 1 + u64_to_i32_saturating(net_stats.total_events.min(99));
            data.net_bytes_sent = u64_to_i32_saturating(event.arg1.min(99));
            data.port_scan_score = u64_to_i32_saturating(net_stats.port_scan_detections.min(99));
            data.dns_query_rate = u64_to_i32_saturating(net_stats.dns_anomalies.min(99));
        }
        engine::EventType::Memory => {
            let mem_stats = hooks::get_mem_stats();
            data.mem_usage = u64_to_i32_saturating(event.arg0.min(99));
            data.anomaly_running_avg = u64_to_i32_saturating(mem_stats.overflow_detections.min(99));
            data.suspicious_path_access = u64_to_i32_saturating(mem_stats.uaf_detections.min(99));
        }
        engine::EventType::Process => {
            let exec_stats = hooks::get_exec_stats();
            data.exec_rate = 1 + u64_to_i32_saturating(exec_stats.total_execs.min(99));
            data.child_fork_rate = u64_to_i32_saturating(exec_stats.chain_anomalies.min(99));
            data.denied_syscall_count = u64_to_i32_saturating(exec_stats.denied_execs.min(99));
        }
        engine::EventType::Ipc | engine::EventType::Capability => {
            data.ipc_msg_rate = 1 + severity;
            data.priv_escalation_attempts = severity;
        }
        _ => {
            data.anomaly_running_avg = severity;
        }
    }

    if hook_result.blocked {
        data.denied_syscall_count = data.denied_syscall_count.saturating_add(10);
    }
    data.anomaly_running_avg = data
        .anomaly_running_avg
        .saturating_add((hook_result.level.as_u8() as i32) * 20);
    data
}

/// Mappe un engine::EventType vers la catégorie behavioral Markov (0..8).
/// Correspondance : behavioral::sequence::EventType (Syscall=0, MemAccess=1, …)
pub fn engine_event_to_markov_cat(et: engine::EventType) -> u8 {
    match et {
        engine::EventType::Syscall    => 0, // behavioral::Syscall
        engine::EventType::Memory     => 1, // behavioral::MemAccess
        engine::EventType::Network    => 2, // behavioral::NetConnect
        engine::EventType::Ipc        => 3, // behavioral::IpcCall
        engine::EventType::Capability => 5, // behavioral::PrivChange
        engine::EventType::Process    => 6, // behavioral::ProcessCreate
        _                             => 8, // behavioral::Custom
    }
}

/// Classifie un événement via l'ensemble ML persistant (MLP+IF+Markov).
/// Remplace l'ancienne version qui recréait un modèle vierge à chaque appel.
pub fn classify_event_ml(
    event: &engine::MonitoredEvent,
    hook_result: &HookPipelineResult,
) -> ml::Classification {
    let data = behaviour_data_for_event(event, hook_result);
    let fv = ml::FeatureExtractor::extract(&data);
    let et_raw = engine_event_to_markov_cat(event.event_type);
    ml::ensemble_classify(event.pid, et_raw, &fv).classification
}

pub fn apply_containment(pid: u32, tick: u64) -> bool {
    let engine_ok = engine::mark_process_contained(pid, tick);
    let sandbox_ok = sandbox::quarantine_pid(pid);
    let firewall_ok = network::block_pid(pid);
    if engine_ok || sandbox_ok || firewall_ok {
        forensics::record_timeline_event(
            forensics::TimelineEventType::ThreatDetected,
            pid,
            0,
            engine::ContainmentAction::Quarantine as u32,
            0,
        );
    }
    engine_ok || sandbox_ok || firewall_ok
}

pub fn release_containment(pid: u32) -> bool {
    let engine_ok = engine::release_process(pid);
    let sandbox_ok = sandbox::release_quarantine(pid);
    let firewall_ok = network::unblock_pid(pid);
    if engine_ok || sandbox_ok || firewall_ok {
        forensics::record_timeline_event(
            forensics::TimelineEventType::PolicyChange,
            pid,
            0,
            engine::ContainmentAction::None as u32,
            0,
        );
    }
    engine_ok || sandbox_ok || firewall_ok
}

pub fn process_security_hooks(event: &engine::MonitoredEvent, payload: &[u8]) -> HookPipelineResult {
    let mut result = HookPipelineResult::pass();

    match event.event_type {
        engine::EventType::Process => {
            let ppid = event.arg0 as u32;
            let uid = event.arg1 as u32;
            let flags = ((event.arg1 >> 32) & 0xFF) as u8;
            let path = trim_payload_tail(payload, 29);
            let action = hooks::pre_exec_validate(event.pid, ppid, path, uid, flags);
            hooks::post_exec_monitor(event.pid, ppid, path, uid, flags, action);
            forensics::record_timeline_event(
                forensics::TimelineEventType::Exec,
                event.pid,
                ppid,
                event.opcode,
                action as u32,
            );
            match action {
                hooks::ExecAction::Kill => {
                    result.blocked = true;
                    result.raise(
                        engine::ThreatLevel::Critical,
                        engine::ThreatCategory::PolicyViol,
                        forensics::TimelineEventType::ExecChainAnomaly,
                        event.opcode,
                        action as u32,
                        b"exec_kill_policy",
                    );
                }
                hooks::ExecAction::Deny => {
                    result.blocked = true;
                    result.raise(
                        engine::ThreatLevel::High,
                        engine::ThreatCategory::PolicyViol,
                        forensics::TimelineEventType::ExecChainAnomaly,
                        event.opcode,
                        action as u32,
                        b"exec_denied_policy",
                    );
                }
                hooks::ExecAction::Monitor => {
                    result.raise(
                        engine::ThreatLevel::Medium,
                        engine::ThreatCategory::Anomaly,
                        forensics::TimelineEventType::ExecChainAnomaly,
                        event.opcode,
                        action as u32,
                        b"exec_monitor_policy",
                    );
                }
                hooks::ExecAction::Allow => {}
            }
        }
        engine::EventType::Network => {
            let (src_ip, dst_ip, src_port, dst_port, protocol, byte_count) =
                decode_network_tuple(event);
            let pid_blocked = network::is_pid_blocked(event.pid);
            let hook_blocked =
                hooks::pre_connect_check(event.pid, src_ip, dst_ip, src_port, dst_port, protocol);
            if !pid_blocked && !hook_blocked {
                hooks::post_connect_monitor(
                    event.pid, src_ip, dst_ip, src_port, dst_port, protocol, byte_count,
                );
            }
            forensics::record_timeline_event(
                forensics::TimelineEventType::NetConnect,
                event.pid,
                0,
                dst_port as u32,
                byte_count,
            );
            if pid_blocked || hook_blocked {
                result.blocked = true;
                result.raise(
                    engine::ThreatLevel::High,
                    engine::ThreatCategory::PolicyViol,
                    forensics::TimelineEventType::SandboxViolation,
                    dst_port as u32,
                    byte_count,
                    b"net_blocked_policy",
                );
            }
            if let Some(count) = hooks::detect_port_scan(src_ip) {
                result.raise(
                    engine::ThreatLevel::High,
                    engine::ThreatCategory::Intrusion,
                    forensics::TimelineEventType::PortScan,
                    src_ip,
                    count,
                    b"port_scan_detected",
                );
            }
            if let Some(total) = hooks::detect_exfiltration(event.pid) {
                result.raise(
                    engine::ThreatLevel::Critical,
                    engine::ThreatCategory::DataExfil,
                    forensics::TimelineEventType::Exfiltration,
                    total as u32,
                    (total >> 32) as u32,
                    b"exfiltration_detected",
                );
            }
        }
        engine::EventType::Memory => {
            let size = event.arg0;
            let addr = event.arg1;
            let flags = (event.opcode & 0xFF) as u8;
            if hooks::pre_alloc_check(event.pid, size, flags) {
                result.blocked = true;
                result.raise(
                    engine::ThreatLevel::High,
                    engine::ThreatCategory::ResourceAbuse,
                    forensics::TimelineEventType::MemAnomaly,
                    size as u32,
                    (size >> 32) as u32,
                    b"memory_rate_blocked",
                );
            } else {
                hooks::post_alloc_monitor(event.pid, addr, size, flags);
            }
            if matches!(hooks::detect_buffer_overflow(event.pid, addr), Some(false)) {
                result.raise(
                    engine::ThreatLevel::High,
                    engine::ThreatCategory::Intrusion,
                    forensics::TimelineEventType::BufferOverflow,
                    addr as u32,
                    (addr >> 32) as u32,
                    b"buffer_overflow",
                );
            }
            if hooks::detect_use_after_free(event.pid, addr).is_some() {
                result.raise(
                    engine::ThreatLevel::High,
                    engine::ThreatCategory::Intrusion,
                    forensics::TimelineEventType::UseAfterFree,
                    addr as u32,
                    (addr >> 32) as u32,
                    b"use_after_free",
                );
            }
        }
        engine::EventType::Syscall => {
            let args = [event.arg0, event.arg1, 0];
            let syscall_nr = event.opcode;
            let sandbox_denied = if syscall_nr <= u8::MAX as u32 {
                !sandbox::quarantine_allows_syscall(event.pid, syscall_nr as u8)
            } else {
                sandbox::is_pid_quarantined(event.pid)
            };
            if sandbox_denied || hooks::pre_syscall_check(event.pid, syscall_nr, args) {
                result.blocked = true;
                result.raise(
                    engine::ThreatLevel::High,
                    engine::ThreatCategory::PolicyViol,
                    forensics::TimelineEventType::SyscallAnomaly,
                    syscall_nr,
                    0,
                    b"syscall_blocked_policy",
                );
            }
            hooks::post_syscall_monitor(event.pid, syscall_nr, args, 0);
            if let Some(desc) = hooks::detect_dangerous_syscall(syscall_nr) {
                let level = threat_level_from_hook((desc & 0xFF) as u8);
                result.raise(
                    level,
                    engine::ThreatCategory::PrivilegeEsc,
                    forensics::TimelineEventType::SyscallAnomaly,
                    syscall_nr,
                    desc,
                    b"dangerous_syscall",
                );
            }
            if let Some((pattern, threat)) = hooks::analyze_syscall_sequence(event.pid) {
                result.raise(
                    threat_level_from_hook(threat),
                    engine::ThreatCategory::Intrusion,
                    forensics::TimelineEventType::SyscallAnomaly,
                    syscall_nr,
                    pattern as u32,
                    b"syscall_sequence_match",
                );
            }
        }
        engine::EventType::Ipc => {
            forensics::record_timeline_event(
                forensics::TimelineEventType::IpcViolation,
                event.pid,
                event.arg0 as u32,
                event.opcode,
                event.severity.as_u8() as u32,
            );
        }
        _ => {}
    }

    if result.category != engine::ThreatCategory::None {
        forensics::record_timeline_event(
            result.timeline_type,
            event.pid,
            0,
            result.detail1,
            result.detail2,
        );
    }

    result
}

