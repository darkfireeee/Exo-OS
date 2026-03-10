// kernel/src/memory/dma/ops/cyclic.rs
//
// DMA cyclique — transferts audio/streaming en tampon circulaire.
//
// Un transfert cyclique DMA tourne indéfiniment dans un tampon circulaire
// découpé en `period_count` périodes. À chaque fin de période, un drapeau
// `period_elapsed` est levé pour que le driver audio soit notifié.
//
// Architecture :
//   - Le tampon est découpé en periods de `period_bytes` octets.
//   - La liste de descripteurs est bouclée (dernier → premier).
//   - Un compteur atomique `elapsed_periods` incrémente à chaque IRQ DMA.
//   - Le driver lit `elapsed_periods` pour savoir combien de périodes à traiter.
//   - `stop()` interrompt le cycle en cours.
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::dma::core::types::{
    DmaChannelId, DmaTransactionId, DmaDirection, DmaError,
};
use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de canaux cycliques simultanés.
pub const MAX_CYCLIC_CHANNELS: usize = 16;

/// Nombre maximum de périodes par transfert cyclique.
pub const MAX_CYCLIC_PERIODS: usize = 32;

/// Taille minimum d'une période (64 octets — alignement cache).
pub const MIN_PERIOD_BYTES: usize = 64;

/// Taille maximum d'une période (1 MiB).
pub const MAX_PERIOD_BYTES: usize = 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// CONFIGURATION D'UN TRANSFERT CYCLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration d'un transfert DMA cyclique.
#[derive(Copy, Clone)]
pub struct CyclicConfig {
    /// Canal DMA à utiliser.
    pub channel:      DmaChannelId,
    /// Adresse physique du tampon circulaire.
    pub buf_phys:     PhysAddr,
    /// Taille totale du tampon (doit être un multiple de `period_bytes`).
    pub buf_bytes:    usize,
    /// Taille d'une période en octets.
    pub period_bytes: usize,
    /// Direction des transferts.
    pub direction:    DmaDirection,
    /// Adresse physique du périphérique (registre FIFO, etc.).
    pub dev_phys:     PhysAddr,
}

/// Résultat de la validation d'une configuration cyclique.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum CyclicConfigError {
    InvalidAlignment,
    TooManyPeriods,
    PeriodTooSmall,
    PeriodTooLarge,
    BufNotMultipleOfPeriod,
    ChannelUnsupported,
}

impl CyclicConfig {
    /// Valide la configuration et retourne le nombre de périodes.
    pub fn validate(&self) -> Result<usize, CyclicConfigError> {
        if self.period_bytes < MIN_PERIOD_BYTES {
            return Err(CyclicConfigError::PeriodTooSmall);
        }
        if self.period_bytes > MAX_PERIOD_BYTES {
            return Err(CyclicConfigError::PeriodTooLarge);
        }
        if self.period_bytes & 63 != 0 {
            return Err(CyclicConfigError::InvalidAlignment);
        }
        if self.buf_bytes % self.period_bytes != 0 {
            return Err(CyclicConfigError::BufNotMultipleOfPeriod);
        }
        let periods = self.buf_bytes / self.period_bytes;
        if periods > MAX_CYCLIC_PERIODS {
            return Err(CyclicConfigError::TooManyPeriods);
        }
        Ok(periods)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ÉTAT D'UN TRANSFERT CYCLIQUE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
#[allow(dead_code)]
enum CyclicState {
    Idle    = 0,
    Running = 1,
    Paused  = 2,
    Error   = 3,
}

/// Descripteur interne d'une période cyclique.
#[allow(dead_code)]
#[derive(Copy, Clone)]
struct PeriodDesc {
    /// Adresse physique de début de la période dans le tampon.
    phys_start: PhysAddr,
    /// Octets dans cette période.
    bytes:      usize,
}

/// Un transfert DMA cyclique actif.
pub struct CyclicTransfer {
    /// Configuration de ce transfert.
    config:          CyclicConfig,
    /// Période[]s découpées.
    periods:         [PeriodDesc; MAX_CYCLIC_PERIODS],
    period_count:    usize,
    /// État courant.
    state:           AtomicU8,   // CyclicState
    /// Nombre de périodes écoulées depuis le démarrage (IRQ incrémente ce compteur).
    elapsed_periods: AtomicU64,
    /// Dernière période lue par le driver audio.
    consumed_periods: AtomicU64,
    /// Transaction DMA courante (réutilisée à chaque periode).
    current_txn:     DmaTransactionId,
    /// Index de la période en cours de transfert.
    current_period:  AtomicU32,
    /// Statistiques.
    pub underruns:   AtomicU32,
    pub overruns:    AtomicU32,
}

impl CyclicTransfer {
    const fn new() -> Self {
        const PD: PeriodDesc = PeriodDesc { phys_start: PhysAddr::new(0), bytes: 0 };
        CyclicTransfer {
            config:           CyclicConfig {
                channel:      DmaChannelId(u32::MAX),
                buf_phys:     PhysAddr::new(0),
                buf_bytes:    0,
                period_bytes: 0,
                direction:    DmaDirection::None,
                dev_phys:     PhysAddr::new(0),
            },
            periods:          [PD; MAX_CYCLIC_PERIODS],
            period_count:     0,
            state:            AtomicU8::new(CyclicState::Idle as u8),
            elapsed_periods:  AtomicU64::new(0),
            consumed_periods: AtomicU64::new(0),
            current_txn:      DmaTransactionId::INVALID,
            current_period:   AtomicU32::new(0),
            underruns:        AtomicU32::new(0),
            overruns:         AtomicU32::new(0),
        }
    }

    /// Prépare le transfert à partir d'une configuration validée.
    fn setup(&mut self, config: CyclicConfig, period_count: usize) {
        self.config       = config;
        self.period_count = period_count;
        for i in 0..period_count {
            self.periods[i] = PeriodDesc {
                phys_start: PhysAddr::new(
                    config.buf_phys.as_u64() + (i * config.period_bytes) as u64
                ),
                bytes: config.period_bytes,
            };
        }
        self.elapsed_periods.store(0, Ordering::Relaxed);
        self.consumed_periods.store(0, Ordering::Relaxed);
        self.current_period.store(0, Ordering::Relaxed);
    }

    /// Démarre le transfert (premier envoi de commande DMA).
    pub fn start(&mut self) -> Result<DmaTransactionId, DmaError> {
        if self.state.load(Ordering::Acquire) == CyclicState::Running as u8 {
            return Err(DmaError::AlreadySubmitted);
        }
        let txn = DmaTransactionId::generate();
        self.current_txn = txn;
        self.state.store(CyclicState::Running as u8, Ordering::Release);
        Ok(txn)
    }

    /// Appelé par l'IRQ handler quand une période se termine.
    ///
    /// Incrémente `elapsed_periods` et programme la période suivante.
    pub fn on_period_complete(&self) {
        let elapsed = self.elapsed_periods.fetch_add(1, Ordering::AcqRel) + 1;
        let next_period = (elapsed as usize) % self.period_count;
        self.current_period.store(next_period as u32, Ordering::Relaxed);

        // Détection de xrun.
        let consumed = self.consumed_periods.load(Ordering::Relaxed);
        let lag = elapsed.saturating_sub(consumed);
        if lag > self.period_count as u64 {
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Retourne combien de périodes sont disponibles pour le driver.
    pub fn available_periods(&self) -> u64 {
        let elapsed  = self.elapsed_periods.load(Ordering::Acquire);
        let consumed = self.consumed_periods.load(Ordering::Relaxed);
        elapsed.saturating_sub(consumed)
    }

    /// Le driver marque `n` périodes comme consommées.
    pub fn consume_periods(&self, n: u64) {
        self.consumed_periods.fetch_add(n, Ordering::Relaxed);
    }

    /// Arrête le transfert cyclique.
    pub fn stop(&self) {
        self.state.store(CyclicState::Idle as u8, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.state.load(Ordering::Acquire) == CyclicState::Running as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DES TRANSFERTS CYCLIQUES
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
struct CyclicTable {
    slots: [CyclicTransfer; MAX_CYCLIC_CHANNELS],
    count: usize,
}

impl CyclicTable {
    const fn new() -> Self {
        const T: CyclicTransfer = CyclicTransfer::new();
        CyclicTable { slots: [T; MAX_CYCLIC_CHANNELS], count: 0 }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.is_running() && slot.period_count == 0 {
                return Some(i);
            }
        }
        None
    }
}

/// Table des transferts cycliques DMA.
pub struct CyclicManager {
    inner: Mutex<CyclicTable>,
}

unsafe impl Sync for CyclicManager {}
unsafe impl Send for CyclicManager {}

impl CyclicManager {
    const fn new() -> Self {
        CyclicManager { inner: Mutex::new(CyclicTable::new()) }
    }

    /// Prépare et démarre un transfert cyclique.
    ///
    /// Retourne l'index du slot alloué (pour les appels `on_period_complete`/`stop`).
    pub fn start(&self, config: CyclicConfig) -> Result<usize, CyclicConfigError> {
        let period_count = config.validate()?;
        let mut table = self.inner.lock();
        let slot_idx = table.alloc_slot().ok_or(CyclicConfigError::TooManyPeriods)?;
        table.slots[slot_idx].setup(config, period_count);
        table.slots[slot_idx]
            .start()
            .map_err(|_| CyclicConfigError::ChannelUnsupported)?;
        Ok(slot_idx)
    }

    /// Notifie la fin d'une période pour le slot `idx`.
    pub fn on_period_complete(&self, idx: usize) {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].on_period_complete();
        }
    }

    /// Retourne le nombre de périodes disponibles pour lecture (slot `idx`).
    pub fn available_periods(&self, idx: usize) -> u64 {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].available_periods()
        } else {
            0
        }
    }

    /// Marque `n` périodes comme consommées par le driver audio.
    pub fn consume_periods(&self, idx: usize, n: u64) {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].consume_periods(n);
        }
    }

    /// Arrête le transfert cyclique du slot `idx`.
    pub fn stop(&self, idx: usize) {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].stop();
        }
    }
}

/// Gestionnaire global des transferts DMA cycliques.
pub static DMA_CYCLIC: CyclicManager = CyclicManager::new();
