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
    // FIX-06: Delegate to canonical capability-first authorization in ipc_gate::access.
    let decision = ipc_gate::authorize_ipc_request(
        req.sender_pid,
        req.msg_type,
        &req.payload,
        |raw_bytes| {
            let token = syscall::ExoCapTokenWire { bytes: *raw_bytes };
            // exo_cap_check exige target_pid == PID de l'appelant (nous-mêmes) :
            // on passe donc notre PID runtime, pas une constante de convention.
            !token.is_empty()
                && unsafe {
                    syscall::exo_cap_check(
                        &token,
                        syscall::EXO_CAP_RIGHT_IPC_SEND,
                        runtime_pid(),
                        syscall::EXO_CAP_TYPE_IPC_ENDPOINT,
                    )
                } == 0
        },
    );

    record_request_audit(
        req,
        decision.action,
        decision.rule_id,
        decision.audit_result,
    );

    if let Some(err_status) = decision.err_status {
        return Err(ShieldReply::new(err_status));
    }
    Ok(())
}

