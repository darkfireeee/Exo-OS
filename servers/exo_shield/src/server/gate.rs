#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{
    behavioral, engine, forensics, hooks,
    ipc_gate::{self, AuditEntry, PolicyAction, ServiceCapRequirement},
    ml, network, sandbox, signatures,
};
use exo_syscall_abi as syscall;
use super::*;

pub fn extract_service_cap_token(req: &ShieldRequest) -> syscall::ExoCapTokenWire {
    let mut token = syscall::ExoCapTokenWire::empty();
    token.bytes.copy_from_slice(
        &req.payload[ipc_gate::EXO_SHIELD_CAP_TOKEN_OFFSET
            ..ipc_gate::EXO_SHIELD_CAP_TOKEN_OFFSET + ipc_gate::EXO_SHIELD_CAP_TOKEN_LEN],
    );
    token
}

pub fn record_request_audit(req: &ShieldRequest, action: u8, rule_id: u32, result: u8) {
    ipc_gate::record_audit(AuditEntry {
        src_pid: req.sender_pid,
        dst_pid: EXO_SHIELD_POLICY_ID,
        msg_type: req.msg_type,
        action,
        rule_id,
        reply_nonce: 0,
        result,
        _pad: [0; 2],
        timestamp: read_tsc(),
    });
}

pub fn authorize_request(req: &ShieldRequest) -> Result<(), ShieldReply> {
    let eval = ipc_gate::evaluate_policy(req.sender_pid, EXO_SHIELD_POLICY_ID, req.msg_type);
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
            // exo_cap_check exige target_pid == PID de l'appelant (nous-mêmes) :
            // on passe donc notre PID runtime, pas une constante de convention.
            let cap_ok = !token.is_empty()
                && unsafe {
                    syscall::exo_cap_check(
                        &token,
                        syscall::EXO_CAP_RIGHT_IPC_SEND,
                        runtime_pid(),
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

