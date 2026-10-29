//! # syscall/table/ipc/channel.rs — Envoi, réception, RPC et contrôle d'accès IPC natif

extern crate alloc;
use alloc::vec::Vec;

use crate::syscall::table::common::*;
use super::endpoints::*;
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, copy_to_user, UserBuf};

pub(super) const IPC_RECV_TIMEOUT_FLAG: u64 = 0x0001;
pub(super) const IPC_FLAG_INJECT_SRC_PID: u64 = 0x0002;
pub(super) const CRYPTO_SERVER_ENDPOINT_ID: u64 = 4;
pub(super) const CRYPTO_PHOENIX_WAKE_ENTROPY: u32 = 255;

/// Seuil de bascule pile/tas pour `sys_exo_ipc_call`.
const STACK_RPC_BUF_MAX: usize = 512;

#[inline]
pub(super) fn zeroed_user_vec(len: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    out.resize(len, 0);
    Ok(out)
}

#[inline]
pub(super) fn is_reserved_kernel_ipc(endpoint: u64, payload: &[u8]) -> bool {
    if endpoint != CRYPTO_SERVER_ENDPOINT_ID
        || payload.len() < crate::ipc::core::constants::ABI_IPC_HEADER_SIZE
    {
        return false;
    }
    let msg_type = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    msg_type == CRYPTO_PHOENIX_WAKE_ENTROPY
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IpcEnvelopeAuth {
    NotRequired,
    TrustedCaller,
    ValidToken,
}

pub(super) fn validate_ipc_envelope_auth(
    endpoint: u64,
    caller_pid: u32,
    caller_can_inject: bool,
    payload: &[u8],
) -> Result<IpcEnvelopeAuth, i64> {
    use crate::ipc::core::constants::{
        ABI_IPC_ENVELOPE_SIZE, IPC_CAP_TOKEN_OFFSET, IPC_CAP_TOKEN_SIZE,
    };

    // FIX-IPC-AUTH (Security_Audit_Passe2 §A-02) : les messages hors-format
    // reçoivent NotRequired UNIQUEMENT pour les endpoints de réponse éphémères du kernel.
    if is_kernel_ephemeral_reply_endpoint(endpoint) {
        return Ok(IpcEnvelopeAuth::NotRequired);
    }
    if caller_can_inject {
        return Ok(IpcEnvelopeAuth::TrustedCaller);
    }
    // Messages hors-format ABI non éphémères → refus explicite.
    if payload.len() != ABI_IPC_ENVELOPE_SIZE {
        return Err(EACCES);
    }

    let token_end = IPC_CAP_TOKEN_OFFSET + IPC_CAP_TOKEN_SIZE;
    let Some(token_slice) = payload.get(IPC_CAP_TOKEN_OFFSET..token_end) else {
        return Err(EINVAL);
    };
    if IPC_CAP_TOKEN_SIZE != crate::security::CAP_TOKEN_WIRE_SIZE {
        return Err(EINVAL);
    }
    let mut token_bytes = [0u8; crate::security::CAP_TOKEN_WIRE_SIZE];
    token_bytes.copy_from_slice(token_slice);
    let Some(token) = crate::security::CapToken::from_bytes(&token_bytes) else {
        return Err(EACCES);
    };

    let target_pid = exo_ipc_endpoint_pid(endpoint);
    if target_pid == 0 || caller_pid == 0 {
        return Err(EACCES);
    }

    crate::security::capability::check_token_owner(
        token,
        crate::security::Rights::IPC_SEND.bits(),
        caller_pid,
        target_pid,
        crate::security::CapObjectType::IpcEndpoint as u32,
    )
    .map_err(|_| EACCES)?;

    Ok(IpcEnvelopeAuth::ValidToken)
}

#[inline]
pub(super) fn is_kernel_ephemeral_reply_endpoint(endpoint: u64) -> bool {
    endpoint & (1u64 << 63) != 0
}

pub(super) fn ipc_error_to_errno(err: IpcError) -> i64 {
    match err {
        IpcError::WouldBlock
        | IpcError::Retry
        | IpcError::Full
        | IpcError::QueueFull
        | IpcError::QueueEmpty => EAGAIN,
        IpcError::EndpointNotFound | IpcError::NotFound => ENOENT,
        IpcError::PermissionDenied => EACCES,
        IpcError::MessageTooLarge => EMSGSIZE,
        IpcError::Timeout => EAGAIN,
        IpcError::ResourceExhausted | IpcError::ShmPoolFull | IpcError::OutOfResources => ENOMEM,
        IpcError::ConnRefused => ENOENT,
        IpcError::AlreadyConnected => EBUSY,
        IpcError::InvalidParam
        | IpcError::InvalidHandle
        | IpcError::Invalid
        | IpcError::NullEndpoint
        | IpcError::InvalidEndpoint
        | IpcError::InvalidArgument => EINVAL,
        IpcError::Interrupted => EINTR,
        IpcError::ChannelClosed | IpcError::Closed => EBUSY,
        IpcError::HandshakeFailed
        | IpcError::OutOfOrder
        | IpcError::ProtocolError
        | IpcError::MappingFailed
        | IpcError::InternalError
        | IpcError::Internal
        | IpcError::Loop => EINVAL,
    }
}

pub(super) fn enforce_direct_ipc_policy(endpoint: u64) -> Result<(), i64> {
    if is_kernel_ephemeral_reply_endpoint(endpoint) {
        return Ok(());
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return Err(EACCES);
    }

    let dst_pid = exo_ipc_endpoint_pid(endpoint);
    if dst_pid == 0 {
        return Err(EINVAL);
    }

    let verdict = crate::security::check_direct_ipc(
        crate::process::core::pid::Pid(caller_pid),
        crate::process::core::pid::Pid(dst_pid),
    );

    match verdict {
        crate::security::IpcPolicyResult::Allowed => Ok(()),
        crate::security::IpcPolicyResult::Denied
        | crate::security::IpcPolicyResult::UnknownService => {
            crate::security::exoledger::exo_ledger_append(
                crate::security::exoledger::ActionTag::IpcUnauthorized {
                    src_pid: caller_pid,
                    dst_pid,
                },
            );
            // TIER 3.1 : forwarder au feed NGAV exo_shield (Ipc, High).
            crate::security::shield_feed::push_event(
                caller_pid,
                crate::security::shield_feed::event_type::IPC,
                crate::security::shield_feed::severity::HIGH,
                0,
                dst_pid as u64,
                0,
            );
            Err(EACCES)
        }
    }
}

pub(super) fn normalize_ipc_recv_args(a1: u64, a2: u64, a3: u64, flags: u64) -> (u64, u64, u64, u64) {
    let shorthand_len = a2 <= 65_536;
    let user_buf = a1 >= 0x1000
        && a1 < crate::syscall::validation::USER_ADDR_MAX
        && a1
            .checked_add(a2)
            .map(|end| end <= crate::syscall::validation::USER_ADDR_MAX)
            .unwrap_or(false);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let explicit_endpoint = ipc_endpoint_owner_pid(a1).is_some();
    if flags == 0 && shorthand_len && user_buf && !explicit_endpoint {
        let endpoint = primary_ipc_endpoint_for_owner(caller_pid).unwrap_or(caller_pid as u64);
        (endpoint, a1, a2, a3)
    } else {
        (a1, a2, a3, flags)
    }
}

pub(super) fn recv_ipc_message(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, nowait: bool) -> i64 {
    const IPC_RECV_IDLE_NAP_NS: u64 = 2_000_000;

    let len = buf_len as usize;
    if len > 65_536 {
        return E2BIG;
    }
    if buf_ptr == 0 && len != 0 {
        return EFAULT;
    }
    let endpoint_id = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || exo_ipc_endpoint_pid(endpoint) != caller_pid {
        return EACCES;
    }

    let recv_cap = len.min(crate::ipc::core::constants::MAX_MSG_SIZE);
    let mut payload = [0u8; crate::ipc::core::constants::MAX_MSG_SIZE];
    let timeout_requested = !nowait && (flags & IPC_RECV_TIMEOUT_FLAG != 0);
    let raw_timeout_ms = flags & !IPC_RECV_TIMEOUT_FLAG;
    let timeout_ms = if timeout_requested && raw_timeout_ms == 0 {
        1
    } else {
        raw_timeout_ms
    };

    let current_tcb = crate::scheduler::core::switch::current_thread_raw();
    let result = if nowait {
        crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001)
    } else if timeout_requested {
        let deadline = crate::scheduler::timer::clock::monotonic_ns()
            .saturating_add(timeout_ms.saturating_mul(1_000_000));
        loop {
            if !current_tcb.is_null() && unsafe { (*current_tcb).has_signal_pending() } {
                crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                break Err(IpcError::Interrupted);
            }
            match crate::ipc::channel::raw::recv_raw_or_register_waiter(
                endpoint_id,
                &mut payload[..recv_cap],
                current_tcb,
            ) {
                Ok(Some(n)) => break Ok(n),
                Ok(None) => {
                    let now = crate::scheduler::timer::clock::monotonic_ns();
                    if now >= deadline {
                        crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                        break Err(IpcError::Timeout);
                    }
                    let nap_ns = deadline.saturating_sub(now).min(IPC_RECV_IDLE_NAP_NS);
                    if !crate::scheduler::timer::sleep_ns(nap_ns) {
                        crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                        break Err(IpcError::Interrupted);
                    }
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                }
                Err(err) => {
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                    break Err(err);
                }
            }
        }
    } else {
        loop {
            if !current_tcb.is_null() && unsafe { (*current_tcb).has_signal_pending() } {
                crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                break Err(IpcError::Interrupted);
            }
            match crate::ipc::channel::raw::recv_raw_or_register_waiter(
                endpoint_id,
                &mut payload[..recv_cap],
                current_tcb,
            ) {
                Ok(Some(n)) => break Ok(n),
                Ok(None) => {
                    if !crate::scheduler::timer::sleep_ns(IPC_RECV_IDLE_NAP_NS) {
                        crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                        break Err(IpcError::Interrupted);
                    }
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                }
                Err(err) => {
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                    break Err(err);
                }
            }
        }
    };

    match result {
        Ok(n) => {
            if n != 0 && copy_to_user(buf_ptr as *mut u8, payload.as_ptr(), n).is_err() {
                return EFAULT;
            }
            n as i64
        }
        Err(IpcError::Timeout) => crate::syscall::errno::ETIMEDOUT,
        Err(err) => ipc_error_to_errno(err),
    }
}

/// `exo_ipc_send(endpoint, msg_ptr, msg_len, flags)`.
pub fn sys_exo_ipc_send(
    endpoint: u64,
    msg_ptr: u64,
    msg_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_SEND);
    let len = msg_len as usize;
    if len > crate::ipc::core::constants::MAX_MSG_SIZE {
        return E2BIG;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }
    let endpoint_id = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    if let Err(e) = UserBuf::validate(msg_ptr, len, crate::ipc::core::constants::MAX_MSG_SIZE) {
        return e.to_errno();
    }

    // Buffer statique sur la pile (MAX_MSG_SIZE = 240 octets) : 0 allocation SLUB/Heap
    let mut payload_buf = [0u8; crate::ipc::core::constants::MAX_MSG_SIZE];
    let payload = &mut payload_buf[..len];
    if len != 0 {
        if copy_from_user(payload.as_mut_ptr(), msg_ptr as *const u8, len).is_err() {
            return EFAULT;
        }
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let caller_can_inject =
        crate::security::can_inject_src_pid(crate::process::core::pid::Pid(caller_pid));
    let envelope_auth =
        match validate_ipc_envelope_auth(endpoint, caller_pid, caller_can_inject, payload) {
            Ok(auth) => auth,
            Err(errno) => return errno,
        };
    if flags & IPC_FLAG_INJECT_SRC_PID != 0 {
        if len < core::mem::size_of::<u32>() {
            return EINVAL;
        }
        if !caller_can_inject && envelope_auth != IpcEnvelopeAuth::ValidToken {
            return EACCES;
        }
        payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    }
    if !caller_can_inject && payload.len() == crate::ipc::core::constants::ABI_IPC_ENVELOPE_SIZE {
        payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    }
    if is_reserved_kernel_ipc(endpoint, payload) {
        return EACCES;
    }
    let raw_flags = if flags & IPC_RECV_TIMEOUT_FLAG != 0 {
        0x0001
    } else {
        0
    };
    match crate::ipc::channel::raw::send_raw(endpoint_id, payload, raw_flags) {
        Ok(_) => 0,
        Err(err) => ipc_error_to_errno(err),
    }
}

/// `exo_ipc_recv(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv(
    endpoint: u64,
    buf_ptr: u64,
    buf_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV);
    let (endpoint, buf_ptr, buf_len, flags) =
        normalize_ipc_recv_args(endpoint, buf_ptr, buf_len, flags);
    recv_ipc_message(endpoint, buf_ptr, buf_len, flags, false)
}

/// `exo_ipc_recv_nb(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv_nb(
    endpoint: u64,
    buf_ptr: u64,
    buf_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV_NB);
    let (endpoint, buf_ptr, buf_len, flags) =
        normalize_ipc_recv_args(endpoint, buf_ptr, buf_len, flags);
    recv_ipc_message(endpoint, buf_ptr, buf_len, flags, true)
}

/// `exo_ipc_call(endpoint, msg_ptr, msg_len, resp_ptr, resp_len, flags)`.
pub fn sys_exo_ipc_call(
    endpoint: u64,
    msg_ptr: u64,
    msg_len: u64,
    resp_ptr: u64,
    resp_len: u64,
    flags: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_CALL);
    let send_len = msg_len as usize;
    let recv_len = resp_len as usize;
    if send_len > crate::ipc::rpc::MAX_CALL_PAYLOAD || recv_len > crate::ipc::rpc::MAX_CALL_PAYLOAD
    {
        return E2BIG;
    }
    if flags != 0 {
        return EINVAL;
    }
    if send_len != 0 && msg_ptr == 0 {
        return EFAULT;
    }
    if recv_len != 0 && resp_ptr == 0 {
        return EFAULT;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }

    let server_ep = match EndpointId::new(endpoint) {
        Some(ep) => ep,
        None => return EINVAL,
    };

    // Chemin rapide sans allocation sur la pile pour toutes les tailles <= 512 octets
    if send_len <= STACK_RPC_BUF_MAX && recv_len <= STACK_RPC_BUF_MAX {
        let mut req_buf = [0u8; STACK_RPC_BUF_MAX];
        let mut resp_buf = [0u8; STACK_RPC_BUF_MAX];
        if send_len != 0 {
            if copy_from_user(req_buf.as_mut_ptr(), msg_ptr as *const u8, send_len).is_err() {
                return EFAULT;
            }
        }
        return match crate::ipc::rpc::call_raw(
            server_ep,
            &req_buf[..send_len],
            &mut resp_buf[..recv_len],
        ) {
            Ok(reply_len) => {
                if reply_len != 0
                    && copy_to_user(resp_ptr as *mut u8, resp_buf.as_ptr(), reply_len).is_err()
                {
                    return EFAULT;
                }
                reply_len as i64
            }
            Err(err) => ipc_error_to_errno(err),
        };
    }

    let mut request = match zeroed_user_vec(send_len) {
        Ok(request) => request,
        Err(errno) => return errno,
    };
    if send_len != 0 {
        if copy_from_user(request.as_mut_ptr(), msg_ptr as *const u8, send_len).is_err() {
            return EFAULT;
        }
    }

    let mut response = match zeroed_user_vec(recv_len) {
        Ok(response) => response,
        Err(errno) => return errno,
    };

    match crate::ipc::rpc::call_raw(server_ep, &request, &mut response) {
        Ok(reply_len) => {
            if reply_len != 0
                && copy_to_user(resp_ptr as *mut u8, response.as_ptr(), reply_len).is_err()
            {
                return EFAULT;
            }
            reply_len as i64
        }
        Err(err) => ipc_error_to_errno(err),
    }
}
