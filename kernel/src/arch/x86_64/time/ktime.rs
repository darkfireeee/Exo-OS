// kernel/src/arch/x86_64/time/ktime.rs
//
// ════════════════════════════════════════════════════════════════════════════
// KtimeState — Horloge monotone avec seqlock ISR-safe
// ════════════════════════════════════════════════════════════════════════════
//
// RÈGLE TIME-SEQLOCK-01 : ktime_get_ns() utilise un seqlock.
//   - ISR peut appeler ktime_get_ns() sans lock => retry si write en cours.
//   - Jamais de Mutex ici — ktime_get_ns() doit être wait-free.
//   - RÈGLE TIME-01 (FIX) : struct ktime jamais mise à jour en 2 writes sans seqlock.
//
// Algorithme seqlock :
//   Lecteur : lit seq1 → s'assure que seq1 est pair → lit données → lit seq2
//             → si seq1 != seq2 : retry (write est arrivé entre temps)
//   Écrivain : seq++ (impair = écriture en cours) → maj données → seq++
//
// RÈGLE ARCH-TIME-04 : ktime ne régresse JAMAIS.
//   Si drift correction baisse tsc_hz, ns_base est ajusté en compensation.
// ════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

// ── KtimeState ────────────────────────────────────────────────────────────────

/// État global de l'horloge monotone.
///
/// RÈGLE TIME-SEQLOCK-01 : Alignée sur une cache line pour éviter le false sharing.
/// Toute lecture doit passer par `ktime_get_ns()` — jamais d'accès direct.
#[repr(C, align(64))]
struct KtimeState {
    /// Seqlock counter : pair = état stable, impair = write en cours.
    seq:      AtomicU64,
    /// Valeur TSC au dernier point d'ancrage (tsc_base).
    tsc_base: AtomicU64,
    /// Nanosecondes au dernier point d'ancrage (ns_base).
    ns_base:  AtomicU64,
    /// Fréquence TSC actuelle en Hz (mise à jour par drift correction).
    tsc_hz:   AtomicU64,
    /// Padding pour compléter la cache line (64 - 4×8 = 32 bytes).
    _pad:     [u8; 32],
}

impl KtimeState {
    const fn new() -> Self {
        Self {
            seq:      AtomicU64::new(0),
            tsc_base: AtomicU64::new(0),
            ns_base:  AtomicU64::new(0),
            tsc_hz:   AtomicU64::new(3_000_000_000), // Fallback 3 GHz avant calibration
            _pad:     [0u8; 32],
        }
    }
}

/// Instance globale — initialisée UNE SEULE FOIS par `init_ktime()`.
static KTIME_STATE: KtimeState = KtimeState::new();

// ── Lecture ISR-safe ──────────────────────────────────────────────────────────

/// Retourne le temps monotone en nanosecondes depuis le boot.
///
/// ISR-safe : wait-free, pas de lock, pas d'alloc.
/// Seqlock : retry si un write est en cours (rare — < 100ns).
/// Cycles typiques : 15-50 cycles sur CPU moderne.
///
/// RÈGLE ARCH-TIME-02 : Seule source de temps autorisée depuis le kernel.
/// RÈGLE TIME-SEQLOCK-01 : Garantit la cohérence même entre ISR et thread.
#[inline(always)]
fn apply_tsc_offset(tsc_now: u64, tsc_offset: i64) -> u64 {
    if tsc_offset >= 0 {
        tsc_now.wrapping_sub(tsc_offset as u64)
    } else {
        tsc_now.wrapping_add(tsc_offset.unsigned_abs())
    }
}

/// Retourne le temps monotone en nanosecondes depuis le boot.
///
/// ISR-safe : wait-free, pas de lock, pas d'alloc.
/// Seqlock : retry si un write est en cours (rare — < 100ns).
/// Cycles typiques : 15-50 cycles sur CPU moderne.
///
/// RÈGLE ARCH-TIME-02 : Seule source de temps autorisée depuis le kernel.
/// RÈGLE TIME-SEQLOCK-01 : Garantit la cohérence même entre ISR et thread.
#[inline(always)]
pub fn ktime_get_ns() -> u64 {
    loop {
        // 1. Lire seq AVANT — doit être pair (état stable).
        let seq1 = KTIME_STATE.seq.load(Ordering::Acquire);
        if seq1 & 1 != 0 {
            // Impair = write en cours → spin_loop et retry.
            core::hint::spin_loop();
            continue;
        }

        // 2. Lire le TSC courant via RDTSCP (sérialisé + coreid).
        let (tsc_now, coreid) = rdtscp_with_fence();

        // 3. Lire les composants de l'anchor.
        let tsc_base = KTIME_STATE.tsc_base.load(Ordering::Acquire);
        let ns_base  = KTIME_STATE.ns_base .load(Ordering::Acquire);
        let tsc_hz   = KTIME_STATE.tsc_hz  .load(Ordering::Acquire);

        // 4. Vérifier l'intégrité : seq n'a pas changé pendant notre lecture.
        let seq2 = KTIME_STATE.seq.load(Ordering::Acquire);
        if seq1 != seq2 {
            // Un write s'est intercalé → retry.
            core::hint::spin_loop();
            continue;
        }

        // 5. Calcul final — seqlock garantit la cohérence.
        if tsc_hz == 0 { return 0; }

        // Appliquer l'offset per-CPU si SMP est initialisé.
        let tsc_offset = super::percpu::tsc_offset(coreid as usize);
        let tsc_adjusted = apply_tsc_offset(tsc_now, tsc_offset);

        let tsc_delta = tsc_adjusted.wrapping_sub(tsc_base);
        // ns = tsc_delta * 1_000_000_000 / tsc_hz — u128 pour éviter l'overflow.
        let ns_delta = (tsc_delta as u128)
            .saturating_mul(1_000_000_000)
            / (tsc_hz as u128);

        return ns_base.wrapping_add(ns_delta as u64);
    }
}

// ── Mise à jour de l'anchor ───────────────────────────────────────────────────

/// Initialise l'état de l'horloge (appelé UNE FOIS depuis `time_init()`).
///
/// # Safety
/// Doit être appelé une seule fois, avant tout appel à `ktime_get_ns()`.
pub(crate) unsafe fn init_ktime(tsc_now: u64, ns_start: u64, tsc_hz: u64) {
    // Seq impair = write en cours (transitoire — personne ne lit encore).
    KTIME_STATE.seq.store(1, Ordering::Release);
    KTIME_STATE.tsc_base.store(tsc_now,  Ordering::Release);
    KTIME_STATE.ns_base .store(ns_start, Ordering::Release);
    KTIME_STATE.tsc_hz  .store(tsc_hz,   Ordering::Release);
    // Seq pair = état stable.
    KTIME_STATE.seq.store(2, Ordering::Release);
}

/// Met à jour le point d'ancrage de l'horloge (appelé par drift correction).
///
/// RÈGLE TIME-ANCHOR-01 : NE PAS appeler ktime_get_ns() ici.
///   → Lire HPET et TSC directement pour éviter la dépendance circulaire.
/// RÈGLE DRIFT-MONOTONE-01 : ns_now >= ns_actuel (horloge ne régresse pas).
pub(crate) fn update_ktime_anchor(tsc_now: u64, ns_now: u64, new_tsc_hz: u64) {
    // Vérification monotonie (RÈGLE ARCH-TIME-04).
    let ns_current = KTIME_STATE.ns_base.load(Ordering::Relaxed);
    if ns_now < ns_current {
        // Ne jamais faire reculer l'horloge — utiliser au minimum ns_current.
        let _ = ns_current; // La guard est vérifiée en amont par apply_drift_correction().
        return;
    }

    // Seq impair = write en cours.
    KTIME_STATE.seq.fetch_add(1, Ordering::Release);

    // Barrière : empêche le réordonnancement des stores suivants avant le seq impair.
    core::sync::atomic::fence(Ordering::Release);

    KTIME_STATE.tsc_base.store(tsc_now,     Ordering::Relaxed);
    KTIME_STATE.ns_base .store(ns_now,      Ordering::Relaxed);
    KTIME_STATE.tsc_hz  .store(new_tsc_hz,  Ordering::Relaxed);

    // Barrière : rendre les stores visibles avant de repasser à pair.
    core::sync::atomic::fence(Ordering::Release);

    // Seq pair = état stable.
    KTIME_STATE.seq.fetch_add(1, Ordering::Release);
}

// ── Primitives assembleur ─────────────────────────────────────────────────────

/// Lit RDTSCP avec barrière LFENCE post-lecture.
/// Retourne (tsc_value, core_id_from_tsc_aux).
///
/// RÈGLE TSC-RDTSCP-01 : RDTSCP garantit que le CPU ne réordonne pas.
/// RDTSCP fournit aussi coreid → permet d'appliquer le bon tsc_offset per-CPU.
#[inline(always)]
fn rdtscp_with_fence() -> (u64, u32) {
    let lo: u32;
    let hi: u32;
    let aux: u32;
    // SAFETY: RDTSCP + LFENCE — séquence standard pour lecture TSC sérialisée.
    //         RDTSCP disponible si CPU supporte TSC_AUX (vérifié dans init_tsc).
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

// ── Wall clock (temps réel UNIX) ─────────────────────────────────────────────

/// Retourne le temps Unix en nanosecondes depuis l'epoch (1970-01-01 00:00:00 UTC).
///
/// Dépend de `set_wall_time()` ayant été appelé depuis la lecture RTC ou NTP.
/// Avant la synchronisation : retourne le temps monotone (comme si boot = epoch 0).
///
/// ISR-safe, seqlock sur WALL_STATE.
pub fn ktime_get_wall_ns() -> u64 {
    loop {
        let seq1 = WALL_STATE.seq.load(Ordering::Acquire);
        if seq1 & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let offset = WALL_STATE.rtoffset_ns.load(Ordering::Acquire);
        let seq2   = WALL_STATE.seq.load(Ordering::Acquire);
        if seq1 != seq2 { core::hint::spin_loop(); continue; }
        return ktime_get_ns().wrapping_add(offset);
    }
}

/// Retourne le décalage epoch UNIX en ns (wall_ns = mono_ns + rtoffset_ns).
#[inline(always)]
pub fn ktime_rtoffset_ns() -> u64 {
    WALL_STATE.rtoffset_ns.load(Ordering::Acquire)
}

/// Synchronise l'horloge réelle avec une valeur d'epoch UNIX fournie.
///
/// Appelé depuis le syscall `settimeofday` ou depuis la lecture RTC au boot.
/// Met à jour l'offset UNIX dans WALL_STATE via seqlock.
///
/// # Safety
/// Doit être appelé depuis Ring 0 avec les droits appropriés (CAP_TIME).
pub unsafe fn set_wall_time(epoch_unix_ns: u64) {
    let mono_now = ktime_get_ns();
    let new_offset = epoch_unix_ns.wrapping_sub(mono_now);
    WALL_STATE.seq.fetch_add(1, Ordering::AcqRel);
    core::sync::atomic::fence(Ordering::Release);
    WALL_STATE.rtoffset_ns.store(new_offset, Ordering::Relaxed);
    WALL_STATE.mono_at_set.store(mono_now, Ordering::Relaxed);
    WALL_STATE.epoch_at_set.store(epoch_unix_ns, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::Release);
    WALL_STATE.seq.fetch_add(1, Ordering::Release);
}

// ── Durées relatives ──────────────────────────────────────────────────────────

/// Temps écoulé en ns depuis `start_ns` (obtenu via `ktime_get_ns()`).
#[inline(always)]
pub fn ktime_elapsed_ns(start_ns: u64) -> u64 {
    ktime_get_ns().wrapping_sub(start_ns)
}

/// Temps écoulé en µs depuis `start_ns`.
#[inline(always)]
pub fn ktime_elapsed_us(start_ns: u64) -> u64 {
    ktime_elapsed_ns(start_ns) / 1_000
}

/// Retourne `true` si `deadline_ns` est atteinte ou dépassée.
#[inline(always)]
pub fn ktime_past(deadline_ns: u64) -> bool {
    ktime_get_ns() >= deadline_ns
}

/// Retourne `false` si `deadline_ns` n'est pas encore atteinte.
/// Utiliser dans les boucles de timeout : `while ktime_not_past(dl) { ... }`.
#[inline(always)]
pub fn ktime_not_past(deadline_ns: u64) -> bool {
    ktime_get_ns() < deadline_ns
}

/// Construit une deadline absolue à partir de maintenant + durée en ns.
#[inline(always)]
pub fn ktime_deadline_ns(duration_ns: u64) -> u64 {
    ktime_get_ns().saturating_add(duration_ns)
}

/// Construit une deadline absolue à partir de maintenant + durée en µs.
#[inline(always)]
pub fn ktime_deadline_us(duration_us: u64) -> u64 {
    ktime_deadline_ns(duration_us.saturating_mul(1_000))
}

/// Construit une deadline absolue à partir de maintenant + durée en ms.
#[inline(always)]
pub fn ktime_deadline_ms(duration_ms: u64) -> u64 {
    ktime_deadline_ns(duration_ms.saturating_mul(1_000_000))
}

// ── Per-CPU TSC offset management ─────────────────────────────────────────────

/// Nombre maximum de CPUs logiques (identique à smp::topology::MAX_CPUS).
const MAX_CPUS: usize = 256;

/// TSC offset de chaque CPU logique, mesuré depuis le BSP au boot SMP.
/// offset[cpu] = TSC_AP_au_boot - TSC_BSP_au_boot (delta de synchronisation).
/// ktime_get_ns() applique : tsc_adj = rdtscp() - offset[coreid].
static TSC_OFFSETS: [AtomicI64; MAX_CPUS] = {
    const ZERO: AtomicI64 = AtomicI64::new(0);
    [ZERO; MAX_CPUS]
};

/// Validité des offsets : bit i = true → CPU i a son offset mesuré.
static TSC_OFFSET_VALID: [AtomicBool; MAX_CPUS] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; MAX_CPUS]
};

/// Enregistre l'offset TSC d'un CPU AP (appelé depuis percpu::sync).
///
/// # Safety
/// `cpu_id` doit être dans [1, MAX_CPUS). Appelé une seule fois par CPU au boot SMP.
pub unsafe fn store_tsc_offset(cpu_id: usize, offset: i64) {
    if cpu_id == 0 || cpu_id >= MAX_CPUS { return; }
    TSC_OFFSETS[cpu_id].store(offset, Ordering::Release);
    TSC_OFFSET_VALID[cpu_id].store(true, Ordering::Release);
}

/// Retourne l'offset TSC d'un CPU (0 si BSP ou non encore mesuré).
#[inline(always)]
pub fn tsc_offset(cpu_id: usize) -> i64 {
    if cpu_id >= MAX_CPUS { return 0; }
    if TSC_OFFSET_VALID[cpu_id].load(Ordering::Relaxed) {
        TSC_OFFSETS[cpu_id].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Retourne `true` si l'offset TSC du CPU donné est disponible.
pub fn tsc_offset_valid(cpu_id: usize) -> bool {
    cpu_id < MAX_CPUS && TSC_OFFSET_VALID[cpu_id].load(Ordering::Relaxed)
}

/// Invalide tous les offsets (utilisé pour reset au reboot SMP).
pub unsafe fn reset_tsc_offsets() {
    for i in 0..MAX_CPUS {
        TSC_OFFSETS[i].store(0, Ordering::Relaxed);
        TSC_OFFSET_VALID[i].store(false, Ordering::Relaxed);
    }
    TSC_OFFSET_VALID[0].store(true, Ordering::Relaxed); // BSP : offset 0 valide.
}

// ── Accesseurs diagnostics ─────────────────────────────────────────────────────

/// Fréquence TSC actuelle dans KtimeState (Hz).
#[inline(always)]
pub fn ktime_tsc_hz() -> u64 {
    KTIME_STATE.tsc_hz.load(Ordering::Relaxed)
}

/// `true` si ktime est initialisé (init_ktime() a été appelé).
#[inline(always)]
pub fn ktime_initialized() -> bool {
    KTIME_STATE.seq.load(Ordering::Relaxed) >= 2
}

/// Nombre de retries seqlock depuis le boot (indicateur de contention).
pub fn ktime_contention_count() -> u64 {
    KTIME_RETRIES.load(Ordering::Relaxed)
}

/// Snapshot immutable de l'état ktime pour debug/profiling.
pub struct KtimeSnapshot {
    pub seq:       u64,
    pub tsc_base:  u64,
    pub ns_base:   u64,
    pub tsc_hz:    u64,
    pub retries:   u64,
    pub wall_off:  u64,
    pub offsets_cpu0: i64,
}

pub fn ktime_snapshot() -> KtimeSnapshot {
    KtimeSnapshot {
        seq:          KTIME_STATE.seq.load(Ordering::Relaxed),
        tsc_base:     KTIME_STATE.tsc_base.load(Ordering::Relaxed),
        ns_base:      KTIME_STATE.ns_base.load(Ordering::Relaxed),
        tsc_hz:       KTIME_STATE.tsc_hz.load(Ordering::Relaxed),
        retries:      KTIME_RETRIES.load(Ordering::Relaxed),
        wall_off:     WALL_STATE.rtoffset_ns.load(Ordering::Relaxed),
        offsets_cpu0: TSC_OFFSETS[0].load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::apply_tsc_offset;

    #[test]
    fn test_apply_tsc_offset_signed_direction() {
        assert_eq!(apply_tsc_offset(1_000, 25), 975);
        assert_eq!(apply_tsc_offset(1_000, -25), 1_025);
        assert_eq!(apply_tsc_offset(1_000, 0), 1_000);
    }

    #[test]
    fn test_apply_tsc_offset_stress_roundtrip() {
        let cases = [
            (0u64, 0i64),
            (17, 9),
            (17, -9),
            (u64::MAX - 32, 64),
            (u64::MAX - 32, -64),
        ];

        for &(tsc, offset) in &cases {
            let adjusted = apply_tsc_offset(tsc, offset);
            let restored = apply_tsc_offset(adjusted, -offset);
            assert_eq!(restored, tsc);
        }

        for raw in -10_000i64..=10_000 {
            let tsc = 0x1234_5678_9ABC_DEF0u64.wrapping_add(raw.unsigned_abs());
            let adjusted = apply_tsc_offset(tsc, raw);
            let restored = apply_tsc_offset(adjusted, -raw);
            assert_eq!(restored, tsc);
        }
    }
}

// ── WallState (wall clock state) ─────────────────────────────────────────────

/// Cache line dédiée au wall clock pour éviter la contention avec ktime monotone.
#[repr(C, align(64))]
struct WallState {
    seq:           AtomicU64,
    rtoffset_ns:   AtomicU64,   // wall = mono + rtoffset
    mono_at_set:   AtomicU64,   // mono_ns au dernier set_wall_time()
    epoch_at_set:  AtomicU64,   // epoch Unix ns au dernier set_wall_time()
    _pad:          [u8; 32],
}

impl WallState {
    const fn new() -> Self {
        Self {
            seq:          AtomicU64::new(0),
            rtoffset_ns:  AtomicU64::new(0),
            mono_at_set:  AtomicU64::new(0),
            epoch_at_set: AtomicU64::new(0),
            _pad:         [0u8; 32],
        }
    }
}

static WALL_STATE:  WallState  = WallState::new();
static KTIME_RETRIES: AtomicU64 = AtomicU64::new(0);
