//! # syscall/table/ipc.rs — Handlers IPC natifs, Capabilities, ExoCordon, ExoShield et ExoFS natif

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use super::common::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, copy_to_user, UserBuf};

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;

// ─────────────────────────────────────────────────────────────────────────────
// Handlers IPC natifs Exo-OS (bloc 300+)
// ─────────────────────────────────────────────────────────────────────────────

const IPC_ENDPOINT_OWNER_SLOTS: usize = 128;

#[derive(Clone, Copy)]
struct IpcEndpointOwner {
    endpoint: u64,
    owner_pid: u32,
}

impl IpcEndpointOwner {
    const EMPTY: Self = Self {
        endpoint: 0,
        owner_pid: 0,
    };
}

static IPC_ENDPOINT_OWNERS: spin::Mutex<[IpcEndpointOwner; IPC_ENDPOINT_OWNER_SLOTS]> =
    spin::Mutex::new([IpcEndpointOwner::EMPTY; IPC_ENDPOINT_OWNER_SLOTS]);

#[inline]
fn service_class_for_endpoint_name(name: &[u8]) -> Option<crate::security::ServiceClass> {
    match name {
        b"memory_server" => Some(crate::security::ServiceClass::MemoryServer),
        b"vfs_server" => Some(crate::security::ServiceClass::VfsServer),
        b"crypto_server" => Some(crate::security::ServiceClass::CryptoServer),
        b"device_server" => Some(crate::security::ServiceClass::DeviceServer),
        b"virtio_drivers" | b"e1000_driver" | b"virtio_net_driver" | b"loopback_driver" => {
            Some(crate::security::ServiceClass::VirtioDriver)
        }
        b"ps2_driver" => Some(crate::security::ServiceClass::Ps2Driver),
        b"network_server" => Some(crate::security::ServiceClass::NetworkServer),
        b"scheduler_server" => Some(crate::security::ServiceClass::SchedulerServer),
        b"input_server" => Some(crate::security::ServiceClass::InputServer),
        b"tty_server" => Some(crate::security::ServiceClass::TtyServer),
        b"fb_server" => Some(crate::security::ServiceClass::FbServer),
        b"exo_shield" => Some(crate::security::ServiceClass::ExoShield),
        b"exosh" => Some(crate::security::ServiceClass::Exosh),
        _ => None,
    }
}

#[cfg(test)]
mod ipc_service_endpoint_class_tests {
    use super::*;
    use crate::security::ServiceClass;

    #[test]
    fn network_driver_service_endpoints_register_driver_class() {
        for name in [
            b"virtio_drivers".as_slice(),
            b"e1000_driver".as_slice(),
            b"virtio_net_driver".as_slice(),
            b"loopback_driver".as_slice(),
        ] {
            assert_eq!(
                service_class_for_endpoint_name(name),
                Some(ServiceClass::VirtioDriver)
            );
        }
    }
}

fn reserve_ipc_endpoint_owner(endpoint: u64, owner_pid: u32) -> Result<bool, i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut empty_slot = None;
    let mut idx = 0usize;
    while idx < owners.len() {
        let entry = owners[idx];
        if entry.endpoint == endpoint {
            if entry.owner_pid == owner_pid {
                return Ok(false);
            }
            if !crate::process::is_alive(entry.owner_pid) {
                owners[idx] = IpcEndpointOwner {
                    endpoint,
                    owner_pid,
                };
                return Ok(true);
            }
            return Err(EBUSY);
        }
        if entry.endpoint == 0 && empty_slot.is_none() {
            empty_slot = Some(idx);
        }
        idx += 1;
    }

    let Some(slot) = empty_slot else {
        return Err(ENOMEM);
    };
    owners[slot] = IpcEndpointOwner {
        endpoint,
        owner_pid,
    };
    Ok(false)
}

fn release_ipc_endpoint_owner(endpoint: u64, owner_pid: u32) -> Result<(), i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut idx = 0usize;
    while idx < owners.len() {
        if owners[idx].endpoint == endpoint {
            if owners[idx].owner_pid != owner_pid {
                return Err(EACCES);
            }
            owners[idx] = IpcEndpointOwner::EMPTY;
            return Ok(());
        }
        idx += 1;
    }
    Ok(())
}

fn ipc_endpoint_owner_pid(endpoint: u64) -> Option<u32> {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        return Some(packed_pid);
    }

    let owners = IPC_ENDPOINT_OWNERS.lock();
    owners
        .iter()
        .find(|entry| entry.endpoint == endpoint && entry.owner_pid != 0)
        .map(|entry| entry.owner_pid)
}

fn primary_ipc_endpoint_for_owner(owner_pid: u32) -> Option<u64> {
    if owner_pid == 0 {
        return None;
    }
    let owners = IPC_ENDPOINT_OWNERS.lock();
    owners
        .iter()
        .find(|entry| entry.owner_pid == owner_pid && entry.endpoint != 0)
        .map(|entry| entry.endpoint)
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
    let _validated_buf =
        match UserBuf::validate(msg_ptr, len, crate::ipc::core::constants::MAX_MSG_SIZE) {
            Ok(b) => b,
            Err(e) => return e.to_errno(),
        };
    let mut payload = match zeroed_user_vec(len) {
        Ok(payload) => payload,
        Err(errno) => return errno,
    };
    if len != 0 {
        if copy_from_user(payload.as_mut_ptr(), msg_ptr as *const u8, len).is_err() {
            return EFAULT;
        }
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let caller_can_inject =
        crate::security::can_inject_src_pid(crate::process::core::pid::Pid(caller_pid));
    let envelope_auth =
        match validate_ipc_envelope_auth(endpoint, caller_pid, caller_can_inject, &payload) {
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
    // FIX-IPC-SENDER-AUTH (Security_Application_Audit §GAP-04/05) : pour un
    // appelant non-trusted, le champ sender_pid de l'enveloppe ABI (octets 0..4)
    // est TOUJOURS forcé à caller_pid, flag INJECT ou non. Sans cela, un
    // processus pouvait forger sender_pid=1 (init) dans l'enveloppe et obtenir
    // des opérations privilégiées des serveurs Ring 1 (memory/scheduler/vfs),
    // qui font confiance à ce champ pour leurs contrôles d'accès.
    // Les services Ring 1 trusted (can_inject) restent libres : l'ipc_router
    // doit pouvoir relayer un message en préservant le PID d'origine.
    if !caller_can_inject && payload.len() == crate::ipc::core::constants::ABI_IPC_ENVELOPE_SIZE {
        payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    }
    if is_reserved_kernel_ipc(endpoint, &payload) {
        return EACCES;
    }
    let raw_flags = if flags & IPC_RECV_TIMEOUT_FLAG != 0 {
        0x0001
    } else {
        0
    };
    // FIX-IPC-SEND-RAW (Security_Audit_Passe2 §A-01) : remplace send_raw() par
    // FIX-A-01 (Security_Audit_Passe2 §A-01) : la vérification de capability
    // IPC_SEND est réalisée par validate_ipc_envelope_auth() ci-dessus qui
    // retourne EACCES si le token est invalide ou absent.
    // KERNEL_CAP_TABLE est private dans crate::security::capability — on utilise
    // crate::ipc::capability_bridge::check_ipc_send_allowed() qui encapsule l'accès.
    // Si l'enveloppe a été validée (ValidToken), on vérifie via la bridge.
    // Pour TrustedCaller/NotRequired, on laisse passer (kernel-internal path).
    // FIX-A-01 (Security_Audit_Passe2 §A-01) : vérification capability IPC_SEND.
    // validate_ipc_envelope_auth() a déjà rejeté les messages non autorisés (EACCES).
    // Pour ValidToken, le token est extrait et vérifié — le validate garantit Rights::IPC_SEND.
    // KERNEL_CAP_TABLE n'est pas accessible directement (private) ; la vérification
    // est encapsulée dans validate_ipc_envelope_auth() qui retourne IpcEnvelopeAuth::ValidToken
    // seulement si le token a passé check_token_owner(). Ici on utilise send_raw car
    // la vérification de capability a déjà été faite dans la fonction validate.
    match crate::ipc::channel::raw::send_raw(endpoint_id, &payload, raw_flags) {
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

/// `exo_ipc_create(name_ptr, name_len, endpoint)` — ouvre la mailbox raw du serveur.
pub fn sys_exo_ipc_create(
    name_ptr: u64,
    name_len: u64,
    endpoint: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_CREATE);
    let len = name_len as usize;
    if len == 0 || len > 128 {
        return EINVAL;
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };

    let _validated = match UserBuf::validate(name_ptr, len, 128) {
        Ok(buf) => buf,
        Err(err) => return err.to_errno(),
    };
    let mut name = match zeroed_user_vec(len) {
        Ok(name) => name,
        Err(errno) => return errno,
    };
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    let replaced_dead_owner = match reserve_ipc_endpoint_owner(endpoint, caller_pid) {
        Ok(replaced) => replaced,
        Err(errno) => return errno,
    };
    if replaced_dead_owner {
        crate::ipc::channel::raw::mailbox_close(ep);
    }

    if crate::ipc::channel::raw::mailbox_open(ep) {
        if let Err(err) = crate::ipc::endpoint::register_endpoint(&name, ep) {
            crate::ipc::channel::raw::mailbox_close(ep);
            let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
            return ipc_error_to_errno(err);
        }
        if let Some(class) = service_class_for_endpoint_name(&name) {
            let _ = crate::security::register_service_class(Pid(caller_pid), class);
        }
        if name.as_slice() == b"fb_server" {
            crate::arch::x86_64::framebuffer_early::deactivate_for_userspace();
        }
        0
    } else {
        let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
        ENOMEM
    }
}

/// `exo_ipc_destroy(endpoint)` — ferme la mailbox raw du serveur.
pub fn sys_exo_ipc_destroy(endpoint: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_DESTROY);
    let endpoint_pid = exo_ipc_endpoint_pid(endpoint);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if endpoint_pid == 0 {
        return EINVAL;
    }
    if caller_pid == 0 || endpoint_pid != caller_pid {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    crate::ipc::channel::raw::mailbox_close(ep);
    crate::ipc::endpoint::unregister_endpoint_by_id(ep);
    let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
    0
}

/// `exo_ipc_lookup(name_ptr, name_len)` — résout un endpoint IPC par nom.
pub fn sys_exo_ipc_lookup(
    name_ptr: u64,
    name_len: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_LOOKUP);

    let len = name_len as usize;
    if len == 0 || len > crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN {
        return EINVAL;
    }
    if name_ptr == 0 {
        return EFAULT;
    }

    let _validated = match UserBuf::validate(
        name_ptr,
        len,
        crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN,
    ) {
        Ok(buf) => buf,
        Err(err) => return err.to_errno(),
    };

    let mut name = match zeroed_user_vec(len) {
        Ok(name) => name,
        Err(errno) => return errno,
    };
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    match crate::ipc::endpoint::lookup_endpoint(&name) {
        Some(endpoint) => endpoint.get() as i64,
        None => ENOENT,
    }
}

#[inline(always)]
fn exo_ipc_endpoint_pid(endpoint: u64) -> u32 {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        packed_pid
    } else if let Some(owner_pid) = ipc_endpoint_owner_pid(endpoint) {
        owner_pid
    } else {
        endpoint as u32
    }
}

fn process_name_eq(name: &[u8; EXO_PROCESS_NAME_LEN], expected: &[u8]) -> bool {
    let mut len = 0usize;
    while len < name.len() && name[len] != 0 {
        len += 1;
    }
    if len != expected.len() {
        return false;
    }
    let mut i = 0usize;
    while i < len {
        if name[i] != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}

const IPC_RECV_TIMEOUT_FLAG: u64 = 0x0001;
const IPC_FLAG_INJECT_SRC_PID: u64 = 0x0002;
const CRYPTO_SERVER_ENDPOINT_ID: u64 = 4;
const CRYPTO_PHOENIX_WAKE_ENTROPY: u32 = 255;

#[inline]
fn zeroed_user_vec(len: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    out.resize(len, 0);
    Ok(out)
}

#[inline]
fn is_reserved_kernel_ipc(endpoint: u64, payload: &[u8]) -> bool {
    if endpoint != CRYPTO_SERVER_ENDPOINT_ID
        || payload.len() < crate::ipc::core::constants::ABI_IPC_HEADER_SIZE
    {
        return false;
    }
    let msg_type = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    msg_type == CRYPTO_PHOENIX_WAKE_ENTROPY
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IpcEnvelopeAuth {
    NotRequired,
    TrustedCaller,
    ValidToken,
}

fn validate_ipc_envelope_auth(
    endpoint: u64,
    caller_pid: u32,
    caller_can_inject: bool,
    payload: &[u8],
) -> Result<IpcEnvelopeAuth, i64> {
    use crate::ipc::core::constants::{
        ABI_IPC_ENVELOPE_SIZE, IPC_CAP_TOKEN_OFFSET, IPC_CAP_TOKEN_SIZE,
    };

    // FIX-IPC-AUTH (Security_Audit_Passe2 §A-02) : la condition originale
    // `payload.len() != ABI_IPC_ENVELOPE_SIZE` permettait à tout attaquant
    // d'envoyer 199 ou 201 bytes pour obtenir IpcEnvelopeAuth::NotRequired
    // et bypasser intégralement la vérification de capability.
    //
    // Correction : les messages hors-format reçoivent NotRequired UNIQUEMENT
    // pour les endpoints de réponse éphémères du kernel (bit 63 = 1).
    // Tous les autres messages, quelle que soit leur taille, doivent soit :
    //   a) être de taille ABI_IPC_ENVELOPE_SIZE → vérification CapToken complète
    //   b) provenir d'un caller_can_inject (trusted kernel caller) → TrustedCaller
    //   c) sinon → EACCES immédiat
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
fn is_kernel_ephemeral_reply_endpoint(endpoint: u64) -> bool {
    endpoint & (1u64 << 63) != 0
}

fn caller_can_manage_target_memory(caller_pid: u32) -> bool {
    if caller_pid == 1 {
        return true;
    }
    match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
        Some(pcb) => process_name_eq(&pcb.name_snapshot(), b"memory_server"),
        None => false,
    }
}

fn caller_can_copy_target_memory(caller_pid: u32) -> bool {
    if caller_pid == 1 {
        return true;
    }
    match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
        Some(pcb) => {
            let name = pcb.name_snapshot();
            process_name_eq(&name, b"memory_server") || process_name_eq(&name, b"vfs_server")
        }
        None => false,
    }
}

fn user_as_for_pid(pid: u32) -> Result<&'static crate::memory::virt::UserAddressSpace, i64> {
    if pid == 0 {
        return Err(EINVAL);
    }
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(ENOENT)?;
    let as_ptr = pcb.address_space_ptr();
    if as_ptr.is_null() {
        return Err(ENOMEM);
    }
    // SAFETY: le PCB reste dans PROCESS_REGISTRY pendant le syscall et
    // address_space pointe vers un UserAddressSpace créé par lifecycle/create.
    Ok(unsafe { &*(as_ptr as *const crate::memory::virt::UserAddressSpace) })
}

fn validate_remote_user_range(addr: u64, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if addr == 0 {
        return Err(EFAULT);
    }
    let end = addr.checked_add(len as u64).ok_or(EFAULT)?;
    if end > crate::syscall::validation::USER_ADDR_MAX {
        return Err(EFAULT);
    }
    Ok(())
}

fn remote_user_phys(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
    write: bool,
) -> Result<(u64, usize), i64> {
    use crate::memory::virt::page_table::{PageTableWalker, WalkResult};

    let walker = PageTableWalker::new(user_as.pml4_phys());
    let virt = VirtAddr::new(addr);
    let (entry, level) = match walker.walk_read(virt) {
        WalkResult::Leaf { entry, level } | WalkResult::HugePage { entry, level } => (entry, level),
        _ => return Err(EFAULT),
    };
    if !entry.is_user() || (write && !entry.is_writable()) {
        return Err(EFAULT);
    }
    let page_size = level.page_size();
    let offset = (addr as usize) & (page_size - 1);
    Ok((
        entry.phys_addr().as_u64() + offset as u64,
        page_size - offset,
    ))
}

/// `exo_mem_copy_from_pid(target_pid, remote_src, local_dst, len, flags)`.
///
/// Réservé aux serveurs racine qui doivent servir des requêtes IPC avec des
/// pointeurs appartenant au client. Le buffer local est toujours copié via
/// `copy_to_user`; le buffer distant est traduit via la PML4 du PID cible.
pub fn sys_exo_mem_copy_from_pid(
    target_pid: u64,
    remote_src: u64,
    local_dst: u64,
    len: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_COPY_FROM_PID);
    if flags != 0 {
        return EINVAL;
    }
    let caller_pid = current_pid_u32();
    if !caller_can_copy_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = validate_remote_user_range(remote_src, len) {
        return e;
    }
    if let Err(e) = validate_remote_user_range(local_dst, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut copied = 0usize;
    let mut scratch = [0u8; 256];
    while copied < len {
        let src = remote_src + copied as u64;
        let (phys, page_avail) = match remote_user_phys(user_as, src, false) {
            Ok(v) => v,
            Err(e) => return if copied == 0 { e } else { copied as i64 },
        };
        let n = (len - copied).min(page_avail).min(scratch.len());
        let src_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *const u8;
        // SAFETY: `remote_user_phys` a validé la traduction du PID cible et
        // `scratch` est un buffer kernel local de taille `n`.
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, scratch.as_mut_ptr(), n);
        }
        if copy_to_user((local_dst + copied as u64) as *mut u8, scratch.as_ptr(), n).is_err() {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        copied += n;
    }
    copied as i64
}

/// `exo_mem_copy_to_pid(target_pid, remote_dst, local_src, len, flags)`.
pub fn sys_exo_mem_copy_to_pid(
    target_pid: u64,
    remote_dst: u64,
    local_src: u64,
    len: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_COPY_TO_PID);
    if flags != 0 {
        return EINVAL;
    }
    let caller_pid = current_pid_u32();
    if !caller_can_copy_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = validate_remote_user_range(remote_dst, len) {
        return e;
    }
    if let Err(e) = validate_remote_user_range(local_src, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut copied = 0usize;
    let mut scratch = [0u8; 256];
    while copied < len {
        let dst = remote_dst + copied as u64;
        let (phys, page_avail) = match remote_user_phys(user_as, dst, true) {
            Ok(v) => v,
            Err(e) => return if copied == 0 { e } else { copied as i64 },
        };
        let n = (len - copied).min(page_avail).min(scratch.len());
        if copy_from_user(
            scratch.as_mut_ptr(),
            (local_src + copied as u64) as *const u8,
            n,
        )
        .is_err()
        {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        let dst_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *mut u8;
        // SAFETY: `remote_user_phys(..., write=true)` garantit une page user
        // présente et writable; `scratch` contient `n` octets copiés du caller.
        unsafe {
            core::ptr::copy_nonoverlapping(scratch.as_ptr(), dst_ptr, n);
        }
        copied += n;
    }
    copied as i64
}

/// `exo_mem_map_pid(target_pid, hint, len, prot, flags)` réservé à memory_server.
pub fn sys_exo_mem_map_pid(
    target_pid: u64,
    hint: u64,
    len: u64,
    prot: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MAP_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mmap_in_as(user_as, hint, len, prot, flags) {
        Ok(addr) => addr as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_munmap_pid(target_pid, addr)` réservé à memory_server.
pub fn sys_exo_mem_munmap_pid(
    target_pid: u64,
    addr: u64,
    _len: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MUNMAP_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap_in_as(user_as, addr) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_mprotect_pid(target_pid, addr, len, prot)` réservé à memory_server.
pub fn sys_exo_mem_mprotect_pid(
    target_pid: u64,
    addr: u64,
    len: u64,
    prot: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MPROTECT_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mprotect_in_as(user_as, addr, len, prot) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

fn normalize_ipc_recv_args(a1: u64, a2: u64, a3: u64, flags: u64) -> (u64, u64, u64, u64) {
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

fn recv_ipc_message(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, nowait: bool) -> i64 {
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
    let timeout_ms = flags & !IPC_RECV_TIMEOUT_FLAG;

    let current_tcb = crate::scheduler::core::switch::current_thread_raw();
    let result = if nowait {
        crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001)
    } else if timeout_requested {
        let deadline = crate::scheduler::timer::clock::monotonic_ns()
            .saturating_add(timeout_ms.saturating_mul(1_000_000));
        loop {
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
                        unsafe {
                            let _ = crate::scheduler::core::switch::cooperative_reschedule();
                        }
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
            match crate::ipc::channel::raw::recv_raw_or_register_waiter(
                endpoint_id,
                &mut payload[..recv_cap],
                current_tcb,
            ) {
                Ok(Some(n)) => break Ok(n),
                Ok(None) => unsafe {
                    if !crate::scheduler::timer::sleep_ns(IPC_RECV_IDLE_NAP_NS) {
                        let _ = crate::scheduler::core::switch::cooperative_reschedule();
                    }
                    crate::ipc::channel::raw::clear_waiter(endpoint_id, current_tcb);
                },
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

fn ipc_error_to_errno(err: IpcError) -> i64 {
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

fn enforce_direct_ipc_policy(endpoint: u64) -> Result<(), i64> {
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



/// `exo_cap_create(type, rights, target_pid, token_out_ptr)` → handle ou errno.
pub fn sys_exo_cap_create(
    cap_type: u64,
    rights: u64,
    target: u64,
    token_out_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_CAP_CREATE);
    if token_out_ptr == 0 {
        return EFAULT;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let cap_type = match checked_u32_sysarg(cap_type) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rights = match checked_u32_sysarg(rights) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target = match checked_u32_sysarg(target) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::create(cap_type, rights, target, caller_pid) {
        Ok(token) => {
            let token_bytes = token.to_bytes();
            if copy_to_user(
                token_out_ptr as *mut u8,
                token_bytes.as_ptr(),
                crate::security::capability::CAP_TOKEN_WIRE_SIZE,
            )
            .is_err()
            {
                return EFAULT;
            }
            token.object_id().as_u64() as i64
        }
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_revoke(handle)`.
pub fn sys_exo_cap_revoke(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_CAP_REVOKE);
    let handle = match checked_u32_sysarg(handle) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::revoke_handle(handle) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_check(token_ptr, rights, target_pid, expected_type)` → 0 ou errno.
pub fn sys_exo_cap_check(
    token_ptr: u64,
    required_rights: u64,
    target_pid: u64,
    expected_type: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_CAP_CHECK);
    if token_ptr == 0 {
        return EFAULT;
    }

    let mut token_bytes = [0u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE];
    if copy_from_user(
        token_bytes.as_mut_ptr(),
        token_ptr as *const u8,
        token_bytes.len(),
    )
    .is_err()
    {
        return EFAULT;
    }

    let token = match crate::security::capability::CapToken::from_bytes(&token_bytes) {
        Some(token) => token,
        None => return EINVAL,
    };

    let required_rights = match checked_u32_sysarg(required_rights) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || caller_pid != target_pid {
        return EACCES;
    }
    let expected_type = match checked_u32_sysarg(expected_type) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::check_token(
        token,
        required_rights,
        target_pid,
        expected_type,
    ) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_pledge(promises, flags, …)` — restreint **irréversiblement** le process
/// courant (sandbox style pledge() OpenBSD, TIER 2.10). `promises` = bitmask de
/// `pledge_flags` ; les capacités NON promises deviennent des restrictions
/// zero-trust enforced au bord syscall (cf. `syscall_restriction_mask` / TIER 1.1).
/// **Monotone** : ne fait que durcir (RÈGLE PLEDGE-01) — un re-pledge ajoute des
/// restrictions, n'en retire jamais. Init (PID 1) ne peut pas pledge (PLEDGE-03).
///
/// Retour : `0` si appliqué (ou rien à restreindre), `EPERM` si init / PID hors-plage.
pub fn sys_exo_pledge(promises: u64, _flags: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PLEDGE);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EPERM;
    }
    let restrictions = crate::security::zero_trust::pledge_promises_to_restrictions(promises);
    if restrictions == 0 {
        // Toutes les capacités enforced sont promises → rien à restreindre (succès).
        return 0;
    }
    // restrict_process est monotone et refuse init (PID 1) / PID hors-plage.
    if crate::security::zero_trust::restrict_process(caller_pid, restrictions) {
        0
    } else {
        EPERM
    }
}

/// `exo_shield_drain(buf_ptr, count, …)` — draine le feed d'événements de sécurité
/// kernel→exo_shield (TIER 3.1). **Réservé au serveur exo_shield** (gaté sur sa
/// classe de service) pour éviter toute fuite d'événements vers un process
/// arbitraire. Copie au plus `min(count, 64)` `ShieldEvent` dans `buf_ptr` ;
/// retourne le nombre drainé (≥ 0) ou un errno négatif.
pub fn sys_exo_shield_drain(
    buf_ptr: u64,
    count: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_SHIELD_DRAIN);
    use crate::security::shield_feed::{ShieldEvent, SHIELD_EVENT_SIZE};

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EPERM;
    }
    // Seul exo_shield draine le feed.
    if crate::security::ipc_policy::service_class_of(crate::process::core::pid::Pid(caller_pid))
        != crate::security::ipc_policy::ServiceClass::ExoShield
    {
        return EACCES;
    }

    let count = count as usize;
    if count == 0 {
        return 0;
    }
    // Au plus 64 événements / appel (≈ 2.5 KiB) — borne la copie + le lock.
    let n = count.min(64);
    let bytes = n * SHIELD_EVENT_SIZE;
    if UserBuf::validate(buf_ptr, bytes, 64 * SHIELD_EVENT_SIZE).is_err() {
        return EFAULT;
    }

    let mut events = [ShieldEvent::zero(); 64];
    let drained = crate::security::shield_feed::drain(&mut events[..n]);
    if drained == 0 {
        return 0;
    }
    let copy_bytes = drained * SHIELD_EVENT_SIZE;
    if copy_to_user(buf_ptr as *mut u8, events.as_ptr() as *const u8, copy_bytes).is_err() {
        return EFAULT;
    }
    drained as i64
}

#[cfg(test)]
mod capability_syscall_arg_tests {
    use super::*;

    #[test]
    fn checked_u32_sysarg_accepts_full_u32_range() {
        assert_eq!(checked_u32_sysarg(0), Ok(0));
        assert_eq!(checked_u32_sysarg(u32::MAX as u64), Ok(u32::MAX));
    }

    #[test]
    fn checked_u32_sysarg_rejects_high_bits_in_syscall_abi() {
        assert_eq!(checked_u32_sysarg(u32::MAX as u64 + 1), Err(EINVAL));
    }

    #[test]
    fn checked_i32_sysarg_accepts_sign_extended_negative_values() {
        assert_eq!(checked_i32_sysarg(u64::MAX), Ok(-1));
        assert_eq!(checked_i32_sysarg(i32::MIN as i64 as u64), Ok(i32::MIN));
        assert_eq!(checked_i32_sysarg(i32::MAX as u64), Ok(i32::MAX));
    }

    #[test]
    fn checked_i32_sysarg_rejects_non_canonical_positive_high_bits() {
        assert_eq!(checked_i32_sysarg(i32::MAX as u64 + 1), Err(EINVAL));
        assert_eq!(checked_i32_sysarg(0x8000_0000_0000_0000), Err(EINVAL));
    }

    #[test]
    fn checked_usize_sysarg_accepts_native_width() {
        assert_eq!(checked_usize_sysarg(0), Ok(0));
        assert_eq!(checked_usize_sysarg(usize::MAX as u64), Ok(usize::MAX));
    }
}

/// `exo_log(buf_ptr, len, level)` — log direct vers le ring buffer kernel.
pub fn sys_exo_log(buf_ptr: u64, len: u64, level: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_LOG);
    let log_len = (len as usize).min(4096);
    let buf = match UserBuf::validate(buf_ptr, log_len, 4096) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    // Copier le message dans un buffer kernel local (stack, NO-ALLOC)
    let mut kbuf = [0u8; 4096];
    if let Err(e) = buf.read_into(&mut kbuf[..log_len]) {
        return e.to_errno();
    }
    crate::arch::x86_64::terminal::debug_write(&kbuf[..log_len]);
    let _ = level;
    0
}

const EXO_PROCESS_NAME_LEN: usize = 16;
const EXO_PROCESS_LIST_MAX: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct ExoProcessInfo {
    pid: u32,
    ppid: u32,
    state: u32,
    threads: u32,
    name: [u8; EXO_PROCESS_NAME_LEN],
    utime_ns: u64,
    stime_ns: u64,
}

/// `exo_process_list(buf, capacity, entry_size)` — snapshot de la registry PCB.
pub fn sys_exo_process_list(
    buf_ptr: u64,
    capacity: u64,
    entry_size: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PROCESS_LIST);
    let expected = core::mem::size_of::<ExoProcessInfo>();
    if entry_size != expected as u64 {
        return EINVAL;
    }
    if capacity == 0 {
        return PROCESS_REGISTRY.count() as i64;
    }
    if capacity > EXO_PROCESS_LIST_MAX as u64 {
        return E2BIG;
    }
    let capacity = capacity as usize;
    let total_len = match capacity.checked_mul(expected) {
        Some(len) => len,
        None => return E2BIG,
    };
    if UserBuf::validate(buf_ptr, total_len, EXO_PROCESS_LIST_MAX * expected).is_err() {
        return EFAULT;
    }

    let mut written = 0usize;
    let mut fault = false;
    PROCESS_REGISTRY.for_each(|pcb| {
        if written >= capacity || fault {
            return;
        }
        let entry = ExoProcessInfo {
            pid: pcb.pid.0,
            ppid: pcb.ppid.load(Ordering::Acquire),
            state: pcb.state.load(Ordering::Acquire),
            threads: pcb.thread_count.load(Ordering::Acquire),
            name: pcb.name_snapshot(),
            utime_ns: pcb.utime_ns.load(Ordering::Acquire),
            stime_ns: pcb.stime_ns.load(Ordering::Acquire),
        };
        let dst = (buf_ptr as *mut u8).wrapping_add(written * expected);
        let src = &entry as *const ExoProcessInfo as *const u8;
        if copy_to_user(dst, src, expected).is_err() {
            fault = true;
        } else {
            written += 1;
        }
    });

    if fault {
        EFAULT
    } else {
        written as i64
    }
}

#[inline]
fn perf_value(value: u64) -> i64 {
    core::cmp::min(value, i64::MAX as u64) as i64
}

/// `exo_perf_read(metric, index)` — lecture de compteurs kernel bornés.
pub fn sys_exo_perf_read(metric: u64, index: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PERF_READ);
    match metric {
        EXO_PERF_SYSCALL_COUNT => {
            if index >= SYSCALL_TABLE_SIZE as u64 {
                EINVAL
            } else {
                perf_value(syscall_table_stat(index as usize))
            }
        }
        EXO_PERF_DISPATCH_TOTAL => perf_value(crate::syscall::dispatch_stats().total),
        EXO_PERF_DISPATCH_FAST_PATH => perf_value(crate::syscall::dispatch_stats().fast_path),
        EXO_PERF_DISPATCH_SLOW_PATH => perf_value(crate::syscall::dispatch_stats().slow_path),
        EXO_PERF_DISPATCH_ENOSYS => perf_value(crate::syscall::dispatch_stats().enosys),
        EXO_PERF_IPC_MESSAGES_SENT => perf_value(crate::ipc::ipc_stats_snapshot().messages_sent()),
        EXO_PERF_IPC_MESSAGES_RECEIVED => {
            perf_value(crate::ipc::ipc_stats_snapshot().messages_received())
        }
        EXO_PERF_IPC_MESSAGES_DROPPED => {
            perf_value(crate::ipc::ipc_stats_snapshot().messages_dropped())
        }
        EXO_PERF_IPC_RPC_CALLS => perf_value(crate::ipc::ipc_stats_snapshot().rpc_calls()),
        EXO_PERF_IPC_RPC_TIMEOUTS => perf_value(crate::ipc::ipc_stats_snapshot().rpc_timeouts()),
        EXO_PERF_IPC_FUTEX_WAITS => perf_value(crate::ipc::ipc_stats_snapshot().futex_waits()),
        EXO_PERF_IPC_FUTEX_WAKES => perf_value(crate::ipc::ipc_stats_snapshot().futex_wakes()),
        _ => EINVAL,
    }
}

pub fn sys_exo_perf_enable(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PERF_ENABLE);
    0
}

pub fn sys_exo_perf_disable(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PERF_DISABLE);
    0
}

/// `exo_phoenix_state_set(state)` — synchronise une transition Phoenix root.
pub fn sys_exo_phoenix_state_set(
    state: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PHOENIX_STATE_SET);
    if state > u8::MAX as u64 {
        return EINVAL;
    }
    if !matches!(state as u8, 1 | 9 | 10) {
        return EINVAL;
    }
    let caller = current_pid_u32();
    if caller != 0 {
        let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(caller)) else {
            return EPERM;
        };
        if !pcb.is_root() {
            return EPERM;
        }
        let name = pcb.name_snapshot();
        if !process_name_eq(&name, b"network_server") && !process_name_eq(&name, b"init_server") {
            return EPERM;
        }
    }
    if crate::exophoenix::try_set_state_raw(state as u8) {
        0
    } else {
        EINVAL
    }
}

/// `exo_phoenix_state_get()` — retourne l'état Phoenix courant.
pub fn sys_exo_phoenix_state_get(
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PHOENIX_STATE_GET);
    crate::exophoenix::state() as u8 as i64
}

