// kernel/src/memory/dma/stats/counters.rs
//
// Compteurs de statistiques DMA — débit, latence, erreurs, timeouts.
// Couche 0 — aucune dépendance externe sauf `core`.
//
// Chaque moteur DMA dispose de son propre jeu de compteurs, plus un
// compteur global agrégé. Tous les accès sont atomiques (relaxed pour
// les compteurs de perf, AcqRel pour les erreurs fatales).

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES PAR MOTEUR
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques d'un moteur DMA individuel.
#[repr(C, align(64))]
pub struct DmaEngineStats {
    /// Nombre de transferts soumis.
    pub submitted:    AtomicU64,
    /// Nombre de transferts complétés avec succès.
    pub completed:    AtomicU64,
    /// Nombre d'erreurs hard (ECC, timeout IOMMU, etc.).
    pub errors:       AtomicU64,
    /// Nombre de timeouts (annulation forcée).
    pub timeouts:     AtomicU64,
    /// Octets transférés (DMA → RAM ou RAM → device).
    pub bytes_tx:     AtomicU64,
    /// Octets reçus (device → RAM).
    pub bytes_rx:     AtomicU64,
    /// Latence cumulée en cycles TSC (pour calcul de moyenne).
    pub latency_sum_cycles: AtomicU64,
    /// Latence max observée en cycles TSC.
    pub latency_max_cycles: AtomicU64,
    /// Nombre de transactions en flight (en cours).
    pub in_flight:    AtomicU32,
    /// Nombre de re-soumissions (retry après erreur transitoire).
    pub retries:      AtomicU32,
}

impl DmaEngineStats {
    pub const fn new() -> Self {
        DmaEngineStats {
            submitted:          AtomicU64::new(0),
            completed:          AtomicU64::new(0),
            errors:             AtomicU64::new(0),
            timeouts:           AtomicU64::new(0),
            bytes_tx:           AtomicU64::new(0),
            bytes_rx:           AtomicU64::new(0),
            latency_sum_cycles: AtomicU64::new(0),
            latency_max_cycles: AtomicU64::new(0),
            in_flight:          AtomicU32::new(0),
            retries:            AtomicU32::new(0),
        }
    }

    /// Enregistre la soumission d'un transfert.
    #[inline]
    pub fn on_submit(&self) {
        self.submitted.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre la complétion d'un transfert.
    ///
    /// `bytes`      : nombre d'octets transférés.
    /// `is_write`   : true = DMA depuis RAM (TX), false = vers RAM (RX).
    /// `latency_cy` : latence en cycles TSC.
    #[inline]
    pub fn on_complete(&self, bytes: u64, is_write: bool, latency_cy: u64) {
        self.completed.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        if is_write { self.bytes_tx.fetch_add(bytes, Ordering::Relaxed); }
        else        { self.bytes_rx.fetch_add(bytes, Ordering::Relaxed); }
        self.latency_sum_cycles.fetch_add(latency_cy, Ordering::Relaxed);
        // Mise à jour latence max (compare-and-swap loop).
        let mut cur = self.latency_max_cycles.load(Ordering::Relaxed);
        while latency_cy > cur {
            match self.latency_max_cycles.compare_exchange_weak(
                cur, latency_cy, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_)  => break,
                Err(v) => cur = v,
            }
        }
    }

    /// Enregistre une erreur de transfert.
    #[inline]
    pub fn on_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Enregistre un timeout.
    #[inline]
    pub fn on_timeout(&self) {
        self.timeouts.fetch_add(1, Ordering::AcqRel);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Enregistre une re-soumission.
    #[inline]
    pub fn on_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// Latence moyenne en cycles (0 si aucune complétion).
    pub fn avg_latency_cycles(&self) -> u64 {
        let completed = self.completed.load(Ordering::Relaxed);
        if completed == 0 { return 0; }
        self.latency_sum_cycles.load(Ordering::Relaxed) / completed
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES GLOBALES DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de moteurs DMA trackés.
pub const MAX_DMA_ENGINES: usize = 16;

/// Statistiques globales DMA, agrégées sur tous les moteurs.
pub struct DmaStats {
    /// Statistiques par moteur (index = engine_id).
    pub engines:    [DmaEngineStats; MAX_DMA_ENGINES],
    /// Nombre de moteurs actifs.
    pub active_engines: AtomicU32,
    /// Compteur global d'erreurs fatales IOMMU.
    pub iommu_faults:   AtomicU64,
    /// Nombre de TLB flush IOMMU effectués.
    pub iommu_tlb_flushes: AtomicU64,
    /// Nombre de domain resets.
    pub domain_resets:  AtomicU64,
}

// SAFETY: DmaStats n'est accessible que via des méthodes atomiques.
unsafe impl Sync for DmaStats {}

impl DmaStats {
    const fn new() -> Self {
        // Impossible de faire new() dans un array en const sans macro dans no_std.
        // On utilise un transmute-from-zeros approach.
        DmaStats {
            engines: [
                DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(),
                DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(),
                DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(),
                DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(), DmaEngineStats::new(),
            ],
            active_engines:    AtomicU32::new(0),
            iommu_faults:      AtomicU64::new(0),
            iommu_tlb_flushes: AtomicU64::new(0),
            domain_resets:     AtomicU64::new(0),
        }
    }

    /// Retourne les stats d'un moteur par son ID.
    #[inline]
    pub fn engine(&self, id: usize) -> Option<&DmaEngineStats> {
        self.engines.get(id)
    }

    /// Enregistre un moteur actif supplémentaire.
    #[inline]
    pub fn register_engine(&self) -> usize {
        let id = self.active_engines.fetch_add(1, Ordering::AcqRel) as usize;
        id.min(MAX_DMA_ENGINES - 1)
    }

    /// Compteurs agrégés de tous les moteurs actifs.
    pub fn total_submitted(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter().map(|e| e.submitted.load(Ordering::Relaxed)).sum()
    }

    pub fn total_completed(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter().map(|e| e.completed.load(Ordering::Relaxed)).sum()
    }

    pub fn total_errors(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter().map(|e| e.errors.load(Ordering::Relaxed)).sum()
    }

    pub fn total_bytes(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter()
            .map(|e| e.bytes_tx.load(Ordering::Relaxed) + e.bytes_rx.load(Ordering::Relaxed))
            .sum()
    }
}

/// Instance globale des statistiques DMA.
pub static DMA_STATS: DmaStats = DmaStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// FONCTIONS HELPER GLOBALES
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre une soumission pour le moteur `engine_id`.
#[inline]
pub fn dma_stat_submit(engine_id: usize) {
    if let Some(e) = DMA_STATS.engine(engine_id) { e.on_submit(); }
}

/// Enregistre une complétion pour le moteur `engine_id`.
#[inline]
pub fn dma_stat_complete(engine_id: usize, bytes: u64, is_write: bool, latency_cy: u64) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        e.on_complete(bytes, is_write, latency_cy);
    }
}

/// Enregistre une erreur pour le moteur `engine_id`.
#[inline]
pub fn dma_stat_error(engine_id: usize) {
    if let Some(e) = DMA_STATS.engine(engine_id) { e.on_error(); }
}

/// Enregistre un timeout pour le moteur `engine_id`.
#[inline]
pub fn dma_stat_timeout(engine_id: usize) {
    if let Some(e) = DMA_STATS.engine(engine_id) { e.on_timeout(); }
}

/// Ajoute des octets transférés pour le moteur `engine_id`.
#[inline]
pub fn dma_bytes_transferred(engine_id: usize, bytes: u64, is_write: bool) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        if is_write { e.bytes_tx.fetch_add(bytes, Ordering::Relaxed); }
        else        { e.bytes_rx.fetch_add(bytes, Ordering::Relaxed); }
    }
}

/// Dump des stats globales (pour diagnostic).
/// Retourne (submitted, completed, errors, bytes_total, iommu_faults).
#[inline]
pub fn dump_dma_stats() -> (u64, u64, u64, u64, u64) {
    (
        DMA_STATS.total_submitted(),
        DMA_STATS.total_completed(),
        DMA_STATS.total_errors(),
        DMA_STATS.total_bytes(),
        DMA_STATS.iommu_faults.load(Ordering::Relaxed),
    )
}
