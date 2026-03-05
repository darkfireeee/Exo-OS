// ipc/channel/sync.rs — Canal synchrone (rendezvous) pour l'IPC noyau Exo-OS
//
// Ce canal implémente le pattern rendezvous : l'émetteur est bloqué jusqu'à
// ce que le récepteur accepte explicitement le message. Garantie de livraison
// forte : aucun message ne peut être perdu (pas de file tampon). Utilisé pour
// les appels synchrones critiques où la confirmation de réception est requise.
//
// Caractéristiques :
//   - Zéro copie optionnelle via ZeroCopyRef
//   - Timeout configurable (délai wall-clock en nanosecondes)
//   - Intégration WaitQueue scheduler pour suspension efficace
//   - Pas d'allocation : tout est statique ou sur la pile

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use crate::ipc::core::types::{ChannelId, IpcError, MsgFlags, MessageId, alloc_channel_id, alloc_message_id};
use crate::ipc::core::constants::{
    MAX_MSG_SIZE, MSG_HEADER_MAGIC, SYNC_CHANNEL_TIMEOUT_NS,
};
use crate::ipc::core::transfer::{MessageHeader, TransferEngine};
use crate::ipc::ring::zerocopy::ZeroCopyRef;
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
use crate::scheduler::sync::spinlock::SpinLock;
// IPC-04 (v6) : vérification capability via security::access_control (appel direct)
use crate::security::capability::{CapTable, CapToken, Rights};
use crate::security::access_control::{check_access, ObjectKind, AccessError};

// ---------------------------------------------------------------------------
// États internes d'un rendez-vous en cours
// ---------------------------------------------------------------------------

/// Phase du protocole rendezvous
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RendezVousState {
    /// Canal vide, personne n'attend
    Idle = 0,
    /// Un émetteur a déposé un message et attend le récepteur
    SenderWaiting = 1,
    /// Le récepteur a pris le message, il signale l'émetteur
    ReceiverAcked = 2,
    /// Timeout ou annulation
    Cancelled = 3,
}

impl RendezVousState {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::SenderWaiting,
            2 => Self::ReceiverAcked,
            3 => Self::Cancelled,
            _ => Self::Cancelled,
        }
    }
}

// ---------------------------------------------------------------------------
// Slot de rendezvous — stockage inline du message (no-alloc)
// ---------------------------------------------------------------------------

/// Taille maximale du payload inline pour un canal synchrone
pub const SYNC_INLINE_SIZE: usize = MAX_MSG_SIZE;

/// Slot inline partagé entre l'émetteur et le récepteur.
/// Protected par un AtomicU32 d'état pour accès sans mutex sur le chemin rapide.
#[repr(C, align(64))]
pub struct SyncSlot {
    /// État courant du rendezvous (RendezVousState encodé)
    pub state: AtomicU32,
    /// Numéro de séquence pour détecter les ABA
    pub sequence: AtomicU64,
    /// Longueur du payload copié dans `data`
    pub payload_len: AtomicUsize,
    /// Flags du message
    pub flags: AtomicU32,
    /// Référence zero-copy (valide si flag ZEROCOPY est positionné)
    pub zc_ref: UnsafeCell<ZeroCopyRef>,
    /// Payload inline
    pub data: UnsafeCell<[u8; SYNC_INLINE_SIZE]>,
    /// Identifiant du message pour corrélation
    pub msg_id: AtomicU64,
    /// Horodatage de dépôt (ns depuis boot)
    pub timestamp_ns: AtomicU64,
    /// TID de l'émetteur bloqué (pour le réveil par le récepteur)
    pub sender_tid: AtomicU32,
    /// TID du récepteur bloqué (pour le réveil par l'émetteur)
    pub receiver_tid: AtomicU32,
}

// SAFETY: SyncSlot est une structure de données partagée entre threads mais
// tous les accès aux champs mutables passent par des atomiques ou sont protégés
// par la sémantique du protocole d'état (state machine stricte).
unsafe impl Sync for SyncSlot {}
unsafe impl Send for SyncSlot {}

impl SyncSlot {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(RendezVousState::Idle as u32),
            sequence: AtomicU64::new(0),
            payload_len: AtomicUsize::new(0),
            flags: AtomicU32::new(0),
            zc_ref: UnsafeCell::new(ZeroCopyRef::null()),
            data: UnsafeCell::new([0u8; SYNC_INLINE_SIZE]),
            msg_id: AtomicU64::new(0),
            timestamp_ns: AtomicU64::new(0),
            sender_tid: AtomicU32::new(0),
            receiver_tid: AtomicU32::new(0),
        }
    }

    /// Réinitialise le slot à Idle pour le prochain rendezvous.
    /// SAFETY: doit être appelé seulement quand le slot est libre (pas de course).
    pub unsafe fn reset(&self) {
        self.payload_len.store(0, Ordering::Relaxed);
        self.flags.store(0, Ordering::Relaxed);
        self.msg_id.store(0, Ordering::Relaxed);
        self.timestamp_ns.store(0, Ordering::Relaxed);
        self.sender_tid.store(0, Ordering::Relaxed);
        self.receiver_tid.store(0, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
        self.state.store(RendezVousState::Idle as u32, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Statistiques locales du canal synchrone
// ---------------------------------------------------------------------------

/// Compteurs de performance propres à un SyncChannel
#[repr(C, align(64))]
pub struct SyncChannelStats {
    pub sends_ok: AtomicU64,
    pub sends_timeout: AtomicU64,
    pub sends_cancelled: AtomicU64,
    pub recvs_ok: AtomicU64,
    pub recvs_would_block: AtomicU64,
    pub total_bytes: AtomicU64,
    _pad: [u8; 16],
}

impl SyncChannelStats {
    pub const fn new() -> Self {
        Self {
            sends_ok: AtomicU64::new(0),
            sends_timeout: AtomicU64::new(0),
            sends_cancelled: AtomicU64::new(0),
            recvs_ok: AtomicU64::new(0),
            recvs_would_block: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }
}

// ---------------------------------------------------------------------------
// SyncChannel — structure principale
// ---------------------------------------------------------------------------

/// Canal synchrone (rendezvous) entre exactement un émetteur et un récepteur.
///
/// Protocole en deux phases :
///  1. `send()` : copie le message dans `slot`, passe à `SenderWaiting`,
///     puis boucle spin (ou suspend via scheduler) jusqu'à `ReceiverAcked`.
///  2. `recv()` : attend `SenderWaiting`, copie depuis `slot`,
///     passe à `ReceiverAcked` pour débloquer l'émetteur.
#[repr(C, align(64))]
pub struct SyncChannel {
    /// Identifiant unique du canal
    pub id: ChannelId,
    /// Slot de données partagé
    pub slot: SyncSlot,
    /// Statistiques locales
    pub stats: SyncChannelStats,
    /// Canal ouvert ou fermé
    pub closed: AtomicU32,
    /// Timeout d'envoi en nanosecondes (0 = infini)
    pub send_timeout_ns: AtomicU64,
    _pad: [u8; 24],
}

// SAFETY: ChannelId est Copy, SyncSlot est Sync+Send, AtomicU32 est Sync+Send.
unsafe impl Sync for SyncChannel {}
unsafe impl Send for SyncChannel {}

impl SyncChannel {
    /// Crée un nouveau canal synchrone avec un identifiant unique alloué.
    pub fn new() -> Self {
        Self {
            id: alloc_channel_id(),
            slot: SyncSlot::new(),
            stats: SyncChannelStats::new(),
            closed: AtomicU32::new(0),
            send_timeout_ns: AtomicU64::new(SYNC_CHANNEL_TIMEOUT_NS),
            _pad: [0u8; 24],
        }
    }

    /// Retourne `true` si le canal est fermé.
    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    /// Ferme le canal — réveille tout émetteur bloqué avec Cancelled.
    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
        // Signaler toute attente en cours
        let state = self.slot.state.load(Ordering::Acquire);
        if RendezVousState::from_u32(state) == RendezVousState::SenderWaiting {
            self.slot.state.store(
                RendezVousState::Cancelled as u32,
                Ordering::Release,
            );
        }
    }

    // -----------------------------------------------------------------------
    // ENVOI — chemin émetteur
    // -----------------------------------------------------------------------

    /// Envoie un message et attend la confirmation du récepteur (rendezvous).
    ///
    /// # Erreurs
    /// - `IpcError::Closed` — canal fermé
    /// - `IpcError::Timeout` — délai dépassé sans récepteur
    /// - `IpcError::WouldBlock` — NOWAIT positionné et aucun récepteur prêt
    /// - `IpcError::MessageTooLarge` — payload > MAX_MSG_SIZE
    pub fn send(&self, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        if data.len() > SYNC_INLINE_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        // Attendre que le slot soit libre (pas d'émetteur précédent)
        let cur = self.slot.state.load(Ordering::Acquire);
        if RendezVousState::from_u32(cur) != RendezVousState::Idle {
            if flags.contains(MsgFlags::NOWAIT) {
                return Err(IpcError::WouldBlock);
            }
            // Spin court, puis blocage réel via scheduler.
            let my_tid = crate::ipc::sync::sched_hooks::current_tid();
            self.slot.sender_tid.store(my_tid, Ordering::Relaxed);
            let mut spins: u32 = 0;
            loop {
                core::hint::spin_loop();
                spins += 1;
                let s = RendezVousState::from_u32(
                    self.slot.state.load(Ordering::Acquire),
                );
                if s == RendezVousState::Idle { break; }
                if self.is_closed() {
                    return Err(IpcError::Closed);
                }
                if spins > 64 {
                    // RÈGLE PREEMPT-BLOCK (B6) : bloquer avec PreemptGuard actif = deadlock garanti.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "SyncChannel::send: block_current() appelé avec PreemptGuard actif"
                    );
                    // Blocage réel — sera réveillé quand l'émetteur précédent termine.
                    // SAFETY: block_current() sûr si depth()==0 (debug_assert ci-dessus), my_tid valide.
                    unsafe { crate::ipc::sync::sched_hooks::block_current(my_tid); }
                    spins = 0;
                }
                if spins > 1_000_000 {
                    self.stats.sends_timeout.fetch_add(1, Ordering::Relaxed);
                    return Err(IpcError::Timeout);
                }
            }
        }

        // Déposer le message dans le slot
        let mid = alloc_message_id();
        self.slot.msg_id.store(mid.get(), Ordering::Relaxed);
        self.slot.payload_len.store(data.len(), Ordering::Relaxed);
        self.slot.flags.store(flags.bits(), Ordering::Relaxed);

        // SAFETY: seul l'émetteur écrit ici (protocole état Idle → SenderWaiting)
        unsafe {
            let dst = self.slot.data.get() as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }

        // Transition Idle → SenderWaiting (Release pour rendre les writes visibles)
        // Enregistrer le TID de l'émetteur avant la transition pour que le
        // récepteur puisse nous réveiller après avoir pris le message.
        let my_tid = crate::ipc::sync::sched_hooks::current_tid();
        self.slot.sender_tid.store(my_tid, Ordering::Relaxed);
        self.slot.state.store(
            RendezVousState::SenderWaiting as u32,
            Ordering::Release,
        );

        // Attendre ReceiverAcked ou Cancelled.
        // Stratégie : spin court (≤ 64 tours) puis blocage réel via scheduler.
        let timeout_ns = self.send_timeout_ns.load(Ordering::Relaxed);
        let mut spins: u64 = 0;
        let spin_hard_limit = if timeout_ns == 0 { u64::MAX } else { timeout_ns / 10 };

        loop {
            core::hint::spin_loop();
            spins += 1;
            let s = RendezVousState::from_u32(
                self.slot.state.load(Ordering::Acquire),
            );
            match s {
                RendezVousState::ReceiverAcked => {
                    // SAFETY: slot.state == ReceiverAcked, seul l'émetteur reset ici
                    unsafe { self.slot.reset() };
                    self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                    self.stats.total_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageSent);
                    return Ok(mid);
                }
                RendezVousState::Cancelled => {
                    // SAFETY: état Cancelled — seul l'émetteur réinitialise ici.
                    unsafe { self.slot.reset() };
                    self.stats.sends_cancelled.fetch_add(1, Ordering::Relaxed);
                    return Err(IpcError::Closed);
                }
                _ => {}
            }

            if self.is_closed() {
                self.slot.state.store(RendezVousState::Idle as u32, Ordering::Release);
                self.stats.sends_cancelled.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Closed);
            }

            if spins >= spin_hard_limit && timeout_ns != 0 {
                let _ = self.slot.state.compare_exchange(
                    RendezVousState::SenderWaiting as u32,
                    RendezVousState::Cancelled as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                self.stats.sends_timeout.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }

            // Après la phase de spin courte : blocage réel.
            if spins == 64 {
                // Vérifier une dernière fois avant de bloquer.
                let s2 = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
                if s2 == RendezVousState::ReceiverAcked || s2 == RendezVousState::Cancelled {
                    continue; // Sera traité à la prochaine itération
                }
                // RÈGLE PREEMPT-BLOCK (B6) : bloquer avec PreemptGuard actif = deadlock.
                debug_assert!(
                    crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                    "SyncChannel::send (attente ack): block_current() avec PreemptGuard actif"
                );
                // SAFETY: block_current() sûr si PreemptGuard::depth() == 0.
                unsafe { crate::ipc::sync::sched_hooks::block_current(my_tid); }
                spins = 0;
            }
        }
    }

    /// Envoi zero-copy : partage une page physique via ZeroCopyRef.
    pub fn send_zerocopy(&self, zc_ref: ZeroCopyRef, flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        let cur = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
        if cur != RendezVousState::Idle {
            if flags.contains(MsgFlags::NOWAIT) {
                return Err(IpcError::WouldBlock);
            }
        }

        let mid = alloc_message_id();
        self.slot.msg_id.store(mid.get(), Ordering::Relaxed);
        self.slot.payload_len.store(0, Ordering::Relaxed);
        let mut f = flags;
        f.insert(MsgFlags::ZEROCOPY);
        self.slot.flags.store(f.bits(), Ordering::Relaxed);

        // SAFETY: seul l'émetteur écrit zc_ref ici (état Idle)
        unsafe { *self.slot.zc_ref.get() = zc_ref; }

        // Enregistrer le TID avant la transition pour le réveil par le récepteur.
        let my_tid = crate::ipc::sync::sched_hooks::current_tid();
        self.slot.sender_tid.store(my_tid, Ordering::Relaxed);
        self.slot.state.store(
            RendezVousState::SenderWaiting as u32,
            Ordering::Release,
        );

        let mut spins: u64 = 0;
        loop {
            core::hint::spin_loop();
            spins += 1;
            let s = RendezVousState::from_u32(
                self.slot.state.load(Ordering::Acquire),
            );
            if s == RendezVousState::ReceiverAcked {
                // SAFETY: ReceiverAcked — seul l'émetteur réinitialise (protocole 1-émetteur).
                unsafe { self.slot.reset() };
                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                return Ok(mid);
            }
            if s == RendezVousState::Cancelled || self.is_closed() {
                // SAFETY: Cancelled / closed — même invariant propriétaire que ci-dessus.
                unsafe { self.slot.reset() };
                return Err(IpcError::Closed);
            }
            // Blocage réel après 64 itérations de spin.
            if spins == 64 {
                let s2 = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
                if s2 != RendezVousState::SenderWaiting {
                    continue;
                }
                // RÈGLE PREEMPT-BLOCK (B6) : bloquer avec PreemptGuard actif = deadlock.
                debug_assert!(
                    crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                    "SyncChannel::send_zerocopy: block_current() avec PreemptGuard actif"
                );
                // SAFETY: block_current() sûr si depth()==0 (debug_assert ci-dessus, send_zerocopy).
                unsafe { crate::ipc::sync::sched_hooks::block_current(my_tid); }
                spins = 0;
            }
            if spins > 2_000_000 {
                let _ = self.slot.state.compare_exchange(
                    RendezVousState::SenderWaiting as u32,
                    RendezVousState::Cancelled as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // RÉCEPTION — chemin récepteur
    // -----------------------------------------------------------------------

    /// Reçoit un message du canal.
    ///
    /// # Retour
    /// - `Ok((MessageId, longueur_copiée, MsgFlags))` — message reçu
    /// - `Err(IpcError::WouldBlock)` — aucun émetteur prêt (si NOWAIT)
    /// - `Err(IpcError::Closed)` — canal fermé
    pub fn recv(&self, buf: &mut [u8], flags: MsgFlags) -> Result<(MessageId, usize, MsgFlags), IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        // Attendre qu'un émetteur soit là.
        // Stratégie : spin court (≤ 64) puis blocage réel.
        if flags.contains(MsgFlags::NOWAIT) {
            let s = RendezVousState::from_u32(
                self.slot.state.load(Ordering::Acquire),
            );
            if s != RendezVousState::SenderWaiting {
                self.stats.recvs_would_block.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::WouldBlock);
            }
        } else {
            let my_tid = crate::ipc::sync::sched_hooks::current_tid();
            self.slot.receiver_tid.store(my_tid, Ordering::Relaxed);
            let mut spins: u32 = 0;
            loop {
                let s = RendezVousState::from_u32(
                    self.slot.state.load(Ordering::Acquire),
                );
                if s == RendezVousState::SenderWaiting { break; }
                if self.is_closed() {
                    return Err(IpcError::Closed);
                }
                core::hint::spin_loop();
                spins += 1;
                if spins == 64 {
                    // RÈGLE PREEMPT-BLOCK (B6) : bloquer avec PreemptGuard actif = deadlock.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "SyncChannel::recv: block_current() appelé avec PreemptGuard actif"
                    );
                    // SAFETY: block_current() sûr si depth()==0 (debug_assert ci-dessus, recv path).
                    unsafe { crate::ipc::sync::sched_hooks::block_current(my_tid); }
                    spins = 0;
                }
                if spins > 2_000_000 {
                    return Err(IpcError::Timeout);
                }
            }
        }

        // --- Lecture des métadonnées (Acquire assure la visibilité du store
        // précédent de l'émetteur) ---
        let msg_flags_raw = self.slot.flags.load(Ordering::Acquire);
        let msg_flags = MsgFlags::from_bits_truncate(msg_flags_raw);
        let len = self.slot.payload_len.load(Ordering::Relaxed);
        let mid_raw = self.slot.msg_id.load(Ordering::Relaxed);

        // SAFETY: NonZeroU64 garantie par alloc_message_id — l'émetteur a
        // écrit msg_id avant la transition vers SenderWaiting.
        let mid = unsafe { MessageId::new_unchecked(mid_raw) };

        if msg_flags.contains(MsgFlags::ZEROCOPY) {
            // Mode zero-copy : retourner la référence sans copie de données
            let copy_len = 0usize.min(buf.len());
            let sender_tid = self.slot.sender_tid.load(Ordering::Relaxed);
            self.slot.state.store(
                RendezVousState::ReceiverAcked as u32,
                Ordering::Release,
            );
            // Réveiller l'émetteur s'il est bloqué.
            if sender_tid != 0 {
                crate::ipc::sync::sched_hooks::wake_thread(sender_tid);
            }
            self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Ok((mid, copy_len, msg_flags));
        }

        // Copie inline
        let copy_len = len.min(buf.len());
        if copy_len > 0 {
            // SAFETY: émetteur terminé avant transition SenderWaiting (Release/Acquire); récepteur unique.
            unsafe {
                let src = self.slot.data.get() as *const u8;
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), copy_len);
            }
        }

        // Signaler l'émetteur — transition vers ReceiverAcked.
        let sender_tid = self.slot.sender_tid.load(Ordering::Relaxed);
        self.slot.state.store(
            RendezVousState::ReceiverAcked as u32,
            Ordering::Release,
        );
        // Réveiller l'émetteur s'il est bloqué dans le scheduler.
        if sender_tid != 0 {
            crate::ipc::sync::sched_hooks::wake_thread(sender_tid);
        }

        self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
        self.stats
            .total_bytes
            .fetch_add(copy_len as u64, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageReceived);
        Ok((mid, copy_len, msg_flags))
    }

    /// Reçoit une référence zero-copy depuis le canal.
    /// Retourne `Some(ZeroCopyRef)` si le message courant est en mode ZC.
    pub fn recv_zerocopy(&self) -> Option<ZeroCopyRef> {
        let s = RendezVousState::from_u32(
            self.slot.state.load(Ordering::Acquire),
        );
        if s != RendezVousState::SenderWaiting {
            return None;
        }
        let msg_flags_raw = self.slot.flags.load(Ordering::Relaxed);
        let msg_flags = MsgFlags::from_bits_truncate(msg_flags_raw);
        if !msg_flags.contains(MsgFlags::ZEROCOPY) {
            return None;
        }
        // SAFETY: état SenderWaiting et flag ZEROCOPY — l'émetteur a écrit zc_ref
        let zc = unsafe { *self.slot.zc_ref.get() };
        self.slot.state.store(
            RendezVousState::ReceiverAcked as u32,
            Ordering::Release,
        );
        self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageReceived);
        Some(zc)
    }

    // -----------------------------------------------------------------------
    // Utilitaires
    // -----------------------------------------------------------------------

    /// Retourne `true` si un émetteur est en attente de rendez-vous.
    #[inline]
    pub fn has_pending_sender(&self) -> bool {
        RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire))
            == RendezVousState::SenderWaiting
    }

    /// Snapshot des statistiques (copie atomique des compteurs).
    pub fn snapshot_stats(&self) -> SyncChannelSnapshot {
        SyncChannelSnapshot {
            sends_ok: self.stats.sends_ok.load(Ordering::Relaxed),
            sends_timeout: self.stats.sends_timeout.load(Ordering::Relaxed),
            sends_cancelled: self.stats.sends_cancelled.load(Ordering::Relaxed),
            recvs_ok: self.stats.recvs_ok.load(Ordering::Relaxed),
            recvs_would_block: self.stats.recvs_would_block.load(Ordering::Relaxed),
            total_bytes: self.stats.total_bytes.load(Ordering::Relaxed),
        }
    }

    // -----------------------------------------------------------------------
    // ENVOI CAP-CHECKED — IPC-04 (v6) : appel direct security::access_control
    // -----------------------------------------------------------------------

    /// Envoie avec vérification capability (RÈGLE IPC-04 v6).
    ///
    /// Appelle `security::access_control::check_access()` avant le vrai envoi.
    /// Points d'entrée syscall utilisent cette variante.
    ///
    /// # Droits requis : `Rights::IPC_SEND`
    #[inline]
    pub fn send_checked(
        &self,
        data:   &[u8],
        flags:  MsgFlags,
        table:  &CapTable,
        token:  CapToken,
    ) -> Result<MessageId, IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_SEND, "ipc::channel")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        self.send(data, flags)
    }

    /// Reçoit avec vérification capability (RÈGLE IPC-04 v6).
    ///
    /// # Droits requis : `Rights::IPC_RECV`
    #[inline]
    pub fn recv_checked(
        &self,
        buf:   &mut [u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<(MessageId, usize, MsgFlags), IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_RECV, "ipc::channel")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        self.recv(buf, flags)
    }
}

/// Snapshot immutable des statistiques
#[derive(Debug, Clone, Copy)]
pub struct SyncChannelSnapshot {
    pub sends_ok: u64,
    pub sends_timeout: u64,
    pub sends_cancelled: u64,
    pub recvs_ok: u64,
    pub recvs_would_block: u64,
    pub total_bytes: u64,
}

// ---------------------------------------------------------------------------
// Table statique globale de canaux synchrones
// ---------------------------------------------------------------------------

/// Capacité de la table globale de canaux synchrones
pub const SYNC_CHANNEL_TABLE_SIZE: usize = 512;

/// Table statique globale de canaux synchrones alloués
static SYNC_CHANNEL_TABLE: SpinLock<SyncChannelTable> =
    SpinLock::new(SyncChannelTable::new());

pub struct SyncChannelTable {
    slots: [MaybeUninit<SyncChannel>; SYNC_CHANNEL_TABLE_SIZE],
    used: [bool; SYNC_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: tous les accès sont protégés par SpinLock<SyncChannelTable>
unsafe impl Send for SyncChannelTable {}

impl SyncChannelTable {
    pub const fn new() -> Self {
        // SAFETY: mem::zeroed() évite la limite mémoire du const-eval pour grands tableaux.
        unsafe { core::mem::zeroed() }
    }

    /// Alloue un nouveau canal synchrone dans la table.
    /// Retourne l'index ou `None` si la table est pleine.
    pub fn alloc(&mut self) -> Option<usize> {
        for i in 0..SYNC_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(SyncChannel::new());
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    /// Libère un canal par index.
    pub fn free(&mut self, idx: usize) -> bool {
        if idx < SYNC_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] garantit que le slot est initialisé
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    /// Accès à un canal par index (reference non-mut).
    /// SAFETY: l'appelant doit s'assurer que used[idx] est true.
    pub unsafe fn get_unchecked(&self, idx: usize) -> &SyncChannel {
        self.slots[idx].assume_init_ref()
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Crée un nouveau canal synchrone et retourne son index dans la table globale.
pub fn sync_channel_create() -> Result<usize, IpcError> {
    let mut tbl = SYNC_CHANNEL_TABLE.lock();
    tbl.alloc().ok_or(IpcError::OutOfResources)
}

/// Envoie sur le canal synchrone identifié par `idx`.
pub fn sync_channel_send(idx: usize, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // SAFETY: used[idx] est true, vérifié ci-dessus sous spinlock
    // Extraire le pointeur brut AVANT drop(tbl) pour éviter le emprunt-après-move.
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    // On doit relâcher le lock avant d'attendre (rendezvous peut bloquer)
    drop(tbl);

    // Accès direct sans lock car SyncChannel est Sync
    // (l'accès concurrent est géré par les atomiques internes)
    // SAFETY: chan_ptr vit dans la table statique SYNC_CHANNEL_TABLE dont la durée
    // de vie est 'static. Le canal ne peut pas être libéré pendant un send
    // car free() nécessite aussi le SpinLock.
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.send(data, flags)
}

/// Reçoit depuis le canal synchrone identifié par `idx`.
pub fn sync_channel_recv(idx: usize, buf: &mut [u8], flags: MsgFlags)
    -> Result<(MessageId, usize, MsgFlags), IpcError>
{
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // SAFETY: used[idx] vérifié sous spinlock ci-dessus.
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    drop(tbl);

    // SAFETY: chan_ptr vit dans SYNC_CHANNEL_TABLE statique ('static).
    // Le canal ne peut pas être libéré pendant recv() car free() requiert le SpinLock.
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.recv(buf, flags)
}

/// Ferme le canal synchrone identifié par `idx`.
pub fn sync_channel_close(idx: usize) -> Result<(), IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // SAFETY: used[idx] vérifié sous spinlock ci-dessus.
    let chan = unsafe { tbl.get_unchecked(idx) };
    chan.close();
    drop(tbl);
    Ok(())
}

/// Détruit et libère le canal synchrone identifié par `idx`.
pub fn sync_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let mut tbl = SYNC_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Retourne le nombre de canaux synchrones actifs.
pub fn sync_channel_count() -> usize {
    SYNC_CHANNEL_TABLE.lock().count()
}

/// Envoie sur le canal synchrone `idx` avec vérification capability (IPC-04 v6).
///
/// Utilisé par la couche syscall. Les appels kernel-interne peuvent utiliser
/// `sync_channel_send()` directement.
pub fn sync_channel_send_checked(
    idx:   usize,
    data:  &[u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<MessageId, IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    drop(tbl);
    // SAFETY: même garanties que sync_channel_send().
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.send_checked(data, flags, table, token)
}

/// Reçoit depuis le canal synchrone `idx` avec vérification capability (IPC-04 v6).
pub fn sync_channel_recv_checked(
    idx:   usize,
    buf:   &mut [u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<(MessageId, usize, MsgFlags), IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    drop(tbl);
    // SAFETY: même garanties que sync_channel_recv().
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.recv_checked(buf, flags, table, token)
}
