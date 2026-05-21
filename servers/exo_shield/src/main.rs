#![no_std]
#![no_main]

//! # exo_shield — AI/Process Containment Security Server (ExoShield Phase 3)
//!
//! Monitors processes, detects anomalies, contains threats, and provides forensics.
//! All other servers delegate security queries here.
//!
//! ## IPC Protocol (incoming messages)
//! Clients send requests via SYS_IPC_SEND to endpoint "exo_shield".
//!
//! ### Message types (msg_type)
//! - SCAN_REQUEST   (0) : request a scan of a process/memory region
//! - EVENT_REPORT   (1) : report a security event for real-time analysis
//! - QUARANTINE_CMD (2) : contain or release a process
//! - THREAT_QUERY   (3) : query threat records and assessments
//! - POLICY_UPDATE  (4) : update scanning/monitoring policies and filters
//! - HEARTBEAT      (5) : liveness check
//! - PMC_ANOMALY    (6) : report hardware counter anomaly samples
//!
//! Public request classes are audited and rate-limited through `ipc_gate`.
//! Administrative mutations, cross-process actions, and detailed threat
//! queries are capability-gated at request classification time.
//!
//! ## Architecture
//! - engine::core     — threat scoring, records, risk profiles
//! - engine::scanner  — signature & heuristic scanning with periodic scheduler
//! - engine::realtime — real-time event monitoring, filtering, rate tracking, alerts

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;

// ── Message Types ───────────────────────────────────────────────────────────

const SCAN_REQUEST: u32 = 0;
const EVENT_REPORT: u32 = 1;
const QUARANTINE_CMD: u32 = 2;
const THREAT_QUERY: u32 = 3;
const POLICY_UPDATE: u32 = 4;
const HEARTBEAT: u32 = 5;
const PMC_ANOMALY_REPORT: u32 = 6;

// ── Reply Status Codes ──────────────────────────────────────────────────────

const SHIELD_OK: u32 = 0;
const SHIELD_ERR_ARGS: u32 = 1;
const SHIELD_ERR_BUSY: u32 = 2;
const SHIELD_ERR_NOT_FOUND: u32 = 3;
const SHIELD_ERR_DENIED: u32 = 4;
const SHIELD_ERR_CAP: u32 = 5;
const SHIELD_ERR_NOT_CONTAINED: u32 = 7;

const POLICY_SIGNATURE_PATTERN_OFFSET: usize = 8;
const POLICY_SIGNATURE_PATTERN_LEN_OFFSET: usize =
    POLICY_SIGNATURE_PATTERN_OFFSET + engine::SIGNATURE_PATTERN_SIZE;
const POLICY_SIGNATURE_NAME_LEN_OFFSET: usize = POLICY_SIGNATURE_PATTERN_LEN_OFFSET + 1;
const POLICY_SIGNATURE_NAME_OFFSET: usize = POLICY_SIGNATURE_NAME_LEN_OFFSET + 1;
const _: () = assert!(
    POLICY_SIGNATURE_NAME_OFFSET + engine::MAX_SIG_NAME <= syscall::IPC_INLINE_PAYLOAD_SIZE,
    "POLICY_UPDATE signature payload must fit in one IPC envelope"
);

// ── IPC Message Structures ──────────────────────────────────────────────────

/// Incoming IPC request (128 bytes).
///
/// When `ipc_gate` classifies a request as privileged, `payload[100..120]`
/// carries an `ExoCapTokenWire` targeting the live exo_shield service PID.
#[repr(C)]
struct ShieldRequest {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

const _: () = assert!(core::mem::size_of::<ShieldRequest>() == syscall::IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(ShieldRequest, payload) == syscall::IPC_HEADER_SIZE);

/// Outgoing IPC reply (64 bytes).
#[repr(C)]
struct ShieldReply {
    status: u32,
    data: [u8; 56],
}

impl ShieldReply {
    fn new(status: u32) -> Self {
        ShieldReply {
            status: status,
            data: [0u8; 56],
        }
    }
}

// ── IPC Constants ───────────────────────────────────────────────────────────

const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = syscall::ETIMEDOUT;
const EXO_SHIELD_ENDPOINT: u64 = 10;
const EXO_SHIELD_PID: u64 = 12;
const _: () = assert!(syscall::EXO_CAP_TOKEN_WIRE_SIZE == ipc_gate::EXO_SHIELD_CAP_TOKEN_LEN);

// ── Global Statistics ───────────────────────────────────────────────────────

static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUESTS_OK: AtomicU64 = AtomicU64::new(0);
static REQUESTS_ERR: AtomicU64 = AtomicU64::new(0);
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static PERIODIC_SCAN_TICKS: AtomicU64 = AtomicU64::new(0);
static MAINTENANCE_TICKS: AtomicU64 = AtomicU64::new(0);

// ── Global Tick Counter ─────────────────────────────────────────────────────

static GLOBAL_TICK: AtomicU64 = AtomicU64::new(0);

fn current_tick() -> u64 {
    GLOBAL_TICK.load(Ordering::Relaxed)
}

fn advance_tick() {
    GLOBAL_TICK.fetch_add(1, Ordering::Relaxed);
}

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Payload Parsing Helpers ─────────────────────────────────────────────────

fn read_u32_le(payload: &[u8], offset: usize) -> u32 {
    if offset + 4 > payload.len() {
        return 0;
    }
    u32::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ])
}

fn read_u64_le(payload: &[u8], offset: usize) -> u64 {
    if offset + 8 > payload.len() {
        return 0;
    }
    u64::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
        payload[offset + 4],
        payload[offset + 5],
        payload[offset + 6],
        payload[offset + 7],
    ])
}

#[derive(Clone, Copy)]
struct HookPipelineResult {
    blocked: bool,
    level: engine::ThreatLevel,
    category: engine::ThreatCategory,
    timeline_type: forensics::TimelineEventType,
    detail1: u32,
    detail2: u32,
    alert_desc: &'static [u8],
}

impl HookPipelineResult {
    const fn pass() -> Self {
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

    fn raise(
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

fn trim_payload_tail(payload: &[u8], offset: usize) -> &[u8] {
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

fn u64_to_i32_saturating(value: u64) -> i32 {
    if value > i32::MAX as u64 {
        i32::MAX
    } else {
        value as i32
    }
}

fn threat_level_from_hook(threat: u8) -> engine::ThreatLevel {
    match threat {
        3 => engine::ThreatLevel::Critical,
        2 => engine::ThreatLevel::High,
        1 => engine::ThreatLevel::Medium,
        _ => engine::ThreatLevel::Low,
    }
}

fn decode_network_tuple(event: &engine::MonitoredEvent) -> (u32, u32, u16, u16, u8, u32) {
    let src_ip = (event.arg0 >> 32) as u32;
    let dst_ip = event.arg0 as u32;
    let src_port = (event.arg1 >> 48) as u16;
    let dst_port = (event.arg1 >> 32) as u16;
    let protocol = ((event.opcode & 0xFF) as u8).max(1);
    let byte_count = event.arg1 as u32;
    (src_ip, dst_ip, src_port, dst_port, protocol, byte_count)
}

fn behaviour_data_for_event(
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

fn classify_event_ml(
    event: &engine::MonitoredEvent,
    hook_result: &HookPipelineResult,
) -> ml::Classification {
    let model = ml::ModelWeights::new_seeded(0xE50_5002, ml::ActivationFn::Relu);
    let inference = ml::InferenceEngine::new(model);
    let data = behaviour_data_for_event(event, hook_result);
    inference.infer_from_behaviour(&data).classification()
}

fn apply_containment(pid: u32, tick: u64) -> bool {
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

fn release_containment(pid: u32) -> bool {
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

fn process_security_hooks(event: &engine::MonitoredEvent, payload: &[u8]) -> HookPipelineResult {
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

fn extract_service_cap_token(req: &ShieldRequest) -> syscall::ExoCapTokenWire {
    let mut token = syscall::ExoCapTokenWire::empty();
    token.bytes.copy_from_slice(
        &req.payload[ipc_gate::EXO_SHIELD_CAP_TOKEN_OFFSET
            ..ipc_gate::EXO_SHIELD_CAP_TOKEN_OFFSET + ipc_gate::EXO_SHIELD_CAP_TOKEN_LEN],
    );
    token
}

fn record_request_audit(req: &ShieldRequest, action: u8, rule_id: u32, result: u8) {
    ipc_gate::record_audit(AuditEntry {
        src_pid: req.sender_pid,
        dst_pid: EXO_SHIELD_PID as u32,
        msg_type: req.msg_type,
        action,
        rule_id,
        reply_nonce: 0,
        result,
        _pad: [0; 2],
        timestamp: read_tsc(),
    });
}

fn authorize_request(req: &ShieldRequest) -> Result<(), ShieldReply> {
    let eval = ipc_gate::evaluate_policy(req.sender_pid, EXO_SHIELD_PID as u32, req.msg_type);
    let action = PolicyAction::from_u8(eval.action);

    if eval.rate_limit > 0 && action == PolicyAction::Deny {
        record_request_audit(req, PolicyAction::Deny as u8, eval.matched_rule_id, 2);
        return Err(ShieldReply::new(SHIELD_ERR_BUSY));
    }

    if action == PolicyAction::Deny {
        record_request_audit(req, PolicyAction::Deny as u8, eval.matched_rule_id, 1);
        return Err(ShieldReply::new(SHIELD_ERR_DENIED));
    }

    match ipc_gate::classify_service_cap_requirement(req.sender_pid, req.msg_type, &req.payload) {
        ServiceCapRequirement::NotRequired => {}
        ServiceCapRequirement::Malformed => {
            record_request_audit(req, PolicyAction::Deny as u8, eval.matched_rule_id, 1);
            return Err(ShieldReply::new(SHIELD_ERR_ARGS));
        }
        ServiceCapRequirement::Required => {
            let token = extract_service_cap_token(req);
            let cap_ok = !token.is_empty()
                && unsafe {
                    syscall::exo_cap_check(
                        &token,
                        syscall::EXO_CAP_RIGHT_IPC_SEND,
                        EXO_SHIELD_PID as u32,
                        syscall::EXO_CAP_TYPE_IPC_ENDPOINT,
                    )
                } == 0;

            if !cap_ok {
                record_request_audit(req, PolicyAction::Deny as u8, eval.matched_rule_id, 1);
                return Err(ShieldReply::new(SHIELD_ERR_CAP));
            }
        }
    }

    record_request_audit(req, eval.action, eval.matched_rule_id, 0);
    Ok(())
}

// ── Message Handlers ────────────────────────────────────────────────────────

/// Handle SCAN_REQUEST (msg_type 0).
///
/// Payload layout:
///   [0..4]   target_pid (LE)
///   [4]      scan_type  (0=full, 1=quick, 2=memory, 3=behavioral)
///   [5]      priority   (0=low, 1=normal, 2=high, 3=critical)
///   [6..10]  scan_data_len (LE, max 100)
///   [10..]   scan_data (raw bytes to scan)
///
/// Reply:
///   data[0..4]  = scan_request_id (LE)
///   data[4..8]  = composite_score (LE)
///   data[8]     = max_severity
///   data[9]     = matched (0/1)
///   data[10..14] = match_count (LE)
fn handle_scan_request(req: &ShieldRequest) -> ShieldReply {
    let target_pid = read_u32_le(&req.payload, 0);
    let scan_type = req.payload[4];
    let priority = req.payload[5];
    let data_len = read_u32_le(&req.payload, 6) as usize;
    let data_end = (10 + data_len).min(120);
    let scan_data = &req.payload[10..data_end];
    let tick = current_tick();

    if target_pid == 0 {
        return ShieldReply::new(SHIELD_ERR_ARGS);
    }

    // Execute the scan directly against provided data
    let profile = engine::active_scan_profile();
    let heuristic_level = profile.map(|p| p.heuristic_level).unwrap_or(1);

    let result = engine::execute_scan(target_pid, scan_data, scan_type, heuristic_level, tick);
    engine::stat_scan_executed(result.matched);

    // Also queue for periodic tracking
    let queued_scan_id: u32 =
        if let Some(scan_id) = engine::queue_scan(target_pid, scan_type, priority, tick) {
            engine::stat_scan_queued();
            scan_id
        } else {
            0
        };

    // Build reply
    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0..4].copy_from_slice(&queued_scan_id.to_le_bytes());
    reply.data[4..8].copy_from_slice(&result.composite.to_le_bytes());
    reply.data[8] = result.max_severity.as_u8();
    reply.data[9] = if result.matched { 1 } else { 0 };
    reply.data[10..14].copy_from_slice(&result.match_count.to_le_bytes());

    // Store the result
    let _ = engine::store_scan_result(&result);

    reply
}

/// Handle EVENT_REPORT (msg_type 1).
///
/// Payload layout:
///   [0..4]   target_pid (LE)
///   [4]      event_type (EventType as u8)
///   [8..12]  opcode (LE)
///   [12..20] arg0 (LE)
///   [20..28] arg1 (LE)
///   [28]     severity (ThreatLevel as u8)
///
/// Reply:
///   data[0..4]  = alert_id (LE, 0 if none)
///   data[4]     = action taken (FilterAction as u8)
///   data[5]     = rate_exceeded (0/1)
///   data[6]     = contained (0/1)
fn handle_event_report(req: &ShieldRequest) -> ShieldReply {
    let target_pid = read_u32_le(&req.payload, 0);
    let event_type = engine::EventType::from_u8(req.payload[4]);
    let opcode = read_u32_le(&req.payload, 8);
    let arg0 = read_u64_le(&req.payload, 12);
    let arg1 = read_u64_le(&req.payload, 20);
    let severity = engine::ThreatLevel::from_u8(req.payload[28]);
    let tick = current_tick();

    if target_pid == 0 {
        return ShieldReply::new(SHIELD_ERR_ARGS);
    }

    let event = engine::MonitoredEvent {
        pid: target_pid,
        event_type: event_type,
        opcode: opcode,
        arg0: arg0,
        arg1: arg1,
        timestamp: tick,
        severity: severity,
    };

    let hook_result = process_security_hooks(&event, &req.payload);
    let result = engine::submit_event(&event, tick);
    let ml_classification = classify_event_ml(&event, &hook_result);
    let mut alert_id = result.alert_id;
    let mut action = result.action;
    let mut contained = result.contained;

    if hook_result.blocked {
        action = engine::FilterAction::Block;
        if alert_id == 0 {
            alert_id = engine::generate_manual_alert(
                target_pid,
                hook_result.level,
                hook_result.category,
                2,
                hook_result.alert_desc,
                tick,
            )
            .unwrap_or(0);
        }
    }

    if matches!(
        ml_classification,
        ml::Classification::Malicious | ml::Classification::Suspicious
    ) && alert_id == 0
    {
        let (level, desc) = if ml_classification == ml::Classification::Malicious {
            (engine::ThreatLevel::High, b"ml_malicious_event" as &[u8])
        } else {
            (engine::ThreatLevel::Medium, b"ml_suspicious_event" as &[u8])
        };
        alert_id = engine::generate_manual_alert(
            target_pid,
            level,
            engine::ThreatCategory::Anomaly,
            3,
            desc,
            tick,
        )
        .unwrap_or(0);
    }

    if hook_result.level >= engine::ThreatLevel::Critical
        || ml_classification == ml::Classification::Malicious
    {
        contained = apply_containment(target_pid, tick) || contained;
    }

    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0..4].copy_from_slice(&alert_id.to_le_bytes());
    reply.data[4] = action as u8;
    reply.data[5] = if result.rate_exceeded { 1 } else { 0 };
    reply.data[6] = if contained { 1 } else { 0 };

    reply
}

/// Handle QUARANTINE_CMD (msg_type 2).
///
/// Payload layout:
///   [0]      command (0=contain, 1=release, 2=query status)
///   [1..5]   target_pid (LE)
///
/// Reply for contain (0):
///   data[0] = success (0/1)
/// Reply for release (1):
///   data[0] = success (0/1)
/// Reply for query (2):
///   data[0]    = is_contained (0/1)
///   data[1..5] = threat_count (LE)
fn handle_quarantine_cmd(req: &ShieldRequest) -> ShieldReply {
    let cmd = req.payload[0];
    let target_pid = read_u32_le(&req.payload, 1);
    let tick = current_tick();

    if target_pid == 0 {
        return ShieldReply::new(SHIELD_ERR_ARGS);
    }

    match cmd {
        0 => {
            // Contain process
            let ok = apply_containment(target_pid, tick);
            if ok {
                engine::stat_containments_inc();
            }
            let mut reply = ShieldReply::new(if ok { SHIELD_OK } else { SHIELD_ERR_NOT_FOUND });
            reply.data[0] = if ok { 1 } else { 0 };
            reply
        }
        1 => {
            // Release process from containment
            let ok = release_containment(target_pid);
            let mut reply = ShieldReply::new(if ok {
                SHIELD_OK
            } else {
                SHIELD_ERR_NOT_CONTAINED
            });
            reply.data[0] = if ok { 1 } else { 0 };
            reply
        }
        2 => {
            // Query containment status
            let profile = engine::get_risk_profile(target_pid);
            let is_contained = profile
                .map(|p| if p.contained { 1u8 } else { 0u8 })
                .unwrap_or(0);
            let is_contained = if is_contained != 0 || sandbox::is_pid_quarantined(target_pid) {
                1u8
            } else {
                0u8
            };
            let threats = engine::active_threat_count();
            let mut reply = ShieldReply::new(SHIELD_OK);
            reply.data[0] = is_contained;
            reply.data[1..5].copy_from_slice(&threats.to_le_bytes());
            reply
        }
        _ => ShieldReply::new(SHIELD_ERR_ARGS),
    }
}

/// Handle THREAT_QUERY (msg_type 3).
///
/// Payload layout:
///   [0]      query_type (0=by_id, 1=by_pid, 2=assess_pid, 3=stats)
///   [1..5]   id_or_pid (LE)
///
/// Reply for by_id (0):
///   data[0..4]   = threat_id (LE)
///   data[4..8]   = pid (LE)
///   data[8]      = level
///   data[9]      = category
///   data[10..14] = score (LE)
///   data[14]     = contained (0/1)
///   data[15]     = resolved (0/1)
/// Reply for by_pid (1):
///   data[0..4]   = count of threats for pid
///   data[4..8]   = first threat id (or 0)
///   data[8..12]  = max score
///   data[12]     = max level
/// Reply for assess_pid (2):
///   data[0..4]   = composite_score (LE)
///   data[4]      = level
///   data[5]      = recommended_action
///   data[6..10]  = active_threats (LE)
///   data[10..14] = history_threats (LE)
/// Reply for stats (3):
///   data[0..8]   = total_assessments (LE)
///   data[8..16]  = total_threats (LE)
///   data[16..24] = containments (LE)
///   data[24..32] = resolved (LE)
///   data[32..36] = critical_alerts (LE)
///   data[36..40] = active_threats (LE)
fn handle_threat_query(req: &ShieldRequest) -> ShieldReply {
    let query_type = req.payload[0];
    let id_or_pid = read_u32_le(&req.payload, 1);
    let tick = current_tick();

    match query_type {
        0 => {
            // Query by threat ID
            match engine::get_threat(id_or_pid) {
                Some(threat) => {
                    let mut reply = ShieldReply::new(SHIELD_OK);
                    reply.data[0..4].copy_from_slice(&threat.id.to_le_bytes());
                    reply.data[4..8].copy_from_slice(&threat.pid.to_le_bytes());
                    reply.data[8] = threat.level.as_u8();
                    reply.data[9] = threat.category as u8;
                    reply.data[10..14].copy_from_slice(&threat.score.to_le_bytes());
                    reply.data[14] = if threat.contained { 1 } else { 0 };
                    reply.data[15] = if threat.resolved { 1 } else { 0 };
                    reply
                }
                None => ShieldReply::new(SHIELD_ERR_NOT_FOUND),
            }
        }
        1 => {
            // Query by PID
            let mut threats = [engine::ThreatRecord::empty(); 16];
            let count = engine::get_threats_by_pid(id_or_pid, &mut threats, 16);
            let mut max_score = 0u32;
            let mut max_level = engine::ThreatLevel::Low;
            let mut first_id = 0u32;

            for i in 0..count {
                if threats[i].score > max_score {
                    max_score = threats[i].score;
                }
                if threats[i].level > max_level {
                    max_level = threats[i].level;
                }
                if first_id == 0 {
                    first_id = threats[i].id;
                }
            }

            let mut reply = ShieldReply::new(SHIELD_OK);
            reply.data[0..4].copy_from_slice(&(count as u32).to_le_bytes());
            reply.data[4..8].copy_from_slice(&first_id.to_le_bytes());
            reply.data[8..12].copy_from_slice(&max_score.to_le_bytes());
            reply.data[12] = max_level.as_u8();
            reply
        }
        2 => {
            // Assess PID
            let assessment = engine::assess_pid(id_or_pid, tick);
            engine::stat_assessments_inc();
            let mut reply = ShieldReply::new(SHIELD_OK);
            reply.data[0..4].copy_from_slice(&assessment.composite_score.to_le_bytes());
            reply.data[4] = assessment.level.as_u8();
            reply.data[5] = assessment.recommended_action as u8;
            reply.data[6..10].copy_from_slice(&assessment.active_threats.to_le_bytes());
            reply.data[10..14].copy_from_slice(&assessment.history_threats.to_le_bytes());
            reply
        }
        3 => {
            // Get core stats
            let stats = engine::get_core_stats();
            let mut reply = ShieldReply::new(SHIELD_OK);
            reply.data[0..8].copy_from_slice(&stats.total_assessments.to_le_bytes());
            reply.data[8..16].copy_from_slice(&stats.total_threats.to_le_bytes());
            reply.data[16..24].copy_from_slice(&stats.containments.to_le_bytes());
            reply.data[24..32].copy_from_slice(&stats.resolved.to_le_bytes());
            reply.data[32..36].copy_from_slice(&stats.critical_alerts.to_le_bytes());
            reply.data[36..40].copy_from_slice(&stats.active_threats.to_le_bytes());
            reply
        }
        _ => ShieldReply::new(SHIELD_ERR_ARGS),
    }
}

/// Handle POLICY_UPDATE (msg_type 4).
///
/// Payload layout:
///   [0]      policy_type (0=add_filter, 1=remove_filter, 2=set_rate_limits,
///                         3=register_periodic, 4=unregister_periodic,
///                         5=add_signature, 6=disable_signature,
///                         7=enable_signature, 8=set_scan_profile,
///                         9=monitor_process, 10=unmonitor_process)
///   [1..]    policy-specific data
fn handle_policy_update(req: &ShieldRequest) -> ShieldReply {
    let policy_type = req.payload[0];

    match policy_type {
        0 => {
            // Add event filter
            // [1] event_type, [2] min_severity, [3] action, [4..8] pid, [8..12] opcode_mask
            let event_type = engine::EventType::from_u8(req.payload[1]);
            let min_severity = engine::ThreatLevel::from_u8(req.payload[2]);
            let action = engine::FilterAction::from_u8(req.payload[3]);
            let pid = read_u32_le(&req.payload, 4);
            let opcode_mask = read_u32_le(&req.payload, 8);

            let filter = engine::EventFilter {
                id: 0,
                event_type: event_type,
                pid: pid,
                opcode_mask: opcode_mask,
                min_severity: min_severity,
                action: action,
                enabled: true,
            };

            match engine::add_event_filter(&filter) {
                Some(id) => {
                    let mut reply = ShieldReply::new(SHIELD_OK);
                    reply.data[0..4].copy_from_slice(&id.to_le_bytes());
                    reply
                }
                None => ShieldReply::new(SHIELD_ERR_BUSY),
            }
        }
        1 => {
            // Remove event filter
            // [1..5] filter_id (LE)
            let filter_id = read_u32_le(&req.payload, 1);
            if engine::remove_event_filter(filter_id) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_NOT_FOUND)
            }
        }
        2 => {
            // Set rate limits for a process
            // [1..5] pid, [5..9] syscall_limit, [9..13] net_limit,
            // [13..17] fs_limit, [17..21] anomaly_limit
            let pid = read_u32_le(&req.payload, 1);
            let sysc_limit = read_u32_le(&req.payload, 5);
            let net_limit = read_u32_le(&req.payload, 9);
            let fs_limit = read_u32_le(&req.payload, 13);
            let anom_limit = read_u32_le(&req.payload, 17);

            if engine::set_process_rate_limits(pid, sysc_limit, net_limit, fs_limit, anom_limit) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_NOT_FOUND)
            }
        }
        3 => {
            // Register periodic scan
            // [1..5] pid, [5..13] interval (LE), [13] scan_type, [14] priority
            let pid = read_u32_le(&req.payload, 1);
            let interval = read_u64_le(&req.payload, 5);
            let scan_type = req.payload[13];
            let priority = req.payload[14];

            if engine::register_periodic_scan(pid, interval, scan_type, priority) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_BUSY)
            }
        }
        4 => {
            // Unregister periodic scan
            // [1..5] pid
            let pid = read_u32_le(&req.payload, 1);
            if engine::unregister_periodic_scan(pid) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_NOT_FOUND)
            }
        }
        5 => {
            // Add signature
            // [1] sig_type, [2] category, [3] severity, [4..8] base_score (LE)
            // [8..72] pattern (64 bytes), [72] pattern_len,
            // [73] name_len, [74..106] name
            let sig_type = req.payload[1];
            let category = engine::ThreatCategory::from_u8(req.payload[2]);
            let severity = engine::ThreatLevel::from_u8(req.payload[3]);
            let base_score = read_u32_le(&req.payload, 4);

            let mut entry = engine::SignatureEntry::empty();
            entry.sig_type = sig_type;
            entry.category = category;
            entry.severity = severity;
            entry.base_score = base_score;

            entry.pattern.copy_from_slice(
                &req.payload[POLICY_SIGNATURE_PATTERN_OFFSET..POLICY_SIGNATURE_PATTERN_LEN_OFFSET],
            );
            entry.pattern_len = req.payload[POLICY_SIGNATURE_PATTERN_LEN_OFFSET]
                .min(engine::SIGNATURE_PATTERN_SIZE as u8);
            entry.name_len =
                req.payload[POLICY_SIGNATURE_NAME_LEN_OFFSET].min(engine::MAX_SIG_NAME as u8);
            let name_start = POLICY_SIGNATURE_NAME_OFFSET;
            let name_len = entry.name_len as usize;
            if name_start + name_len <= req.payload.len() {
                entry.name[..name_len]
                    .copy_from_slice(&req.payload[name_start..name_start + name_len]);
            }
            entry.enabled = true;

            match engine::add_signature(&entry) {
                Some(id) => {
                    let mut reply = ShieldReply::new(SHIELD_OK);
                    reply.data[0..4].copy_from_slice(&id.to_le_bytes());
                    reply
                }
                None => ShieldReply::new(SHIELD_ERR_BUSY),
            }
        }
        6 => {
            // Disable signature
            let sig_id = read_u32_le(&req.payload, 1);
            if engine::disable_signature(sig_id) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_NOT_FOUND)
            }
        }
        7 => {
            // Enable signature
            let sig_id = read_u32_le(&req.payload, 1);
            if engine::enable_signature(sig_id) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_NOT_FOUND)
            }
        }
        8 => {
            // Set scan profile
            // [1] profile_index, [2] scan_memory, [3] scan_syscalls,
            // [4] scan_network, [5] scan_fs, [6] scan_behavior,
            // [7] max_depth, [8..10] timeout_ticks (LE), [10] heuristic_level
            let idx = req.payload[1] as usize;
            let mut profile = engine::ScanProfile::empty();
            profile.scan_memory = req.payload[2] != 0;
            profile.scan_syscalls = req.payload[3] != 0;
            profile.scan_network = req.payload[4] != 0;
            profile.scan_fs = req.payload[5] != 0;
            profile.scan_behavior = req.payload[6] != 0;
            profile.max_depth = req.payload[7];
            profile.timeout_ticks = req.payload[8] as u16 | ((req.payload[9] as u16) << 8);
            profile.heuristic_level = req.payload[10];
            profile.enabled = true;
            // Copy name from payload if available
            let name_start = 11;
            let name_len = if req.payload.len() > name_start {
                let nl = (req.payload.len() - name_start).min(24);
                profile.name[..nl].copy_from_slice(&req.payload[name_start..name_start + nl]);
                nl as u8
            } else {
                0
            };
            profile.name_len = name_len;

            if engine::set_scan_profile(idx, &profile) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_ARGS)
            }
        }
        9 => {
            // Monitor process
            // [1..5] pid, [5] watch_level
            let pid = read_u32_le(&req.payload, 1);
            let watch_level = engine::ThreatLevel::from_u8(req.payload[5]);
            if engine::monitor_process(pid, watch_level) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_BUSY)
            }
        }
        10 => {
            // Unmonitor process
            // [1..5] pid
            let pid = read_u32_le(&req.payload, 1);
            if engine::unmonitor_process(pid) {
                ShieldReply::new(SHIELD_OK)
            } else {
                ShieldReply::new(SHIELD_ERR_NOT_FOUND)
            }
        }
        _ => ShieldReply::new(SHIELD_ERR_ARGS),
    }
}

/// Handle HEARTBEAT (msg_type 5).
///
/// Reply:
///   data[0]    = engine_init (0/1)
///   data[1..5] = active_threats (LE)
///   data[5..9] = pending_scans (LE)
///   data[9..13] = unacknowledged_alerts (LE)
fn handle_heartbeat(_req: &ShieldRequest) -> ShieldReply {
    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0] = if engine::engine_is_init() { 1 } else { 0 };
    let active = engine::active_threat_count();
    let pending = engine::pending_scan_count();
    let unack = engine::unacknowledged_alert_count();
    reply.data[1..5].copy_from_slice(&active.to_le_bytes());
    reply.data[5..9].copy_from_slice(&pending.to_le_bytes());
    reply.data[9..13].copy_from_slice(&unack.to_le_bytes());
    reply
}

/// Handle PMC_ANOMALY_REPORT (msg_type 6).
///
/// Payload layout:
///   [0..4]   target_pid (LE)
///   [4..12]  instructions retired (LE)
///   [12..20] cycles unhalted (LE)
///   [20..28] LLC misses (LE)
///   [28..36] branch mispredicts (LE)
///   [36..40] discordance score 0..1000 (LE)
///
/// Reply:
///   data[0..4] = alert_id (LE, 0 if none)
///   data[4]    = contained (0/1)
fn handle_pmc_anomaly_report(req: &ShieldRequest) -> ShieldReply {
    const PMC_ANOMALY_OPCODE: u32 = 0x504D_4300;

    let target_pid = read_u32_le(&req.payload, 0);
    let inst_retired = read_u64_le(&req.payload, 4);
    let clk_unhalted = read_u64_le(&req.payload, 12);
    let l3_miss = read_u64_le(&req.payload, 20);
    let br_mispred = read_u64_le(&req.payload, 28);
    let discordance = read_u32_le(&req.payload, 36).min(1000);
    let tick = current_tick();

    if target_pid == 0 {
        return ShieldReply::new(SHIELD_ERR_ARGS);
    }

    let level = if discordance >= 750 {
        engine::ThreatLevel::Critical
    } else if discordance >= 500 {
        engine::ThreatLevel::High
    } else {
        engine::ThreatLevel::Medium
    };

    let event = engine::MonitoredEvent {
        pid: target_pid,
        event_type: engine::EventType::Custom(0xF0),
        opcode: PMC_ANOMALY_OPCODE,
        arg0: inst_retired ^ clk_unhalted,
        arg1: l3_miss ^ br_mispred,
        timestamp: tick,
        severity: level,
    };
    let result = engine::submit_event(&event, tick);

    forensics::record_timeline_event(
        forensics::TimelineEventType::ThreatDetected,
        target_pid,
        0,
        discordance,
        (l3_miss ^ br_mispred) as u32,
    );

    let alert_id = engine::generate_manual_alert(
        target_pid,
        level,
        engine::ThreatCategory::Anomaly,
        4,
        b"pmc_anomaly",
        tick,
    )
    .unwrap_or(result.alert_id);

    let contained = if level >= engine::ThreatLevel::Critical {
        apply_containment(target_pid, tick)
    } else {
        result.contained
    };

    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0..4].copy_from_slice(&alert_id.to_le_bytes());
    reply.data[4] = if contained { 1 } else { 0 };
    reply
}

// ── Dispatch ────────────────────────────────────────────────────────────────

fn handle_request(req: &ShieldRequest) -> ShieldReply {
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    advance_tick();

    if let Err(reply) = authorize_request(req) {
        REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
        return reply;
    }

    let reply = match req.msg_type {
        SCAN_REQUEST => handle_scan_request(req),
        EVENT_REPORT => handle_event_report(req),
        QUARANTINE_CMD => handle_quarantine_cmd(req),
        THREAT_QUERY => handle_threat_query(req),
        POLICY_UPDATE => handle_policy_update(req),
        HEARTBEAT => handle_heartbeat(req),
        PMC_ANOMALY_REPORT => handle_pmc_anomaly_report(req),
        _ => ShieldReply::new(SHIELD_ERR_ARGS),
    };

    if reply.status == SHIELD_OK {
        REQUESTS_OK.fetch_add(1, Ordering::Relaxed);
    } else {
        REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
    }

    reply
}

// ── Periodic Maintenance ────────────────────────────────────────────────────

/// Called on IPC timeout to perform periodic maintenance tasks.
fn perform_maintenance() {
    MAINTENANCE_TICKS.fetch_add(1, Ordering::Relaxed);
    let tick = current_tick();

    // Run periodic scan scheduler — enqueue scans that are due
    let enqueued = engine::periodic_scan_tick(tick);
    PERIODIC_SCAN_TICKS.fetch_add(enqueued as u64, Ordering::Relaxed);

    // Process pending scan requests from the queue
    let mut processed = 0u32;
    while let Some(scan_req) = engine::next_scan_request() {
        // For queued scans without inline data, perform a lightweight assessment
        let assessment = engine::assess_pid(scan_req.pid, tick);
        engine::stat_assessments_inc();

        // If assessment shows high threat, auto-contain
        if assessment.recommended_action as u8 >= engine::ContainmentAction::Quarantine as u8 {
            if apply_containment(scan_req.pid, tick) {
                engine::stat_containments_inc();
            }

            // Generate alert
            let desc: &[u8] = if assessment.composite_score >= 750 {
                b"auto_contain_critical"
            } else {
                b"auto_contain_high"
            };
            engine::generate_manual_alert(
                scan_req.pid,
                assessment.level,
                engine::ThreatCategory::Anomaly,
                2,
                desc,
                tick,
            );
        }

        engine::complete_scan_request(scan_req.id, tick);
        engine::stat_scan_executed(false);

        processed += 1;
        if processed >= 8 {
            // Limit work per maintenance cycle
            break;
        }
    }
}

// ── Entry Point ─────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 0. Initialize all engine sub-modules ────────────────────────────────
    ipc_gate::policy_init();
    ipc_gate::audit_init();
    engine::engine_init();
    signatures::signatures_init();
    behavioral::behavioral_init();
    hooks::exec_hooks_init();
    hooks::net_hooks_init();
    hooks::mem_hooks_init();
    hooks::syscall_hooks_init();
    sandbox::sandbox_init();
    network::firewall_init();
    forensics::memory_dump_init();
    forensics::timeline_init();
    forensics::report_init();

    // ── 1. Register public exo_shield endpoint ─────────────────────────────
    let name = b"exo_shield";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            EXO_SHIELD_ENDPOINT,
        )
    };

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
