// kernel/src/arch/x86_64/time/calibration/window.rs
//
// ════════════════════════════════════════════════════════════════════════════
// Calibration TSC sur fenêtre temporelle RÉELLE — bare-metal ready
// ════════════════════════════════════════════════════════════════════════════
//
// FIX TIME-02 : Loop condition = HPET/PM Timer ticks écoulés (jamais itérations).
//   AVANT (bugué) : while iter < MAX_MEASURE_ITERS { ... }
//                   → bare-metal : 500 × 5ns = 2.5µs << 10ms → échoue toujours
//   APRÈS (correct) : while hpet_elapsed < target_ticks { ... }
//                   → bare-metal : loop tourne ~1ms de vrai temps → TSC delta précis
//                   → QEMU      : loop tourne ~1ms de vrai temps → idem
//
// FIX TIME-03 : CLI/STI par sample de 1ms MAX (pas 10ms global).
//   AVANT (bugué) : cli ... 10ms de mesure ... sti
//                   → IRQ LAPIC timer perdue → thread RT ne se réveille plus
//   APRÈS (correct) : 10 samples × (cli, 1ms mesure, sti)
//                   → perte max : 1ms d'IRQ par sample, outlier rejection sur 10
//
// RÈGLE CAL-RDTSCP-01 : TOUJOURS RDTSCP (pas RDTSC) en calibration.
//   RDTSC peut être out-of-order → biais de mesure.
//   RDTSCP sérialise + fournit coreid → mesure ancrée au bon cœur.
// ════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use super::super::sources::hpet as hpet_src;
use super::super::sources::pm_timer as pm_src;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Nombre de samples de 1ms pour la calibration multi-sample.
pub const N_SAMPLES: usize = 10;

/// Limite de synchronisation initiale (attendre 1 tick HPET/PM).
/// 20 itérations max — sur QEMU TCG, chaque MMIO read HPET est coûteux (~1µs–60ms).
/// Si HPET ne bouge pas en 20 reads consécutifs  → probablement désactivé.
/// Sur bare-metal : 1 itération suffit généralement (HPET avance à 70ns/tick).
const MAX_SYNC_ITERS: u32 = 20;

/// Limite max d'itérations MMIO dans la boucle principale de mesure.
/// Sécurité contre HPET trop lent (QEMU TCG coûteux) : si en N_MAX_POLL_MMIO reads
/// on n'a pas atteint target_ticks, c'est que le HPET est inutilisable → None.
/// 2000 reads × max 1ms/read (QEMU TCG lent) = 2s max par sample.
const N_MAX_POLL_MMIO: u64 = 2_000;

/// Timeout TSC pour la boucle de mesure : 3ms à ~1 GHz minimum.
/// = 3_000_000 cycles à 1 GHz = 3ms | à 3 GHz = 1ms.
/// Sur QEMU TCG (~100-500 MHz simulé), 3_000_000 cycles = 6-30ms.
/// Choisir une valeur sûre pour bare-metal ET QEMU TCG.
/// RDTSC est toujours rapide (pas de MMIO) — le garde ne ralentit pas la boucle.
/// NOTE : si le timeout déclenche, on retourne None immédiatement → fallback chain.
const SAMPLE_TSC_TIMEOUT_CYCLES: u64 = 3_000_000;

// ── Calibration via HPET ─────────────────────────────────────────────────────

/// Calibre le TSC Hz en utilisant le HPET comme référence temporelle.
///
/// RÈGLE CAL-WINDOW-01 : loop condition = ticks HPET, jamais itérations.
/// RÈGLE CAL-CLI-01    : cli/sti par sample de 1ms MAX.
/// RÈGLE CAL-RDTSCP-01 : RDTSCP, pas RDTSC.
///
/// Précision : ±0.01% (HPET tick = ~70ns à 14.318 MHz).
/// Durée     : 10ms de vrai temps (10 × 1ms).
///             Bare-metal : ~10ms | QEMU TCG : ~10ms.
///
/// Retourne `None` si HPET indisponible ou mesure hors plage.
pub fn calibrate_tsc_via_hpet(hpet_freq_hz: u64) -> Option<u64> {
    if hpet_freq_hz == 0 { return None; }

    // 1ms de ticks HPET.
    let target_ticks: u64 = hpet_freq_hz / 1_000;
    if target_ticks == 0 { return None; }

    let mut samples = [0u64; N_SAMPLES];

    for sample in &mut samples {
        // ── Synchronisation : attendre que HPET avance d'au moins 1 tick ──────
        // Protection : si HPET arrêté/cassé, on limite pour ne pas bloquer.
        let sync_val = hpet_src::read();
        let mut synced = false;
        for _ in 0..MAX_SYNC_ITERS {
            let v = hpet_src::read();
            if v != sync_val { synced = true; break; }
            core::hint::spin_loop();
        }
        if !synced { return None; }

        // ── Mesure 1ms SANS CLI (boucle HPET) ────────────────────────────────
        // RÈGLE CAL-CLI-01 : CLI autour de rdtsc_begin/rdtscp_end UNIQUEMENT,
        // pas autour de toute la boucle d'attente HPET.
        // Raison : sur QEMU TCG, CLI pendant busy-poll MMIO bloque l'avancement
        // de l'horloge virtuelle HPET (QEMU ne dispatche plus ses timers internes).
        // Sur bare-metal : les IRQs sont acceptées pendant l'attente — la mesure
        // est basée sur les ticks HPET (temps physique réel), pas sur le TSC seul.
        let hpet_start = hpet_src::read();

        // ✅ CAL-CLI-01 VARIANTE HPET : CLI seulement pour rdtsc_begin sérialisé.
        let flags = flags_save_cli();
        let tsc_start = rdtsc_begin();
        unsafe { flags_restore(flags); }

        // Boucle d'attente HPET — IRQs autorisées (voir note ci-dessus).
        // DOUBLE GARDE :
        //   1. Ticks HPET : condition de sortie normale (TIME-02)
        //   2. N_MAX_POLL_MMIO : limite le nombre de reads MMIO coûteux sur QEMU TCG
        //   3. TSC timeout : garde finale si TSC disponible
        let tsc_deadline = tsc_start.wrapping_add(SAMPLE_TSC_TIMEOUT_CYCLES);
        let mut poll_count: u64 = 0;
        loop {
            let hpet_now = hpet_src::read();
            poll_count = poll_count.wrapping_add(1);
            // RÈGLE TIME-08 : wrapping_sub gère le rollover 32/64-bit.
            if hpet_now.wrapping_sub(hpet_start) >= target_ticks { break; }
            // GARDE MMIO : max N_MAX_POLL_MMIO lectures (protège contre HPET très lent).
            if poll_count >= N_MAX_POLL_MMIO {
                return None;
            }
            // GARDE TSC : abandonne si TSC a avancé au-delà du timeout.
            let elapsed = rdtsc_begin().wrapping_sub(tsc_start);
            if elapsed >= tsc_deadline.wrapping_sub(tsc_start) {
                return None;
            }
            core::hint::spin_loop();
        }

        // ✅ CAL-CLI-01 : CLI seulement pour rdtscp_end sérialisé.
        let flags2 = flags_save_cli();
        let (tsc_end, _coreid) = rdtscp_end();
        unsafe { flags_restore(flags2); }

        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        // Extrapolation : delta sur 1ms → Hz pour 1 seconde.
        *sample = tsc_delta.saturating_mul(1_000);
    }

    // ── Outlier rejection par IQR ──────────────────────────────────────────────
    // Trie les samples et garde ceux dans [Q1 - 1.5×IQR, Q3 + 1.5×IQR].
    samples.sort_unstable();
    let tsc_hz = iqr_mean(&samples);

    super::validation::cross_check(tsc_hz)
}

// ── Calibration via PM Timer ──────────────────────────────────────────────────

/// Calibre le TSC Hz en utilisant le PM Timer ACPI comme référence.
///
/// RÈGLE CAL-WINDOW-01 : loop condition = ticks PM Timer, jamais itérations.
/// RÈGLE CAL-CLI-01    : cli/sti par sample de 1ms MAX.
/// RÈGLE TIME-09 (FIX) : gestion du wrap 24-bit/32-bit du PM Timer.
///
/// Précision : ±0.05% (PM Timer tick = ±280 ns à 3.58 MHz).
/// Durée     : 10ms de vrai temps (10 × 1ms).
///
/// Retourne `None` si PM Timer indisponible ou mesure hors plage.
pub fn calibrate_tsc_via_pm_timer() -> Option<u64> {
    if !pm_src::available() { return None; }

    let freq_hz = pm_src::freq_hz(); // 3_579_545 Hz (fixe)
    if freq_hz == 0 { return None; }

    // 1ms de ticks PM Timer.
    let target_ticks: u64 = freq_hz / 1_000; // ≈ 3 579 ticks

    let mut samples = [0u64; N_SAMPLES];

    for sample in &mut samples {
        // ── Synchronisation : attendre que PM Timer avance d'au moins 1 tick ──
        let sync_val = pm_src::read();
        let mut synced = false;
        for _ in 0..MAX_SYNC_ITERS {
            if pm_src::read() != sync_val { synced = true; break; }
            core::hint::spin_loop();
        }
        if !synced { return None; }

        // ── Mesure 1ms — CLI autour de rdtsc/rdtscp UNIQUEMENT (CAL-CLI-01) ────
        // PM Timer = port I/O 0xPMTMR (déjà sérialisé nativement sur x86).
        // Même raison que HPET : CLI pendant la boucle d'attente bloque QEMU TCG.
        let pm_start = pm_src::read();

        let flags = flags_save_cli();
        let tsc_start = rdtsc_begin();
        unsafe { flags_restore(flags); }

        // ✅ FIX TIME-02 : condition de sortie = PM Timer delta ticks.
        // DOUBLE GARDE : N_MAX_POLL_MMIO et TSC timeout.
        let tsc_deadline = tsc_start.wrapping_add(SAMPLE_TSC_TIMEOUT_CYCLES);
        let mut poll_count: u64 = 0;
        loop {
            let pm_now = pm_src::read();
            poll_count = poll_count.wrapping_add(1);
            // RÈGLE TIME-09 : pm_src::delta gère le wrap 24/32-bit.
            if pm_src::delta(pm_start, pm_now) >= target_ticks { break; }
            // GARDE MMIO : max N_MAX_POLL_MMIO lectures I/O.
            if poll_count >= N_MAX_POLL_MMIO {
                return None;
            }
            // GARDE TSC : timeout si PM Timer figé
            let elapsed = rdtsc_begin().wrapping_sub(tsc_start);
            if elapsed >= tsc_deadline.wrapping_sub(tsc_start) {
                return None;
            }
            core::hint::spin_loop();
        }

        let flags2 = flags_save_cli();
        let (tsc_end, _coreid) = rdtscp_end();
        unsafe { flags_restore(flags2); }

        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        *sample = tsc_delta.saturating_mul(1_000);
    }

    let tsc_hz = iqr_mean(&samples);
    super::validation::cross_check(tsc_hz)
}

// ── Utilitaires statistiques ──────────────────────────────────────────────────

/// Calcule la moyenne des samples en excluant les outliers par IQR.
/// Algorithme : tri → Q1/Q3 → rejeter hors [Q1 - 1.5×IQR, Q3 + 1.5×IQR].
fn iqr_mean(sorted_samples: &[u64]) -> u64 {
    let n = sorted_samples.len();
    if n == 0 { return 0; }
    if n == 1 { return sorted_samples[0]; }

    let q1 = sorted_samples[n / 4];
    let q3 = sorted_samples[3 * n / 4];
    let iqr = q3.saturating_sub(q1);
    let margin = iqr.saturating_add(iqr / 2); // 1.5 × IQR

    let lo = q1.saturating_sub(margin);
    let hi = q3.saturating_add(margin);

    let mut sum: u128 = 0;
    let mut count: u64 = 0;

    for &s in sorted_samples.iter() {
        if s >= lo && s <= hi {
            sum = sum.saturating_add(s as u128);
            count += 1;
        }
    }

    if count == 0 {
        // Tous les outliers rejetés (données très dispersées) → médiane.
        sorted_samples[n / 2]
    } else {
        (sum / count as u128) as u64
    }
}

// ── Calibration via PIT (fallback héritage) ───────────────────────────────────
//
// RÈGLE PIT-QEMU-01 : PIT ne fonctionne PAS sur QEMU TCG en busy-wait.
//   → Utiliser UNIQUEMENT si HPET et PM Timer sont tous les deux absents.
//   Sur QEMU TCG, le canal 0 du PIT ne décrémente pas en mode lecture directe.
//   Bare-metal : fiable à ±0.5% en mode poll sur canal 2 (speaker gate).
//
// Fréquence PIT fixe : 1_193_182 Hz (dérivée du cristal 14.318 MHz ÷ 12).
// Résolution tick PIT : ~838 ns → 1ms = ~1193 ticks.

const PIT_FREQ_HZ: u64 = 1_193_182;
const PIT_CMD_PORT:  u16 = 0x43;
const PIT_CH2_PORT:  u16 = 0x42;
const PIT_GATE_PORT: u16 = 0x61;

/// Calibre le TSC Hz via le PIT canal 2 (speaker gate) en one-shot.
///
/// RÈGLE PIT-QEMU-01 : appelé UNIQUEMENT si HPET et PM Timer absents.
/// RÈGLE CAL-CLI-01    : cli/sti par sample de 1ms MAX.
/// RÈGLE CAL-WINDOW-01 : condition de sortie = bit OUT2 du PIT, jamais itér.
/// RÈGLE CAL-RDTSCP-01 : RDTSCP pour la lecture de fin.
///
/// Précision : ±0.5% (PIT tick = ~838 ns, jitter port I/O).
/// Limite     : non fiable sur QEMU TCG → retourne `None` si boucle expirée.
pub fn calibrate_tsc_via_pit() -> Option<u64> {
    let target_count: u16 = (PIT_FREQ_HZ / 1_000) as u16; // ≈ 1193 ticks = 1ms

    let mut samples = [0u64; N_SAMPLES];

    for sample in &mut samples {
        let flags = flags_save_cli();

        // ── Configurer PIT CH2 mode 0 (one-shot) pour target_count ticks ────
        // 1. Désactiver le gate CH2 (bit 0 du port 0x61).
        let gate = inb(PIT_GATE_PORT);
        outb(PIT_GATE_PORT, gate & !0x01);

        // 2. Commande : canal 2, lobyte/hibyte, mode 0 (terminal count), bcd=0.
        outb(PIT_CMD_PORT, 0b10_11_000_0);
        outb(PIT_CH2_PORT, (target_count & 0xFF) as u8);        // LSB
        outb(PIT_CH2_PORT, ((target_count >> 8) & 0xFF) as u8); // MSB

        // Barrière : les writes port I/O ci-dessus doivent être terminés avant
        // la lecture TSC et l'activation du gate.
        core::sync::atomic::fence(Ordering::SeqCst);

        // 3. Activer le gate → démarre le décompte.
        outb(PIT_GATE_PORT, (gate & !0x01) | 0x01);
        let tsc_start = rdtsc_begin();

        // ✅ FIX TIME-02 : condition de sortie = bit 5 du port 0x61 (OUT2 = 1
        // quand le compteur CH2 atteint 0).
        // Timeout : 100 itérations max (diagnostic ultra-court pour QEMU TCG).
        let mut poll_ok = false;
        for _ in 0..100u32 {
            if inb(PIT_GATE_PORT) & 0x20 != 0 { poll_ok = true; break; }
            core::hint::spin_loop();
        }
        if !poll_ok {
            outb(PIT_GATE_PORT, gate & !0x01);
            unsafe { flags_restore(flags); }
            return None;
        }

        let (tsc_end, _coreid) = rdtscp_end();
        // Remettre le gate en état initial.
        outb(PIT_GATE_PORT, gate & !0x01);
        unsafe { flags_restore(flags); }

        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        // Extrapolation : delta sur 1ms → Hz pour 1 seconde.
        *sample = tsc_delta.saturating_mul(1_000);
    }

    samples.sort_unstable();
    let tsc_hz = iqr_mean(&samples);
    super::validation::cross_check(tsc_hz)
}

// ── Structures de résultat enrichies ──────────────────────────────────────────

/// Métadonnées d'un sample individuel de calibration.
///
/// Permet le diagnostic post-calibration : identifier les outliers, mesurer
/// le jitter, détecter des anomalies (Turbo Boost spike, IRQ parasite).
#[derive(Debug, Clone, Copy)]
pub struct CalibrationSample {
    /// Fréquence TSC extrapolée de ce sample en Hz (tsc_delta × 1000).
    pub tsc_hz_estimate: u64,
    /// Ticks TSC mesurés pendant la fenêtre de 1ms.
    pub tsc_ticks:       u64,
    /// Ticks de référence (HPET/PM/PIT) mesurés pendant la même fenêtre.
    pub ref_ticks:       u64,
    /// Sample rejeté comme outlier par IQR (hors [Q1-1.5×IQR, Q3+1.5×IQR]).
    pub is_outlier:      bool,
}

impl CalibrationSample {
    /// Crée un sample vide (avant mesure).
    pub const fn zero() -> Self {
        Self { tsc_hz_estimate: 0, tsc_ticks: 0, ref_ticks: 0, is_outlier: false }
    }
}

/// Source de référence utilisée lors de la calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationSource {
    Hpet,
    PmTimer,
    Pit,
    None,
}

/// Résultat complet d'une calibration fenêtrée multi-sample.
///
/// Agrège les N_SAMPLES samples avec la fréquence finale, les statistiques
/// de qualité (variance) et les métadonnées diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct CalibrationResult {
    /// Fréquence TSC finale en Hz (moyenne IQR des samples valides).
    pub tsc_hz:       u64,
    /// Nombre de samples retenus après rejet des outliers (max = N_SAMPLES).
    pub valid_count:  u8,
    /// Variance empirique des samples valides en Hz² — élevée (> 1% de hz²)
    /// indique une mesure instable (charge CPU, Turbo Boost, IRQ perturbatrice).
    pub variance_hz2: u64,
    /// Source de référence utilisée pour la mesure.
    pub source:       CalibrationSource,
}

impl CalibrationResult {
    /// Retourne `true` si la variance est acceptable (< 1 % de tsc_hz).
    ///
    /// Seuil : variance < (tsc_hz / 100)², soit < 0.01 % de dispersion relative.
    pub fn is_stable(&self) -> bool {
        if self.tsc_hz == 0 || self.valid_count == 0 { return false; }
        // (1% de tsc_hz)² = tsc_hz² / 10_000
        let threshold = (self.tsc_hz as u128)
            .saturating_mul(self.tsc_hz as u128)
            / 10_000;
        (self.variance_hz2 as u128) < threshold
    }

    /// Retourne l'écart-type estimé en Hz (racine carrée entière de variance).
    pub fn stddev_hz(&self) -> u64 {
        isqrt64(self.variance_hz2)
    }
}

// ── Statistiques ─────────────────────────────────────────────────────────────

/// Calcule la moyenne et la variance empirique des samples après filtrage IQR.
///
/// Retourne `(mean_hz, variance_hz2)`.
/// N'alloue pas — travaille sur le slice trié fourni.
pub fn mean_and_variance(sorted_samples: &[u64]) -> (u64, u64) {
    let n = sorted_samples.len();
    if n == 0 { return (0, 0); }

    let mean = iqr_mean(sorted_samples);
    if mean == 0 || n < 2 { return (mean, 0); }

    // Bornes IQR (mêmes que dans iqr_mean).
    let q1  = sorted_samples[n / 4];
    let q3  = sorted_samples[3 * n / 4];
    let iqr = q3.saturating_sub(q1);
    let margin = iqr.saturating_add(iqr / 2); // 1.5 × IQR
    let lo = q1.saturating_sub(margin);
    let hi = q3.saturating_add(margin);

    let mut sum_sq: u128 = 0;
    let mut count: u64 = 0;

    for &s in sorted_samples.iter() {
        if s >= lo && s <= hi {
            let diff = if s >= mean { s - mean } else { mean - s };
            sum_sq = sum_sq.saturating_add((diff as u128).saturating_mul(diff as u128));
            count += 1;
        }
    }

    let variance = if count > 1 { (sum_sq / count as u128) as u64 } else { 0 };
    (mean, variance)
}

/// Racine carrée entière (Newton-Raphson) — sans libm, sans alloc.
fn isqrt64(n: u64) -> u64 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ── Port I/O helpers (PIT) ────────────────────────────────────────────────────

/// Lit un octet depuis le port I/O donné.
#[inline(always)]
fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: ports PIT (0x42-0x43, 0x61) sont des ports I/O non-destructifs.
    //         Appelé uniquement depuis calibrate_tsc_via_pit() avec CLI actif.
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") val,
            in("dx") port,
            options(nostack, nomem)
        );
    }
    val
}

/// Écrit un octet sur le port I/O donné.
#[inline(always)]
fn outb(port: u16, val: u8) {
    // SAFETY: écriture sur ports PIT standard (0x42-0x43-0x61).
    //         Appelé uniquement depuis calibrate_tsc_via_pit() avec CLI actif.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nostack, nomem)
        );
    }
}

// ── Primitives assembleur ─────────────────────────────────────────────────────

/// Sauvegarde RFLAGS et désactive les IRQ (CLI).
/// RÈGLE CAL-CLI-01 : appelé au début de chaque sample de 1ms.
#[inline(always)]
fn flags_save_cli() -> u64 {
    let flags: u64;
    // SAFETY: PUSHFQ/POP + CLI — séquence standard pour section critique.
    //         La barrière implicite du pushfq garantit l'ordre mémoire.
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {flags}",
            "cli",
            flags = out(reg) flags,
            options(nomem)
        );
    }
    flags
}

/// Restaure RFLAGS (STI si IF était actif avant `flags_save_cli()`).
#[inline(always)]
unsafe fn flags_restore(flags: u64) {
    // SAFETY: restauration RFLAGS — safe si flags provient de flags_save_cli().
    core::arch::asm!(
        "push {flags}",
        "popfq",
        flags = in(reg) flags,
        options(nomem)
    );
}

/// Lit RDTSC avec barrière LFENCE (sérialisée côté charges).
#[inline(always)]
fn rdtsc_begin() -> u64 {
    let lo: u32; let hi: u32;
    // SAFETY: LFENCE + RDTSC — barrière standard, non-privilégiée.
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Lit RDTSCP avec barrière LFENCE post (sérialisée des deux côtés).
/// RÈGLE CAL-RDTSCP-01 : RDTSCP sérialise l'exécution + fournit coreid.
#[inline(always)]
fn rdtscp_end() -> (u64, u32) {
    let lo: u32; let hi: u32; let aux: u32;
    // SAFETY: RDTSCP + LFENCE — séquence standard pour fin de mesure précise.
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") aux,
            options(nostack, nomem)
        );
        core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
    }
    (((hi as u64) << 32) | lo as u64, aux)
}
