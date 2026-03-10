// kernel/src/memory/utils/shrinker.rs
//
// Shrinker — table de rappels de shrinkage mémoire.
//
// Principe :
//   Chaque sous-système détenant un cache (slab, dcache, inode cache, DMA
//   pool, etc.) enregistre une `ShrinkerFn` qui, lorsqu'appelée, libère
//   `target_pages` pages si possible et retourne le nombre effectivement libéré.
//
//   Le chemin de basse mémoire (low-memory path de l'OOM killer ou du buddy)
//   appelle `run_shrinkers(target)` qui parcourt la table dans l'ordre
//   d'enregistrement et cumule les pages libérées.
//
// Design :
//   • SHRINKER_TABLE : [Option<ShrinkerEntry>; MAX_SHRINKERS] = 32 slots.
//   • Les shrinkers sont appelés dans l'ordre d'enregistrement (priority fifo).
//   • Pas de lock pendant l'exécution des callbacks : la table est write-once
//     (enregistrement uniquement à l'init) puis read-only pendant l'exploitation.
//
// COUCHE 0 — pas de dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Type fondamental
// ─────────────────────────────────────────────────────────────────────────────

/// Type d'un callback shrinker.
/// `target_pages` : nombre de pages à libérer.
/// Retourne : nombre de pages effectivement libérées.
pub type ShrinkerFn = fn(target_pages: u64) -> u64;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de shrinkers enregistrables.
pub const MAX_SHRINKERS: usize = 32;

// ─────────────────────────────────────────────────────────────────────────────
// Entrée shrinker
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans la table de shrinkers.
pub struct ShrinkerEntry {
    /// Callback à appeler.
    pub callback:    ShrinkerFn,
    /// Nom du sous-système (pour debug/stats), max 15 chars + NUL.
    pub name:        [u8; 16],
    /// L'entrée est active.
    pub active:      bool,
    /// Priorité : les shrinkers de haute priorité sont appelés en premier.
    /// Valeur plus petite = plus haute priorité.
    pub priority:    u8,
    /// Statistiques du shrinker.
    pub total_freed: AtomicU64,
    pub call_count:  AtomicU64,
}

impl ShrinkerEntry {
    fn new(callback: ShrinkerFn, name: &[u8], priority: u8) -> Self {
        let mut n = [0u8; 16];
        let len = name.len().min(15);
        n[..len].copy_from_slice(&name[..len]);
        Self {
            callback,
            name: n,
            active: true,
            priority,
            total_freed: AtomicU64::new(0),
            call_count:  AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for ShrinkerEntry {}

// ─────────────────────────────────────────────────────────────────────────────
// Table
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un shrinker enregistré.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShrinkerId(pub usize);

struct ShrinkerTableInner {
    entries: [Option<ShrinkerEntry>; MAX_SHRINKERS],
    count:   usize,
}

impl ShrinkerTableInner {
    const fn new() -> Self {
        const NONE: Option<ShrinkerEntry> = None;
        Self {
            entries: [NONE; MAX_SHRINKERS],
            count: 0,
        }
    }
}

static SHRINKER_TABLE: Mutex<ShrinkerTableInner> = Mutex::new(ShrinkerTableInner::new());

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ShrinkerStats {
    /// Nombre total d'appels `run_shrinkers`.
    pub run_count:      AtomicU64,
    /// Pages totales libérées par tous les shrinkers.
    pub pages_freed:    AtomicU64,
    /// Nombre de fois ou l'objectif n'a pas pu être atteint.
    pub goal_missed:    AtomicU64,
    /// Nombre de shrinkers enregistrés.
    pub registered:     AtomicUsize,
}

impl ShrinkerStats {
    const fn new() -> Self {
        Self {
            run_count:   AtomicU64::new(0),
            pages_freed: AtomicU64::new(0),
            goal_missed: AtomicU64::new(0),
            registered:  AtomicUsize::new(0),
        }
    }
}

unsafe impl Sync for ShrinkerStats {}
pub static SHRINKER_STATS: ShrinkerStats = ShrinkerStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre un shrinker.
/// `name` : slice d'octets du nom (jusqu'à 15).
/// `priority` : 0 = le plus prioritaire.
/// Retourne `Some(ShrinkerId)` ou `None` si table pleine.
pub fn register_shrinker(callback: ShrinkerFn, name: &[u8], priority: u8) -> Option<ShrinkerId> {
    let mut table = SHRINKER_TABLE.lock();
    if table.count >= MAX_SHRINKERS {
        return None;
    }
    for i in 0..MAX_SHRINKERS {
        if table.entries[i].is_none() {
            table.entries[i] = Some(ShrinkerEntry::new(callback, name, priority));
            table.count += 1;
            SHRINKER_STATS.registered.fetch_add(1, Ordering::Relaxed);
            return Some(ShrinkerId(i));
        }
    }
    None
}

/// Désenregistre un shrinker par son ID.
pub fn unregister_shrinker(id: ShrinkerId) -> bool {
    let mut table = SHRINKER_TABLE.lock();
    if id.0 >= MAX_SHRINKERS { return false; }
    if table.entries[id.0].is_some() {
        table.entries[id.0] = None;
        table.count -= 1;
        SHRINKER_STATS.registered.fetch_sub(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Exécute les shrinkers dans l'ordre de priorité croissante jusqu'à avoir
/// libéré `target_pages` pages ou épuisé tous les shrinkers.
///
/// Retourne le nombre total de pages libérées.
pub fn run_shrinkers(target_pages: u64) -> u64 {
    SHRINKER_STATS.run_count.fetch_add(1, Ordering::Relaxed);

    // Construire un snapshot trié par priorité pour ne pas maintenir le lock
    // pendant l'exécution des callbacks (qui peuvent allouer).
    let mut snapshot: [Option<(u8, ShrinkerFn, usize)>; MAX_SHRINKERS] = [None; MAX_SHRINKERS];
    let mut snap_len = 0;
    {
        let table = SHRINKER_TABLE.lock();
        for i in 0..MAX_SHRINKERS {
            if let Some(ref e) = table.entries[i] {
                if e.active {
                    snapshot[snap_len] = Some((e.priority, e.callback, i));
                    snap_len += 1;
                }
            }
        }
    }
    // Trier par priorité croissante.
    for i in 0..snap_len {
        for j in i + 1..snap_len {
            if let (Some(a), Some(b)) = (snapshot[i], snapshot[j]) {
                if b.0 < a.0 {
                    snapshot.swap(i, j);
                }
            }
        }
    }

    let mut total_freed = 0u64;
    let mut remaining   = target_pages;

    for k in 0..snap_len {
        if let Some((_, cb, idx)) = snapshot[k] {
            if remaining == 0 { break; }
            let freed = cb(remaining);
            total_freed  += freed;
            remaining     = remaining.saturating_sub(freed);

            // Mise à jour stats de l'entrée.
            let table = SHRINKER_TABLE.lock();
            if let Some(ref e) = table.entries[idx] {
                e.total_freed.fetch_add(freed, Ordering::Relaxed);
                e.call_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    SHRINKER_STATS.pages_freed.fetch_add(total_freed, Ordering::Relaxed);
    if total_freed < target_pages {
        SHRINKER_STATS.goal_missed.fetch_add(1, Ordering::Relaxed);
    }
    total_freed
}

/// Exécute les shrinkers avec un objectif minimal de `pages` pages ; ne s'arrête
/// pas avant d'avoir tout essayé.
pub fn shrink_all(pages: u64) -> u64 {
    run_shrinkers(pages)
}

// ─────────────────────────────────────────────────────────────────────────────
// Shrinkers built-in
// ─────────────────────────────────────────────────────────────────────────────

/// Shrinker de secours no-op (enregistré en priorité basse pour les tests).
fn nop_shrinker(_target: u64) -> u64 { 0 }

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Enregistrer le shrinker no-op comme dernier recours (priorité 255).
    register_shrinker(nop_shrinker, b"nop", 255);
}
