// ipc/channel/raw.rs — API raw IPC pour la couche syscall (Exo-OS)
//
// Fournit `send_raw` / `recv_raw` : pont entre les handles u64 du syscall
// exo_ipc_send / exo_ipc_recv et les canaux IPC internes.
//
// Architecture : table statique de MAX_RAW_SLOTS mailboxes ring-buffer.
// Chaque mailbox est identifiée par un EndpointId (u64) et contient un
// anneau circulaire de messages inline (no-alloc).
//
// RÈGLE NO-ALLOC : zéro Vec/Box/Arc. Tout est statique.
// RÈGLE IPC-RAW-01 : auto-open à la première écriture (send_raw crée le slot).
// RÈGLE IPC-RAW-02 : un dépassement de ring (slot plein) incrémente drop_count.


use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError, MessageId, alloc_message_id};
use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
use crate::scheduler::sync::spinlock::SpinLock;
// IPC-04 (v6) : vérification capability via security::access_control
use crate::security::capability::{CapTable, CapToken, Rights};
use crate::security::access_control::{check_access, ObjectKind, AccessError};

// ─────────────────────────────────────────────────────────────────────────────
// Dimensionnement
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de mailboxes raw simultanées.
pub const MAX_RAW_SLOTS: usize = 64;

/// Profondeur de l'anneau par mailbox (puissance de 2).
pub const RAW_RING_DEPTH: usize = 16;

const RAW_RING_MASK: usize = RAW_RING_DEPTH - 1;

const _: () = assert!(
    RAW_RING_DEPTH.is_power_of_two(),
    "RAW_RING_DEPTH doit être une puissance de 2"
);

// ─────────────────────────────────────────────────────────────────────────────
// InnerMsg — un message dans l'anneau (no-alloc)
// ─────────────────────────────────────────────────────────────────────────────

struct InnerMsg {
    len:  usize,
    data: [u8; MAX_MSG_SIZE],
}

impl InnerMsg {
    const fn empty() -> Self {
        Self { len: 0, data: [0u8; MAX_MSG_SIZE] }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InnerRing — anneau circulaire (sous SpinLock)
// ─────────────────────────────────────────────────────────────────────────────

struct InnerRing {
    head:  usize,                            // lecture (consommateur)
    tail:  usize,                            // écriture (producteur)
    count: usize,                            // messages présents
    msgs:  [InnerMsg; RAW_RING_DEPTH],
}

impl InnerRing {
    const fn empty() -> Self {
        Self {
            head: 0, tail: 0, count: 0,
            // NOTE: const array init avec fonction const
            msgs: {
                let arr = [const { InnerMsg::empty() }; RAW_RING_DEPTH];
                arr
            },
        }
    }

    /// Enfile un message. Retourne `false` si l'anneau est plein.
    #[inline]
    fn enqueue(&mut self, data: &[u8]) -> bool {
        if self.count == RAW_RING_DEPTH { return false; }
        let len = data.len().min(MAX_MSG_SIZE);
        let slot = &mut self.msgs[self.tail & RAW_RING_MASK];
        slot.len = len;
        slot.data[..len].copy_from_slice(&data[..len]);
        self.tail = self.tail.wrapping_add(1);
        self.count += 1;
        true
    }

    /// Défile un message. Retourne `None` si vide.
    #[inline]
    fn dequeue(&mut self, buf: &mut [u8]) -> Option<usize> {
        if self.count == 0 { return None; }
        let slot = &self.msgs[self.head & RAW_RING_MASK];
        let len = slot.len.min(buf.len());
        buf[..len].copy_from_slice(&slot.data[..len]);
        self.head = self.head.wrapping_add(1);
        self.count -= 1;
        Some(len)
    }

    #[inline(always)]
    #[allow(dead_code)]
    fn is_empty(&self) -> bool { self.count == 0 }

    #[inline(always)]
    fn is_full(&self) -> bool  { self.count == RAW_RING_DEPTH }

    /// Réinitialise l'anneau (utilisé lors de `mailbox_close`).
    fn reset(&mut self) {
        self.head  = 0;
        self.tail  = 0;
        self.count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RawSlot — une mailbox assignée à un endpoint
// ─────────────────────────────────────────────────────────────────────────────

struct RawSlot {
    /// 0 = slot libre, >0 = endpoint_id propriétaire.
    endpoint_id: AtomicU64,
    /// Anneau de messages protégé par spinlock.
    ring:        SpinLock<InnerRing>,
    /// Compteurs statistiques.
    send_count:  AtomicU64,
    recv_count:  AtomicU64,
    drop_count:  AtomicU64,
}

impl RawSlot {
    const fn new() -> Self {
        Self {
            endpoint_id: AtomicU64::new(0),
            ring:        SpinLock::new(InnerRing::empty()),
            send_count:  AtomicU64::new(0),
            recv_count:  AtomicU64::new(0),
            drop_count:  AtomicU64::new(0),
        }
    }
}

// SAFETY: toutes les mutations passent par SpinLock ou AtomicU64.
unsafe impl Sync for RawSlot {}
unsafe impl Send for RawSlot {}

// ─────────────────────────────────────────────────────────────────────────────
// Table globale statique
// ─────────────────────────────────────────────────────────────────────────────

static RAW_TABLE: [RawSlot; MAX_RAW_SLOTS] = [const { RawSlot::new() }; MAX_RAW_SLOTS];

/// Nombre de mailboxes ouvertes.
static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions internes
// ─────────────────────────────────────────────────────────────────────────────

/// Lookup interne : retourne l'index du slot pour `ep_id` ou `None`.
#[inline]
fn find_slot(ep_id: u64) -> Option<usize> {
    // Sonde linéaire depuis la position idéale.
    let start = (ep_id as usize).wrapping_mul(2654435761) % MAX_RAW_SLOTS;
    for i in 0..MAX_RAW_SLOTS {
        let idx = (start + i) % MAX_RAW_SLOTS;
        if RAW_TABLE[idx].endpoint_id.load(Ordering::Acquire) == ep_id {
            return Some(idx);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// API de gestion de mailbox
// ─────────────────────────────────────────────────────────────────────────────

/// Ouvre une mailbox pour `ep_id`.
///
/// Idempotent : si un slot existe déjà pour cet `ep_id`, retourne `true`.
/// Retourne `false` si la table est pleine.
pub fn mailbox_open(ep_id: EndpointId) -> bool {
    let id = ep_id.get();
    if id == 0 { return false; }
    // Idempotence : déjà ouvert ?
    if find_slot(id).is_some() { return true; }
    // Trouver un slot libre.
    let start = (id as usize).wrapping_mul(2654435761) % MAX_RAW_SLOTS;
    for i in 0..MAX_RAW_SLOTS {
        let idx = (start + i) % MAX_RAW_SLOTS;
        if RAW_TABLE[idx].endpoint_id
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            OPEN_COUNT.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    false // table pleine
}

/// Ferme la mailbox associée à `ep_id` et vide son anneau.
pub fn mailbox_close(ep_id: EndpointId) {
    let id = ep_id.get();
    if let Some(idx) = find_slot(id) {
        if RAW_TABLE[idx].endpoint_id
            .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            RAW_TABLE[idx].ring.lock().reset();
            OPEN_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Nombre de mailboxes actuellement ouvertes.
#[inline]
pub fn mailbox_open_count() -> usize {
    OPEN_COUNT.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// API principale — send_raw / recv_raw
// ─────────────────────────────────────────────────────────────────────────────

/// Envoie `data` dans la mailbox de `ep_id`.
///
/// Si le slot n'existe pas encore, il est créé automatiquement (auto-open).
/// `flags` : bitmask
///   - `0x0001` (NOWAIT) — retourne `Err(WouldBlock)` si anneau plein.
///
/// Retourne un `MessageId` unique alloué côté kernel.
pub fn send_raw(ep_id: EndpointId, data: &[u8], flags: u32) -> Result<MessageId, IpcError> {
    if data.len() > MAX_MSG_SIZE {
        return Err(IpcError::MessageTooLarge);
    }

    let id = ep_id.get();
    if id == 0 { return Err(IpcError::NullEndpoint); }

    // Auto-open si nécessaire.
    if find_slot(id).is_none() {
        if !mailbox_open(ep_id) {
            return Err(IpcError::OutOfResources);
        }
    }

    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let nowait = flags & 0x0001 != 0;

    {
        let mut ring = slot.ring.lock();
        if !ring.is_full() {
            ring.enqueue(data);
            slot.send_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageSent);
            return Ok(alloc_message_id());
        }
        // Anneau plein.
        if nowait {
            slot.drop_count.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::WouldBlock);
        }
    }

    // Attente spin courte avec relâchement du lock.
    let mut spins: u32 = 0;
    loop {
        core::hint::spin_loop();
        spins = spins.saturating_add(1);
        let mut ring = slot.ring.lock();
        if ring.enqueue(data) {
            slot.send_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageSent);
            return Ok(alloc_message_id());
        }
        if spins > 200_000 {
            slot.drop_count.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::Timeout);
        }
    }
}

/// Variante strictement non bloquante de `send_raw`.
///
/// Contraintes :
/// - ne spin jamais sur le verrou d'anneau ;
/// - retourne `WouldBlock` si le slot est occupé ou si l'anneau est plein ;
/// - adaptée aux contextes IRQ/ISR qui doivent rester bornés.
pub fn try_send_raw_nowait(ep_id: EndpointId, data: &[u8]) -> Result<MessageId, IpcError> {
    if data.len() > MAX_MSG_SIZE {
        return Err(IpcError::MessageTooLarge);
    }

    let id = ep_id.get();
    if id == 0 {
        return Err(IpcError::NullEndpoint);
    }

    if find_slot(id).is_none() && !mailbox_open(ep_id) {
        return Err(IpcError::OutOfResources);
    }

    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let Some(mut ring) = slot.ring.try_lock() else {
        slot.drop_count.fetch_add(1, Ordering::Relaxed);
        return Err(IpcError::WouldBlock);
    };

    if !ring.is_full() {
        ring.enqueue(data);
        slot.send_count.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageSent);
        return Ok(alloc_message_id());
    }

    slot.drop_count.fetch_add(1, Ordering::Relaxed);
    Err(IpcError::WouldBlock)
}

/// Reçoit un message de la mailbox de `ep_id` dans `buf`.
///
/// `flags` :
///   - `0x0001` (NOWAIT) — retourne `Err(WouldBlock)` si aucun message.
///
/// Retourne le nombre d'octets copiés dans `buf`.
pub fn recv_raw(ep_id: EndpointId, buf: &mut [u8], flags: u32) -> Result<usize, IpcError> {
    let id = ep_id.get();
    if id == 0 { return Err(IpcError::NullEndpoint); }

    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let nowait = flags & 0x0001 != 0;

    {
        let mut ring = slot.ring.lock();
        if let Some(n) = ring.dequeue(buf) {
            slot.recv_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Ok(n);
        }
        if nowait {
            return Err(IpcError::WouldBlock);
        }
    }

    // Attente spin.
    let mut spins: u32 = 0;
    loop {
        core::hint::spin_loop();
        spins = spins.saturating_add(1);
        let mut ring = slot.ring.lock();
        if let Some(n) = ring.dequeue(buf) {
            slot.recv_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Ok(n);
        }
        if spins > 1_000_000 {
            return Err(IpcError::Timeout);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques d'une mailbox active.
#[derive(Copy, Clone, Debug, Default)]
pub struct RawSlotStats {
    pub endpoint_id: u64,
    pub send_count:  u64,
    pub recv_count:  u64,
    pub drop_count:  u64,
}

/// Snapshot des stats de toutes les mailboxes actives.
/// Retourne un tableau de `MAX_RAW_SLOTS` éléments (`None` = slot libre).
pub fn raw_stats_snapshot() -> [Option<RawSlotStats>; MAX_RAW_SLOTS] {
    let mut out = [None; MAX_RAW_SLOTS];
    for i in 0..MAX_RAW_SLOTS {
        let ep = RAW_TABLE[i].endpoint_id.load(Ordering::Relaxed);
        if ep != 0 {
            out[i] = Some(RawSlotStats {
                endpoint_id: ep,
                send_count:  RAW_TABLE[i].send_count.load(Ordering::Relaxed),
                recv_count:  RAW_TABLE[i].recv_count.load(Ordering::Relaxed),
                drop_count:  RAW_TABLE[i].drop_count.load(Ordering::Relaxed),
            });
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// API capability-checked — IPC-04 (v6) : appel direct security/access_control/
// ─────────────────────────────────────────────────────────────────────────────

/// Envoie dans la mailbox de `ep_id` avec vérification capability (RÈGLE IPC-04 v6).
///
/// Identique à `send_raw()` mais vérifie `Rights::IPC_SEND` via
/// `security::access_control::check_access()` avant toute opération.
/// Utilisé par la couche syscall (appels depuis l'espace utilisateur).
/// Les appels kernel-internes peuvent utiliser `send_raw()` directement.
pub fn send_raw_checked(
    ep_id: EndpointId,
    data:  &[u8],
    flags: u32,
    table: &CapTable,
    token: CapToken,
) -> Result<MessageId, IpcError> {
    // IPC-04 (v6) : vérification capability — security::access_control
    check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_SEND, "ipc::channel::raw")
        .map_err(|e| match e {
            AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
            _ => IpcError::PermissionDenied,
        })?;
    send_raw(ep_id, data, flags)
}

/// Reçoit depuis la mailbox de `ep_id` avec vérification capability (RÈGLE IPC-04 v6).
///
/// # Droits requis : `Rights::IPC_RECV`
pub fn recv_raw_checked(
    ep_id: EndpointId,
    buf:   &mut [u8],
    flags: u32,
    table: &CapTable,
    token: CapToken,
) -> Result<usize, IpcError> {
    // IPC-04 (v6) : vérification capability — security::access_control
    check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_RECV, "ipc::channel::raw")
        .map_err(|e| match e {
            AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
            _ => IpcError::PermissionDenied,
        })?;
    recv_raw(ep_id, buf, flags)
}

#[cfg(test)]
mod tests {
    use super::{mailbox_close, mailbox_open, recv_raw, send_raw};
    use crate::ipc::core::types::EndpointId;

    #[test]
    fn test_raw_mailbox_open_send_recv_roundtrip() {
        let ep = EndpointId::new(5).unwrap();
        mailbox_close(ep);
        assert!(mailbox_open(ep));

        let payload = *b"exo-ipc";
        send_raw(ep, &payload, 0).expect("send raw");

        let mut out = [0u8; 32];
        let n = recv_raw(ep, &mut out, 0x0001).expect("recv raw");
        assert_eq!(&out[..n], &payload);

        mailbox_close(ep);
    }

    #[test]
    fn test_raw_mailbox_send_recv_stress() {
        let ep = EndpointId::new(9).unwrap();
        mailbox_close(ep);
        assert!(mailbox_open(ep));

        for round in 0..1024u16 {
            let payload = [
                (round & 0xFF) as u8,
                (round >> 8) as u8,
                (round as u8).wrapping_mul(3),
                (round as u8).wrapping_mul(7),
            ];
            send_raw(ep, &payload, 0).expect("send raw stress");

            let mut out = [0u8; 32];
            let n = recv_raw(ep, &mut out, 0x0001).expect("recv raw stress");
            assert_eq!(n, payload.len());
            assert_eq!(&out[..n], &payload);
        }

        mailbox_close(ep);
    }
}
