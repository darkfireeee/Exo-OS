// kernel/src/memory/integrity/canary.rs
//
// Stack canary — génération, stockage par CPU et vérification.
//
// Principe :
//   - À l'init, une valeur pseudo-aléatoire est générée via RDTSC + XOR.
//   - Cette valeur est placée dans la table CANARY_TABLE[cpu_id] (lecture seule
//     après init).
//   - Le prologue de chaque fonction instrumentée lit la valeur courante et
//     la pousse sur sa stack frame.  L'épilogue revérifie.
//   - En cas de divergence → `canary_violation_handler`.
//
// Exo-OS ajoute un second niveau : « thread canary » stocké dans le TCB,
// initialisé avec la valeur cpu_canary XOR tid.
//
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de CPUs supportés.
const MAX_CPUS: usize = 256;
/// Valeur sentinelle indiquant que la table n'est pas encore initialisée.
const CANARY_UNINIT: u64 = 0xDEAD_BEEF_CAFE_BABE;
/// Poison pour détecter une réutilisation de stack après violation.
const CANARY_POISON: u64 = 0x0000_0000_0000_0000;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct CanaryStats {
    /// Nombre de canaries initialisés (BSP + APs).
    pub init_count:      AtomicU64,
    /// Violations détectées.
    pub violation_count: AtomicU64,
    /// Régénérations forcées (rotation de clé).
    pub rotate_count:    AtomicU64,
    /// Vérifications réussies (métriques debug, coûteux en prod).
    pub check_ok:        AtomicU64,
}

impl CanaryStats {
    const fn new() -> Self {
        Self {
            init_count:      AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            rotate_count:    AtomicU64::new(0),
            check_ok:        AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for CanaryStats {}
pub static CANARY_STATS: CanaryStats = CanaryStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Table des canaries par CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Un slot de la table de canaries.
/// `align(64)` evite le false sharing entre CPUs adjacents.
#[repr(C, align(64))]
struct CanarySlot {
    value:       AtomicU64,
    generation:  AtomicU64,
    initialized: AtomicBool,
    _pad:        [u8; 39],
}

impl CanarySlot {
    const fn uninit() -> Self {
        Self {
            value:       AtomicU64::new(CANARY_UNINIT),
            generation:  AtomicU64::new(0),
            initialized: AtomicBool::new(false),
            _pad:        [0u8; 39],
        }
    }
}

/// Table des canaries CPU — MAX_CPUS × 64 octets = 16 KiB.
struct CanaryTable {
    slots: [CanarySlot; MAX_CPUS],
}

/// # SAFETY : accès par cpu_id exclusif → Sync garanti par protocole d'usage.
unsafe impl Sync for CanaryTable {}

/// Construction const de la table.
/// Rust stable ne supporte pas encore `[CanarySlot::uninit(); N]` pour N > 0
/// avec types non-Copy.  On contourne avec une macro de répétition.
macro_rules! canary_table_init {
    () => {
        CanaryTable {
            slots: {
                // SAFETY: CanarySlot #[repr(C)] sans padding invalide; zeros = CANARY_UNINIT initial.
                unsafe { core::mem::transmute::<[u8; MAX_CPUS * 64], [CanarySlot; MAX_CPUS]>([0u8; MAX_CPUS * 64]) }
            }
        }
    }
}

static CANARY_TABLE: CanaryTable = canary_table_init!();

// ─────────────────────────────────────────────────────────────────────────────
// Génération pseudo-aléatoire via RDTSC
// ─────────────────────────────────────────────────────────────────────────────

/// Lit TSC via `rdtsc`.
/// # Safety : CPL 0 (ou RDTSC user si CR4.TSD=0).
#[inline(always)]
unsafe fn rdtsc() -> u64 {
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

/// Génère une valeur de canary pseudo-aléatoire pour `cpu_id`.
///
/// Mélange TSC + cpu_id + constante fixe pour casser les prédictions.
/// Ce n'est pas du vrai aléa mais c'est suffisant contre les attaques
/// de corruption de stack aveugle.
///
/// # Safety : CPL 0.
unsafe fn generate_canary(cpu_id: u32) -> u64 {
    let tsc = rdtsc();
    // Splitmix64-like : mélange TSC avec une constante de Fibonacci.
    let mut v = tsc ^ (cpu_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^ (v >> 31)
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le canary pour `cpu_id`.  Doit être appelé une fois par CPU
/// (BSP depuis `init()`, APs depuis leur entry point).
///
/// # Safety : CPL 0 exclusif pour ce `cpu_id`.
pub unsafe fn init_cpu_canary(cpu_id: u32) {
    if cpu_id as usize >= MAX_CPUS {
        return;
    }
    let slot = &CANARY_TABLE.slots[cpu_id as usize];
    let val = generate_canary(cpu_id);
    slot.value.store(val, Ordering::Release);
    slot.generation.fetch_add(1, Ordering::Release);
    slot.initialized.store(true, Ordering::Release);
    CANARY_STATS.init_count.fetch_add(1, Ordering::Relaxed);
}

/// Retourne le canary courant pour `cpu_id`, ou `CANARY_UNINIT` si non init.
#[inline]
pub fn cpu_canary(cpu_id: u32) -> u64 {
    if cpu_id as usize >= MAX_CPUS {
        return CANARY_UNINIT;
    }
    CANARY_TABLE.slots[cpu_id as usize].value.load(Ordering::Acquire)
}

/// Génère un canary de thread = cpu_canary XOR tid.
/// Placer ce canary dans le TCB du thread à la création.
#[inline]
pub fn thread_canary(cpu_id: u32, tid: u64) -> u64 {
    cpu_canary(cpu_id) ^ tid.wrapping_mul(0x517C_C1B7_2722_0A95)
}

/// Vérifie le canary de thread `actual` contre `expected`.
/// Retourne `true` si OK, `false` si violation.
#[inline]
pub fn verify_thread_canary(expected: u64, actual: u64) -> bool {
    if expected == actual {
        CANARY_STATS.check_ok.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        canary_violation_handler(expected, actual);
    }
}

/// Appelé lors d'une violation de canary — log + incrémente compteur.
/// Le caller doit déclencher un kernel panic.
pub fn canary_violation_handler(expected: u64, actual: u64) -> ! {
    CANARY_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = (expected, actual);
    panic!("STACK CANARY VIOLATION: expected={:#018x} actual={:#018x}", expected, actual);
}

/// Effectue une rotation du canary pour tous les CPUs initialisés.
/// Usages : rotation périodique, réponse à incident, hardening proactif.
///
/// # Safety : CPL 0 ; doit être appelé sur BSP avec quiescence globale.
pub unsafe fn rotate_all_canaries() {
    for cpu_id in 0..MAX_CPUS as u32 {
        let slot = &CANARY_TABLE.slots[cpu_id as usize];
        if slot.initialized.load(Ordering::Acquire) {
            let val = generate_canary(cpu_id);
            slot.value.store(val, Ordering::Release);
            slot.generation.fetch_add(1, Ordering::Release);
            CANARY_STATS.rotate_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Initialisation du sous-système canary pour le BSP (cpu_id = 0).
///
/// # Safety : CPL 0.
pub unsafe fn init() {
    init_cpu_canary(0);
}
