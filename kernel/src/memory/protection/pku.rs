// kernel/src/memory/protection/pku.rs
//
// PKU — Protection Keys for Userspace (Intel MPX successor).
//
// Le registre PKRU (32 bits) associe à chaque clé (0..15) 2 bits :
//   bit[2k]   = AD (Access Disable)   — désactive lecture + écriture
//   bit[2k+1] = WD (Write Disable)    — désactive écriture uniquement
//
// Les pages portent un « pkey » (4 bits) dans leurs PTEs (bits 62:59).
// CR4.PKE = 1 active le mécanisme en user-mode.
// CR4.PKS = 1 active le mécanisme en kernel-mode (nouvelle feature Intel).
//
// Références :
//   Intel SDM Vol.3A § 4.6.2 — "Protection Keys"
//   AMD APM Vol.2 § 5.20 — "Page-Based Protection Keys"
//
// Couche 0 — pas de dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de clés de protection disponibles.
pub const PKU_KEY_COUNT: usize = 16;
/// Masque permettant d'extraire les bits pkey d'une PTE (bits 62:59).
pub const PTE_PKEY_MASK: u64 = 0x7800_0000_0000_0000;
/// Décalage des bits pkey dans une PTE.
pub const PTE_PKEY_SHIFT: u64 = 59;

/// CR4 bit 22 — PKE (Protection Keys Enable, user).
pub const CR4_PKE_BIT: u64 = 1 << 22;
/// CR4 bit 24 — PKS (Protection Key Supervisor, kernel).
pub const CR4_PKS_BIT: u64 = 1 << 24;

/// Bit AD dans PKRU pour la clé `k` : bit 2k.
#[inline(always)]
pub const fn pkru_ad_bit(k: u8) -> u32 {
    1u32 << (k as u32 * 2)
}

/// Bit WD dans PKRU pour la clé `k` : bit 2k+1.
#[inline(always)]
pub const fn pkru_wd_bit(k: u8) -> u32 {
    1u32 << (k as u32 * 2 + 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// Clé réservée 0 : clé par défaut, toujours accessible.
/// Clé 0 est la clé « publique » — toutes les pages sans pkey explicite l'utilisent.
pub const PKU_DEFAULT_KEY: u8 = 0;
/// Clé 1 réservée au heap noyau.
pub const PKU_KERNEL_HEAP_KEY: u8 = 1;
/// Clé 2 réservée aux pages de garde (inaccessibles).
pub const PKU_GUARD_KEY: u8 = 2;
/// Clé 3 réservée aux données MMIO.
pub const PKU_MMIO_KEY: u8 = 3;
/// Clé 4..15 libre pour attribution dynamique par userland / driver.

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct PkuStats {
    pub enable_count:    AtomicU64,
    pub alloc_count:     AtomicU64,
    pub free_count:      AtomicU64,
    pub pkru_write_count: AtomicU64,
    pub violation_count: AtomicU64,
    pub exhausted_count: AtomicU64,
}

impl PkuStats {
    const fn new() -> Self {
        Self {
            enable_count:     AtomicU64::new(0),
            alloc_count:      AtomicU64::new(0),
            free_count:       AtomicU64::new(0),
            pkru_write_count: AtomicU64::new(0),
            violation_count:  AtomicU64::new(0),
            exhausted_count:  AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for PkuStats {}
pub static PKU_STATS: PkuStats = PkuStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Allocateur de clés
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur d'une clé PKU allouée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PkuKeyDesc {
    /// Index de la clé (0..15).
    pub key:  u8,
    /// Propriétaire (TID, 0 = noyau).
    pub owner: u64,
    /// Clé actuellement allouée.
    pub in_use: bool,
}

impl PkuKeyDesc {
    const fn free() -> Self {
        Self { key: 0, owner: 0, in_use: false }
    }
}

struct PkuKeyAllocator {
    keys: [PkuKeyDesc; PKU_KEY_COUNT],
}

impl PkuKeyAllocator {
    const fn new() -> Self {
        // Les clés 0..PKU_DEFAULT+3 sont réservées noyau.
        let mut keys = [PkuKeyDesc::free(); PKU_KEY_COUNT];
        keys[PKU_DEFAULT_KEY as usize]     = PkuKeyDesc { key: PKU_DEFAULT_KEY,     owner: 0, in_use: true };
        keys[PKU_KERNEL_HEAP_KEY as usize] = PkuKeyDesc { key: PKU_KERNEL_HEAP_KEY, owner: 0, in_use: true };
        keys[PKU_GUARD_KEY as usize]       = PkuKeyDesc { key: PKU_GUARD_KEY,       owner: 0, in_use: true };
        keys[PKU_MMIO_KEY as usize]        = PkuKeyDesc { key: PKU_MMIO_KEY,        owner: 0, in_use: true };
        Self { keys }
    }

    /// Alloue la première clé libre (>=4). Retourne `None` si épuisé.
    fn alloc(&mut self, owner: u64) -> Option<u8> {
        for k in 4..PKU_KEY_COUNT {
            if !self.keys[k].in_use {
                self.keys[k] = PkuKeyDesc { key: k as u8, owner, in_use: true };
                return Some(k as u8);
            }
        }
        None
    }

    /// Libère une clé.
    fn free(&mut self, key: u8) -> bool {
        let k = key as usize;
        if k < 4 || k >= PKU_KEY_COUNT {
            return false; // clé réservée ou invalide
        }
        if !self.keys[k].in_use {
            return false;
        }
        self.keys[k].in_use = false;
        self.keys[k].owner = 0;
        true
    }
}

static PKU_KEY_ALLOC: Mutex<PkuKeyAllocator> = Mutex::new(PkuKeyAllocator::new());

// ─────────────────────────────────────────────────────────────────────────────
// Lecture / écriture PKRU via XSAVE instructions
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le registre PKRU sur le CPU courant.
///
/// # Safety : CPL 0, PKE doit être actif.
#[inline(always)]
pub unsafe fn rdpkru() -> u32 {
    let val: u32;
    core::arch::asm!(
        "xor ecx, ecx",
        "rdpkru",
        out("eax") val,
        lateout("edx") _,
        lateout("ecx") _,
        options(nostack, nomem),
    );
    val
}

/// Écrit `val` dans le registre PKRU.
///
/// # Safety : CPL 0 ou user (RDPKRU user-mode ok si CR4.PKE).
#[inline(always)]
pub unsafe fn wrpkru(val: u32) {
    core::arch::asm!(
        "xor ecx, ecx",
        "xor edx, edx",
        "wrpkru",
        in("eax") val,
        options(nostack, nomem),
    );
    PKU_STATS.pkru_write_count.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Lecture CR4
// ─────────────────────────────────────────────────────────────────────────────

/// # Safety : CPL 0.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let v: u64;
    core::arch::asm!("mov {v}, cr4", v = out(reg) v, options(nostack, nomem, preserves_flags));
    v
}

/// # Safety : CPL 0.
#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {v}", v = in(reg) val, options(nostack, nomem));
}

// ─────────────────────────────────────────────────────────────────────────────
// Détection CPUID
// ─────────────────────────────────────────────────────────────────────────────

/// PKU supporté : CPUID.7.0:ECX bit 3.
#[inline]
pub fn pku_supported() -> bool {
    let ecx: u32;
    // SAFETY: CPUID disponible sur x86_64; xchg préserve rbx réservé par LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            out("ecx") ecx,
            lateout("eax") _,
            lateout("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem),
        );
    }
    ecx & (1 << 3) != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Activation PKU
// ─────────────────────────────────────────────────────────────────────────────

/// Active PKU (CR4.PKE = 1) et configure PKRU initial.
///
/// Config initiale :
///   - Clé 0 (default) : AD=0 WD=0 → accessible en R/W.
///   - Clé 1 (heap)    : AD=0 WD=0 → accessible R/W (géré par hyperviseur de clé).
///   - Clé 2 (guard)   : AD=1 WD=0 → inaccessible.
///   - Clé 3 (MMIO)    : AD=0 WD=1 → lecture seule (protection données MMIO).
///   - Clés 4..15      : AD=0 WD=0 → accessible (allouées dynamiquement).
///
/// # Safety : CPL 0.
pub unsafe fn enable_pku() {
    if !pku_supported() {
        return;
    }
    let cr4 = read_cr4();
    write_cr4(cr4 | CR4_PKE_BIT);
    PKU_STATS.enable_count.fetch_add(1, Ordering::Relaxed);

    // PKRU initial : clé 2 (guard) inaccessible.
    let pkru: u32 = pkru_ad_bit(PKU_GUARD_KEY) // clé 2 AD
                  | pkru_wd_bit(PKU_MMIO_KEY);  // clé 3 WD
    wrpkru(pkru);
}

// ─────────────────────────────────────────────────────────────────────────────
// API clés publique
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue une clé PKU pour le thread `owner_tid`.
/// Retourne `Some(key)` ou `None` si toutes épuisées.
pub fn pku_alloc_key(owner_tid: u64) -> Option<u8> {
    let key = PKU_KEY_ALLOC.lock().alloc(owner_tid);
    if key.is_some() {
        PKU_STATS.alloc_count.fetch_add(1, Ordering::Relaxed);
    } else {
        PKU_STATS.exhausted_count.fetch_add(1, Ordering::Relaxed);
    }
    key
}

/// Libère la clé `key`. Retourne `true` si succès.
pub fn pku_free_key(key: u8) -> bool {
    let ok = PKU_KEY_ALLOC.lock().free(key);
    if ok {
        PKU_STATS.free_count.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Active l'accès R/W pour la clé `key` dans le PKRU courant.
///
/// # Safety : CPL 0. Seulement si PKU actif.
pub unsafe fn pku_allow_key(key: u8) {
    if key >= PKU_KEY_COUNT as u8 {
        return;
    }
    let pkru = rdpkru();
    let mask = pkru_ad_bit(key) | pkru_wd_bit(key);
    wrpkru(pkru & !mask);
}

/// Désactive l'accès total pour la clé `key` (AD = 1).
///
/// # Safety : CPL 0.
pub unsafe fn pku_deny_key(key: u8) {
    if key >= PKU_KEY_COUNT as u8 {
        return;
    }
    let pkru = rdpkru();
    wrpkru(pkru | pkru_ad_bit(key));
}

/// Active uniquement la lecture pour la clé `key` (WD = 1, AD = 0).
///
/// # Safety : CPL 0.
pub unsafe fn pku_readonly_key(key: u8) {
    if key >= PKU_KEY_COUNT as u8 {
        return;
    }
    let pkru = rdpkru();
    let new = (pkru & !pkru_ad_bit(key)) | pkru_wd_bit(key);
    wrpkru(new);
}

/// Encode le pkey dans les bits 62:59 d'une PTE.
#[inline]
pub const fn pte_set_pkey(pte: u64, key: u8) -> u64 {
    (pte & !PTE_PKEY_MASK) | (((key as u64) & 0xF) << PTE_PKEY_SHIFT)
}

/// Extrait le pkey des bits 62:59 d'une PTE.
#[inline]
pub const fn pte_get_pkey(pte: u64) -> u8 {
    ((pte & PTE_PKEY_MASK) >> PTE_PKEY_SHIFT) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// Guard RAII : accès temporaire à une clé
// ─────────────────────────────────────────────────────────────────────────────

/// Guard RAII qui autorise l'accès à `key` pendant sa durée de vie,
/// puis restaure l'état PKRU précédent au drop.
pub struct PkuAccessGuard {
    key:       u8,
    saved_pkru: u32,
}

impl PkuAccessGuard {
    /// # Safety : CPL 0, PKU actif.
    pub unsafe fn new(key: u8) -> Self {
        let saved_pkru = rdpkru();
        pku_allow_key(key);
        PkuAccessGuard { key, saved_pkru }
    }
}

impl Drop for PkuAccessGuard {
    fn drop(&mut self) {
        // SAFETY: wrpkru restaure la valeur PKRU sauvegardée dans new(); CPL 0 requis.
        unsafe { wrpkru(self.saved_pkru) };
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Violation handler
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé lors d'une #PF PKU. Retourne `false` → non récupérable.
#[inline]
pub fn pku_handle_violation(fault_addr: u64, key: u8) -> bool {
    PKU_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = (fault_addr, key);
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Init PKU. Doit être appelé sur BSP + chaque AP après init page tables.
///
/// # Safety : CPL 0.
pub unsafe fn init() {
    enable_pku();
}
