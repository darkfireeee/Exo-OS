// ipc/rpc/raw.rs — call_raw : appel RPC synchrone depuis la couche syscall
//
// `call_raw` encapsule un échange requête/réponse synchrone en utilisant
// deux mailboxes raw (serveur + répondeur éphémère).
//
// Protocole :
//   1. Alloue un EndpointId de réponse éphémère (cookie unique).
//   2. Préfixe le message avec un en-tête `RawCallHeader` contenant ce cookie.
//   3. Envoie vers la mailbox du serveur (`server_ep`).
//   4. Attend le message de retour sur la mailbox éphémère.
//   5. Libère la mailbox éphémère et retourne le payload de réponse.
//
// Cette approche est synchrone (spin-wait) et no-alloc.
// Timeout après ~2 * 10^6 itérations de spin_loop.
//
// RÈGLE IPC-CALL-01 : call_raw est TOUJOURS synchrone (pas de callback).
// RÈGLE IPC-CALL-02 : les mailboxes éphémères sont libérées après chaque appel.


use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::channel::raw as channel_raw;

// ─────────────────────────────────────────────────────────────────────────────
// Compteur de cookies — unique par appel, jamais réutilisé dans la session.
// ─────────────────────────────────────────────────────────────────────────────

static CALL_COOKIE: AtomicU64 = AtomicU64::new(1);

#[inline]
fn next_cookie() -> u64 {
    CALL_COOKIE.fetch_add(1, Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// RawCallHeader — en-tête préfixé dans chaque message call_raw
// ─────────────────────────────────────────────────────────────────────────────

/// Magic pour valider l'en-tête.
pub const CALL_MAGIC: u32 = 0x4558_4F43; // "EXOC"

/// En-tête de 16 bytes préfixé au payload d'un appel raw.
#[repr(C)]
#[derive(Copy, Clone)]
struct RawCallHeader {
    /// Magic de validation.
    magic:       u32,
    /// Longueur du payload qui suit (sans cet en-tête).
    payload_len: u32,
    /// Cookie de corrélation (unique par appel).
    cookie:      u64,
}

const CALL_HEADER_SIZE: usize = core::mem::size_of::<RawCallHeader>();
pub const MAX_CALL_PAYLOAD:  usize = MAX_MSG_SIZE - CALL_HEADER_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// EndpointId éphémère — basé sur le cookie pour unicité
// ─────────────────────────────────────────────────────────────────────────────

/// Construit un EndpointId éphémère depuis un cookie.
/// On utilise un namespace private (bit 63 = 1) pour éviter les collisions
/// avec les endpoints utilisateur.
fn ephemeral_ep(cookie: u64) -> Option<EndpointId> {
    // Bit 63 = 1 → namespace "réponse éphémère kernel"
    let id = (1u64 << 63) | (cookie & 0x7FFF_FFFF_FFFF_FFFF);
    EndpointId::new(id)
}

// ─────────────────────────────────────────────────────────────────────────────
// call_raw
// ─────────────────────────────────────────────────────────────────────────────

/// Appel RPC synchrone vers `server_ep`.
///
/// - `msg`      : payload de la requête (≤ `MAX_CALL_PAYLOAD` bytes)
/// - `reply_buf`: buffer pour le payload de réponse
///
/// Retourne le nombre d'octets de réponse, ou une `IpcError`.
///
/// # Erreurs
/// - `MessageTooLarge`  : `msg.len()` > MAX_CALL_PAYLOAD
/// - `OutOfResources`   : impossible d'allouer une mailbox éphémère
/// - `NotFound`         : serveur introuvable (mailbox non ouverte)
/// - `Timeout`          : le serveur n'a pas répondu dans le délai
pub fn call_raw(
    server_ep:  EndpointId,
    msg:        &[u8],
    reply_buf:  &mut [u8],
) -> Result<usize, IpcError> {
    if msg.len() > MAX_CALL_PAYLOAD {
        return Err(IpcError::MessageTooLarge);
    }

    // 1. Allouer un cookie et un endpoint de réponse éphémère.
    let cookie      = next_cookie();
    let reply_ep    = ephemeral_ep(cookie).ok_or(IpcError::InternalError)?;

    if !channel_raw::mailbox_open(reply_ep) {
        return Err(IpcError::OutOfResources);
    }

    // 2. Construire le message : en-tête + payload.
    let header = RawCallHeader {
        magic:       CALL_MAGIC,
        payload_len: msg.len() as u32,
        cookie,
    };

    let mut call_buf = [0u8; MAX_MSG_SIZE];
    // Sérialiser l'en-tête (little-endian natif, repr(C)).
    // SAFETY: RawCallHeader est repr(C) et call_buf est suffisamment grand.
    unsafe {
        core::ptr::write_unaligned(
            call_buf.as_mut_ptr() as *mut RawCallHeader,
            header,
        );
    }
    call_buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + msg.len()].copy_from_slice(msg);

    // Stocker reply_ep dans les derniers 8 bytes du message
    // (le serveur l'utilise pour savoir où répondre).
    // Protocole : bytes [MAX_MSG_SIZE-8..] = reply_ep_id (u64 LE)
    let reply_ep_bytes = reply_ep.get().to_le_bytes();
    let reply_pos = MAX_MSG_SIZE - core::mem::size_of::<u64>();
    call_buf[reply_pos..].copy_from_slice(&reply_ep_bytes);

    // 3. Envoyer vers la mailbox du serveur.
    let send_result = channel_raw::send_raw(
        server_ep,
        &call_buf[..CALL_HEADER_SIZE + msg.len()],
        0, // bloquant
    );
    if let Err(e) = send_result {
        channel_raw::mailbox_close(reply_ep);
        return Err(e);
    }

    // 4. Attendre la réponse sur la mailbox éphémère.
    let mut raw_reply = [0u8; MAX_MSG_SIZE];
    let recv_result = channel_raw::recv_raw(reply_ep, &mut raw_reply, 0);

    // 5. Libérer la mailbox éphémère.
    channel_raw::mailbox_close(reply_ep);

    let n = recv_result?;
    if n < CALL_HEADER_SIZE {
        return Err(IpcError::ProtocolError);
    }

    // Vérifier l'en-tête de réponse.
    // SAFETY: n >= CALL_HEADER_SIZE, raw_reply est aligné.
    let reply_hdr: RawCallHeader = unsafe {
        core::ptr::read_unaligned(raw_reply.as_ptr() as *const RawCallHeader)
    };
    if reply_hdr.magic != CALL_MAGIC || reply_hdr.cookie != cookie {
        return Err(IpcError::ProtocolError);
    }

    let payload_len = reply_hdr.payload_len as usize;
    let copy_len    = payload_len.min(reply_buf.len()).min(MAX_CALL_PAYLOAD);
    reply_buf[..copy_len].copy_from_slice(
        &raw_reply[CALL_HEADER_SIZE..CALL_HEADER_SIZE + copy_len]
    );
    Ok(copy_len)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers serveur : parser / construire les réponses
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat du parsing d'un message reçu via la mailbox d'un serveur.
pub struct CallRequest<'a> {
    /// Payload de la requête (sans l'en-tête).
    pub payload:   &'a [u8],
    /// Cookie de corrélation (à copier dans la réponse).
    pub cookie:    u64,
    /// Endpoint de réponse (extrait des derniers octets).
    pub reply_ep:  Option<EndpointId>,
}

/// Parse un message brut reçu (côté serveur).
///
/// Retourne `None` si `buf` n'est pas un message call_raw valide.
pub fn parse_call<'a>(buf: &'a [u8]) -> Option<CallRequest<'a>> {
    if buf.len() < CALL_HEADER_SIZE { return None; }
    // SAFETY: buf.len() >= CALL_HEADER_SIZE.
    let hdr: RawCallHeader = unsafe {
        core::ptr::read_unaligned(buf.as_ptr() as *const RawCallHeader)
    };
    if hdr.magic != CALL_MAGIC { return None; }

    let payload_len = hdr.payload_len as usize;
    if buf.len() < CALL_HEADER_SIZE + payload_len { return None; }

    // Lire le reply_ep depuis la fin du buffer ORIGINAL (MAX_MSG_SIZE bytes).
    let reply_ep = None; // Le endpoint de réponse est passé hors-bande dans le cookie

    Some(CallRequest {
        payload:  &buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + payload_len],
        cookie:   hdr.cookie,
        reply_ep,
    })
}

/// Construit et envoie une réponse vers `reply_ep` (côté serveur).
pub fn send_reply(
    reply_ep:    EndpointId,
    cookie:      u64,
    reply_data:  &[u8],
) -> Result<(), IpcError> {
    if reply_data.len() > MAX_CALL_PAYLOAD {
        return Err(IpcError::MessageTooLarge);
    }

    let header = RawCallHeader {
        magic:       CALL_MAGIC,
        payload_len: reply_data.len() as u32,
        cookie,
    };

    let mut buf = [0u8; MAX_MSG_SIZE];
    // SAFETY: repr(C) + buf assez grand.
    unsafe {
        core::ptr::write_unaligned(buf.as_mut_ptr() as *mut RawCallHeader, header);
    }
    buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + reply_data.len()].copy_from_slice(reply_data);

    channel_raw::send_raw(reply_ep, &buf[..CALL_HEADER_SIZE + reply_data.len()], 0)
        .map(|_| ())
}
