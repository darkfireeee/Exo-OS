#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use super::*;

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
pub fn handle_scan_request(req: &ShieldRequest) -> ShieldReply {
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
pub fn handle_event_report(req: &ShieldRequest) -> ShieldReply {
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
pub fn handle_quarantine_cmd(req: &ShieldRequest) -> ShieldReply {
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
pub fn handle_threat_query(req: &ShieldRequest) -> ShieldReply {
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
pub fn handle_policy_update(req: &ShieldRequest) -> ShieldReply {
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
pub fn handle_heartbeat(_req: &ShieldRequest) -> ShieldReply {
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
pub fn handle_pmc_anomaly_report(req: &ShieldRequest) -> ShieldReply {
    pub const PMC_ANOMALY_OPCODE: u32 = 0x504D_4300;

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

pub fn handle_request(req: &ShieldRequest) -> ShieldReply {
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

