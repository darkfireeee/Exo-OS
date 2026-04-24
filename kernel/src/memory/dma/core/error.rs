// kernel/src/memory/dma/core/error.rs
//
// Contexte enrichi d'erreurs DMA, descriptions statiques et compteurs globaux.
// Complète les variants `DmaError` définis dans `types.rs`.
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::dma::core::types::{DmaChannelId, DmaError, DmaTransactionId};

// ─────────────────────────────────────────────────────────────────────────────
// CONTEXTE D'ERREUR ENRICHI
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte complet d'une erreur DMA associant l'erreur à son origine.
///
/// Utilisé par les handlers d'interruption et la completion manager pour
/// propager en une seule valeur toutes les informations de diagnostic.
#[derive(Copy, Clone, Debug)]
pub struct DmaErrorContext {
    /// Cause de l'erreur.
    pub error: DmaError,
    /// Canal DMA concerné.
    pub channel: DmaChannelId,
    /// Transaction concernée (`DmaTransactionId::INVALID` si non applicable).
    pub transaction: DmaTransactionId,
    /// Timestamp TSC au moment de l'erreur (0 si non disponible).
    pub tsc: u64,
    /// Adresse physique fautive (0 si non applicable — e.g. IOMMU fault).
    pub fault_addr: u64,
}

impl DmaErrorContext {
    /// Construit un contexte avec timestamp TSC courant.
    #[inline]
    pub fn new(error: DmaError, channel: DmaChannelId, txn: DmaTransactionId) -> Self {
        DmaErrorContext {
            error,
            channel,
            transaction: txn,
            tsc: read_tsc(),
            fault_addr: 0,
        }
    }

    /// Construit un contexte sans transaction associée.
    #[inline]
    pub fn channel_error(error: DmaError, channel: DmaChannelId) -> Self {
        Self::new(error, channel, DmaTransactionId::INVALID)
    }

    /// Associe une adresse physique fautive (IOMMU fault).
    #[inline]
    pub fn with_fault_addr(mut self, addr: u64) -> Self {
        self.fault_addr = addr;
        self
    }

    /// Retourne une description textuelle statique de l'erreur.
    #[inline]
    pub fn description(self) -> &'static str {
        self.error.description()
    }

    /// `true` si l'erreur rend le canal inutilisable et nécessite une réinitialisation.
    #[inline]
    pub fn is_fatal(self) -> bool {
        matches!(
            self.error,
            DmaError::HardwareError | DmaError::IommuFault | DmaError::NotInitialized
        )
    }

    /// `true` si l'erreur est transitoire (la prochaine requête peut réussir).
    #[inline]
    pub fn is_transient(self) -> bool {
        matches!(
            self.error,
            DmaError::Timeout | DmaError::OutOfMemory | DmaError::Cancelled
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTIONS STATIQUES
// ─────────────────────────────────────────────────────────────────────────────

impl DmaError {
    /// Retourne une chaîne de caractères statique décrivant l'erreur.
    pub const fn description(self) -> &'static str {
        match self {
            DmaError::NoChannel => "no DMA channel available",
            DmaError::OutOfMemory => "DMA memory exhausted",
            DmaError::InvalidParams => "invalid DMA parameters (size/alignment)",
            DmaError::Timeout => "DMA transfer timeout",
            DmaError::HardwareError => "DMA hardware error (bus/parity)",
            DmaError::IommuFault => "IOMMU page fault during DMA",
            DmaError::NotInitialized => "DMA channel not initialized",
            DmaError::AlreadySubmitted => "transaction already submitted to channel",
            DmaError::Cancelled => "DMA transaction cancelled by caller",
            DmaError::MisalignedBuffer => "DMA buffer not aligned to channel granularity",
            DmaError::WrongZone => "physical address outside required DMA zone",
            DmaError::NotSupported => "operation not supported by this channel",
        }
    }

    /// `true` si l'erreur indique un problème matériel irrecupérable.
    #[inline]
    pub const fn is_hardware(self) -> bool {
        matches!(self, DmaError::HardwareError | DmaError::IommuFault)
    }

    /// `true` si l'erreur peut être résolue en réessayant.
    #[inline]
    pub const fn is_retriable(self) -> bool {
        matches!(
            self,
            DmaError::Timeout | DmaError::OutOfMemory | DmaError::NoChannel
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// COMPTEURS GLOBAUX D'ERREURS
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs globaux d'erreurs DMA — un compteur par variant + total.
///
/// Permet au monitoring kernel de détecter des tendances sans accès aux logs.
pub struct DmaErrorCounters {
    pub no_channel: AtomicU64,
    pub out_of_memory: AtomicU64,
    pub invalid_params: AtomicU64,
    pub timeout: AtomicU64,
    pub hardware_error: AtomicU64,
    pub iommu_fault: AtomicU64,
    pub not_initialized: AtomicU64,
    pub already_submitted: AtomicU64,
    pub cancelled: AtomicU64,
    pub misaligned_buffer: AtomicU64,
    pub wrong_zone: AtomicU64,
    pub not_supported: AtomicU64,
    /// Total toutes causes confondues.
    pub total: AtomicU64,
}

impl DmaErrorCounters {
    pub const fn new() -> Self {
        DmaErrorCounters {
            no_channel: AtomicU64::new(0),
            out_of_memory: AtomicU64::new(0),
            invalid_params: AtomicU64::new(0),
            timeout: AtomicU64::new(0),
            hardware_error: AtomicU64::new(0),
            iommu_fault: AtomicU64::new(0),
            not_initialized: AtomicU64::new(0),
            already_submitted: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            misaligned_buffer: AtomicU64::new(0),
            wrong_zone: AtomicU64::new(0),
            not_supported: AtomicU64::new(0),
            total: AtomicU64::new(0),
        }
    }

    /// Incrémente le compteur correspondant à l'erreur.
    pub fn record(&self, err: DmaError) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let counter = match err {
            DmaError::NoChannel => &self.no_channel,
            DmaError::OutOfMemory => &self.out_of_memory,
            DmaError::InvalidParams => &self.invalid_params,
            DmaError::Timeout => &self.timeout,
            DmaError::HardwareError => &self.hardware_error,
            DmaError::IommuFault => &self.iommu_fault,
            DmaError::NotInitialized => &self.not_initialized,
            DmaError::AlreadySubmitted => &self.already_submitted,
            DmaError::Cancelled => &self.cancelled,
            DmaError::MisalignedBuffer => &self.misaligned_buffer,
            DmaError::WrongZone => &self.wrong_zone,
            DmaError::NotSupported => &self.not_supported,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Retourne le total des erreurs enregistrées.
    pub fn get_total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Retourne le nombre d'erreurs matérielles (fatal).
    pub fn hardware_total(&self) -> u64 {
        self.hardware_error.load(Ordering::Relaxed) + self.iommu_fault.load(Ordering::Relaxed)
    }

    /// Retourne le nombre de timeouts.
    pub fn timeout_total(&self) -> u64 {
        self.timeout.load(Ordering::Relaxed)
    }

    /// Instantané des compteurs pour le monitoring.
    pub fn snapshot(&self) -> DmaErrorSnapshot {
        DmaErrorSnapshot {
            no_channel: self.no_channel.load(Ordering::Relaxed),
            out_of_memory: self.out_of_memory.load(Ordering::Relaxed),
            invalid_params: self.invalid_params.load(Ordering::Relaxed),
            timeout: self.timeout.load(Ordering::Relaxed),
            hardware_error: self.hardware_error.load(Ordering::Relaxed),
            iommu_fault: self.iommu_fault.load(Ordering::Relaxed),
            not_initialized: self.not_initialized.load(Ordering::Relaxed),
            already_submitted: self.already_submitted.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            misaligned_buffer: self.misaligned_buffer.load(Ordering::Relaxed),
            wrong_zone: self.wrong_zone.load(Ordering::Relaxed),
            not_supported: self.not_supported.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

/// Instantané des compteurs d'erreurs (valeurs non-atomiques pour lecture cohérente).
#[derive(Copy, Clone, Debug, Default)]
pub struct DmaErrorSnapshot {
    pub no_channel: u64,
    pub out_of_memory: u64,
    pub invalid_params: u64,
    pub timeout: u64,
    pub hardware_error: u64,
    pub iommu_fault: u64,
    pub not_initialized: u64,
    pub already_submitted: u64,
    pub cancelled: u64,
    pub misaligned_buffer: u64,
    pub wrong_zone: u64,
    pub not_supported: u64,
    pub total: u64,
}

/// Compteurs globaux d'erreurs DMA.
pub static DMA_ERROR_COUNTERS: DmaErrorCounters = DmaErrorCounters::new();

/// Enregistre une erreur DMA dans les compteurs globaux.
#[inline]
pub fn record_error(err: DmaError) {
    DMA_ERROR_COUNTERS.record(err);
}

/// Enregistre un contexte d'erreur complet.
#[inline]
pub fn record_error_ctx(ctx: &DmaErrorContext) {
    DMA_ERROR_COUNTERS.record(ctx.error);
}

// ─────────────────────────────────────────────────────────────────────────────
// UTILITAIRES
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le TSC courant via RDTSC (x86_64 seulement).
/// Retourne 0 sur d'autres architectures.
#[inline(always)]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: RDTSC disponible sur x86_64; non-sérialisé suffisant pour diagnostic.
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}
