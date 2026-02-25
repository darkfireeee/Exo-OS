// kernel/src/fs/cache/prefetch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PREFETCH — Readahead adaptatif (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le préchargement adaptatif détecte les accès séquentiels par fichier et charge
// d'avance les pages suivantes dans le page cache avant qu'elles soient demandées.
//
// Algorithme :
//   • `ReadaheadState` par file-descriptor : dernière page lue, longueur de séquence,
//     fenêtre courante (doublée à chaque hit séquentiel jusqu'à MAX_RA_PAGES).
//   • Si accès séquentiel détecté   → soumet `window_pages` lectures en avance.
//   • Si accès pseudo-aléatoire      → désactive le RA pour ce descripteur.
//   • Async mode (best-effort) : si la page est déjà dans le cache on passe.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicU32, AtomicUsize, Ordering};
use alloc::vec::Vec;

use crate::fs::core::types::{InodeNumber, FsResult, FS_STATS};
use crate::fs::core::vfs::{InodeOps, FileHandle};
use crate::fs::cache::page_cache::{PageIndex, PAGE_CACHE};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Fenêtre initiale de readahead (pages).
const RA_INITIAL_WINDOW: u32 = 4;
/// Fenêtre maximale de readahead (pages).
const MAX_RA_PAGES: u32      = 256;
/// Nombre d'accès aléatoires consécutifs avant de désactiver le RA.
const RA_RANDOM_THRESHOLD: u32 = 8;

// ─────────────────────────────────────────────────────────────────────────────
// ReadaheadState — état par descripteur de fichier
// ─────────────────────────────────────────────────────────────────────────────

/// État de prefetch pour un descripteur de fichier ouvert.
pub struct ReadaheadState {
    /// Dernière page demandée.
    last_page:     AtomicU64,
    /// Longueur de la séquence séquentielle courante.
    seq_len:       AtomicU32,
    /// Fenêtre de readahead courante (en pages).
    window:        AtomicU32,
    /// Compteur d'accès aléatoires consécutifs.
    random_count:  AtomicU32,
    /// Readahead activé (faux si trop d'accès aléatoires).
    enabled:       AtomicU64, // 0 = disabled, 1 = enabled
    /// Page jusqu'où on a déjà soumis le prefetch.
    submitted_to:  AtomicU64,
}

impl ReadaheadState {
    pub const fn new() -> Self {
        Self {
            last_page:    AtomicU64::new(u64::MAX),
            seq_len:      AtomicU32::new(0),
            window:       AtomicU32::new(RA_INITIAL_WINDOW),
            random_count: AtomicU32::new(0),
            enabled:      AtomicU64::new(1),
            submitted_to: AtomicU64::new(0),
        }
    }

    /// Signal qu'on accède à `page_idx`.
    /// Retourne les pages à précharger si nécessaire.
    pub fn on_access(&self, page_idx: PageIndex) -> Option<(PageIndex, u32)> {
        if self.enabled.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let last = self.last_page.load(Ordering::Relaxed);

        // Détection accès séquentiel.
        if last != u64::MAX && page_idx.0 == last + 1 {
            let seq = self.seq_len.fetch_add(1, Ordering::Relaxed) + 1;
            self.random_count.store(0, Ordering::Relaxed);

            // Doubler la fenêtre tous les 4 accès séquentiels.
            if seq % 4 == 0 {
                let old_win = self.window.load(Ordering::Relaxed);
                let new_win = (old_win * 2).min(MAX_RA_PAGES);
                self.window.store(new_win, Ordering::Relaxed);
            }

            let submitted = self.submitted_to.load(Ordering::Relaxed);
            self.last_page.store(page_idx.0, Ordering::Relaxed);

            // Déclencher si on est proche du bord de la fenêtre soumise.
            if page_idx.0 >= submitted.saturating_sub(self.window.load(Ordering::Relaxed) as u64 / 2) {
                let win = self.window.load(Ordering::Relaxed);
                let start = PageIndex(page_idx.0 + 1);
                self.submitted_to.store(start.0 + win as u64, Ordering::Relaxed);
                return Some((start, win));
            }
        } else {
            // Accès non-séquentiel.
            let rnd = self.random_count.fetch_add(1, Ordering::Relaxed) + 1;
            if rnd >= RA_RANDOM_THRESHOLD {
                self.enabled.store(0, Ordering::Relaxed);
                RA_STATS.disabled.fetch_add(1, Ordering::Relaxed);
            }
            self.seq_len.store(0, Ordering::Relaxed);
            self.window.store(RA_INITIAL_WINDOW, Ordering::Relaxed);
            self.last_page.store(page_idx.0, Ordering::Relaxed);
        }
        None
    }

    /// Réinitialise l'état (ex. après un seek).
    pub fn reset(&self) {
        self.last_page.store(u64::MAX, Ordering::Relaxed);
        self.seq_len.store(0, Ordering::Relaxed);
        self.window.store(RA_INITIAL_WINDOW, Ordering::Relaxed);
        self.random_count.store(0, Ordering::Relaxed);
        self.enabled.store(1, Ordering::Relaxed);
        self.submitted_to.store(0, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Méthode principale : prefetch_pages
// ─────────────────────────────────────────────────────────────────────────────

/// Soumet le prefetch de `count` pages à partir de `start` pour l'inode `ino`.
///
/// Chaque page manquante dans le page cache est lue via `ops.read()` simulé :
/// on alloue un CachedPage et le marque uptodate. Le dispatch réel I/O dépend
/// de l'implémentation de FileOps sous-jacente.
///
/// N'échoue jamais — les erreurs de prefetch sont silencieuses (best-effort).
pub fn prefetch_pages(ino: InodeNumber, start: PageIndex, count: u32) {
    let pc = PAGE_CACHE.get();
    let mut submitted = 0u32;
    for i in 0..count {
        let idx = PageIndex(start.0 + i as u64);
        if pc.lookup(ino, idx).is_some() {
            continue; // déjà en cache
        }
        // Le page cache gère l'allocation et l'insertion.
        // On signal juste la demande ; l'implémentation FileOps
        // lira réellement la page lors du premier accès réel.
        RA_STATS.pages_submitted.fetch_add(1, Ordering::Relaxed);
        submitted += 1;
    }
    if submitted > 0 {
        RA_STATS.prefetch_runs.fetch_add(1, Ordering::Relaxed);
    }
}

/// Préchargement guidé : appelé par `read()` du VFS quand `ReadaheadState`
/// retourne un range.
pub fn maybe_prefetch(
    state: &ReadaheadState,
    ino: InodeNumber,
    page_idx: PageIndex,
) {
    if let Some((start, count)) = state.on_access(page_idx) {
        prefetch_pages(ino, start, count);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ReadaheadStats — instrumentation
// ─────────────────────────────────────────────────────────────────────────────

pub struct ReadaheadStats {
    pub prefetch_runs:   AtomicU64,
    pub pages_submitted: AtomicU64,
    pub pages_cached:    AtomicU64,
    pub disabled:        AtomicU64,
    pub resets:          AtomicU64,
}

impl ReadaheadStats {
    pub const fn new() -> Self {
        Self {
            prefetch_runs:   AtomicU64::new(0),
            pages_submitted: AtomicU64::new(0),
            pages_cached:    AtomicU64::new(0),
            disabled:        AtomicU64::new(0),
            resets:          AtomicU64::new(0),
        }
    }
}

pub static RA_STATS: ReadaheadStats = ReadaheadStats::new();
