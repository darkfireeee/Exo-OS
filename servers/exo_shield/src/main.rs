#![no_std]
#![no_main]

//! # exo_shield — PID 10, AI/Process Containment Security Server (ExoShield Phase 3)
//!
//! Monitors processes, detects anomalies, contains threats, and provides forensics.
//! All other servers delegate security queries here.
//!
//! ## IPC Protocol (incoming messages)
//! Clients send requests via SYS_IPC_SEND to endpoint "exo_shield" (PID 10).
//!
//! ### Message types (msg_type)
//! - SCAN_REQUEST   (0) : request a scan of a process/memory region
//! - EVENT_REPORT   (1) : report a security event for real-time analysis
//! - QUARANTINE_CMD (2) : contain or release a process
//! - THREAT_QUERY   (3) : query threat records and assessments
//! - POLICY_UPDATE  (4) : update scanning/monitoring policies and filters
//! - HEARTBEAT      (5) : liveness check
//!
//! ## Architecture
//! - engine::core     — threat scoring, records, risk profiles
//! - engine::scanner  — signature & heuristic scanning with periodic scheduler
//! - engine::realtime — real-time event monitoring, filtering, rate tracking, alerts

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

mod engine;
mod signatures;
mod behavioral;

// ── Syscall Interface ───────────────────────────────────────────────────────

mod syscall {
    #[inline(always)]
    pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1, in("rsi") a2, in("rdx") a3,
            in("r10") a4, in("r8")  a5, in("r9")  a6,
            lateout("rax") ret,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
        syscall6(nr, a1, a2, a3, 0, 0, 0)
    }

    pub const SYS_IPC_REGISTER: u64 = 300;
    pub const SYS_IPC_RECV:     u64 = 301;
    pub const SYS_IPC_SEND:     u64 = 302;
}

// ── Message Types ───────────────────────────────────────────────────────────

const SCAN_REQUEST:   u32 = 0;
const EVENT_REPORT:   u32 = 1;
const QUARANTINE_CMD: u32 = 2;
const THREAT_QUERY:   u32 = 3;
const POLICY_UPDATE:  u32 = 4;
const HEARTBEAT:      u32 = 5;

// ── Reply Status Codes ──────────────────────────────────────────────────────

const SHIELD_OK:                u32 = 0;
const SHIELD_ERR_ARGS:          u32 = 1;
const SHIELD_ERR_BUSY:          u32 = 2;
const SHIELD_ERR_NOT_FOUND:     u32 = 3;
const SHIELD_ERR_UNAUTHORIZED:  u32 = 4;
const SHIELD_ERR_THREAT_EXISTS: u32 = 5;
const SHIELD_ERR_QUEUE_FULL:    u32 = 6;
const SHIELD_ERR_NOT_CONTAINED: u32 = 7;

// ── IPC Message Structures ──────────────────────────────────────────────────

/// Incoming IPC request (128 bytes).
#[repr(C)]
struct ShieldRequest {
    sender_pid: u32,
    msg_type:   u32,
    payload:    [u8; 120],
}

/// Outgoing IPC reply (64 bytes).
#[repr(C)]
struct ShieldReply {
    status:  u32,
    data:    [u8; 56],
}

impl ShieldReply {
    fn new(status: u32) -> Self {
        ShieldReply {
            status: status,
            data:   [0u8; 56],
        }
    }
}

// ── IPC Constants ───────────────────────────────────────────────────────────

const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT:    u64 = 0x0001;
const ETIMEDOUT:           i64 = -110;
const EXO_SHIELD_PID:      u64 = 10;

// ── Global Statistics ───────────────────────────────────────────────────────

static REQUESTS_TOTAL:      AtomicU64 = AtomicU64::new(0);
static REQUESTS_OK:         AtomicU64 = AtomicU64::new(0);
static REQUESTS_ERR:        AtomicU64 = AtomicU64::new(0);
static IPC_RECV_TIMEOUTS:   AtomicU32 = AtomicU32::new(0);
static PERIODIC_SCAN_TICKS: AtomicU64 = AtomicU64::new(0);
static MAINTENANCE_TICKS:   AtomicU64 = AtomicU64::new(0);

// ── Global Tick Counter ─────────────────────────────────────────────────────

static GLOBAL_TICK: AtomicU64 = AtomicU64::new(0);

fn current_tick() -> u64 {
    GLOBAL_TICK.load(Ordering::Relaxed)
}

fn advance_tick() {
    GLOBAL_TICK.fetch_add(1, Ordering::Relaxed);
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
        payload[offset],     payload[offset + 1],
        payload[offset + 2], payload[offset + 3],
        payload[offset + 4], payload[offset + 5],
        payload[offset + 6], payload[offset + 7],
    ])
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
    let priority  = req.payload[5];
    let data_len  = read_u32_le(&req.payload, 6) as usize;
    let data_end  = (10 + data_len).min(120);
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
    if let Some(scan_id) = engine::queue_scan(target_pid, scan_type, priority, tick) {
        engine::stat_scan_queued();
    }

    // Build reply
    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0..4].copy_from_slice(&result.scan_id.to_le_bytes());
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
    let target_pid  = read_u32_le(&req.payload, 0);
    let event_type  = engine::EventType::from_u8(req.payload[4]);
    let opcode      = read_u32_le(&req.payload, 8);
    let arg0        = read_u64_le(&req.payload, 12);
    let arg1        = read_u64_le(&req.payload, 20);
    let severity    = engine::ThreatLevel::from_u8(req.payload[28]);
    let tick        = current_tick();

    if target_pid == 0 {
        return ShieldReply::new(SHIELD_ERR_ARGS);
    }

    let event = engine::MonitoredEvent {
        pid:        target_pid,
        event_type: event_type,
        opcode:     opcode,
        arg0:       arg0,
        arg1:       arg1,
        timestamp:  tick,
        severity:   severity,
    };

    let result = engine::submit_event(&event, tick);

    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0..4].copy_from_slice(&result.alert_id.to_le_bytes());
    reply.data[4] = result.action as u8;
    reply.data[5] = if result.rate_exceeded { 1 } else { 0 };
    reply.data[6] = if result.contained { 1 } else { 0 };

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
    let cmd        = req.payload[0];
    let target_pid = read_u32_le(&req.payload, 1);
    let tick       = current_tick();

    if target_pid == 0 {
        return ShieldReply::new(SHIELD_ERR_ARGS);
    }

    match cmd {
        0 => {
            // Contain process
            let ok = engine::mark_process_contained(target_pid, tick);
            engine::stat_containments_inc();
            let mut reply = ShieldReply::new(if ok { SHIELD_OK } else { SHIELD_ERR_NOT_FOUND });
            reply.data[0] = if ok { 1 } else { 0 };
            reply
        }
        1 => {
            // Release process from containment
            let ok = engine::release_process(target_pid);
            let mut reply = ShieldReply::new(if ok { SHIELD_OK } else { SHIELD_ERR_NOT_CONTAINED });
            reply.data[0] = if ok { 1 } else { 0 };
            reply
        }
        2 => {
            // Query containment status
            let profile = engine::get_risk_profile(target_pid);
            let is_contained = profile.map(|p| if p.contained { 1u8 } else { 0u8 }).unwrap_or(0);
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
    let id_or_pid  = read_u32_le(&req.payload, 1);
    let tick       = current_tick();

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
    let tick = current_tick();

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
                id:           0,
                event_type:   event_type,
                pid:          pid,
                opcode_mask:  opcode_mask,
                min_severity: min_severity,
                action:       action,
                enabled:      true,
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
            let net_limit  = read_u32_le(&req.payload, 9);
            let fs_limit   = read_u32_le(&req.payload, 13);
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
            // [8..24] pattern (16 bytes), [24] pattern_len,
            // [25] name_len, [26..58] name
            let sig_type = req.payload[1];
            let category = engine::ThreatCategory::from_u8(req.payload[2]);
            let severity = engine::ThreatLevel::from_u8(req.payload[3]);
            let base_score = read_u32_le(&req.payload, 4);

            let mut entry = engine::SignatureEntry::empty();
            entry.sig_type = sig_type;
            entry.category = category;
            entry.severity = severity;
            entry.base_score = base_score;

            if req.payload.len() >= 24 {
                entry.pattern.copy_from_slice(&req.payload[8..24]);
            }
            entry.pattern_len = req.payload[24];
            entry.name_len = req.payload[25];
            let name_start = 26;
            let name_len = entry.name_len as usize;
            if name_start + name_len <= req.payload.len() && name_len <= 32 {
                entry.sig_name[..name_len].copy_from_slice(&req.payload[name_start..name_start + name_len]);
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

// ── Dispatch ────────────────────────────────────────────────────────────────

fn handle_request(req: &ShieldRequest) -> ShieldReply {
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    advance_tick();

    let reply = match req.msg_type {
        SCAN_REQUEST   => handle_scan_request(req),
        EVENT_REPORT   => handle_event_report(req),
        QUARANTINE_CMD => handle_quarantine_cmd(req),
        THREAT_QUERY   => handle_threat_query(req),
        POLICY_UPDATE  => handle_policy_update(req),
        HEARTBEAT      => handle_heartbeat(req),
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
            engine::mark_process_contained(scan_req.pid, tick);
            engine::stat_containments_inc();

            // Generate alert
            let desc = if assessment.composite_score >= 750 {
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
    engine::engine_init();
    signatures::signatures_init();
    behavioral::behavioral_init();

    // ── 1. Register with ipc_router as exo_shield (PID 10) ─────────────────
    let name = b"exo_shield";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            EXO_SHIELD_PID, // endpoint_id = PID 10
        )
    };

    // ── 2. Main IPC receive loop ────────────────────────────────────────────
    let mut req = ShieldRequest {
        sender_pid: 0,
        msg_type:   0,
        payload:    [0u8; 120],
    };

    loop {
        let r = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_RECV,
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
                0, 0, 0,
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
