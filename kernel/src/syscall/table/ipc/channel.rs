//! # syscall/table/ipc/channel.rs — Sending, receiving, RPC, and native IPC access control

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

/// Stack/heap threshold switch for `sys_exo_ipc_call`.
const STACK_RPC_BUF_MAX: usize = 512;
const IPC_RECV_IDLE_NAP_NS: u64 = 50_000_000;

#[inline]
pub(super) fn zeroed_user_vec(len: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    out.resize(len, 0);
    Ok(out)
}

#[inline]
pub(super) fn is_reserved_kernel_ipc(endpoint: u64, payload: &[u8]) -> bool {
    if payload.len() < crate::ipc::core::constants::ABI_IPC_HEADER_SIZE {
        return false;
    }
    // FIX-02: cover both static endpoint 4 and any PID-encoded CryptoServer endpoint.
    let dst_pid = exo_ipc_endpoint_pid(endpoint);
    let is_crypto_ep = endpoint == CRYPTO_SERVER_ENDPOINT_ID
        || crate::security::service_class_of(crate::process::core::pid::Pid(dst_pid))
            == crate::security::ServiceClass::CryptoServer;
    if !is_crypto_ep {
        return false;
    }
    let msg_type = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    msg_type == CRYPTO_PHOENIX_WAKE_ENTROPY
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    // FIX-02: kernel ephemeral reply endpoints require an active registered reply capability
    // bound to the responding server PID (never a blanket bypass).
    if is_kernel_ephemeral_reply_endpoint(endpoint) {
        if crate::ipc::rpc::raw::is_ephemeral_reply_authorized(endpoint, caller_pid) {
            return Ok(IpcEnvelopeAuth::NotRequired);
        }
        return Err(EACCES);
    }
    let caller_class = crate::security::service_class_of(crate::process::core::pid::Pid(caller_pid));
    if caller_can_inject {
        return Ok(IpcEnvelopeAuth::TrustedCaller);
    }
    // FIX-03: Remove blanket `ServiceClass::Exosh => TrustedCaller` bypass.
    // Exosh only gets untokenized typed IPC to its non-critical interactive/console/broker
    // endpoints; critical services (CryptoServer, ExoShield, MemoryServer, DeviceServer,
    // InitServer, SchedulerServer) MUST present a valid ExoCapTokenWire and obey POLICY.
    if caller_class == crate::security::ServiceClass::Exosh {
        let dst_pid = exo_ipc_endpoint_pid(endpoint);
        let dst_class = crate::security::service_class_of(crate::process::core::pid::Pid(dst_pid));
        if matches!(
            dst_class,
            crate::security::ServiceClass::IpcBroker
                | crate::security::ServiceClass::InputServer
                | crate::security::ServiceClass::TtyServer
                | crate::security::ServiceClass::FbServer
                | crate::security::ServiceClass::AudioServer
                | crate::security::ServiceClass::ExoCompositor
        ) {
            return Ok(IpcEnvelopeAuth::TrustedCaller);
        }
    }
    // S5B: Allow isolated Strate 2 `exo_compositor` to send typed SurfaceGrant/DamageRect/Power IPC
    // strictly to `fb_server`, `input_server`, `memory_server`, or `init_server` (enforced by ExoCordon POLICY).
    if caller_class == crate::security::ServiceClass::ExoCompositor {
        let dst_pid = exo_ipc_endpoint_pid(endpoint);
        let dst_class = crate::security::service_class_of(crate::process::core::pid::Pid(dst_pid));
        if matches!(
            dst_class,
            crate::security::ServiceClass::FbServer
                | crate::security::ServiceClass::InputServer
                | crate::security::ServiceClass::MemoryServer
                | crate::security::ServiceClass::InitServer
        ) {
            return Ok(IpcEnvelopeAuth::TrustedCaller);
        }
    }
    // Non-ephemeral ABI non-format messages -> explicit rejection + audit.
    if payload.len() != ABI_IPC_ENVELOPE_SIZE {
        let dst_pid = exo_ipc_endpoint_pid(endpoint);
        crate::security::shield_feed::push_event(
            caller_pid,
            crate::security::shield_feed::event_type::IPC,
            crate::security::shield_feed::severity::HIGH,
            0xC049,
            dst_pid as u64,
            payload.len() as u64,
        );
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
        let dst_pid = exo_ipc_endpoint_pid(endpoint);
        crate::security::shield_feed::push_event(
            caller_pid,
            crate::security::shield_feed::event_type::IPC,
            crate::security::shield_feed::severity::HIGH,
            0xC04A,
            dst_pid as u64,
            0,
        );
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
    .map_err(|err| {
        crate::security::shield_feed::push_event(
            caller_pid,
            crate::security::shield_feed::event_type::CAPABILITY,
            crate::security::shield_feed::severity::HIGH,
            0xC04B,
            target_pid as u64,
            err.to_kernel_errno() as u64,
        );
        if err == crate::security::capability::KernelCapError::Expired {
            crate::syscall::errno::ETIMEDOUT
        } else {
            EACCES
        }
    })?;

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
        IpcError::Timeout => crate::syscall::errno::ETIMEDOUT,
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
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if is_kernel_ephemeral_reply_endpoint(endpoint) {
        // FIX-02: only allow bit-63 ephemeral reply sends backed by an active
        // reply capability bound to the responding server PID.
        if caller_pid != 0
            && crate::ipc::rpc::raw::is_ephemeral_reply_authorized(endpoint, caller_pid)
        {
            return Ok(());
        }
        crate::security::exoledger::exo_ledger_append(
            crate::security::exoledger::ActionTag::IpcUnauthorized {
                src_pid: caller_pid,
                dst_pid: 0,
            },
        );
        crate::security::shield_feed::push_event(
            caller_pid,
            crate::security::shield_feed::event_type::IPC,
            crate::security::shield_feed::severity::HIGH,
            0xC063,
            endpoint,
            0,
        );
        return Err(EACCES);
    }

    if caller_pid == 0 {
        return Err(EACCES);
    }

    let dst_pid = exo_ipc_endpoint_pid(endpoint);
    if dst_pid == 0 {
        return Err(EINVAL);
    }

    // S2.3: Block confined compat processes (`NO_RING1_CRITICAL_IPC`) from sending
    // IPC messages directly to critical Ring 1 services.
    if (crate::security::zero_trust::process_restrictions(caller_pid)
        & crate::security::zero_trust::restriction_flags::NO_RING1_CRITICAL_IPC)
        != 0
    {
        let dst_class = crate::security::service_class_of(crate::process::core::pid::Pid(dst_pid));
        let is_critical_dst = matches!(
            dst_class,
            crate::security::ServiceClass::InitServer
                | crate::security::ServiceClass::CryptoServer
                | crate::security::ServiceClass::MemoryServer
                | crate::security::ServiceClass::DeviceServer
                | crate::security::ServiceClass::ExoShield
        );
        if is_critical_dst {
            crate::security::exoledger::exo_ledger_append(
                crate::security::exoledger::ActionTag::IpcUnauthorized {
                    src_pid: caller_pid,
                    dst_pid,
                },
            );
            crate::security::shield_feed::push_event(
                caller_pid,
                crate::security::shield_feed::event_type::IPC,
                crate::security::shield_feed::severity::HIGH,
                0xC048,
                dst_pid as u64,
                0,
            );
            return Err(EACCES);
        }
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
                    let slept = crate::scheduler::timer::sleep_ns_if(nap_ns, || {
                        crate::ipc::channel::raw::has_registered_waiter(endpoint_id, current_tcb)
                    });
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                    if !slept {
                        break Err(IpcError::Interrupted);
                    }
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
                    let slept = crate::scheduler::timer::sleep_ns_if(IPC_RECV_IDLE_NAP_NS, || {
                        crate::ipc::channel::raw::has_registered_waiter(endpoint_id, current_tcb)
                    });
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                    if !slept {
                        break Err(IpcError::Interrupted);
                    }
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

/// Validates envelope capability auth, applies anti-spoofing PID stamping, and
/// rejects kernel-reserved IPC opcodes for both `send` and `call`.
fn authorize_and_stamp_ipc_send(endpoint: u64, payload: &mut [u8], flags: u64) -> Result<(), i64> {
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let caller_can_inject =
        crate::security::can_inject_src_pid(crate::process::core::pid::Pid(caller_pid));
    let envelope_auth = validate_ipc_envelope_auth(endpoint, caller_pid, caller_can_inject, payload)?;
    if flags & IPC_FLAG_INJECT_SRC_PID != 0 {
        if payload.len() < core::mem::size_of::<u32>() {
            return Err(EINVAL);
        }
        if !caller_can_inject && envelope_auth != IpcEnvelopeAuth::ValidToken {
            return Err(EACCES);
        }
        payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    } else if !caller_can_inject
        && !is_kernel_ephemeral_reply_endpoint(endpoint)
        && payload.len() >= core::mem::size_of::<u32>()
    {
        payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    } else if caller_can_inject
        && !is_kernel_ephemeral_reply_endpoint(endpoint)
        && (endpoint >> 32) == 0
        && payload.len() >= core::mem::size_of::<u32>()
    {
        if crate::security::service_class_of(crate::process::core::pid::Pid(caller_pid))
            == crate::security::ServiceClass::IpcBroker
        {
            // FIX-01 (defense-in-depth): IpcBroker forwarding to a server endpoint
            // must never deliver a forged `sender_pid == 0` (kernel PID) or dead PID.
            let claimed_pid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            if claimed_pid == 0 {
                return Err(EACCES);
            }
            #[cfg(not(test))]
            if !crate::process::is_alive(claimed_pid) {
                return Err(EACCES);
            }
        } else if is_registered_service_request_mailbox(endpoint, caller_pid) {
            // FIX-01 / FIX-13: Non-broker Ring 1 services sending requests to a
            // registered Ring 1 server mailbox ALWAYS have `sender_pid` kernel-stamped
            // (both preventing Ring 1->Ring 1 spoofing and populating `sender_pid: 0`
            // requests from ps2_driver, tty_server, usb_hid, exo_shield, etc., without
            // corrupting `Reply.status` on replies to PID-encoded/ephemeral/client targets).
            payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
        }
    }
    if is_reserved_kernel_ipc(endpoint, payload) {
        return Err(EACCES);
    }
    Ok(())
}

/// Validates and stamps the outgoing RPC request, invokes synchronous `call_raw`,
/// and copies any non-empty reply back to userspace.
fn dispatch_ipc_call(
    endpoint: u64,
    server_ep: EndpointId,
    request: &mut [u8],
    response: &mut [u8],
    resp_ptr: u64,
) -> i64 {
    if let Err(errno) = authorize_and_stamp_ipc_send(endpoint, request, 0) {
        return errno;
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let server_pid = exo_ipc_endpoint_pid(endpoint);
    match crate::ipc::rpc::raw::call_raw_for_pids(
        server_ep,
        caller_pid,
        server_pid,
        request,
        response,
    ) {
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

    // Static buffer on the stack (MAX_MSG_SIZE = 240 bytes): 0 SLUB/Heap allocation
    let mut payload_buf = [0u8; crate::ipc::core::constants::MAX_MSG_SIZE];
    let payload = &mut payload_buf[..len];
    if len != 0 {
        if copy_from_user(payload.as_mut_ptr(), msg_ptr as *const u8, len).is_err() {
            return EFAULT;
        }
    }

    if let Err(errno) = authorize_and_stamp_ipc_send(endpoint, payload, flags) {
        return errno;
    }

    // FIX-02: Enforce single-use ephemeral reply capability consumption and forbid
    // unbounded auto-open of RAW_TABLE slots from userspace SYS_EXO_IPC_SEND.
    let is_ephemeral = is_kernel_ephemeral_reply_endpoint(endpoint);
    let allow_auto_open = if is_ephemeral {
        let caller_pid = crate::syscall::fast_path::syscall_current_pid();
        if !crate::ipc::rpc::raw::consume_ephemeral_reply_cap(endpoint, caller_pid) {
            return EACCES;
        }
        false
    } else if crate::ipc::channel::raw::has_mailbox(endpoint_id) {
        false
    } else {
        let packed_pid = (endpoint >> 32) as u32;
        packed_pid != 0
            && crate::process::is_alive(packed_pid)
            && crate::ipc::channel::raw::count_mailboxes_for_pid(packed_pid) < 8
    };

    let do_send = |send_flags: u32| {
        if allow_auto_open {
            crate::ipc::channel::raw::send_raw(endpoint_id, payload, send_flags)
        } else {
            crate::ipc::channel::raw::send_raw_no_auto_open(endpoint_id, payload, send_flags)
        }
    };

    let nowait = (flags & !(IPC_RECV_TIMEOUT_FLAG | IPC_FLAG_INJECT_SRC_PID)) == 0
        && (flags & IPC_RECV_TIMEOUT_FLAG != 0);
    if nowait {
        return match do_send(0x0001) {
            Ok(_) => 0,
            Err(err) => ipc_error_to_errno(err),
        };
    }
    let timeout_ms = if flags & IPC_RECV_TIMEOUT_FLAG != 0 {
        flags & !(IPC_RECV_TIMEOUT_FLAG | IPC_FLAG_INJECT_SRC_PID)
    } else {
        0
    };
    if timeout_ms > 0 {
        let deadline = crate::scheduler::timer::clock::monotonic_ns()
            .saturating_add(timeout_ms.saturating_mul(1_000_000));
        loop {
            match do_send(0x0001) {
                Ok(_) => return 0,
                Err(
                    IpcError::Full
                    | IpcError::QueueFull
                    | IpcError::WouldBlock
                    | IpcError::Retry,
                ) => {
                    let now = crate::scheduler::timer::clock::monotonic_ns();
                    if now >= deadline {
                        return ipc_error_to_errno(IpcError::Timeout);
                    }
                    let nap_ns = deadline.saturating_sub(now).min(IPC_RECV_IDLE_NAP_NS);
                    if !crate::scheduler::timer::sleep_ns(nap_ns) {
                        return ipc_error_to_errno(IpcError::Interrupted);
                    }
                }
                Err(err) => return ipc_error_to_errno(err),
            }
        }
    }
    match do_send(0) {
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
    if send_len != 0 {
        if let Err(e) =
            UserBuf::validate(msg_ptr, send_len, crate::ipc::core::constants::MAX_MSG_SIZE)
        {
            return e.to_errno();
        }
    }
    if recv_len != 0 {
        if let Err(e) =
            UserBuf::validate(resp_ptr, recv_len, crate::ipc::core::constants::MAX_MSG_SIZE)
        {
            return e.to_errno();
        }
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }

    let server_ep = match EndpointId::new(endpoint) {
        Some(ep) => ep,
        None => return EINVAL,
    };

    // Fast path without allocation on the stack for all sizes <= 512 bytes
    if send_len <= STACK_RPC_BUF_MAX && recv_len <= STACK_RPC_BUF_MAX {
        let mut req_buf = [0u8; STACK_RPC_BUF_MAX];
        let mut resp_buf = [0u8; STACK_RPC_BUF_MAX];
        if send_len != 0 {
            if copy_from_user(req_buf.as_mut_ptr(), msg_ptr as *const u8, send_len).is_err() {
                return EFAULT;
            }
        }
        return dispatch_ipc_call(
            endpoint,
            server_ep,
            &mut req_buf[..send_len],
            &mut resp_buf[..recv_len],
            resp_ptr,
        );
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

    dispatch_ipc_call(endpoint, server_ep, &mut request, &mut response, resp_ptr)
}

#[cfg(test)]
mod p0_security_tests {
    use super::*;
    use crate::process::core::pid::Pid;
    use crate::security::{register_service_class, unregister_service, ServiceClass};

    #[test]
    fn fix02_reserved_phoenix_wake_blocked_on_pid_encoded_crypto_endpoint() {
        let crypto_pid = 84u32;
        assert!(register_service_class(Pid(crypto_pid), ServiceClass::CryptoServer));
        let encoded_ep = ((crypto_pid as u64) << 32) | 0x1;
        let mut payload = [0u8; 16];
        payload[4..8].copy_from_slice(&CRYPTO_PHOENIX_WAKE_ENTROPY.to_le_bytes());
        assert!(is_reserved_kernel_ipc(CRYPTO_SERVER_ENDPOINT_ID, &payload));
        assert!(is_reserved_kernel_ipc(encoded_ep, &payload));
        let _ = unregister_service(Pid(crypto_pid));
    }

    #[test]
    fn fix02_unregistered_bit63_ephemeral_endpoint_denied_in_envelope_auth() {
        let bogus_bit63 = (1u64 << 63) | 0xDEAD_BEEF;
        let payload = [0u8; 16];
        assert_eq!(
            validate_ipc_envelope_auth(bogus_bit63, 25, false, &payload),
            Err(EACCES)
        );
    }

    #[test]
    fn fix03_exosh_cannot_bypass_token_for_crypto_or_critical_servers() {
        let exosh_pid = 85u32;
        let crypto_pid = 86u32;
        let fb_pid = 87u32;
        assert!(register_service_class(Pid(exosh_pid), ServiceClass::Exosh));
        assert!(register_service_class(Pid(crypto_pid), ServiceClass::CryptoServer));
        assert!(register_service_class(Pid(fb_pid), ServiceClass::FbServer));

        let payload = [0u8; 32];
        // Exosh -> CryptoServer without valid ABI envelope token MUST be denied.
        assert_eq!(
            validate_ipc_envelope_auth(((crypto_pid as u64) << 32) | 1, exosh_pid, false, &payload),
            Err(EACCES)
        );
        // Exosh -> FbServer remains allowed as interactive console client.
        assert!(matches!(
            validate_ipc_envelope_auth(((fb_pid as u64) << 32) | 1, exosh_pid, false, &payload),
            Ok(IpcEnvelopeAuth::TrustedCaller)
        ));

        let _ = unregister_service(Pid(exosh_pid));
        let _ = unregister_service(Pid(crypto_pid));
        let _ = unregister_service(Pid(fb_pid));
    }

    #[test]
    fn fix01_ring1_service_mailbox_detection_matches_servers_not_replies() {
        let ps2_pid = 91u32;
        let input_pid = 92u32;
        let ring3_pid = 95u32;
        let input_ep = 111u64;
        assert!(register_service_class(Pid(ps2_pid), ServiceClass::Ps2Driver));
        assert!(register_service_class(Pid(input_pid), ServiceClass::InputServer));
        assert!(reserve_ipc_endpoint_owner(input_ep, input_pid, true).is_ok());

        // Sending from ps2_driver -> registered InputServer mailbox MUST stamp sender_pid.
        assert!(is_registered_service_request_mailbox(input_ep, ps2_pid));
        // Replying to PID-encoded endpoint or unregistered Ring 3 client PID MUST NOT stamp.
        assert!(!is_registered_service_request_mailbox(((ps2_pid as u64) << 32) | 1, input_pid));
        assert!(!is_registered_service_request_mailbox(ring3_pid as u64, input_pid));
        // Sending to own mailbox is not a cross-service request.
        assert!(!is_registered_service_request_mailbox(input_ep, input_pid));

        let _ = release_ipc_endpoint_owner(input_ep, input_pid);
        let _ = unregister_service(Pid(ps2_pid));
        let _ = unregister_service(Pid(input_pid));
    }
}
