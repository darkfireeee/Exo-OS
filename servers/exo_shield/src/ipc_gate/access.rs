//! # access — request-level capability classification for exo_shield IPC
//!
//! This module decides when a request can remain on the public self-service
//! path and when it must carry a kernel-verified service capability token.

pub const EXO_SHIELD_CAP_TOKEN_OFFSET: usize = 172;
pub const EXO_SHIELD_CAP_TOKEN_LEN: usize = 20;

const SCAN_REQUEST: u32 = 0;
const EVENT_REPORT: u32 = 1;
const QUARANTINE_CMD: u32 = 2;
const THREAT_QUERY: u32 = 3;
const POLICY_UPDATE: u32 = 4;
const HEARTBEAT: u32 = 5;
const PMC_ANOMALY_REPORT: u32 = 6;
pub const AUDIT_QUERY: u32 = 7;

const SHIELD_ERR_ARGS: u32 = 1;
const SHIELD_ERR_BUSY: u32 = 2;
const SHIELD_ERR_DENIED: u32 = 4;
const SHIELD_ERR_CAP: u32 = 5;

const THREAT_QUERY_BY_ID: u8 = 0;
const THREAT_QUERY_BY_PID: u8 = 1;
const THREAT_QUERY_ASSESS_PID: u8 = 2;
const THREAT_QUERY_STATS: u8 = 3;

const SCAN_HEADER_LEN: usize = 10;
const MAX_SCAN_INLINE_DATA_WITH_CAP: usize = EXO_SHIELD_CAP_TOKEN_OFFSET - SCAN_HEADER_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceCapRequirement {
    NotRequired,
    Required,
    Malformed,
}

#[inline(always)]
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

/// Decides whether the request can stay on the public self-service path or
/// must carry a kernel-verified `IPC_SEND` capability targeting exo_shield.
///
/// FIX-06: Removed blind `sender_pid <= 2` bypass. Only Ring 0 kernel
/// telemetry (`sender_pid == 0`, which cannot be forged from userspace after
/// FIX-01) may submit `PMC_ANOMALY_REPORT` without a user capability token.
/// All Ring 1/Ring 3 callers (including PID 1 & PID 2) MUST present a valid
/// kernel-verified `ExoCapTokenWire` for privileged operations.
pub fn classify_service_cap_requirement(
    sender_pid: u32,
    msg_type: u32,
    payload: &[u8],
) -> ServiceCapRequirement {
    if sender_pid == 0 && msg_type == PMC_ANOMALY_REPORT {
        return ServiceCapRequirement::NotRequired;
    }

    match msg_type {
        QUARANTINE_CMD | POLICY_UPDATE | PMC_ANOMALY_REPORT | AUDIT_QUERY => {
            ServiceCapRequirement::Required
        }
        HEARTBEAT => ServiceCapRequirement::NotRequired,
        SCAN_REQUEST => {
            let target_pid = read_u32_le(payload, 0);
            if target_pid == 0 || target_pid == sender_pid {
                return ServiceCapRequirement::NotRequired;
            }

            let inline_len = read_u32_le(payload, 6) as usize;
            if inline_len > MAX_SCAN_INLINE_DATA_WITH_CAP {
                ServiceCapRequirement::Malformed
            } else {
                ServiceCapRequirement::Required
            }
        }
        EVENT_REPORT => {
            let target_pid = read_u32_le(payload, 0);
            if target_pid != 0 && target_pid != sender_pid {
                ServiceCapRequirement::Required
            } else {
                ServiceCapRequirement::NotRequired
            }
        }
        THREAT_QUERY => {
            let query_type = payload.first().copied().unwrap_or(0xff);
            let target_pid = read_u32_le(payload, 1);
            match query_type {
                THREAT_QUERY_BY_ID | THREAT_QUERY_STATS => ServiceCapRequirement::Required,
                THREAT_QUERY_BY_PID | THREAT_QUERY_ASSESS_PID => {
                    if target_pid != 0 && target_pid != sender_pid {
                        ServiceCapRequirement::Required
                    } else {
                        ServiceCapRequirement::NotRequired
                    }
                }
                _ => ServiceCapRequirement::Malformed,
            }
        }
        _ => ServiceCapRequirement::Required,
    }
}

/// Decision returned by `authorize_ipc_request` for audit + status dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateDecision {
    pub action: u8,
    pub rule_id: u32,
    pub audit_result: u8,
    pub err_status: Option<u32>,
}

/// FIX-06: Canonical authorization order for `exo_shield` IPC requests.
///
/// 1. Classify capability requirement (`Malformed` -> `SHIELD_ERR_ARGS`).
/// 2. If `Required`: verify `ExoCapTokenWire` at `payload[172..192]` BEFORE
///    wildcard policy rules so a legitimate capability holder (`sender_pid = 25`)
///    is not prematurely rejected by `WILDCARD_PID -> Deny`, while any caller
///    without a valid token (including `sender_pid <= 2`) is rejected with
///    `SHIELD_ERR_CAP (5)`. An explicit per-PID Deny rule still overrides.
/// 3. If `NotRequired`: enforce `ipc_gate::evaluate_policy` (rate-limit / deny).
pub fn authorize_ipc_request<F>(
    sender_pid: u32,
    msg_type: u32,
    payload: &[u8],
    cap_verifier: F,
) -> GateDecision
where
    F: FnOnce(&[u8; EXO_SHIELD_CAP_TOKEN_LEN]) -> bool,
{
    use super::policy::{self, PolicyAction, EXO_SHIELD_PID};

    match classify_service_cap_requirement(sender_pid, msg_type, payload) {
        ServiceCapRequirement::Malformed => GateDecision {
            action: PolicyAction::Deny as u8,
            rule_id: 0,
            audit_result: 1,
            err_status: Some(SHIELD_ERR_ARGS),
        },
        ServiceCapRequirement::Required => {
            if let Some(ban_rule) = policy::explicit_pid_deny_rule(sender_pid, EXO_SHIELD_PID, msg_type)
            {
                return GateDecision {
                    action: PolicyAction::Deny as u8,
                    rule_id: ban_rule,
                    audit_result: 1,
                    err_status: Some(SHIELD_ERR_DENIED),
                };
            }
            if payload.len() < EXO_SHIELD_CAP_TOKEN_OFFSET + EXO_SHIELD_CAP_TOKEN_LEN {
                return GateDecision {
                    action: PolicyAction::Deny as u8,
                    rule_id: 0,
                    audit_result: 1,
                    err_status: Some(SHIELD_ERR_CAP),
                };
            }
            let mut raw = [0u8; EXO_SHIELD_CAP_TOKEN_LEN];
            raw.copy_from_slice(
                &payload[EXO_SHIELD_CAP_TOKEN_OFFSET
                    ..EXO_SHIELD_CAP_TOKEN_OFFSET + EXO_SHIELD_CAP_TOKEN_LEN],
            );
            let non_empty = raw.iter().any(|&b| b != 0);
            if !non_empty || !cap_verifier(&raw) {
                return GateDecision {
                    action: PolicyAction::Deny as u8,
                    rule_id: 0,
                    audit_result: 1,
                    err_status: Some(SHIELD_ERR_CAP),
                };
            }
            GateDecision {
                action: PolicyAction::Allow as u8,
                rule_id: 0,
                audit_result: 0,
                err_status: None,
            }
        }
        ServiceCapRequirement::NotRequired => {
            let eval = policy::evaluate_policy(sender_pid, EXO_SHIELD_PID, msg_type);
            let action = PolicyAction::from_u8(eval.action);
            if eval.rate_limit > 0 && action == PolicyAction::Deny {
                return GateDecision {
                    action: PolicyAction::Deny as u8,
                    rule_id: eval.matched_rule_id,
                    audit_result: 2,
                    err_status: Some(SHIELD_ERR_BUSY),
                };
            }
            if action == PolicyAction::Deny {
                return GateDecision {
                    action: PolicyAction::Deny as u8,
                    rule_id: eval.matched_rule_id,
                    audit_result: 1,
                    err_status: Some(SHIELD_ERR_DENIED),
                };
            }
            GateDecision {
                action: eval.action,
                rule_id: eval.matched_rule_id,
                audit_result: 0,
                err_status: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_scan_stays_public() {
        let mut payload = [0u8; 192];
        payload[0..4].copy_from_slice(&42u32.to_le_bytes());
        payload[6..10].copy_from_slice(&32u32.to_le_bytes());

        assert_eq!(
            classify_service_cap_requirement(42, SCAN_REQUEST, &payload),
            ServiceCapRequirement::NotRequired
        );
    }

    #[test]
    fn cross_process_scan_requires_cap_with_tail_budget() {
        let mut payload = [0u8; 192];
        payload[0..4].copy_from_slice(&7u32.to_le_bytes());
        payload[6..10].copy_from_slice(&(MAX_SCAN_INLINE_DATA_WITH_CAP as u32).to_le_bytes());

        assert_eq!(
            classify_service_cap_requirement(42, SCAN_REQUEST, &payload),
            ServiceCapRequirement::Required
        );

        payload[6..10].copy_from_slice(&((MAX_SCAN_INLINE_DATA_WITH_CAP + 1) as u32).to_le_bytes());
        assert_eq!(
            classify_service_cap_requirement(42, SCAN_REQUEST, &payload),
            ServiceCapRequirement::Malformed
        );
    }

    #[test]
    fn cross_process_event_report_requires_cap() {
        let mut payload = [0u8; 192];
        payload[0..4].copy_from_slice(&7u32.to_le_bytes());

        assert_eq!(
            classify_service_cap_requirement(42, EVENT_REPORT, &payload),
            ServiceCapRequirement::Required
        );

        payload[0..4].copy_from_slice(&42u32.to_le_bytes());
        assert_eq!(
            classify_service_cap_requirement(42, EVENT_REPORT, &payload),
            ServiceCapRequirement::NotRequired
        );
    }

    #[test]
    fn detailed_threat_queries_require_cap() {
        let mut payload = [0u8; 192];

        payload[0] = THREAT_QUERY_BY_ID;
        assert_eq!(
            classify_service_cap_requirement(42, THREAT_QUERY, &payload),
            ServiceCapRequirement::Required
        );

        payload[0] = THREAT_QUERY_BY_PID;
        payload[1..5].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(
            classify_service_cap_requirement(42, THREAT_QUERY, &payload),
            ServiceCapRequirement::Required
        );

        payload[1..5].copy_from_slice(&42u32.to_le_bytes());
        assert_eq!(
            classify_service_cap_requirement(42, THREAT_QUERY, &payload),
            ServiceCapRequirement::NotRequired
        );

        payload[0] = THREAT_QUERY_STATS;
        assert_eq!(
            classify_service_cap_requirement(42, THREAT_QUERY, &payload),
            ServiceCapRequirement::Required
        );

        // Unknown threat query type is Malformed
        payload[0] = 0xee;
        assert_eq!(
            classify_service_cap_requirement(42, THREAT_QUERY, &payload),
            ServiceCapRequirement::Malformed
        );
    }

    #[test]
    fn administrative_requests_always_require_cap() {
        let payload = [0u8; 192];

        assert_eq!(
            classify_service_cap_requirement(42, QUARANTINE_CMD, &payload),
            ServiceCapRequirement::Required
        );
        assert_eq!(
            classify_service_cap_requirement(42, POLICY_UPDATE, &payload),
            ServiceCapRequirement::Required
        );
        assert_eq!(
            classify_service_cap_requirement(42, PMC_ANOMALY_REPORT, &payload),
            ServiceCapRequirement::Required
        );
        assert_eq!(
            classify_service_cap_requirement(42, HEARTBEAT, &payload),
            ServiceCapRequirement::NotRequired
        );
        assert_eq!(
            classify_service_cap_requirement(0, PMC_ANOMALY_REPORT, &payload),
            ServiceCapRequirement::NotRequired
        );
        // FIX-06: Ring 1 callers (PID 1 / PID 2) MUST carry a valid capability token.
        for ring1_pid in 1..=2u32 {
            assert_eq!(
                classify_service_cap_requirement(ring1_pid, QUARANTINE_CMD, &payload),
                ServiceCapRequirement::Required
            );
            assert_eq!(
                classify_service_cap_requirement(ring1_pid, PMC_ANOMALY_REPORT, &payload),
                ServiceCapRequirement::Required
            );
        }
        assert_eq!(
            classify_service_cap_requirement(42, AUDIT_QUERY, &payload),
            ServiceCapRequirement::Required
        );
        // Unknown message type fails closed (Required)
        assert_eq!(
            classify_service_cap_requirement(42, 9999, &payload),
            ServiceCapRequirement::Required
        );
    }
}
