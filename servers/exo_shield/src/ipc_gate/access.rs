//! # access — request-level capability classification for exo_shield IPC
//!
//! This module decides when a request can remain on the public self-service
//! path and when it must carry a kernel-verified service capability token.

pub const EXO_SHIELD_CAP_TOKEN_OFFSET: usize = 100;
pub const EXO_SHIELD_CAP_TOKEN_LEN: usize = 20;

const SCAN_REQUEST: u32 = 0;
const EVENT_REPORT: u32 = 1;
const QUARANTINE_CMD: u32 = 2;
const THREAT_QUERY: u32 = 3;
const POLICY_UPDATE: u32 = 4;

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
pub fn classify_service_cap_requirement(
    sender_pid: u32,
    msg_type: u32,
    payload: &[u8],
) -> ServiceCapRequirement {
    match msg_type {
        QUARANTINE_CMD | POLICY_UPDATE => ServiceCapRequirement::Required,
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
                _ => ServiceCapRequirement::NotRequired,
            }
        }
        _ => ServiceCapRequirement::NotRequired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_scan_stays_public() {
        let mut payload = [0u8; 120];
        payload[0..4].copy_from_slice(&42u32.to_le_bytes());
        payload[6..10].copy_from_slice(&32u32.to_le_bytes());

        assert_eq!(
            classify_service_cap_requirement(42, SCAN_REQUEST, &payload),
            ServiceCapRequirement::NotRequired
        );
    }

    #[test]
    fn cross_process_scan_requires_cap_with_tail_budget() {
        let mut payload = [0u8; 120];
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
        let mut payload = [0u8; 120];
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
        let mut payload = [0u8; 120];

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
    }

    #[test]
    fn administrative_requests_always_require_cap() {
        let payload = [0u8; 120];

        assert_eq!(
            classify_service_cap_requirement(42, QUARANTINE_CMD, &payload),
            ServiceCapRequirement::Required
        );
        assert_eq!(
            classify_service_cap_requirement(42, POLICY_UPDATE, &payload),
            ServiceCapRequirement::Required
        );
    }
}
