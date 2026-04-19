//! # arch/x86_64/acpi/hpet.rs — High Precision Event Timer
//!
//! Implementation du HPET conformément à la table ACPI HPET.
//! Le HPET est utilisé comme source de temps de référence en attendant
//! la calibration TSC, ou comme fallback si le TSC n'est pas invariant.
//!
//! ## Registres HPET (MMIO)
//! - 0x000 : GCAP_ID — capabilities et fréquence (10 ns = 100 MHz si périodique)
//! - 0x010 : GEN_CFG — config globale (bit 0 = ENABLE_CNF, bit 1 = LEG_RT_CNF)
//! - 0x020 : GINTR_STA — statut des interruptions
//! - 0x0F0 : MAIN_CTR — compteur principal 64 bits
//! - 0x100+N*0x20 : Timer N comparateurs


use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
use crate::memory::core::{fixmap_slot_addr, Frame, PageFlags, PhysAddr, FIXMAP_HPET};
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::address_space::tlb;

// ── Offsets registres HPET ────────────────────────────────────────────────────

#[allow(dead_code)]
const HPET_GCAP_ID:   u64 = 0x000; // capabilities (64 bits)
const HPET_GEN_CFG:   u64 = 0x010; // config globale (64 bits)
#[allow(dead_code)]
const HPET_GINTR_STA: u64 = 0x020; // interrupt status
const HPET_MAIN_CTR:  u64 = 0x0F0; // compteur principal (64 bits)
#[allow(dead_code)]
const HPET_T0_CFG:    u64 = 0x100; // Timer 0 config (64 bits)
#[allow(dead_code)]
const HPET_T0_CMP:    u64 = 0x108; // Timer 0 comparateur (64 bits)

// Bits GEN_CFG
const HPET_ENABLE:    u64 = 1 << 0;
#[allow(dead_code)]
const HPET_LEG_RT:    u64 = 1 << 1; // Legacy Replacement Mapping

// GCAP_ID champs
const HPET_CLK_PERIOD_SHIFT: u32 = 32;  // bits 63:32 = période en femtosecondes
#[allow(dead_code)]
const HPET_NUM_TIMERS_SHIFT: u32 = 8;   // bits 12:8 = nombre de timers - 1

// ── État HPET ─────────────────────────────────────────────────────────────────

/// Informations HPET
#[derive(Debug, Clone, Copy)]
pub struct HpetInfo {
    pub mmio_base:      u64,   // Adresse MMIO physique
    pub clock_period_fs:u32,   // Période d'horloge en femtosecondes
    pub freq_hz:        u64,   // Fréquence en Hz
    pub n_timers:       u8,    // Nombre de timers (au moins 3)
    pub is_64bit:       bool,  // Compteur principal 64 bits
}

static HPET_BASE:      AtomicU64 = AtomicU64::new(0);
static HPET_PERIOD_FS: AtomicU32 = AtomicU32::new(0);
static HPET_FREQ_HZ:   AtomicU64 = AtomicU64::new(0);

// ── Structures table ACPI HPET ────────────────────────────────────────────────

/// Table ACPI HPET (après SdtHeader)
#[repr(C, packed)]
#[allow(dead_code)]
struct AcpiHpetTable {
    event_timer_block_id: u32,
    base_address:         [u8; 12], // Generic Address Structure (GAS)
    hpet_number:          u8,
    minimum_clock_tick:   u16,
    page_protection:      u8,
}

// ── Lecture / écriture HPET MMIO ─────────────────────────────────────────────

/// Lit un registre HPET 64 bits
#[inline]
fn hpet_read(offset: u64) -> u64 {
    let base = HPET_BASE.load(Ordering::Relaxed);
    // SAFETY: base HPET validée lors de l'init, offset registre connu
    unsafe { read_volatile((base + offset) as *const u64) }
}

/// Écrit un registre HPET 64 bits
#[inline]
#[allow(dead_code)]
fn hpet_write(offset: u64, val: u64) {
    let base = HPET_BASE.load(Ordering::Relaxed);
    // SAFETY: base HPET validée lors de l'init
    unsafe { write_volatile((base + offset) as *mut u64, val); }
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise le HPET depuis la table ACPI
///
/// Appelé par `init_acpi()` après avoir localisé la table HPET.
/// Lors du boot précoce, on enregistre seulement l'adresse MMIO sans
/// accéder aux registres (le MMIO HPET à 0xFED00000 dépasse notre
/// identity map de 1 GiB — l'accès MMIO aura lieu après init mémoire).
pub fn init_hpet(hpet_table_phys: u64) -> Option<HpetInfo> {
    if hpet_table_phys == 0 || hpet_table_phys >= 0x4000_0000 { return None; }

    use super::parser::SdtHeader;
    // SAFETY: adresse passée par le parseur ACPI (dans notre identity map)
    let header = unsafe { &*(hpet_table_phys as *const SdtHeader) };
    let sig = unsafe { core::ptr::read_unaligned(&raw const (*header).signature) };
    if &sig != b"HPET" { return None; }

    // GAS base address : offset SdtHeader(36) + block_id(4) + GAS header(4) = 44
    // Dans la GAS (Generic Address Structure) l'adresse 64 bits est aux derniers 8 octets
    let gas_addr_offset = core::mem::size_of::<SdtHeader>() + 4 + 4;
    // read_unaligned : la table HPET peut être à une adresse non-alignée sur 8 octets
    let mmio_base = unsafe {
        core::ptr::read_unaligned((hpet_table_phys as usize + gas_addr_offset) as *const u64)
    };

    // Enregistrer la base MMIO HPET pour utilisation future (après init mémoire)
    // Ne PAS accéder aux registres HPET ici : 0xFED00000 > 1 GiB (hors identity map boot)
    HPET_BASE.store(mmio_base, Ordering::Release);

    // Retourner des infos minimales — fréquence et n_timers seront lus plus tard
    Some(HpetInfo {
        mmio_base,
        clock_period_fs: 0, // rempli lors de l'init post-mémoire
        freq_hz: 0,
        n_timers: 0,
        is_64bit: false,
    })
}

// ── Primitives ────────────────────────────────────────────────────────────────

/// Lit le compteur principal HPET (64 bits)
#[inline]
pub fn hpet_read_counter() -> u64 {
    hpet_read(HPET_MAIN_CTR)
}

/// Convertit des µs en ticks HPET
pub fn hpet_us_to_ticks(us: u64) -> u64 {
    let freq = HPET_FREQ_HZ.load(Ordering::Relaxed);
    if freq == 0 { return 0; }
    us.saturating_mul(freq) / 1_000_000
}

/// Délai actif via HPET (en µs)
///
/// Usage : calibration dans les premières phases du boot avant TSC.
pub fn hpet_delay_us(us: u64) {
    let start = hpet_read_counter();
    let ticks = hpet_us_to_ticks(us);
    while hpet_read_counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// Fréquence HPET en Hz
pub fn hpet_freq_hz() -> u64 {
    HPET_FREQ_HZ.load(Ordering::Relaxed)
}

/// `true` si le HPET est disponible et initialisé
pub fn hpet_available() -> bool {
    HPET_BASE.load(Ordering::Relaxed) != 0 && HPET_FREQ_HZ.load(Ordering::Relaxed) != 0
}

// ── Initialisation post-mémoire ───────────────────────────────────────────────

/// Initialise le HPET après que le sous-système mémoire est opérationnel.
///
/// Actions :
/// 1. Mappe la page MMIO HPET avec `PAGE_FLAGS_MMIO` (UC + NX) dans le fixmap
///    **si le physmap le permet**, sinon utilise l'adresse physique directement
///    (identity map 0–4 GiB du trampoline — sans UC, fonctionnel sur QEMU).
/// 2. Lit GCAP_ID → extrait la période d'horloge en femtosecondes.
/// 3. Active le compteur principal (HPET_ENABLE dans GEN_CFG).
/// 4. Vérifie que le compteur avance (avec timeout de sécurité).
/// 5. Stocke la fréquence dans `HPET_FREQ_HZ`.
///
/// Appelé depuis `kernel_init()` après `hybrid::init()`.
/// Sans effet si le HPET n'a pas été détecté lors de `init_hpet()`.
///
/// Retourne `true` si le HPET est maintenant opérationnel.
pub fn init_hpet_post_memory() -> bool {
    let hpet_phys = HPET_BASE.load(Ordering::Acquire);
    if hpet_phys == 0 { return false; }

    // ── 1. Mapping MMIO fixmap ───────────────────────────────────────────────
    let hpet_page_phys = PhysAddr::new(hpet_phys & !0xFFF);
    let hpet_page_off = hpet_phys & 0xFFF;
    let hpet_fix_virt = fixmap_slot_addr(FIXMAP_HPET);

    // MMIO: PRESENT|WRITABLE|NX|NO_CACHE|GLOBAL
    let flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXECUTE
        | PageFlags::NO_CACHE
        | PageFlags::GLOBAL;

    if KERNEL_AS.translate(hpet_fix_virt).is_none() {
        let frame = Frame::containing(hpet_page_phys);
        // SAFETY: mapping fixmap noyau en ring0 avec allocateur noyau valide.
        if unsafe { KERNEL_AS.map(hpet_fix_virt, frame, flags, &KERNEL_FAULT_ALLOC) }.is_err() {
            return false;
        }
        // SAFETY: invalidation locale de l'entrée fixmap nouvellement mappée.
        unsafe { tlb::flush_single(hpet_fix_virt); }
    }

    let virt = hpet_fix_virt.as_u64().saturating_add(hpet_page_off);
    HPET_BASE.store(virt, Ordering::Release);

    // ── 2. Lire GCAP_ID via la fixmap MMIO ───────────────────────────────────
    let gcap = unsafe { core::ptr::read_volatile(virt as *const u64) };
    let period_fs = (gcap >> HPET_CLK_PERIOD_SHIFT) as u32;

    // Valider la période : QEMU retourne 69841279 fs (~14.318 MHz PIT reference).
    // Plage acceptable : 100 ps (10 GHz) à 100 ns (10 MHz).
    if period_fs < 100 || period_fs > 100_000_000 {
        return false; // GCAP_ID invalide — HPET non disponible ou MMIO inaccessible
    }

    let freq = 1_000_000_000_000_000u64 / period_fs as u64;
    HPET_PERIOD_FS.store(period_fs, Ordering::Release);
    HPET_FREQ_HZ.store(freq, Ordering::Release);

    // ── 3. Activer le compteur HPET ──────────────────────────────────────────
    // Désactiver → reset compteur → activer.
    unsafe {
        core::ptr::write_volatile((virt + HPET_GEN_CFG) as *mut u64, 0);
        core::ptr::write_volatile((virt + HPET_MAIN_CTR) as *mut u64, 0);
        core::ptr::write_volatile((virt + HPET_GEN_CFG) as *mut u64, HPET_ENABLE);
    }

    // ── 4. Vérifier que le compteur avance (limite d'itérations, pas TSC) ──────
    // Sur QEMU/TCG, chaque read_volatile MMIO peut prendre ~50µs.
    // 100 itérations = 5ms max sur QEMU, quelques µs sur bare-metal.
    let counter_start = unsafe { core::ptr::read_volatile((virt + HPET_MAIN_CTR) as *const u64) };
    let mut ok = false;
    for _ in 0u32..100 {
        let c = unsafe { core::ptr::read_volatile((virt + HPET_MAIN_CTR) as *const u64) };
        if c != counter_start { ok = true; break; }
        core::hint::spin_loop();
    }

    if !ok {
        // HPET ne compte pas — désactiver et signaler échec
        unsafe { core::ptr::write_volatile((virt + HPET_GEN_CFG) as *mut u64, 0); }
        HPET_FREQ_HZ.store(0, Ordering::Release);
        return false;
    }

    true
}

/// Retourne l'adresse virtuelle MMIO HPET courante (= adresse physique via identity map).
pub fn hpet_virt_base() -> u64 {
    HPET_BASE.load(Ordering::Relaxed)
}
