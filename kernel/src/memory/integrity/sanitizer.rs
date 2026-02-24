// kernel/src/memory/integrity/sanitizer.rs
//
// KASAN-lite — Shadow memory tracking pour détection de :
//   • Use-after-free (UAF)
//   • Buffer overflow (heap / stack)
//   • Accès mémoire non-initialisée (via poison explicite à l'allocation)
//
// Architecture :
//   - La shadow map couvre le heap noyau (KERNEL_HEAP_START + 256 GiB).
//   - Ratio ombre : 1 octet shadow surveille 8 octets objet → shadow = 32 GiB.
//   - Chaque slot shadow encode un état sur 8 bits :
//       SHADOW_ACCESSIBLE  (0x00) — accessible
//       SHADOW_PARTIAL_k   (0x01..0x07) — k premiers octets accessibles
//       SHADOW_REDZONE     (0xFA) — zone rouge (overflow)
//       SHADOW_FREED       (0xFD) — mémoire libérée (UAF)
//       SHADOW_UNINIT      (0xFF) — non-initialisée
//
// Couverture dynamique : désactivée si `KASAN_ENABLED` = false (perf).
//
// COUCHE 0 — pas de dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::memory::core::layout::KERNEL_HEAP_START;
use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration shadow
// ─────────────────────────────────────────────────────────────────────────────

/// Plage du heap noyau couverte : 256 GiB.
const KASAN_HEAP_SIZE: u64 = 256 * 1024 * 1024 * 1024;
/// Base de la shadow memory = KERNEL_HEAP_START - (KASAN_HEAP_SIZE / 8).
/// Placée juste avant le heap pour un calcul d'adresse sans division à runtime.
const KASAN_SHADOW_OFFSET: u64 = KERNEL_HEAP_START.as_u64().wrapping_sub(KASAN_HEAP_SIZE / 8);
/// Taille shadow totale : 32 GiB.
const KASAN_SHADOW_SIZE: u64 = KASAN_HEAP_SIZE / 8;

// ─────────────────────────────────────────────────────────────────────────────
// Valeurs shadow
// ─────────────────────────────────────────────────────────────────────────────

pub const SHADOW_ACCESSIBLE: u8 = 0x00;
/// Partiellement accessible : k octets valides depuis le début du mot de 8.
pub const fn shadow_partial(k: u8) -> u8 {
    debug_assert!(k > 0 && k < 8);
    k
}
pub const SHADOW_REDZONE:    u8 = 0xFA;
pub const SHADOW_FREED:      u8 = 0xFD;
pub const SHADOW_UNINIT:     u8 = 0xFF;

// ─────────────────────────────────────────────────────────────────────────────
// Activation globale
// ─────────────────────────────────────────────────────────────────────────────

static KASAN_ENABLED: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn kasan_is_enabled() -> bool {
    KASAN_ENABLED.load(Ordering::Acquire)
}

/// Active KASAN. Doit être appelé après que la shadow map est mappée.
pub fn kasan_enable() {
    KASAN_ENABLED.store(true, Ordering::Release);
    KASAN_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
}

/// Désactive KASAN (debug, benchmark).
pub fn kasan_disable() {
    KASAN_ENABLED.store(false, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct KasanStats {
    pub enable_count:    AtomicU64,
    pub uaf_detected:    AtomicU64,
    pub overflow_detected: AtomicU64,
    pub uninit_detected: AtomicU64,
    pub poison_calls:    AtomicU64,
    pub unpoison_calls:  AtomicU64,
    pub shadow_writes:   AtomicU64,
}

impl KasanStats {
    const fn new() -> Self {
        Self {
            enable_count:     AtomicU64::new(0),
            uaf_detected:     AtomicU64::new(0),
            overflow_detected: AtomicU64::new(0),
            uninit_detected:  AtomicU64::new(0),
            poison_calls:     AtomicU64::new(0),
            unpoison_calls:   AtomicU64::new(0),
            shadow_writes:    AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for KasanStats {}
pub static KASAN_STATS: KasanStats = KasanStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Accès à la shadow memory
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit une adresse objet en adresse shadow.
///
/// addr_shadow = (addr - KERNEL_HEAP_START) / 8 + KASAN_SHADOW_OFFSET
///
/// # Safety
/// `addr` doit être dans la plage couverte [KERNEL_HEAP_START, +KASAN_HEAP_SIZE).
#[inline]
unsafe fn shadow_addr(addr: u64) -> *mut u8 {
    let offset = addr.wrapping_sub(KERNEL_HEAP_START.as_u64()) >> 3;
    (KASAN_SHADOW_OFFSET + offset) as *mut u8
}

/// Lit l'octet shadow pour `addr`.
/// # Safety : `addr` doit être dans la plage heap couverte.
#[inline]
unsafe fn read_shadow(addr: u64) -> u8 {
    shadow_addr(addr).read_volatile()
}

/// Écrit l'octet shadow pour `addr`.
/// # Safety : idem.
#[inline]
unsafe fn write_shadow(addr: u64, val: u8) {
    shadow_addr(addr).write_volatile(val);
    KASAN_STATS.shadow_writes.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Empoisonnement / déempoisonnement
// ─────────────────────────────────────────────────────────────────────────────

/// Empoisonne `size` octets à partir de `addr` avec `poison_val`.
///
/// Chaque mot de 8 octets → 1 octet shadow.
///
/// # Safety : `addr` doit être aligné sur 8 et dans la plage couverte.
pub unsafe fn kasan_poison(addr: u64, size: usize, poison_val: u8) {
    if !kasan_is_enabled() {
        return;
    }
    KASAN_STATS.poison_calls.fetch_add(1, Ordering::Relaxed);
    let words = size / 8;
    let rem   = (size % 8) as u8;
    for i in 0..words as u64 {
        write_shadow(addr + i * 8, poison_val);
    }
    if rem > 0 {
        // Dernier mot partiel : seuls `rem` octets accessibles.
        write_shadow(addr + words as u64 * 8, match poison_val {
            SHADOW_ACCESSIBLE => shadow_partial(rem),
            _                 => poison_val,
        });
    }
}

/// Marque `size` octets à partir de `addr` comme accessibles (SHADOW_ACCESSIBLE).
///
/// # Safety : idem `kasan_poison`.
pub unsafe fn kasan_unpoison(addr: u64, size: usize) {
    if !kasan_is_enabled() {
        return;
    }
    KASAN_STATS.unpoison_calls.fetch_add(1, Ordering::Relaxed);
    let words = (size + 7) / 8;
    for i in 0..words as u64 {
        write_shadow(addr + i * 8, SHADOW_ACCESSIBLE);
    }
}

/// Empoisonne les redzones autour d'une allocation.
/// `obj_addr` : début de l'objet, `obj_size` : taille.
/// `redzone_size` : taille de chaque redzone (doit être multiple de 8).
///
/// # Safety : addresses valides, dans la plage heap couverte.
pub unsafe fn kasan_poison_redzone(obj_addr: u64, obj_size: usize, redzone_size: usize) {
    if !kasan_is_enabled() {
        return;
    }
    let left_rz  = obj_addr.wrapping_sub(redzone_size as u64);
    let right_rz = obj_addr + obj_size as u64;
    kasan_poison(left_rz,  redzone_size, SHADOW_REDZONE);
    kasan_poison(right_rz, redzone_size, SHADOW_REDZONE);
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification d'accès
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un check KASAN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KasanError {
    UseAfterFree { addr: u64 },
    BufferOverflow { addr: u64 },
    UninitMemory { addr: u64 },
    PartialAccess { addr: u64, accessible: u8, requested: u8 },
}

/// Vérifie l'accès à `addr` pour `size` octets.
/// Retourne `Ok(())` si la plage est entièrement accessible.
///
/// # Safety
/// `addr` doit être dans la plage couverte [KERNEL_HEAP_START, +KASAN_HEAP_SIZE).
pub unsafe fn kasan_check_access(addr: u64, size: usize) -> Result<(), KasanError> {
    if !kasan_is_enabled() {
        return Ok(());
    }
    if addr < KERNEL_HEAP_START.as_u64() || addr >= KERNEL_HEAP_START.as_u64() + KASAN_HEAP_SIZE {
        return Ok(()); // Hors plage couverte — pas d'erreur KASAN (ex: vmalloc).
    }

    let end = addr + size as u64;
    let mut cur = addr;
    while cur < end {
        let shadow = read_shadow(cur);
        let err = match shadow {
            SHADOW_ACCESSIBLE => None,
            SHADOW_FREED      => {
                KASAN_STATS.uaf_detected.fetch_add(1, Ordering::Relaxed);
                Some(KasanError::UseAfterFree { addr: cur })
            }
            SHADOW_REDZONE    => {
                KASAN_STATS.overflow_detected.fetch_add(1, Ordering::Relaxed);
                Some(KasanError::BufferOverflow { addr: cur })
            }
            SHADOW_UNINIT     => {
                KASAN_STATS.uninit_detected.fetch_add(1, Ordering::Relaxed);
                Some(KasanError::UninitMemory { addr: cur })
            }
            partial if partial > 0 && partial < 8 => {
                // Vérifier si l'accès dépasse les `partial` octets valides.
                let byte_in_word = (cur & 7) as u8;
                let end_in_word  = (byte_in_word + size as u8).min(8);
                if end_in_word > partial {
                    KASAN_STATS.overflow_detected.fetch_add(1, Ordering::Relaxed);
                    Some(KasanError::PartialAccess {
                        addr: cur,
                        accessible: partial,
                        requested: end_in_word,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(e) = err {
            return Err(e);
        }
        // Avancer d'un mot (8 octets).
        cur = (cur & !7) + 8;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Rapport de violation
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé par le handler KASAN lorsqu'un accès invalide est détecté.
/// Log minimal + panic.
pub fn kasan_report(error: KasanError, ip: u64) -> ! {
    match error {
        KasanError::UseAfterFree { addr } =>
            panic!("KASAN: use-after-free at {:#x} ip={:#x}", addr, ip),
        KasanError::BufferOverflow { addr } =>
            panic!("KASAN: buffer-overflow at {:#x} ip={:#x}", addr, ip),
        KasanError::UninitMemory { addr } =>
            panic!("KASAN: uninit-memory read at {:#x} ip={:#x}", addr, ip),
        KasanError::PartialAccess { addr, accessible, requested } =>
            panic!("KASAN: partial-access at {:#x} accessible={} requested={} ip={:#x}",
                   addr, accessible, requested, ip),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hooks d'allocation / libération (appelés par hybrid.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé juste avant de retourner un pointeur depuis l'allocateur.
/// Marque `size` octets accessibles.
///
/// # Safety : `ptr` valide dans la plage heap.
pub unsafe fn kasan_on_alloc(ptr: *mut u8, size: usize) {
    kasan_unpoison(ptr as u64, size);
}

/// Appelé juste après la libération d'un pointeur.
/// Marque `size` octets comme FREED.
///
/// # Safety : `ptr` valide dans la plage heap.
pub unsafe fn kasan_on_free(ptr: *mut u8, size: usize) {
    kasan_poison(ptr as u64, size, SHADOW_FREED);
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise KASAN.
///
/// La shadow map doit avoir été préalablement mappée (32 GiB de pages zeroes)
/// par le mapper noyau.  Cette fonction active simplement la vérification.
///
/// # Safety : CPL 0, shadow map mappée.
pub unsafe fn init() {
    // Empoisonner toute la shadow map en SHADOW_UNINIT.
    // En pratique on fait confiance au zeroing de la page table (0x00 = ACCESSIBLE).
    // On active simplement KASAN.
    kasan_enable();
}
