// kernel/src/fs/exofs/core/epoch_id.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EpochId — compteur monotone, machine d'états, arithmétique d'epoch
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES :
//   EPOCH-01 : EpochId monotone croissant, jamais décroissant.
//   EPOCH-02 : Valeur 0 = invalide. Le premier epoch valide = 1.
//   EPOCH-03 : Overflow u64 → panic (jamais silencieux, wrap interdit).
//   EPOCH-04 : Transitions d'état strictement séquentielles.
//   EPOCH-05 : GC ne peut collecter qu'un epoch dans état GcEligible.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::types::EpochId;

// ─────────────────────────────────────────────────────────────────────────────
// Compteur global monotone des epochs
// ─────────────────────────────────────────────────────────────────────────────

/// Compteur global du prochain EpochId à allouer.
/// Initialisé à 1 (0 = invalide, règle EPOCH-02).
static EPOCH_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Alloue et retourne le prochain EpochId valide (incrémente atomiquement).
///
/// # Panics
/// Panique si le compteur atteint u64::MAX (règle EPOCH-03).
#[inline]
pub fn next_epoch_id() -> EpochId {
    let prev = EPOCH_COUNTER.fetch_add(1, Ordering::SeqCst);
    if prev == u64::MAX {
        panic!("exofs: EpochId counter overflow — filesystem doit être reformaté");
    }
    EpochId(prev)
}

/// Retourne l'EpochId courant sans l'incrémenter.
#[inline]
pub fn current_epoch_id() -> EpochId {
    EpochId(EPOCH_COUNTER.load(Ordering::Relaxed).saturating_sub(1).max(1))
}

/// Restaure le compteur d'epoch au boot depuis la valeur persistée sur disque.
///
/// # Safety
/// Doit être appelé UNE SEULE FOIS pendant le montage, avant toute allocation.
/// Aucun thread ne doit allouer des EpochIds en parallèle.
pub fn restore_epoch_counter(last_committed: EpochId) {
    let next = last_committed.0.saturating_add(1);
    // Ne jamais régresser sous la valeur courante (sécurité).
    let cur = EPOCH_COUNTER.load(Ordering::Relaxed);
    if next > cur {
        EPOCH_COUNTER.store(next, Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochState — machine d'états du cycle de vie d'un epoch
// ─────────────────────────────────────────────────────────────────────────────

/// État du cycle de vie d'un Epoch.
///
/// Transitions autorisées :
///   Open → Committing → Committed → GcEligible → GcPending → Collected
///   Open → Aborted
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EpochState {
    /// Epoch ouvert : transactions en cours d'accumulation.
    Open        = 0,
    /// Commit en cours (3 barrières NVMe en vol) — écriture EPOCH_LOCK.
    Committing  = 1,
    /// Epoch durci sur disque avec les 3 barrières NVMe.
    Committed   = 2,
    /// Délai GC ecoulé (>= GC_MIN_EPOCH_DELAY epochs plus récents existent).
    GcEligible  = 3,
    /// GC a marqué l'epoch pour collecte — work item en file.
    GcPending   = 4,
    /// Tous les P-Blobs de l'epoch ont été libérés.
    Collected   = 5,
    /// Commit annulé (panique, perte de courant pendant Committing).
    Aborted     = 6,
}

impl EpochState {
    /// Convertit depuis la valeur on-disk (u8).
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Open),
            1 => Some(Self::Committing),
            2 => Some(Self::Committed),
            3 => Some(Self::GcEligible),
            4 => Some(Self::GcPending),
            5 => Some(Self::Collected),
            6 => Some(Self::Aborted),
            _ => None,
        }
    }

    /// Sérialisation on-disk.
    #[inline]
    pub fn as_u8(self) -> u8 { self as u8 }

    /// Retourne vrai si l'epoch est dans un état terminal (Collected ou Aborted).
    #[inline]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Collected | Self::Aborted)
    }

    /// Retourne vrai si l'epoch est lisible (committed ou plus récent).
    #[inline]
    pub fn is_readable(self) -> bool {
        matches!(self, Self::Committed | Self::GcEligible | Self::GcPending)
    }

    /// Retourne vrai si l'epoch peut être commité.
    #[inline]
    pub fn can_commit(self) -> bool {
        matches!(self, Self::Open)
    }

    /// Retourne vrai si le GC peut collecter cet epoch.
    #[inline]
    pub fn is_gc_eligible(self) -> bool {
        matches!(self, Self::GcEligible | Self::GcPending)
    }

    /// Applique la transition demandée. Retourne Err si illégale.
    pub fn transition(self, to: Self) -> Result<Self, &'static str> {
        let ok = match (self, to) {
            (Self::Open,       Self::Committing)  => true,
            (Self::Open,       Self::Aborted)     => true,
            (Self::Committing, Self::Committed)   => true,
            (Self::Committing, Self::Aborted)     => true,
            (Self::Committed,  Self::GcEligible)  => true,
            (Self::GcEligible, Self::GcPending)   => true,
            (Self::GcPending,  Self::Collected)   => true,
            _                                     => false,
        };
        if ok { Ok(to) } else { Err("illegal epoch state transition") }
    }
}

impl core::fmt::Display for EpochState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Open       => write!(f, "Open"),
            Self::Committing => write!(f, "Committing"),
            Self::Committed  => write!(f, "Committed"),
            Self::GcEligible => write!(f, "GcEligible"),
            Self::GcPending  => write!(f, "GcPending"),
            Self::Collected  => write!(f, "Collected"),
            Self::Aborted    => write!(f, "Aborted"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochStats — statistiques d'un epoch individuel
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques collectées pendant la vie d'un epoch.
///
/// Stockées en RAM uniquement — jamais persistées on-disk directement.
/// Le résumé est inclus dans l'EpochRecord lors du commit.
#[derive(Copy, Clone, Debug, Default)]
pub struct EpochStats {
    /// Nombre d'objets créés dans cet epoch.
    pub objects_created: u32,
    /// Nombre d'objets supprimés (soft-delete) dans cet epoch.
    pub objects_deleted: u32,
    /// Nombre de blobs alloués dans cet epoch.
    pub blobs_allocated: u32,
    /// Nombre de blobs libérés (décréments ref → 0) dans cet epoch.
    pub blobs_freed: u32,
    /// Octets écrits dans cet epoch (données brutes).
    pub bytes_written: u64,
    /// Nombre de relations créées dans cet epoch.
    pub relations_created: u32,
    /// Nombre de snapshots créés dans cet epoch.
    pub snapshots_created: u16,
    /// Nombre de paths modifiés dans cet epoch.
    pub paths_modified: u32,
    /// Tick kernel au début de l'epoch (pour durée commit).
    pub open_tick: u64,
    /// Tick kernel à la fin du commit.
    pub commit_tick: u64,
}

impl EpochStats {
    pub const fn new(open_tick: u64) -> Self {
        let mut s = Self { open_tick, ..EpochStats {
            objects_created: 0, objects_deleted: 0, blobs_allocated: 0,
            blobs_freed: 0, bytes_written: 0, relations_created: 0,
            snapshots_created: 0, paths_modified: 0, open_tick: 0, commit_tick: 0,
        }};
        s.open_tick = open_tick;
        s
    }

    /// Durée du commit en ticks.
    #[inline]
    pub fn commit_duration_ticks(&self) -> u64 {
        self.commit_tick.saturating_sub(self.open_tick)
    }

    /// Vrai si aucune modification n'a eu lieu (epoch vide).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.objects_created == 0
            && self.objects_deleted == 0
            && self.blobs_allocated == 0
            && self.bytes_written == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochRange — itérateur d'intervals d'epochs
// ─────────────────────────────────────────────────────────────────────────────

/// Intervalle inclusif d'EpochIds [start, end].
///
/// Utilisé par le GC, le recovery et les snapshots pour itérer
/// sur des plages d'epochs sans collection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EpochRange {
    pub start: EpochId,
    pub end:   EpochId,
}

impl EpochRange {
    /// Crée un EpochRange [start, end] (inclusif des deux bornes).
    ///
    /// # Panics
    /// Panique si start > end (intervalle inversé = bug).
    #[inline]
    pub fn new(start: EpochId, end: EpochId) -> Self {
        assert!(start.0 <= end.0, "EpochRange: start > end");
        Self { start, end }
    }

    /// Crée une plage d'un seul epoch.
    #[inline]
    pub fn single(id: EpochId) -> Self {
        Self { start: id, end: id }
    }

    /// Nombre d'epochs dans la plage (peut dépasser usize sur archs 32 bits).
    #[inline]
    pub fn count(&self) -> u64 {
        self.end.0.saturating_sub(self.start.0).saturating_add(1)
    }

    /// Vrai si l'epoch est dans la plage.
    #[inline]
    pub fn contains(&self, id: EpochId) -> bool {
        id.0 >= self.start.0 && id.0 <= self.end.0
    }

    /// Vrai si les deux plages se chevauchent.
    #[inline]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start.0 <= other.end.0 && other.start.0 <= self.end.0
    }

    /// Intersection de deux plages. None si disjointes.
    #[inline]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let s = self.start.0.max(other.start.0);
        let e = self.end.0.min(other.end.0);
        if s > e { None } else { Some(Self { start: EpochId(s), end: EpochId(e) }) }
    }

    /// Étend la plage pour inclure `id`.
    #[inline]
    pub fn extend_to(&mut self, id: EpochId) {
        if id.0 < self.start.0 { self.start = id; }
        if id.0 > self.end.0   { self.end   = id; }
    }

    /// Itérateur simple (retourne tous les EpochIds comme u64).
    pub fn iter_u64(&self) -> impl Iterator<Item = u64> {
        self.start.0..=self.end.0
    }
}

impl core::fmt::Display for EpochRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[{}..={}]", self.start.0, self.end.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires d'EpochId
// ─────────────────────────────────────────────────────────────────────────────

/// Comparateur d'epochs : retourne l'epoch la plus récente.
#[inline]
pub fn max_epoch(a: EpochId, b: EpochId) -> EpochId {
    if a.0 >= b.0 { a } else { b }
}

/// Comparateur d'epochs : retourne l'epoch la plus ancienne.
#[inline]
pub fn min_epoch(a: EpochId, b: EpochId) -> EpochId {
    if a.0 <= b.0 { a } else { b }
}

/// Vérifie si deux epochs peuvent coexister sans risque de wrap u64.
///
/// Distance > u64::MAX/2 → probable wrap ou données corrompues (règle EPOCH-03).
#[inline]
pub fn epoch_distance_sane(old: EpochId, new: EpochId) -> bool {
    if new.0 < old.0 { return false; }
    new.0 - old.0 < (u64::MAX / 2)
}

/// Retourne vrai si l'epoch `query` est dans la fenêtre [base, base+window).
#[inline]
pub fn epoch_in_window(base: EpochId, window: u64, query: EpochId) -> bool {
    query.0 >= base.0 && query.0 < base.0.saturating_add(window)
}

/// Retourne le nombre d'epochs entre `old` et `new` (signe positif = avance).
///
/// Retourne None si la distance dépasse u64::MAX/2 (probable corruption).
#[inline]
pub fn epoch_distance(old: EpochId, new: EpochId) -> Option<u64> {
    if new.0 < old.0 { return None; }
    let d = new.0 - old.0;
    if d >= u64::MAX / 2 { None } else { Some(d) }
}

/// Vrai si `candidate` est au moins `min_delay` epochs après `reference`.
///
/// Utilisé par le GC pour vérifier la règle GC_MIN_EPOCH_DELAY (RÈGLE GC-02).
#[inline]
pub fn epoch_gc_eligible(reference: EpochId, candidate: EpochId, min_delay: u64) -> bool {
    candidate.0 >= reference.0.saturating_add(min_delay)
}

/// Retourne l'EpochId précédent, ou INVALID si déjà à 1.
#[inline]
pub fn epoch_prev(id: EpochId) -> EpochId {
    if id.0 <= 1 { EpochId::INVALID } else { EpochId(id.0 - 1) }
}

/// Clamp un EpochId entre two bornes (utile pour le parcours de journaux).
#[inline]
pub fn epoch_clamp(id: EpochId, lo: EpochId, hi: EpochId) -> EpochId {
    if id.0 < lo.0 { lo }
    else if id.0 > hi.0 { hi }
    else { id }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochCommitSummary — résumé compact pour persistance
// ─────────────────────────────────────────────────────────────────────────────

/// Résumé compact d'un epoch committé — inclus dans l'EpochRecord on-disk.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct EpochCommitSummary {
    pub epoch_id:        u64,
    pub objects_delta:   i32,   // = created - deleted (peut être négatif)
    pub bytes_written:   u64,
    pub blobs_freed:     u32,
    pub commit_duration: u32,   // ticks (saturé à u32::MAX)
    pub state:           u8,    // EpochState as u8
    pub flags:           u8,    // EpochFlags
    pub _pad:            [u8; 6],
}

const _: () = assert!(core::mem::size_of::<EpochCommitSummary>() == 32);

impl EpochCommitSummary {
    pub fn from_stats(epoch_id: EpochId, stats: &EpochStats, state: EpochState, flags: u8) -> Self {
        let objects_delta = (stats.objects_created as i64 - stats.objects_deleted as i64)
            .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let commit_duration = stats.commit_duration_ticks()
            .min(u32::MAX as u64) as u32;
        Self {
            epoch_id: epoch_id.0,
            objects_delta,
            bytes_written: stats.bytes_written,
            blobs_freed:   stats.blobs_freed,
            commit_duration,
            state: state as u8,
            flags,
            _pad: [0; 6],
        }
    }

    /// Retourne l'EpochId encapsulé.
    #[inline]
    pub fn epoch_id(&self) -> EpochId { EpochId(self.epoch_id) }

    /// Retourne l'état de l'epoch depuis le résumé.
    #[inline]
    pub fn state(&self) -> Option<EpochState> { EpochState::from_u8(self.state) }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochWindow — fenêtre glissante sur les epochs actifs
// ─────────────────────────────────────────────────────────────────────────────

/// Fenêtre glissante sur les N derniers epochs (pour le GC et la rétention).
///
/// Maintient [oldest_pinned .. current] comme la fenêtre des epochs actifs.
/// Tout epoch en dehors de cette fenêtre est candidat au GC.
#[derive(Copy, Clone, Debug)]
pub struct EpochWindow {
    /// Premier epoch encore référencé (ancré par un snapshot ou un lecteur actif).
    pub oldest_pinned: u64,
    /// Epoch actuel (epoch en cours de construction).
    pub current:       u64,
    /// Taille maximale admise de la fenêtre (configuration).
    pub max_window:    u32,
}

impl EpochWindow {
    /// Crée une fenêtre initiale avec un seul epoch (boot).
    pub fn new(initial_epoch: u64, max_window: u32) -> Self {
        Self { oldest_pinned: initial_epoch, current: initial_epoch, max_window }
    }

    /// Avance la fenêtre vers un nouvel epoch courant.
    pub fn advance(&mut self, new_current: u64) {
        if new_current > self.current {
            self.current = new_current;
        }
        // Nettoie oldest_pinned si la fenêtre devient trop grande.
        let window_size = self.current.saturating_sub(self.oldest_pinned);
        if window_size > self.max_window as u64 {
            self.oldest_pinned = self.current.saturating_sub(self.max_window as u64);
        }
    }

    /// Épingle un epoch (l'empêche d'être collecté).
    ///
    /// Si `epoch` est plus ancien que oldest_pinned, met à jour oldest_pinned.
    pub fn pin(&mut self, epoch: u64) {
        if epoch < self.oldest_pinned {
            self.oldest_pinned = epoch;
        }
    }

    /// Retourne la taille de la fenêtre actuelle en epochs.
    pub fn size(&self) -> u64 {
        self.current.saturating_sub(self.oldest_pinned)
    }

    /// Vrai si un epoch est dans la fenêtre active (non candidat GC).
    pub fn is_active(&self, epoch: u64) -> bool {
        epoch >= self.oldest_pinned && epoch <= self.current
    }

    /// Vrai si un epoch est candidat au GC (en dehors de la fenêtre).
    pub fn is_gc_candidate(&self, epoch: u64) -> bool {
        epoch < self.oldest_pinned
    }

    /// Vrai si la fenêtre est à sa capacité maximale.
    pub fn is_full(&self) -> bool {
        self.size() >= self.max_window as u64
    }
}

// ─────────────────────────────────────────────────────────────────────────────
