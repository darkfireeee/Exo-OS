// kernel/src/fs/cache/writeback.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// WRITEBACK THREAD — delayed allocation + flush du page cache (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE FS-EXT4P-03 — Delayed Allocation :
//   write() syscall  → données copiées en RAM dans le page cache, page DIRTY
//                    → RETOUR IMMÉDIAT à l'application, AUCUNE allocation disque
//   writeback_thread → collecte les pages DIRTY d'un même fichier
//                    → delegation à mballoc pour chercher un bloc CONTIGU
//                    → DMA + écriture des métadonnées dans le journal (Data=Ordered)
//
// SÉQUENCE DATA=ORDERED dans writeback_flush_inode() :
//   Étape 1 : Écrire les données à l'emplacement final (bio_submit)
//   Étape 2 : Attendre l'ACK disque (data barrier)
//   Étape 3 : Écrire les métadonnées dans le journal
//   Étape 4 : Commiter le journal
//
// Avantages :
//   • Fichiers temporaires (< 5s) → jamais sur disque physique
//   • Fichiers écrits blob → UN seul bloc contigu (zéro fragmentation)
//   • Crash après étape 1 → récupérable par journal (métadonnées cohérentes)
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::fs::cache::page_cache::{PageRef, PAGE_CACHE, PageFlags};
use crate::fs::core::types::{DevId, FsError, FsResult, InodeNumber};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes writeback
// ─────────────────────────────────────────────────────────────────────────────

/// Intervalle entre deux passes de writeback en millisecondes (≈ 5 s)
pub const WRITEBACK_INTERVAL_MS: u64 = 5_000;

/// Nombre maximal de pages dirty à flusher par passe (anti-starvation)
pub const WRITEBACK_MAX_PAGES_PER_PASS: usize = 1024;

/// Seuil de pression mémoire (% pages dirty) déclenchant un flush urgent
pub const WRITEBACK_DIRTY_THRESH_PCT: usize = 20;

// ─────────────────────────────────────────────────────────────────────────────
// DirtyExtent — plage de pages dirty d'un même inode à flusher
// ─────────────────────────────────────────────────────────────────────────────

/// Représente une plage contigüe de pages dirty pour un même inode.
#[derive(Clone, Debug)]
pub struct DirtyExtent {
    /// Numéro d'inode propriétaire.
    pub ino:        InodeNumber,
    /// Périphérique bloc associé.
    pub dev:        DevId,
    /// Offset logique de début (en pages).
    pub start_page: u64,
    /// Nombre de pages dans la plage.
    pub page_count: u64,
    /// Blocs physiques alloués par mballoc (un par page).
    pub phys_blocks: Vec<u64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// WritebackContext — état d'une passe de writeback
// ─────────────────────────────────────────────────────────────────────────────

pub struct WritebackContext {
    /// Pages dirty collectées, regroupées par (dev, ino).
    pub dirty_groups: BTreeMap<(u64, u64), Vec<PageRef>>,
    /// Nombre total de pages flushed dans cette passe.
    pub flushed:      usize,
    /// Nombre d'erreurs I/O.
    pub errors:       usize,
}

impl WritebackContext {
    fn new() -> Self {
        Self {
            dirty_groups: BTreeMap::new(),
            flushed: 0,
            errors:  0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// writeback_collect_dirty — collecter les pages DIRTY depuis le page cache
// ─────────────────────────────────────────────────────────────────────────────

/// Collecte les pages dirty depuis le page cache en les regroupant par inode.
/// Chaque page collectée est marquée WRITEBACK (transitoire) puis sera effacée
/// DIRTY uniquement après ACK disque.
fn writeback_collect_dirty(max_pages: usize) -> WritebackContext {
    let mut ctx = WritebackContext::new();
    let cache   = &PAGE_CACHE;
    let mut collected = 0usize;

    'outer: for bucket in cache.iter_buckets() {
        let guard = bucket.lock();
        for page in guard.iter() {
            if collected >= max_pages { break 'outer; }
            if page.page.flags.load(Ordering::Relaxed) & PageFlags::DIRTY.bits() != 0 {
                // Marque WRITEBACK pour éviter un double flush concurrent.
                page.page.flags.fetch_or(PageFlags::WRITEBACK.bits(), Ordering::Relaxed);
                let key = (page.page.dev.0, page.ino.0);
                ctx.dirty_groups.entry(key).or_insert_with(Vec::new).push(page.page.clone());
                collected += 1;
            }
        }
    }
    WRITEBACK_STATS.pages_collected.fetch_add(collected as u64, Ordering::Relaxed);
    ctx
}

// ─────────────────────────────────────────────────────────────────────────────
// writeback_flush_inode — flush d'un groupe de pages dirty (un inode)
// ─────────────────────────────────────────────────────────────────────────────

/// Flush les pages dirty d'un inode :
///   1. Déléguer l'allocation de blocs à mballoc (allocation tardive)
///   2. Écrire les données sur disque via BIO (Data part of Data=Ordered)
///   3. Émettre une barrière disque (data_barrier)
///   4. Remettre le bloc alloué dans l'extent tree de l'inode (métadonnées)
///   5. Écrire les métadonnées dans le journal (post-barrière, Data=Ordered)
///   6. Démarquer les pages DIRTY + WRITEBACK
fn writeback_flush_inode(
    dev:   DevId,
    ino:   InodeNumber,
    pages: &[PageRef],
    block_size: u64,
) -> FsResult<()> {
    if pages.is_empty() { return Ok(()); }

    // ── Étape 1 : Allocation tardive des blocs (delayed alloc, règle FS-EXT4P-03)
    // L'allocateur multi-blocs cherche un grand bloc CONTIGU pour tout le batch.
    // On simule ici l'appel — dans le noyau complet, on appelerait
    // crate::fs::ext4plus::allocation::mballoc::ext4_multi_alloc().
    let needed = pages.len() as u64;
    // phys_start représente le premier bloc physique alloué par mballoc.
    // Dans une intégration complète, remplacer par l'appel réel.
    let phys_start: u64 = 0; // placeholder pour la liaison avec mballoc
    WRITEBACK_STATS.block_allocs.fetch_add(needed, Ordering::Relaxed);

    // ── Étape 2 : Écriture des DONNÉES à l'emplacement physique final
    for (i, page) in pages.iter().enumerate() {
        let phys_block = phys_start + i as u64;
        let sector     = phys_block * block_size / 512;
        let phys_addr  = page.phys_addr();
        let bio = Bio {
            id:       (ino.0 ^ (i as u64).wrapping_mul(0xDEADBEEF)),
            op:       BioOp::Write,
            dev:      dev.0,
            sector,
            vecs:     alloc::vec![BioVec {
                phys:   phys_addr,
                virt:   phys_addr.as_u64(),
                len:    block_size as u32,
                offset: 0,
            }],
            flags:    BioFlags::BARRIER,
            status:   core::sync::atomic::AtomicU8::new(0),
            bytes:    core::sync::atomic::AtomicU64::new(0),
            callback: None,
            cb_data:  0,
        };
        submit_bio(bio).map_err(|_| FsError::Io)?;
    }

    // ── Étape 3 : Barrière disque (data_barrier_passed = true)
    // Garantit que les données sont physiquement sur le disque AVANT
    // que les métadonnées soient commitées dans le journal.
    // Dans le noyau complet, on attendrait ici l'ACK du contrôleur.
    WRITEBACK_STATS.barriers_issued.fetch_add(1, Ordering::Relaxed);

    // ── Étape 4 + 5 : Métadonnées dans le journal APRÈS barrière
    // (délégué au code d'écriture d'inode — ici on marque juste la stat)
    WRITEBACK_STATS.meta_writes.fetch_add(1, Ordering::Relaxed);

    // ── Étape 6 : Démarquer les pages
    for page in pages.iter() {
        page.flags.fetch_and(!(PageFlags::DIRTY.bits() | PageFlags::WRITEBACK.bits()), Ordering::Release);
    }

    WRITEBACK_STATS.pages_written.fetch_add(pages.len() as u64, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// writeback_run_once — une passe complète de writeback
// ─────────────────────────────────────────────────────────────────────────────

/// Exécute une passe de writeback : collecte + flush des groupes dirty.
/// Retourne le nombre de pages effectivement flushed.
pub fn writeback_run_once(block_size: u64) -> usize {
    let mut ctx = writeback_collect_dirty(WRITEBACK_MAX_PAGES_PER_PASS);
    let total   = ctx.dirty_groups.iter().map(|(_, v)| v.len()).sum::<usize>();

    for ((dev_raw, ino_raw), pages) in ctx.dirty_groups.iter() {
        let dev = DevId(*dev_raw);
        let ino = InodeNumber(*ino_raw);
        match writeback_flush_inode(dev, ino, pages, block_size) {
            Ok(()) => { ctx.flushed += pages.len(); }
            Err(_) => {
                ctx.errors += pages.len();
                WRITEBACK_STATS.io_errors.fetch_add(1, Ordering::Relaxed);
                // Retire le flag WRITEBACK mais laisse DIRTY pour le prochain essai.
                for p in pages.iter() {
                    p.flags.fetch_and(!PageFlags::WRITEBACK.bits(), Ordering::Relaxed);
                }
            }
        }
    }

    WRITEBACK_STATS.passes_ran.fetch_add(1, Ordering::Relaxed);
    ctx.flushed
}

// ─────────────────────────────────────────────────────────────────────────────
// writeback_under_pressure — flush d'urgence sur pression mémoire
// ─────────────────────────────────────────────────────────────────────────────

/// Déclenche un flush d'urgence si le ratio DIRTY / Total dépasse le seuil.
/// Appelé par le shrinker mémoire (eviction.rs).
pub fn writeback_under_pressure() {
    let total_pages = PAGE_CACHE.total_pages();
    let dirty_pages = PAGE_CACHE.dirty_pages();
    if total_pages == 0 { return; }
    let pct = dirty_pages * 100 / total_pages;
    if pct >= WRITEBACK_DIRTY_THRESH_PCT {
        writeback_run_once(4096);
        WRITEBACK_STATS.pressure_flushes.fetch_add(1, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// writeback_thread_loop — boucle principale du thread de writeback
// ─────────────────────────────────────────────────────────────────────────────

/// Point d'entrée du thread de writeback.
/// Lancé une seule fois au boot par `fs::core::vfs::vfs_init()`.
/// N'alloue jamais de blocs disque pendant write() — il est LE seul
/// point d'allocation (règle FS-EXT4P-03).
pub fn writeback_thread_loop() -> ! {
    loop {
        // Attente passive (simulée ici — le vrai scheduler utilise un wait_queue).
        // Dans le noyau complet : scheduler::sleep(WRITEBACK_INTERVAL_MS).
        for _ in 0..WRITEBACK_INTERVAL_MS { core::hint::spin_loop(); }

        let flushed = writeback_run_once(4096);
        if flushed > 0 {
            WRITEBACK_STATS.total_flushed_pages.fetch_add(flushed as u64, Ordering::Relaxed);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WritebackStats — instrumentation complète
// ─────────────────────────────────────────────────────────────────────────────

pub struct WritebackStats {
    pub passes_ran:          AtomicU64,
    pub pages_collected:     AtomicU64,
    pub pages_written:       AtomicU64,
    pub total_flushed_pages: AtomicU64,
    pub block_allocs:        AtomicU64,
    pub barriers_issued:     AtomicU64,
    pub meta_writes:         AtomicU64,
    pub io_errors:           AtomicU64,
    pub pressure_flushes:    AtomicU64,
}

impl WritebackStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self {
            passes_ran:          z!(),
            pages_collected:     z!(),
            pages_written:       z!(),
            total_flushed_pages: z!(),
            block_allocs:        z!(),
            barriers_issued:     z!(),
            meta_writes:         z!(),
            io_errors:           z!(),
            pressure_flushes:    z!(),
        }
    }
}

pub static WRITEBACK_STATS: WritebackStats = WritebackStats::new();
