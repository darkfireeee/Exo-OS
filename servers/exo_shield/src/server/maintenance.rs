#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use super::*;

// ── Periodic Maintenance ────────────────────────────────────────────────────

/// Called on IPC timeout to perform periodic maintenance tasks.
pub fn perform_maintenance() {
    MAINTENANCE_TICKS.fetch_add(1, Ordering::Relaxed);
    let tick = current_tick();

    // TIER 3.1 : drainer le feed d'événements de sécurité du kernel et les
    // injecter dans le moteur NGAV (le shield n'est plus aveugle).
    drain_shield_feed();

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

