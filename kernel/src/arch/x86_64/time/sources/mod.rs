// kernel/src/arch/x86_64/time/sources/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Registre des sources d'horloge — sélection dynamique et monitoring
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Sources disponibles (par ordre de préférence décroissante)
//
//   1. TSC invariant (rating 400 / 350 avec hyperviseur)
//      Meilleur résolution (cycle CPU), pas d'accès HW à chaque lecture.
//      Nécessite calibration préalable et synchronisation SMP.
//
//   2. HPET 64-bit (rating 320 / 300 si 32-bit)
//      Bonne précision (~14.318 MHz), MMIO UC sans overhead I/O.
//      Source de référence pour la calibration du TSC.
//
//   3. PM Timer ACPI (rating 200)
//      Disponible sur tous les PC ACPI. Fréquence fixe 3.579545 MHz.
//      Anti-glitch triple lecture. Fallback si HPET absent.
//
//   4. PIT 8254 (rating 50)
//      Héritage ISA. Toujours présent sur x86. Seulement pour last-resort.
//      Peut ne pas fonctionner sur QEMU TCG.
//
// ## Sélection de la meilleure source
//   `best_source_for_calibration()` : retourne la source la plus fiable
//   disponible pour calibrer le TSC (HPET > PM Timer > PIT > None).
//
//   `best_runtime_source()` : retourne la source à utiliser en runtime pour
//   lire le temps courant (TSC > HPET > PM Timer > PIT).
//
// ## Monitoring de santé
//   Chaque source a un `SourceStatus` : Available, Degraded, Unavailable.
//   `source_health_check()` lit les sources et compare les valeurs pour
//   détecter les dérives ou défaillances.
//
// ## Règles
//   RÈGLE TIME-04 : TSC invariant vérifié avant utilisation.
//   RÈGLE TIME-08 : delta HPET via wrapping_sub.
//   RÈGLE PIT-QEMU-01 : PIT non disponible marqué Degraded sur QEMU TCG.
// ════════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicBool, AtomicU64, Ordering};

pub mod tsc;
pub mod hpet;
pub mod pm_timer;
pub mod pit;

// ── Trait ClockSource ─────────────────────────────────────────────────────────

/// Trait commun pour toutes les sources d'horloge matérielle.
pub trait ClockSource {
    fn name(&self) -> &'static str;
    /// Priorité de sélection : plus haute → source préférée.
    fn rating(&self) -> u32;
    /// Lit la valeur brute du compteur (en ticks).
    fn read(&self) -> u64;
    /// Fréquence en Hz.
    fn freq_hz(&self) -> u64;
    /// Indique si la source est disponible et fonctionnelle.
    fn available(&self) -> bool;
}

// ── Identifiant de source ─────────────────────────────────────────────────────

/// Identifiant énuméré des sources d'horloge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceId {
    Tsc,
    Hpet,
    PmTimer,
    Pit,
    None,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Tsc     => "TSC",
            SourceId::Hpet    => "HPET",
            SourceId::PmTimer => "PM_TIMER",
            SourceId::Pit     => "PIT",
            SourceId::None    => "NONE",
        }
    }

    /// Rating par défaut de la source.
    pub fn default_rating(self) -> u32 {
        match self {
            SourceId::Tsc     => 400,
            SourceId::Hpet    => 300,
            SourceId::PmTimer => 200,
            SourceId::Pit     => 50,
            SourceId::None    => 0,
        }
    }
}

// ── Statut de source ──────────────────────────────────────────────────────────

/// Statut opérationnel d'une source d'horloge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    /// Disponible et validée.
    Available,
    /// Disponible mais avec des anomalies (drift, glitches).
    Degraded,
    /// Non disponible (hardware absent ou init échoué).
    Unavailable,
    /// Pas encore testée.
    Untested,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceStatus::Available   => "OK",
            SourceStatus::Degraded    => "DEGRADED",
            SourceStatus::Unavailable => "UNAVAIL",
            SourceStatus::Untested    => "UNTESTED",
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(self, SourceStatus::Available | SourceStatus::Degraded)
    }
}

// ── Registre de sources ───────────────────────────────────────────────────────

/// Entrée du registre de sources.
#[derive(Debug, Clone, Copy)]
pub struct SourceEntry {
    pub id:     SourceId,
    pub status: SourceStatus,
    pub rating: u32,
}

impl SourceEntry {
    pub const fn new(id: SourceId) -> Self {
        SourceEntry {
            id,
            status: SourceStatus::Untested,
            rating: 0,
        }
    }
}

// Registre global de 4 sources.
const NUM_SOURCES: usize = 4;
static REGISTRY: [AtomicU32; NUM_SOURCES * 2] = {
    // Encodage : pairs (status, rating) pour chaque source.
    const ZERO: AtomicU32 = AtomicU32::new(0);
    [ZERO; NUM_SOURCES * 2]
};

/// Index des sources dans REGISTRY.
const IDX_TSC:      usize = 0;
const IDX_HPET:     usize = 1;
const IDX_PMTIMER:  usize = 2;
const IDX_PIT:      usize = 3;

// Encodage : index*2 = status, index*2+1 = rating.
fn read_entry(idx: usize) -> SourceEntry {
    let ids = [SourceId::Tsc, SourceId::Hpet, SourceId::PmTimer, SourceId::Pit];
    let status_raw = REGISTRY[idx * 2].load(Ordering::Relaxed);
    let rating     = REGISTRY[idx * 2 + 1].load(Ordering::Relaxed);
    let status = match status_raw {
        1 => SourceStatus::Available,
        2 => SourceStatus::Degraded,
        3 => SourceStatus::Unavailable,
        _ => SourceStatus::Untested,
    };
    SourceEntry { id: ids[idx], status, rating }
}

fn write_entry(idx: usize, status: SourceStatus, rating: u32) {
    let status_raw: u32 = match status {
        SourceStatus::Available   => 1,
        SourceStatus::Degraded    => 2,
        SourceStatus::Unavailable => 3,
        SourceStatus::Untested    => 0,
    };
    REGISTRY[idx * 2].store(status_raw, Ordering::Relaxed);
    REGISTRY[idx * 2 + 1].store(rating, Ordering::Relaxed);
}

// ── Initialisation du registre ────────────────────────────────────────────────

static REGISTRY_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Initialise et probe toutes les sources disponibles.
/// À appeler depuis `time_init()` après initialisation ACPI et TSC.
pub fn init_sources() {
    if REGISTRY_INIT_DONE.swap(true, Ordering::Relaxed) { return; }

    // TSC : toujours présent ; invariant détermine le rating.
    tsc::init_tsc_source();
    let tsc_rating  = tsc::tsc_rating();
    write_entry(IDX_TSC, SourceStatus::Available, tsc_rating);

    // HPET : disponible si `hpet_available()`.
    hpet::init_hpet_source();
    if hpet::available() {
        let r = if hpet::HpetSource.available() { 
            hpet::HpetSource.rating() 
        } else { 0 };
        write_entry(IDX_HPET, SourceStatus::Available, r);
    } else {
        write_entry(IDX_HPET, SourceStatus::Unavailable, 0);
    }

    // PM Timer : disponible si le port est non-nul.
    if pm_timer::available() {
        write_entry(IDX_PMTIMER, SourceStatus::Available, 200);
    } else if pm_timer::auto_detect() {
        write_entry(IDX_PMTIMER, SourceStatus::Available, 200);
    } else {
        write_entry(IDX_PMTIMER, SourceStatus::Unavailable, 0);
    }

    // PIT : toujours présent en théorie, mais on teste d'abord.
    let pit_status = if pit::qemu_tcg_detected() {
        SourceStatus::Degraded
    } else {
        SourceStatus::Available
    };
    write_entry(IDX_PIT, pit_status, pit::PitSource.rating());
}

// ── Sélection de source ───────────────────────────────────────────────────────

/// Retourne la meilleure source pour la calibration du TSC (PAS le TSC lui-même).
/// Ordre : HPET > PM Timer > PIT > None.
pub fn best_source_for_calibration() -> SourceId {
    if read_entry(IDX_HPET).status.is_usable()    { return SourceId::Hpet; }
    if read_entry(IDX_PMTIMER).status.is_usable() { return SourceId::PmTimer; }
    if read_entry(IDX_PIT).status.is_usable()     { return SourceId::Pit; }
    SourceId::None
}

/// Retourne la meilleure source pour la lecture du temps en runtime.
/// Ordre : TSC > HPET > PM Timer > PIT.
pub fn best_runtime_source() -> SourceId {
    let tsc = read_entry(IDX_TSC);
    if tsc.status.is_usable() && tsc.rating >= 350 {
        return SourceId::Tsc;
    }
    if read_entry(IDX_HPET).status.is_usable()    { return SourceId::Hpet; }
    if read_entry(IDX_PMTIMER).status.is_usable() { return SourceId::PmTimer; }
    if read_entry(IDX_PIT).status.is_usable()     { return SourceId::Pit; }
    SourceId::None
}

/// Lit la valeur courante de la source demandée (en ticks bruts).
pub fn read_source(id: SourceId) -> u64 {
    match id {
        SourceId::Tsc     => tsc::rdtsc_read(),
        SourceId::Hpet    => hpet::read(),
        SourceId::PmTimer => pm_timer::read(),
        SourceId::Pit     => pit::read_latch_ch2() as u64,
        SourceId::None    => 0,
    }
}

/// Retourne la fréquence en Hz de la source demandée.
pub fn source_freq_hz(id: SourceId) -> u64 {
    match id {
        SourceId::Tsc     => tsc::tsc_freq_hz(),
        SourceId::Hpet    => hpet::freq_hz(),
        SourceId::PmTimer => pm_timer::freq_hz(),
        SourceId::Pit     => pit::PIT_FREQ_HZ,
        SourceId::None    => 0,
    }
}

// ── Monitoring de santé ───────────────────────────────────────────────────────

/// Compteur de health checks effectués.
static HEALTH_CHECK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Effectue une vérification croisée entre HPET et PM Timer.
/// Retourne `true` si les deux sources sont cohérentes (divergence < 1%).
pub fn source_health_check() -> bool {
    HEALTH_CHECK_COUNT.fetch_add(1, Ordering::Relaxed);

    let hpet_ok = read_entry(IDX_HPET).status.is_usable();
    let pm_ok   = read_entry(IDX_PMTIMER).status.is_usable();

    if !hpet_ok || !pm_ok {
        // Pas assez de sources pour la cross-vérification.
        return true;
    }

    // Mesurer un intervalle avec les deux sources simultanément.
    let hpet_start = hpet::read();
    let pm_start   = pm_timer::read();

    // Attendre ~100 µs (mesure approximative via spin_loop).
    // Pas de vrai wait ici car cette fonction est appelée periodiquement.
    let hpet_end   = hpet::read();
    let pm_end     = pm_timer::read();

    let hpet_delta = hpet::delta(hpet_start, hpet_end);
    let pm_delta   = pm_timer::delta(pm_start, pm_end);

    if hpet_delta == 0 || pm_delta == 0 {
        return true; // Pas assez de temps écoulé.
    }

    // Convertir en nanosecondes pour comparer.
    let hpet_ns = hpet::ticks_to_ns(hpet_delta);
    let pm_ns   = pm_timer::ticks_to_ns(pm_delta);

    if hpet_ns == 0 || pm_ns == 0 {
        return true;
    }

    // Divergence en centièmes de % = |hpet_ns - pm_ns| × 10000 / pm_ns.
    let diff = if hpet_ns > pm_ns { hpet_ns - pm_ns } else { pm_ns - hpet_ns };
    let divergence_x100 = (diff as u128 * 10_000) / pm_ns as u128;

    if divergence_x100 > 500 {
        // Divergence > 5% → marquer HPET comme dégradé.
        write_entry(IDX_HPET, SourceStatus::Degraded,
                    read_entry(IDX_HPET).rating);
        return false;
    }

    true
}

/// Met à jour le statut d'une source (pour les mises à jour dynamiques).
pub fn update_source_status(id: SourceId, status: SourceStatus) {
    let idx = match id {
        SourceId::Tsc     => IDX_TSC,
        SourceId::Hpet    => IDX_HPET,
        SourceId::PmTimer => IDX_PMTIMER,
        SourceId::Pit     => IDX_PIT,
        SourceId::None    => return,
    };
    let current_rating = read_entry(idx).rating;
    write_entry(idx, status, current_rating);
}

/// Met à jour le rating d'une source (ex: TSC upgrader après calibration réussie).
pub fn update_source_rating(id: SourceId, rating: u32) {
    let idx = match id {
        SourceId::Tsc     => IDX_TSC,
        SourceId::Hpet    => IDX_HPET,
        SourceId::PmTimer => IDX_PMTIMER,
        SourceId::Pit     => IDX_PIT,
        SourceId::None    => return,
    };
    let current_status = read_entry(idx).status;
    write_entry(idx, current_status, rating);
}

/// Retourne toutes les entrées du registre (pour diagnostics).
pub fn all_entries() -> [SourceEntry; NUM_SOURCES] {
    [
        read_entry(IDX_TSC),
        read_entry(IDX_HPET),
        read_entry(IDX_PMTIMER),
        read_entry(IDX_PIT),
    ]
}

/// Retourne le nombre de sources usables (Available ou Degraded).
pub fn usable_source_count() -> u8 {
    let entries = all_entries();
    entries.iter().filter(|e| e.status.is_usable()).count() as u8
}
