// kernel/src/ipc/ring/spsc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SPSC RING — Single-Producer Single-Consumer ultra-fast
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémentation d'une file FIFO lock-free SPSC avec barrières Release/Acquire.
// C'est le chemin le plus rapide pour l'IPC kernel-to-kernel ou thread-to-thread
// sur le même pair de comunicants.
//
// ALGORITHME :
//   • Producteur : lit tail, accède cell[tail % N], écrit données, avance tail.
//   • Consommateur : lit head, accède cell[head % N], lit données, avance head.
//   • Tail et Head sont sur des cache lines séparées (false-sharing éliminé).
//   • Séquences dans chaque slot évitent d'avoir besoin d'un lock global.
//
// PERFORMANCE CIBLE : > 50 millions de msgs/s en SPSC par canal @ 3 GHz.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering, fence};
use core::cell::UnsafeCell;
use crate::ipc::core::{
    IpcError, MsgFlags, MessageId, alloc_message_id,
    RING_SIZE, RING_MASK,
    array_index_nospec,
};
use crate::ipc::core::transfer::{RingSlot, MessageHeader};
use crate::ipc::core::{IpcFastMsg};
use super::slot::SlotCell;

// ─────────────────────────────────────────────────────────────────────────────
// Capacité SPSC — alias public de RING_SIZE
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de slots dans un ring SPSC (= RING_SIZE).
/// Re-exporté pour usage dans les modules message/ et channel/.
pub const SPSC_CAPACITY: usize = RING_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// SpscRing — la structure principale
// ─────────────────────────────────────────────────────────────────────────────

/// Cache line padding pour AtomicU64 — sépare tête et queue du ring.
/// AtomicU64 = 8 bytes, padding = 56 bytes → total 64 bytes = 1 cache line.
#[repr(C, align(64))]
struct CachePad(AtomicU64, [u8; 56]);

/// Ring SPSC avec head et tail sur des cache lines distinctes.
///
/// # Utilisation
/// ```
/// let ring = SpscRing::new();
/// ring.init();
/// // Dans le producteur :
/// ring.push_copy(&data, data.len(), MsgFlags::default())?;
/// // Dans le consommateur :
/// ring.pop_into(&mut buf)?;
/// ```
pub struct SpscRing {
    /// Position du prochain slot à écrire (cache line 0).
    head: CachePad,
    /// Position du prochain slot à lire (cache line 1).
    tail: CachePad,
    /// Slots du ring (16 MiB environ — alloués en SHM en production).
    cells: UnsafeCell<[SlotCell; RING_SIZE]>,
}

// SAFETY: accès aux cells régulé par le protocole head/tail/sequence.
unsafe impl Send for SpscRing {}
unsafe impl Sync for SpscRing {}

impl SpscRing {
    /// Construit un SpscRing — head et tail sur des cache lines séparées.
    /// Note: `CachePad` = AtomicU64(8B) + padding(56B) = 64B = 1 cache line.
    pub const fn new() -> Self {
        const INIT_CELL: SlotCell = SlotCell::new_at(0);
        Self {
            head:  CachePad(AtomicU64::new(0), [0u8; 56]),
            tail:  CachePad(AtomicU64::new(0), [0u8; 56]),
            cells: UnsafeCell::new([INIT_CELL; RING_SIZE]),
        }
    }

    /// Initialise les séquences de toutes les cellules.
    /// Doit être appelé une seule fois après construction.
    pub fn init(&self) {
        // SAFETY: init() est appelé une seule fois avant toute utilisation.
        let cells = unsafe { &mut *self.cells.get() };
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.sequence.store(i as u64, Ordering::Relaxed);
        }
        fence(Ordering::Release);
    }

    /// Retourne la cellule à la position `pos`.
    /// Utilise array_index_nospec (RÈGLE IPC-08 — Spectre v1).
    #[inline(always)]
    fn cell_at(&self, pos: u64) -> &SlotCell {
        let cells = unsafe { &*self.cells.get() };
        // array_index_nospec : empêche la spéculation CPU hors bornes (Spectre v1).
        let idx = array_index_nospec((pos as usize) & RING_MASK, RING_SIZE);
        &cells[idx]
    }

    // ───────────────────────── PRODUCTEUR ─────────────────────────────────

    /// Envoie un message (copie de données depuis slice).
    ///
    /// # Retour
    /// - `Ok(MessageId)`  : succès.
    /// - `Err(WouldBlock)`: ring plein (non bloquant).
    /// - `Err(MessageTooLarge)`: len > MAX_MSG_SIZE.
    #[inline]
    pub fn push_copy(
        &self,
        src:   &[u8],
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        if src.len() > crate::ipc::core::MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        let pos = self.head.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        let seq = cell.load_seq();

        // Slot libre si seq == pos (pas encore occupé).
        if seq != pos {
            // seq < pos → ring plein ; seq > pos → impossible en SPSC.
            return Err(IpcError::QueueFull);
        }

        let id = alloc_message_id();
        // SAFETY: seul producteur (SPSC); seq == pos prouve que le consommateur a libéré ce slot.
        unsafe {
            let slot = (*cell.slot.get()).assume_init_mut();
            slot.header = MessageHeader::new_inline(id, flags, src.len());
            if !src.is_empty() {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    slot.payload.as_mut_ptr(),
                    src.len(),
                );
            }
        }

        // Release : rend le slot visible au consommateur.
        cell.store_seq(pos + 1);
        self.head.0.store(pos + 1, Ordering::Relaxed);
        Ok(id)
    }

    /// Envoie un message zero-copy (données déjà dans un buffer partagé).
    /// `zc_ref` est encodé dans les 8 premiers bytes du payload.
    #[inline]
    pub fn push_zerocopy(
        &self,
        zc_ref: crate::ipc::core::transfer::ZeroCopyRef,
        flags:  MsgFlags,
    ) -> Result<MessageId, IpcError> {
        let pos = self.head.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        if cell.load_seq() != pos {
            return Err(IpcError::QueueFull);
        }
        let id = alloc_message_id();
        let f  = MsgFlags(flags.0 | MsgFlags::ZEROCOPY.0);
        // SAFETY: SPSC — seul producteur actif.
        unsafe {
            let slot = (*cell.slot.get()).assume_init_mut();
            slot.header = MessageHeader::new_zerocopy(id, f);
            core::ptr::copy_nonoverlapping(
                &zc_ref as *const _ as *const u8,
                slot.payload.as_mut_ptr(),
                core::mem::size_of::<crate::ipc::core::transfer::ZeroCopyRef>(),
            );
        }
        cell.store_seq(pos + 1);
        self.head.0.store(pos + 1, Ordering::Relaxed);
        Ok(id)
    }

    // ───────────────────────── CONSOMMATEUR ──────────────────────────────

    /// Reçoit un message dans `dst`. Retourne le nombre de bytes copiés.
    ///
    /// # Retour
    /// - `Ok((n, flags))` : n bytes copiés, drapeaux du message.
    /// - `Err(QueueEmpty)` : ring vide.
    #[inline]
    pub fn pop_into(&self, dst: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        let pos  = self.tail.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        let seq  = cell.load_seq();

        // Slot disponible si seq == pos + 1.
        if seq != pos + 1 {
            return Err(IpcError::QueueEmpty);
        }

        // SAFETY: seq == pos + 1 → produit et visible. SPSC → seul consommateur.
        let (header, len) = unsafe {
            let slot = (*cell.slot.get()).assume_init_ref();
            let h = slot.header;
            let ln = slot.header.len as usize;
            if ln > dst.len() {
                // Libérer le slot quand même pour ne pas bloquer le ring.
                cell.store_seq(pos + RING_SIZE as u64);
                self.tail.0.store(pos + 1, Ordering::Relaxed);
                return Err(IpcError::MessageTooLarge);
            }
            if ln > 0 {
                core::ptr::copy_nonoverlapping(
                    slot.payload.as_ptr(),
                    dst.as_mut_ptr(),
                    ln,
                );
            }
            (h, ln)
        };
        let _ = header;

        // Libère le slot : pos + RING_SIZE = prochain tour du producteur.
        cell.store_seq(pos + RING_SIZE as u64);
        self.tail.0.store(pos + 1, Ordering::Relaxed);
        Ok((len, MsgFlags(header.flags)))
    }

    /// Inspecte le prochain message sans le consommer (peek).
    /// Retourne l'en-tête sans copier le payload.
    #[inline]
    pub fn peek_header(&self) -> Option<MessageHeader> {
        let pos  = self.tail.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        if cell.load_seq() != pos + 1 {
            return None;
        }
        // SAFETY: seq == pos + 1 → données disponibles.
        let h = unsafe { (*cell.slot.get()).assume_init_ref().header };
        Some(h)
    }

    /// Retourne vrai si le ring est vide.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        let pos  = self.tail.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        cell.load_seq() != pos + 1
    }

    /// Retourne vrai si le ring est plein.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        let pos  = self.head.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        cell.load_seq() != pos
    }

    /// Nombre approximatif de messages en attente.
    #[inline(always)]
    pub fn len_approx(&self) -> usize {
        let h = self.head.0.load(Ordering::Relaxed);
        let t = self.tail.0.load(Ordering::Relaxed);
        h.wrapping_sub(t) as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions no_mangle appelées depuis fastcall_asm.s
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale de rings SPSC actifs (indexée par channel_id % MAX_SPSC_RINGS).
/// En production, remplacée par une structure allouée dynamiquement depuis
/// le pool SHM. Ici : table statique pour le bootstrap et les tests.
const MAX_SPSC_RINGS: usize = 256;

static SPSC_RINGS: [SpscRing; MAX_SPSC_RINGS] = {
    const ZERO: SpscRing = SpscRing::new();
    [ZERO; MAX_SPSC_RINGS]
};

static SPSC_INIT: AtomicU64 = AtomicU64::new(0);

/// Initialise tous les rings de la table statique (appelé au boot IPC).
pub fn init_spsc_rings() {
    if SPSC_INIT.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
        for ring in &SPSC_RINGS {
            ring.init();
        }
    }
}

/// Accède au ring correspondant à un channel_id.
/// array_index_nospec (RÈGLE IPC-08) : empêche la spéculation hors-bornes.
#[inline(always)]
fn ring_for(channel_id: u64) -> &'static SpscRing {
    let idx = array_index_nospec((channel_id as usize) % MAX_SPSC_RINGS, MAX_SPSC_RINGS);
    &SPSC_RINGS[idx]
}

/// Écriture rapide pour fastcall_asm.s.
///
/// # Safety
/// `msg` doit être un pointeur valide vers une `IpcFastMsg`.
pub unsafe fn spsc_fast_write(msg: *const IpcFastMsg, channel_id: u64) -> u64 {
    let m = &*msg;
    let len = m.len as usize;
    let flags = MsgFlags(m.flags);
    let ring = ring_for(channel_id);

    match ring.push_copy(&m.data[..len.min(64)], flags) {
        Ok(_)  => 0,
        Err(e) => e as u64,
    }
}

/// Lecture rapide pour fastcall_asm.s.
///
/// # Safety
/// `dst` doit être un pointeur valide vers une `IpcFastMsg`.
pub unsafe fn spsc_fast_read(dst: *mut IpcFastMsg, channel_id: u64) -> u64 {
    let m = &mut *dst;
    let ring = ring_for(channel_id);
    let buf = &mut m.data[..];

    match ring.pop_into(buf) {
        Ok(n)  => { m.len = n.0 as u16; 0 },
        Err(e) => e as u64,
    }
}

/// Attente de réponse (polling loop + yield) pour fast call.
///
/// # Safety
/// `dst` doit être un pointeur valide vers une `IpcFastMsg`.
pub unsafe fn spsc_wait_reply(
    dst:        *mut IpcFastMsg,
    channel_id: u64,
    timeout_ns: u64,
) -> u64 {
    // Polling avec compteur de spin avant yield.
    const SPIN_LIMIT: u64 = 10_000;
    let ring = ring_for(channel_id);
    let m    = &mut *dst;
    let buf  = &mut m.data[..];

    let mut spins: u64 = 0;
    loop {
        match ring.pop_into(buf) {
            Ok(n) => { m.len = n.0 as u16; return 0; },
            Err(IpcError::QueueEmpty) => {}
            Err(e) => return e as u64,
        }
        spins += 1;
        if spins > SPIN_LIMIT {
            // Yield au scheduler pour laisser d'autres threads tourner.
            // SAFETY: appel kernel safe depuis contexte kernel.
            extern "C" { fn arch_cpu_relax(); }
            arch_cpu_relax();
        }
        if timeout_ns > 0 {
            // Vérification timeout simplifiée.
            // En production, lire le TSC et comparer.
            if spins > timeout_ns / 100 {
                return IpcError::Timeout as u64;
            }
        }
        core::hint::spin_loop();
    }
}
