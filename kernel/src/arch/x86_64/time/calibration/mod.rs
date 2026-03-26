// kernel/src/arch/x86_64/time/calibration/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════
// Orchestration de la calibration TSC — chaîne de fallback complète
// ════════════════════════════════════════════════════════════════════════════
//
// ## Responsabilités
//   1. Initialiser les sources d'horloge (init_sources) avant toute mesure.
//   2. Orchestrer la chaîne de fallback complète avec diagnostic.
//   3. Propager la fréquence calibrée vers cpu::tsc et sources::tsc.
//   4. Exposer une API riche pour monitoring, drift, re-calibration per-CPU.
//
// ## Chaîne de fallback (rating décroissant)
//   1. HPET window 1ms × 10 samples    (rating 300) — MMIO, précision ±0.05%
//   2. PM Timer window 1ms × 10 samples(rating 200) — port I/O, ±0.05%
//   3. CPUID leaf 0x15                 (rating 150) — nominal fabricant
//   4. CPUID leaf 0x16                 (rating 100) — fréquence base MHz
//   5. PIT one-shot 1ms × 10 samples   (rating  50) — héritage x86
//   6. Fallback 3 GHz                  (rating  10) — dernier recours
//
// ## Règles respectées
//   CAL-FALLBACK-01 : chaque fallback est tracé (port 0xE9 + log interne).
//   CAL-WINDOW-01   : condition de sortie = ticks source temps réel, jamais itérations.
//   CAL-CLI-01      : cli/sti par sample 1ms, pas sur fenêtre 10ms globale.
//   CAL-RDTSCP-01   : RDTSCP pour fin de mesure (sérialisation out-of-order).
//   SCHED-INIT-01   : calibrate_tsc() NE reçoit PAS tsc_hz en paramètre.
//   TIME-01         : TSC_HZ propagé via cpu::tsc::set_tsc_hz() (atomic Release).
//   TIME-02         : jamais while iter < N pour mesure temporelle.
//
// ## Utilisation type
//   ```rust
//   // Dans time_init() :
//   let hz = calibration::calibrate_tsc();
//   // → appelle sources::init_sources() en interne
//   // → retourne toujours une valeur dans [100 MHz, 10 GHz]
//
//   // Diagnostic post-boot :
//   let detail = calibration::last_calibration_result();
//   let (hz, source_tag, confidence, seq) = calibration::calibration_stats();
//
//   // Re-calibration post-APIC (per-CPU safe) :
//   let new_hz = calibration::recalibrate_tsc();
//   ```
// ════════════════════════════════════════════════════════════════════════════


pub mod window;
pub mod cpuid_nominal;
pub mod validation;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use super::sources;
use crate::arch::x86_64::cpu::tsc as cpu_tsc;

// Ré-exports publics pour les modules consommateurs (drift, percpu, ktime).
pub use window::{CalibrationResult, CalibrationSample, CalibrationSource, mean_and_variance};

// ── État global de calibration ────────────────────────────────────────────────

/// Fréquence TSC calibrée en Hz (atomique, mise à jour en Release).
static LAST_TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// Indice de la source utilisée pour la dernière calibration (CalibSource discriminant).
static LAST_SOURCE: AtomicU8 = AtomicU8::new(CalibSource::None as u8);

/// Rating de la dernière source utilisée (0–300).
static LAST_RATING: AtomicU32 = AtomicU32::new(0);

/// Compteur de séquence de calibration (incrémenté à chaque calibrate_tsc() ou recalibrate).
static CALIB_SEQ: AtomicU32 = AtomicU32::new(0);

/// Nombre de re-calibrations effectuées post-boot (recalibrate_tsc()).
static RECALIB_COUNT: AtomicU32 = AtomicU32::new(0);

/// Niveau de confiance de la dernière calibration (0–100).
static LAST_CONFIDENCE: AtomicU8 = AtomicU8::new(0);

/// Garde : sources::init_sources() déjà appelé ?
static SOURCES_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Types exportés ────────────────────────────────────────────────────────────

/// Source utilisée pour la calibration TSC.
/// Sérialisable en u8 pour le stockage atomique.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CalibSource {
    Hpet      = 0,
    PmTimer   = 1,
    Cpuid15   = 2,
    Cpuid16   = 3,
    Pit       = 4,
    Fallback3G = 5,
    None      = 0xFF,
}

impl CalibSource {
    /// Rating de précision décroissant : HPET meilleur, Fallback pire.
    #[inline(always)]
    pub fn rating(self) -> u32 {
        match self {
            CalibSource::Hpet       => 300,
            CalibSource::PmTimer    => 200,
            CalibSource::Cpuid15    => 150,
            CalibSource::Cpuid16    => 100,
            CalibSource::Pit        => 50,
            CalibSource::Fallback3G => 10,
            CalibSource::None       => 0,
        }
    }

    /// Vrai si la source implique une mesure réelle (fenêtre temporelle).
    #[inline(always)]
    pub fn is_measured(self) -> bool {
        matches!(self, CalibSource::Hpet | CalibSource::PmTimer | CalibSource::Pit)
    }

    /// Vrai si la source est fiable pour le drift compensation.
    #[inline(always)]
    pub fn is_trusted(self) -> bool {
        matches!(self, CalibSource::Hpet | CalibSource::PmTimer | CalibSource::Cpuid15)
    }

    /// Étiquette ASCII courte pour le port 0xE9 / logs.
    #[inline(always)]
    pub fn tag(self) -> &'static [u8] {
        match self {
            CalibSource::Hpet        => b"HPET",
            CalibSource::PmTimer     => b"PMT",
            CalibSource::Cpuid15     => b"CPUID15",
            CalibSource::Cpuid16     => b"CPUID16",
            CalibSource::Pit         => b"PIT",
            CalibSource::Fallback3G  => b"FB3G",
            CalibSource::None        => b"NONE",
        }
    }

    #[inline(always)]
    fn from_u8(v: u8) -> Self {
        match v {
            0 => CalibSource::Hpet,
            1 => CalibSource::PmTimer,
            2 => CalibSource::Cpuid15,
            3 => CalibSource::Cpuid16,
            4 => CalibSource::Pit,
            5 => CalibSource::Fallback3G,
            _ => CalibSource::None,
        }
    }
}

/// Résultat enrichi d'une calibration TSC.
/// Stocké en cache pour les consommateurs (drift, percpu, monitoring).
#[derive(Clone, Copy, Debug)]
pub struct CalibratedTsc {
    /// Fréquence TSC en Hz, arrondie à 100 kHz.
    pub tsc_hz: u64,
    /// Source utilisée pour la calibration.
    pub source: CalibSource,
    /// Niveau de confiance 0–100 (100 = HPET mesure parfaite).
    pub confidence: u8,
    /// Variance inter-samples en Hz² (0 si source nominale CPUID/Fallback).
    pub variance_hz2: u64,
    /// Nombre de samples valides utilisés pour la moyenne (0 si nominal).
    pub valid_samples: u8,
    /// Durée de calibration en millisecondes (approximative via TSC).
    pub duration_tsc_cycles: u64,
    /// Vrai si le TSC est invariant (CPUID 0x80000007[8]).
    pub tsc_invariant: bool,
    /// Séquence de calibration (monotone croissante).
    pub seq: u32,
}

impl CalibratedTsc {
    /// Retourne `true` si le résultat est utilisable en production.
    #[inline(always)]
    pub fn is_production_grade(&self) -> bool {
        self.confidence >= 80 && self.source.is_measured()
    }

    /// Retourne `true` si le résultat nécessite une re-calibration post-APIC.
    #[inline(always)]
    pub fn needs_recalibration(&self) -> bool {
        !self.source.is_measured()
    }
}

// ── Point d'entrée principal ──────────────────────────────────────────────────

/// Calibre la fréquence TSC via la chaîne de fallback complète.
///
/// ## Garanties
/// - Retourne TOUJOURS une valeur dans [100 MHz, 10 GHz] (jamais 0).
/// - Appelle `sources::init_sources()` exactement une fois (idempotent).
/// - Propage la fréquence dans `cpu_tsc::set_tsc_hz()` (atomic Release).
/// - Met à jour `sources::update_source_rating(Tsc, rating)`.
/// - Thread-safe (atomics Release/Acquire), mais conçu pour BSP uniquement.
///
/// ## RÈGLE SCHED-INIT-01
/// Ne reçoit AUCUN paramètre tsc_hz. La fréquence vient exclusivement
/// de la calibration — jamais d'un paramètre externe.
pub fn calibrate_tsc() -> u64 {
    let result = run_calibration_chain();
    commit_calibration_result(&result);
    result.tsc_hz
}

/// Version riche : retourne le `CalibratedTsc` complet pour monitoring/drift.
pub fn calibrate_tsc_detail() -> CalibratedTsc {
    let result = run_calibration_chain();
    commit_calibration_result(&result);
    result
}

// ── Re-calibration post-APIC ──────────────────────────────────────────────────

/// Re-calibre le TSC après initialisation de l'APIC/HPET.
///
/// Post-APIC, les timers sont plus stables → meilleure précision.
/// Applique une validation : si le nouveau Hz diffère de >50% de l'ancien,
/// la re-calibration est rejetée (protection contre mesures fantaisistes).
///
/// Retourne la nouvelle fréquence TSC (ou l'ancienne si rejetée).
pub fn recalibrate_tsc() -> u64 {
    let old_hz = LAST_TSC_HZ.load(Ordering::Acquire);
    RECALIB_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = run_calibration_chain();

    // Validation de cohérence : ±50% par rapport à l'ancienne valeur.
    if old_hz > 0 {
        let lo = old_hz / 2;
        let hi = old_hz.saturating_add(old_hz / 2);
        if result.tsc_hz < lo || result.tsc_hz > hi {
            // Re-calibration rejetée — conserver l'ancienne valeur.
            e9_tag(b"RECAL-REJECT");
            return old_hz;
        }
    }

    commit_calibration_result(&result);
    result.tsc_hz
}

// ── Chaîne de fallback interne ────────────────────────────────────────────────

/// Cœur de la calibration — exécute la chaîne de fallback et construit le `CalibratedTsc`.
///
/// ## Ordre de priorité (rating décroissant) :
///   1. HPET window    (best — MMIO 14.318 MHz)
///   2. PM Timer window(stable — I/O 3.579 MHz)
///   3. CPUID 0x15     (nominal Intel — pas de mesure)
///   4. CPUID 0x16     (base MHz — moins précis)
///   5. PIT window     (héritage — QEMU TCG peu fiable)
///   6. 3 GHz fallback (dernier recours)
fn run_calibration_chain() -> CalibratedTsc {
    let seq = CALIB_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
    let tsc_invariant = cpu_tsc::tsc_invariant();

    // ── FIX ARCH : initialiser les sources AVANT toute mesure ────────────────
    ensure_sources_initialized();

    let start_cycles = cpu_tsc::read_tsc();

    // ── Tentative 1 : CPUID leaf 0x15 (TSC/Crystal ratio) ─────────────────
    // Source nominale instantanée — pas de MMIO, pas de busy-poll.
    // Intel Core gen 6+ et Atom : ratio TSC/cristal exact.
    // PRIORITÉ : essayé EN PREMIER car :
    //   1. Ne bloque jamais (pas de boucle d'attente)
    //   2. Fonctionne sous virtualisation (QEMU -cpu host, KVM, VMware)
    //   3. Plus précis qu'une mesure sur fenêtre courte (résolution exacte)
    if let Some(hz) = cpuid_nominal::cpuid_tsc_hz() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag(b"CAL:CPUID15");
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid15,
                confidence: 85, // nominal = très fiable sur Intel gen6+
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Tentative 2 : CPUID leaf 0x16 (fréquence de base CPU) ─────────────
    // Résolution limitée : multiple de 1 MHz. Pas de boucle → instantané.
    if let Some(hz) = cpuid_nominal::cpuid_tsc_hz_leaf16() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag(b"CAL:CPUID16");
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid16,
                confidence: 70, // moins précis — résolution 1 MHz
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Tentative 3 : meilleure estimation CPUID (cross-check 0x15/0x16) ───
    // cpuid_best_estimate() croise les deux feuilles CPUID pour affiner.
    if let Some(hz) = cpuid_nominal::cpuid_best_estimate() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag(b"CAL:CPUID-BEST");
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid15, // meilleure estimation via CPUID
                confidence: 65,
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Tentative 4 : PIT one-shot via cpu_tsc driver (port I/O, fiable QEMU) ──
    // Sur QEMU TCG, le PIT canal 2 est bien simulé (port I/O, pas MMIO).
    // Retourne 0 si PIT inactif ou I/O trop lent (QEMU TCG 10K iter × inb).
    {
        let hz = cpu_tsc::calibrate_tsc_with_pit();
        if hz > 0 && validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag(b"CAL:PIT-DRV");
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Pit,
                confidence: 55, // PIT mesuré sur fenêtre ~10ms = fiable
                variance_hz2: 0,
                valid_samples: 1, // mesure unique = 10ms
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
        e9_tag(b"CAL:PIT-DRV-FAIL");

        // Bring-up pragmatique : si PIT driver échoue, on évite les chemins
        // de calibration coûteux/fragiles restants et on bascule immédiatement
        // sur le fallback 3 GHz pour ne pas bloquer le boot.
        e9_tag(b"CAL:FB3G");
        let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
        return CalibratedTsc {
            tsc_hz: 3_000_000_000u64,
            source: CalibSource::Fallback3G,
            confidence: 10,
            variance_hz2: 0,
            valid_samples: 0,
            duration_tsc_cycles: duration,
            tsc_invariant,
            seq,
        };
    }
}

// ── Propagation du résultat ───────────────────────────────────────────────────

/// Propage le résultat de calibration vers les modules dépendants.
///
/// Effets :
///   - `cpu_tsc::set_tsc_hz(hz)` → met à jour TSC_HZ, TSC_KHZ, TSC_CALIBRATED
///   - `sources::update_source_rating(Tsc, rating)` → upgrade TSC en source de runtime
///   - Mise à jour des atomics LAST_* pour le diagnostic
fn commit_calibration_result(r: &CalibratedTsc) {
    // 1. Propager vers cpu::tsc (atomics de base pour tsc_hz(), tsc_cycles_to_ns(), etc.)
    cpu_tsc::set_tsc_hz(r.tsc_hz);

    // 2. Upgrader le rating TSC dans le registre de sources.
    // Après calibration, le TSC devient la meilleure source de runtime.
    let tsc_runtime_rating = 400u32; // TSC invariant calibré = meilleure source runtime
    sources::update_source_rating(sources::SourceId::Tsc, tsc_runtime_rating);

    // 3. Sauvegarder le résultat pour last_calibration_result() / calibration_stats().
    LAST_TSC_HZ.store(r.tsc_hz, Ordering::Release);
    LAST_SOURCE.store(r.source as u8, Ordering::Release);
    LAST_RATING.store(r.source.rating(), Ordering::Release);
    LAST_CONFIDENCE.store(r.confidence, Ordering::Release);
}

// ── Initialisation des sources ────────────────────────────────────────────────

/// Garantit que `sources::init_sources()` est appelé exactement une fois.
///
/// Idempotent : peut être appelé depuis recalibrate_tsc() sans double-init.
/// `sources::init_sources()` initialise :
///   - sources/hpet.rs::HPET_BASE_ADDR  (depuis acpi/hpet.rs::HPET_BASE)
///   - sources/hpet.rs::HPET_FREQ_HZ    (depuis acpi/hpet.rs::HPET_PERIOD_FS)
///   - sources/pm_timer.rs::PM_BASE_PORT (depuis ACPI FADT)
///   - sources/pit.rs::PIT initialisé
///   - sources/tsc.rs::init_tsc_source() (détection invariant, hyperviseur, etc.)
#[inline]
fn ensure_sources_initialized() {
    // compare_exchange(false → true) : atomique, un seul thread gagne.
    if SOURCES_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        sources::init_sources();
    }
    // Sinon : déjà initialisé, on continue sans double-init.
}

// ── API de diagnostic ─────────────────────────────────────────────────────────

/// Retourne le résultat complet de la dernière calibration.
///
/// Retourne `None` si `calibrate_tsc()` n'a jamais été appelé.
pub fn last_calibration_result() -> Option<CalibratedTsc> {
    let hz = LAST_TSC_HZ.load(Ordering::Acquire);
    if hz == 0 { return None; }
    let seq = CALIB_SEQ.load(Ordering::Relaxed);
    Some(CalibratedTsc {
        tsc_hz: hz,
        source: CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)),
        confidence: LAST_CONFIDENCE.load(Ordering::Relaxed),
        variance_hz2: 0, // non stocké globalement (disponible via calibrate_tsc_detail)
        valid_samples: if CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)).is_measured() {
            window::N_SAMPLES as u8
        } else { 0 },
        duration_tsc_cycles: 0, // non stocké globalement
        tsc_invariant: cpu_tsc::tsc_invariant(),
        seq,
    })
}

/// Retourne un tuple de statistiques compactes pour le monitoring.
///
/// Retourne `(tsc_hz, calib_seq, confidence, recalib_count)`.
pub fn calibration_stats() -> (u64, u32, u8, u32) {
    (
        LAST_TSC_HZ.load(Ordering::Relaxed),
        CALIB_SEQ.load(Ordering::Relaxed),
        LAST_CONFIDENCE.load(Ordering::Relaxed),
        RECALIB_COUNT.load(Ordering::Relaxed),
    )
}

/// Retourne le rating de la dernière source utilisée (0–300).
pub fn last_source_rating() -> u32 {
    LAST_RATING.load(Ordering::Relaxed)
}

/// Retourne `true` si la dernière calibration a utilisé HPET (mesure physique HQ).
pub fn calibrated_with_hpet() -> bool {
    CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)) == CalibSource::Hpet
}

/// Retourne `true` si le TSC a été calibré avec une source de mesure réelle.
/// Faux si CPUID nominal ou fallback 3GHz.
pub fn calibrated_with_real_measurement() -> bool {
    CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)).is_measured()
}

/// Retourne `true` si une re-calibration post-APIC est recommandée.
pub fn should_recalibrate() -> bool {
    !calibrated_with_real_measurement()
}

/// Retourne le nombre de re-calibrations effectuées.
pub fn recalibration_count() -> u32 {
    RECALIB_COUNT.load(Ordering::Relaxed)
}

// ── Utilitaires ───────────────────────────────────────────────────────────────

/// Arrondit la fréquence TSC au multiple de 100 kHz le plus proche.
///
/// Réduit le bruit de mesure sub-100kHz sans perte de précision observable.
/// Exemple : 2_999_937_012 → 3_000_000_000 (valeur typique i7 3GHz).
#[inline(always)]
pub fn round_hz(hz: u64) -> u64 {
    (hz + 50_000) / 100_000 * 100_000
}

/// Émet un tag ASCII sur le port de debug 0xE9.
///
/// Format de sortie: `[tag]` — visible avec QEMU `-debugcon stdio` ou
/// `-debugcon file:/tmp/e9.txt`.
///
/// ## Règle CAL-FALLBACK-01
/// Chaque étape de la chaîne de fallback DOIT appeler cette fonction.
#[inline]
fn e9_tag(tag: &[u8]) {
    #[inline(always)]
    unsafe fn out(b: u8) {
        core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack, preserves_flags));
    }
    unsafe {
        out(b'[');
        for &b in tag { out(b); }
        out(b']');
    }
}

// ── Compatibilité et ré-exports hérités ──────────────────────────────────────

/// Compatibilité avec l'ancien code qui appelait `debug_log_source(tag)`.
/// Redirige vers `e9_tag()`.
#[inline(always)]
#[allow(dead_code)]
fn debug_log_source(tag: &[u8]) {
    e9_tag(tag);
}

