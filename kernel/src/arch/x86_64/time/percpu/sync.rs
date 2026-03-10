// kernel/src/arch/x86_64/time/percpu/sync.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Synchronisation du TSC inter-processeurs (SMP boot)
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Problème
//   Sur les systèmes multi-socket et après INIT/SIPI, les CPU peuvent avoir
//   des valeurs TSC différentes. Lire RDTSC sur le CPU 2 peut retourner
//   une valeur décalée de ±50 000 ns par rapport au BSP.
//
//   RÈGLE TSC-SYNC-01 : Mesurer le décalage TSC de chaque AP (Application Processor)
//     par rapport au BSP (Bootstrap Processor) au démarrage SMP.
//
// ## Méthode : IPI Round-Trip Timing
//   1. BSP note tsc_bsp_start = RDTSCP()
//   2. BSP envoie un IPI à l'AP
//   3. AP répond avec sa valeur tsc_ap = RDTSCP() dès réception de l'IPI
//   4. BSP note tsc_bsp_end = RDTSCP()
//   5. Propagation estimée = (tsc_bsp_end - tsc_bsp_start) / 2
//   6. Offset = tsc_ap - (tsc_bsp_start + propagation) [en cycles TSC]
//
//   Pour ktime : si offset > 0 → AP est en avance → on soustrait l'offset.
//               si offset < 0 → AP est en retard  → on ajoute |offset|.
//
// ## Alternative sur processeurs modernes
//   Sur Intel multi-socket avec TSC_ADJUST MSR (Skylake+), les offsets sont
//   gérés par le firmware (BIOS reset TSC à 0 via INIT). Dans ce cas,
//   on vérifie la cohérence et on adopte l'offset BIOS si < seuil.
//
// ## Limitation Phase 1
//   L'infrastructure IPI (LAPIC ICR) n'est pas encore disponible.
//   On utilise un mécanisme simplifié : le BSP lit le TSC juste avant le
//   déverrouillage de l'AP, l'AP lit le sien juste après le wake-up.
//   La précision est ~100–500 ns (suffisant pour corriger des skews de µs).
//
// ════════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use crate::arch::x86_64::time::ktime;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Nombre maximum de CPU supportés.
const MAX_CPUS: usize = 256;
/// Seuil d'offset TSC au-dessus duquel on loggue un avertissement (100 µs en cycles à 3 GHz).
const OFFSET_WARN_CYCLES: u64 = 300_000; // ~100 µs à 3 GHz
/// Nombre de mesures répétées pour la précision de la synchronisation.
const SYNC_ITERATIONS: usize = 10;

// ── Zone de communication inter-CPU ──────────────────────────────────────────
//
// Ces tableaux sont partagés entre BSP et APs pendant la procédure de sync.
// Utilisation : chaque AP écrit dans la case de son CPU ID.

/// TSC de l'AP au moment du wake-up (écrit par l'AP, lu par le BSP).
static AP_TSC: [AtomicU64; MAX_CPUS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_CPUS]
};

/// Signal BSP → AP : le BSP a noté tsc_start, l'AP peut lire son TSC.
static BSP_READY: [AtomicBool; MAX_CPUS] = {
    const INIT: AtomicBool = AtomicBool::new(false);
    [INIT; MAX_CPUS]
};

/// TSC du BSP juste avant d'envoyer le signal à l'AP.
static BSP_TSC_BEFORE: [AtomicU64; MAX_CPUS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_CPUS]
};

/// TSC du BSP juste après confirmation de l'AP.
static BSP_TSC_AFTER: [AtomicU64; MAX_CPUS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_CPUS]
};

/// Confirmation que l'AP a terminé sa mesure.
static AP_DONE: [AtomicBool; MAX_CPUS] = {
    const INIT: AtomicBool = AtomicBool::new(false);
    [INIT; MAX_CPUS]
};

// ── Mesure depuis le BSP ──────────────────────────────────────────────────────

/// Mesure le décalage TSC d'un AP par rapport au BSP et le stocke dans ktime.
///
/// Doit être appelé depuis le BSP (CPU 0) pendant la procédure de boot SMP,
/// après que l'AP a été wake-up mais avant qu'il ne commence à exécuter du code
/// non-synchronisé.
///
/// # Paramètres
///   `ap_cpu_id` : ID du CPU AP à mesurer (1..255)
///
/// # Comportement
///   Exécute SYNC_ITERATIONS mesures et prend la médiane pour réduire le jitter.
///   Stocke l'offset via `ktime::store_tsc_offset(ap_cpu_id, offset_signed)`.
///
/// # Sécurité
///   Unsafe : accès direct aux MSR TSC, coordination avec l'AP via atomics partagés.
pub unsafe fn measure_tsc_offset_for_ap(ap_cpu_id: u32) {
    if ap_cpu_id as usize >= MAX_CPUS { return; }
    let idx = ap_cpu_id as usize;

    // Reset des signaux de communication.
    BSP_READY[idx].store(false, Ordering::Release);
    AP_TSC[idx].store(0, Ordering::Release);
    AP_DONE[idx].store(false, Ordering::Release);

    let mut samples: [i64; SYNC_ITERATIONS] = [0; SYNC_ITERATIONS];

    for i in 0..SYNC_ITERATIONS {
        // Étape 1 : BSP note tsc_before juste avant le signal.
        let tsc_before = rdtscp();
        BSP_TSC_BEFORE[idx].store(tsc_before, Ordering::Release);

        // Étape 2 : Signaler à l'AP qu'il peut lire son TSC.
        BSP_READY[idx].store(true, Ordering::Release);

        // Étape 3 : Attendre que l'AP ait lu son TSC.
        let mut timeout = 0u64;
        while !AP_DONE[idx].load(Ordering::Acquire) {
            core::arch::asm!("pause", options(nostack, nomem));
            timeout += 1;
            if timeout > 10_000_000 {
                // L'AP ne répond pas — probablement pas encore en ligne.
                return;
            }
        }

        // Étape 4 : BSP note tsc_after.
        let tsc_after = rdtscp();
        BSP_TSC_AFTER[idx].store(tsc_after, Ordering::Release);

        let ap_tsc = AP_TSC[idx].load(Ordering::Acquire);

        // Étape 5 : Calcul de l'offset.
        // propagation_half = (tsc_after - tsc_before) / 2
        let propagation_half = (tsc_after.wrapping_sub(tsc_before)) / 2;
        // Référence BSP au moment de la lecture AP = tsc_before + propagation_half
        let bsp_ref = tsc_before.wrapping_add(propagation_half);
        // offset = ap_tsc - bsp_ref (signé : positif = AP en avance)
        let offset = ap_tsc as i64 - bsp_ref as i64;
        samples[i] = offset;

        // Reset pour la prochaine itération.
        BSP_READY[idx].store(false, Ordering::Release);
        AP_DONE[idx].store(false, Ordering::Release);
    }

    // Calculer la médiane des échantillons.
    let offset_median = median_i64(&mut samples[..]);

    // Avertissement si l'offset est anormalement grand.
    if offset_median.unsigned_abs() > OFFSET_WARN_CYCLES {
        log_tsc_skew_warning(ap_cpu_id, offset_median);
    }

    // Stocker l'offset (signé, en cycles) dans ktime.
    // ktime::ktime_get_ns() soustrait l'offset si AP en avance :
    //   effective_tsc = rdtscp_value - tsc_offset[coreid]
    // Si AP en avance (offset > 0) → soustraire → correct.
    // Stockage en u64 (two's complement) : wrapping_sub dans ktime_get_ns() gère les offsets négatifs.
    ktime::store_tsc_offset(ap_cpu_id as usize, offset_median as u64);
}

// ── Côté AP ───────────────────────────────────────────────────────────────────

/// Exécuté par l'AP lors du boot SMP pour répondre au BSP.
///
/// Appelé depuis le code d'init AP (arch/x86_64/smp/ap_entry.rs ou équivalent)
/// après la configuration des registres de base (GDT/IDT/paging) mais avant
/// l'intégration dans le scheduler.
///
/// RÈGLE TSC-SYNC-01 : L'AP doit attendre le signal BSP_READY avant de lire son TSC.
pub unsafe fn ap_sync_tsc_response(ap_cpu_id: u32) {
    if ap_cpu_id as usize >= MAX_CPUS { return; }
    let idx = ap_cpu_id as usize;

    // Attendre que le BSP soit prêt.
    let mut timeout = 0u64;
    while !BSP_READY[idx].load(Ordering::Acquire) {
        core::arch::asm!("pause", options(nostack, nomem));
        timeout += 1;
        if timeout > 50_000_000 {
            // BSP ne signal pas — timeout → on abandonne la sync.
            return;
        }
    }

    // Lire le TSC de l'AP dès réception du signal.
    let ap_tsc = rdtscp();
    AP_TSC[idx].store(ap_tsc, Ordering::Release);
    AP_DONE[idx].store(true, Ordering::Release);
}

// ── BSP init ──────────────────────────────────────────────────────────────────

/// Initialise les données per-CPU du BSP (CPU 0).
/// L'offset du BSP est toujours 0 (référence).
pub fn init_bsp_percpu() {
    // SAFETY: cpu_id = 0, offset = 0. BSP est toujours la référence.
    unsafe { ktime::store_tsc_offset(0, 0); }
}

/// Vérifie si la synchronisation TSC a été effectuée pour un CPU donné.
pub fn tsc_synced(cpu_id: u32) -> bool {
    ktime::tsc_offset(cpu_id as usize) != 0 || cpu_id == 0
}

// ── Utilitaires ───────────────────────────────────────────────────────────────

/// Calcule la médiane d'un tableau d'entiers signés (tri in-place).
fn median_i64(samples: &mut [i64]) -> i64 {
    // Tri par insertion — tableau petit (SYNC_ITERATIONS = 10).
    for i in 1..samples.len() {
        let key = samples[i];
        let mut j = i;
        while j > 0 && samples[j - 1] > key {
            samples[j] = samples[j - 1];
            j -= 1;
        }
        samples[j] = key;
    }
    let n = samples.len();
    if n == 0 { return 0; }
    if n % 2 == 1 {
        samples[n / 2]
    } else {
        // Moyenne des deux médianes (sans overflow).
        let a = samples[n / 2 - 1];
        let b = samples[n / 2];
        a / 2 + b / 2 + (a % 2 + b % 2) / 2
    }
}

/// Émet une trace port 0xE9 pour signaler un skew TSC anormal.
fn log_tsc_skew_warning(cpu_id: u32, offset_cycles: i64) {
    // Port 0xE9 = canal de debug QEMU.
    // Format : "[TSC-SKEW cpu=XX off=XXXXXXXX]\n"
    unsafe {
        let buf = b"[TSC-SKEW cpu=";
        for &b in buf { core::arch::asm!("out 0xe9, al", in("al") b, options(nostack, nomem)); }
        // cpu_id decimal simple (0–255)
        let d2 = (cpu_id / 100) as u8 + b'0';
        let d1 = ((cpu_id / 10) % 10) as u8 + b'0';
        let d0 = (cpu_id % 10) as u8 + b'0';
        core::arch::asm!("out 0xe9, al", in("al") d2, options(nostack, nomem));
        core::arch::asm!("out 0xe9, al", in("al") d1, options(nostack, nomem));
        core::arch::asm!("out 0xe9, al", in("al") d0, options(nostack, nomem));
        let buf2 = b" off=";
        for &b in buf2 { core::arch::asm!("out 0xe9, al", in("al") b, options(nostack, nomem)); }
        // offset en hex (16 chiffres).
        let val = offset_cycles as u64;
        for shift in (0..64).step_by(4).rev() {
            let nib = ((val >> (60 - shift)) & 0xF) as u8;
            let ch = if nib < 10 { b'0' + nib } else { b'a' + nib - 10 };
            core::arch::asm!("out 0xe9, al", in("al") ch, options(nostack, nomem));
        }
        core::arch::asm!("out 0xe9, al", in("al") b']', options(nostack, nomem));
        core::arch::asm!("out 0xe9, al", in("al") b'\n', options(nostack, nomem));
    }
}

/// Lit le TSC avec sérialisation (RDTSCP fournit aussi le coreid).
#[inline(always)]
fn rdtscp() -> u64 {
    let lo: u32; let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") _,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}
