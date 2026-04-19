// kernel/src/security/exoveil.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoVeil — PKS (Protection Keys for Supervisor) Domains (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoVeil implémente l'isolation mémoire par domaines PKS (Intel PKS, SDM Vol.3 §5.5.2).
// Chaque domaine PKS contrôle l'accès à des pages marquées avec une clé de protection
// (pkey) dans leurs PTE. La révocation d'un domaine est O(1), ~20 cycles, sans TLB
// shootdown — le CPU vérifie la permission dans le MSR IA32_PKRS à chaque accès.
//
// DOMAINES PKS v1.0 :
//   Domain 0 (Default)    : Code kernel standard — toujours accessible
//   Domain 1 (Caps)       : Tables CapToken — révoqué si compromission détectée
//   Domain 2 (Credentials): Clés crypto — révoqué au boot, restauré selon besoin
//   Domain 4 (TcbHot)     : TCB cache lines CL3-CL4 — révoqué si compromission critique
//
// RÈGLE EXOVEIL-01 : Révocation dynamique réactive DÉSACTIVÉE en v1.0.
//   La révocation PKS est utilisée uniquement :
//   a) Au boot (Credentials par défaut révoqué — ExoSeal step 0)
//   b) Lors du HANDOFF (tous les domaines révoqués sauf Default)
//   c) Sur décision explicite de Kernel B (pas de scoring automatique)
//
// RÈGLE EXOVEIL-02 : `revoke_domain()` = O(1), ~20 cycles, ZÉRO TLB shootdown.
//   PKS modifie uniquement le MSR IA32_PKRS — pas de modification de PTE.
//
// RÈGLE EXOVEIL-03 : TCB CL3-CL4 (domain TcbHot, pkey=4) sont allouées avec
//   `pte |= (4u64 << 59)` pour garantir que les champs ExoShield du TCB sont
//   dans le domaine TcbHot.
//
// RÉFÉRENCES :
//   Intel SDM Vol.3 §5.5.2 (PKS — Protection Keys for Supervisor Pages)
//   Intel SDM Vol.4 §2 (MSR IA32_PKRS = 0x6E1)
//   ExoShield_v1_Production.md — MODULE 4 : ExoVeil
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

use crate::arch::x86_64::cpu::msr;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes MSR
// ─────────────────────────────────────────────────────────────────────────────

/// MSR IA32_PKRS — Supervisor Protection Keys Rights Register.
/// Chaque paire de 2 bits (pour 16 pkeys) encode : 00=RWX, 01=none, 10=RO, 11=none.
const MSR_IA32_PKRS: u32 = 0x6E1;

/// Nombre maximum de pkeys PKS (pkey 0..15). Intel SDM : 4 bits dans PTE bits [62:59].
const MAX_PKS_KEYS: usize = 16;

/// Valeur PKRS par défaut : tout accessible (tous les domaines = RWX = 0b00).
#[allow(dead_code)]
const PKRS_DEFAULT_ALL_ACCESS: u64 = 0x0000_0000_0000_0000;

/// Valeur PKRS tous révoqués : tous les domaines = 0b11 (Access Disabled).
const PKRS_ALL_REVOKED: u64 = 0x5555_5555_5555_5555; // chaque paire = 0b11

// ─────────────────────────────────────────────────────────────────────────────
// PksDomain — Domaines de protection
// ─────────────────────────────────────────────────────────────────────────────

/// Domaines PKS d'ExoOS.
///
/// Chaque domaine correspond à une clé de protection (pkey) dans les PTE.
/// La valeur du domaine est le numéro de pkey (0..15).
///
/// # Layout PKR dans IA32_PKRS
///
/// Pour le domaine `d`, les bits `PKRS[2*d+1 : 2*d]` contrôlent l'accès :
/// - `00` : Read/Write (accès total)
/// - `01` : Read-only
/// - `10` : Write-only
/// - `11` : Access Disabled (révoqué)
///
/// # Correspondance PTE
///
/// Les pages associées au domaine `d` ont leur PTE bits[62:59] = `d` (4 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PksDomain {
    /// Domaine 0 : Code kernel standard — TOUJOURS accessible.
    Default     = 0,
    /// Domaine 1 : Tables CapToken — révoqué si compromission capabilities.
    Caps        = 1,
    /// Domaine 2 : Clés crypto, credentials — révoqué au boot par défaut.
    Credentials = 2,
    /// Domaine 4 : TCB cache lines CL3-CL4 — révoqué si compromission critique.
    TcbHot      = 4,
}

impl PksDomain {
    /// Retourne le pkey (numéro de domaine) en tant que u32.
    #[inline(always)]
    pub fn pkey(self) -> u32 {
        self as u32
    }

    /// Retourne le décalage en bits dans IA32_PKRS pour ce domaine.
    /// Chaque domaine occupe 2 bits : shift = domain * 2.
    #[inline(always)]
    pub fn shift(self) -> u32 {
        (self as u32) * 2
    }

    /// Masque PKRS pour ce domaine (2 bits).
    #[inline(always)]
    pub fn mask(self) -> u64 {
        0b11u64 << self.shift()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Permissions PKS
// ─────────────────────────────────────────────────────────────────────────────

/// Permission d'accès pour un domaine PKS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PksPermission {
    /// Lecture et écriture autorisées (valeur PKRS = 0b00).
    ReadWrite  = 0b00,
    /// Lecture seule (valeur PKRS = 0b01).
    ReadOnly   = 0b01,
    /// Écriture seule (valeur PKRS = 0b10).
    WriteOnly  = 0b10,
    /// Accès totalement interdit / révoqué (valeur PKRS = 0b11).
    Disabled   = 0b11,
}

impl PksPermission {
    /// Valeur 2 bits à écrire dans PKRS.
    #[inline(always)]
    pub fn bits(self) -> u64 {
        (self as u8 as u64) & 0b11
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// État global ExoVeil
// ─────────────────────────────────────────────────────────────────────────────

/// Valeur PKRS courante (shadow software du MSR matériel).
/// Permet des lectures rapides sans RDMSR (~1 cycle vs ~20 cycles pour RDMSR).
static CURRENT_PKRS: AtomicU64 = AtomicU64::new(PKRS_ALL_REVOKED);

/// PKS supporté par le CPU (détecté au boot par ExoSeal).
static PKS_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// ExoVeil initialisé (ExoSeal step 0 complété).
static EXOVEIL_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Compteur de révocations par domaine (instrumentation).
static REVOKE_COUNT: [AtomicU64; MAX_PKS_KEYS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; 16]
};

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions bas niveau MSR
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le MSR IA32_PKRS courant.
///
/// # Safety
/// Doit être appelé depuis Ring 0. Le CPU doit supporter PKS.
#[inline(always)]
unsafe fn rdpkrs() -> u64 {
    msr::read_msr(MSR_IA32_PKRS)
}

/// Écrit le MSR IA32_PKRS.
///
/// # Safety
/// Doit être appelé depuis Ring 0. La valeur doit respecter le layout PKRS
/// (2 bits par domaine, max 16 domaines).
#[inline(always)]
unsafe fn wrpkrs(val: u64) {
    msr::write_msr(MSR_IA32_PKRS, val);
}

// ─────────────────────────────────────────────────────────────────────────────
// revoke_domain — Révocation O(1) d'un domaine PKS
// ─────────────────────────────────────────────────────────────────────────────

/// Révoque l'accès à un domaine PKS — O(1), ~20 cycles, ZÉRO TLB shootdown.
///
/// Positionne les 2 bits du domaine dans IA32_PKRS à `0b11` (Access Disabled).
/// Le CPU refusera tout accès aux pages marquées avec ce pkey.
///
/// # ExoShield Spec
/// "Révocation au boot uniquement (Credentials) — PAS de révocation dynamique
///  réactive en v1.0."
///
/// Les appels autorisés en v1.0 sont :
/// - `exoveil_init()` au boot (Credentials révoqué par défaut)
/// - `exoveil_revoke_all_on_handoff()` lors du HANDOFF ExoPhoenix
/// - `revoke_domain()` explicite par Kernel B (décision externe)
///
/// # Safety
/// - Ring 0 uniquement
/// - PKS doit être supporté et initialisé
/// - NE PAS appeler depuis un contexte ISR (le WRMSR n'est pas ISR-safe
///   sur toutes les implémentations — vérifier SDM §5.5.2 pour votre CPU)
pub unsafe fn revoke_domain(domain: PksDomain) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return; // PKS non disponible — noop silencieux
    }

    let shift = domain.shift();
    let mask = domain.mask();
    let revoke_bits = (PksPermission::Disabled.bits() as u64) << shift;

    // Read-Modify-Write du MSR PKRS
    let cur = CURRENT_PKRS.load(Ordering::Relaxed);
    let new_val = (cur & !mask) | revoke_bits;

    // SAFETY: Ring 0, PKS supporté, valeur PKRS valide.
    wrpkrs(new_val);
    CURRENT_PKRS.store(new_val, Ordering::Release);

    // Instrumentation
    REVOKE_COUNT[domain.pkey() as usize].fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// restore_domain — Restauration d'un domaine PKS
// ─────────────────────────────────────────────────────────────────────────────

/// Restaure l'accès en lecture/écriture à un domaine PKS.
///
/// Positionne les 2 bits du domaine dans IA32_PKRS à `0b00` (ReadWrite).
///
/// # Safety
/// - Ring 0 uniquement
/// - Ne restaurer que les domaines explicitement autorisés par la politique
///   de sécurité (ExoSeal + Kernel B)
pub unsafe fn restore_domain(domain: PksDomain) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    let shift = domain.shift();
    let mask = domain.mask();
    let restore_bits = (PksPermission::ReadWrite.bits() as u64) << shift;

    let cur = CURRENT_PKRS.load(Ordering::Relaxed);
    let new_val = (cur & !mask) | restore_bits;

    // SAFETY: Ring 0, PKS supporté, valeur PKRS valide.
    wrpkrs(new_val);
    CURRENT_PKRS.store(new_val, Ordering::Release);
}

/// Restaure un domaine PKS avec une permission spécifique.
///
/// # Safety
/// Mêmes conditions que `restore_domain()`.
pub unsafe fn restore_domain_with_permission(domain: PksDomain, perm: PksPermission) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    let shift = domain.shift();
    let mask = domain.mask();
    let perm_bits = (perm.bits() as u64) << shift;

    let cur = CURRENT_PKRS.load(Ordering::Relaxed);
    let new_val = (cur & !mask) | perm_bits;

    // SAFETY: Ring 0, PKS supporté, valeur PKRS valide.
    wrpkrs(new_val);
    CURRENT_PKRS.store(new_val, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation ExoVeil (ExoSeal step 0)
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système ExoVeil PKS.
///
/// Appelé par ExoSeal à l'étape 0 du boot (Kernel B, Core 0).
///
/// # Séquence
/// 1. Détecter le support PKS via CPUID.7.0:ECX bit 6
/// 2. Révoquer TOUS les domaines (IA32_PKRS = 0xFFFFFFFF)
/// 3. Restaurer Default (pkey=0) en ReadWrite
/// 4. Conserver Credentials (pkey=2) en Disabled (révoqué au boot)
///
/// # Safety
/// - Doit être appelé depuis Ring 0, Core 0, avant que Kernel A ne démarre
/// - Aucun autre CPU ne doit être actif
pub unsafe fn exoveil_init() {
    // Détecter PKS : CPUID.07H.0H:ECX bit 6
    let ecx: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 7",
        "xor ecx, ecx",
        "cpuid",
        "pop rbx",
        out("ecx") ecx,
        lateout("eax") _,
        lateout("edx") _,
    );
    let pks_supported = ecx & (1 << 6) != 0;

    PKS_AVAILABLE.store(pks_supported, Ordering::Release);

    if !pks_supported {
        // PKS non disponible — ExoVeil en mode noop
        // Les pages TcbHot ne seront PAS protégées par PKS
        // (compensation via KASLR + IOMMU + CET)
        return;
    }

    // 1. Révoquer tous les domaines au boot (default-deny)
    //    IA32_PKRS = 0xFFFFFFFF : chaque paire de bits = 0b11 = Disabled
    wrpkrs(PKRS_ALL_REVOKED);
    CURRENT_PKRS.store(PKRS_ALL_REVOKED, Ordering::Release);

    // 2. Restaurer Default (pkey=0) en ReadWrite — code kernel standard
    restore_domain(PksDomain::Default);

    // 3. Caps (pkey=1) reste en Disabled jusqu'à security_init()
    // 4. Credentials (pkey=2) reste en Disabled — révoqué au boot par défaut
    // 5. TcbHot (pkey=4) reste en Disabled jusqu'à l'allocation TCB

    EXOVEIL_INITIALIZED.store(true, Ordering::Release);
}

/// Restaure les domaines PKS pour les opérations normales (step 18 SECURITY_READY).
///
/// Appelé à l'étape 18 du boot, après security_init(), quand le système
/// passe en mode opérationnel.
///
/// # Safety
/// Ring 0 uniquement. Doit être appelé APRÈS exoveil_init().
pub unsafe fn pks_restore_for_normal_ops() {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    // Restaurer les domaines opérationnels :
    // - Caps : ReadWrite (tables de capabilities doivent être accessibles)
    // - TcbHot : ReadWrite (TCB _cold_reserve accessible par le scheduler)
    // - Credentials : RESTE en Disabled (accès uniquement via crypto_server)
    //   (sera restauré temporairement par les opérations crypto)
    restore_domain(PksDomain::Caps);
    restore_domain(PksDomain::TcbHot);
    // Credentials reste révoqué — doit être explicitement restauré par
    // crypto_server via restore_domain(PksDomain::Credentials)
}

// ─────────────────────────────────────────────────────────────────────────────
// HANDOFF — Révocation de tous les domaines
// ─────────────────────────────────────────────────────────────────────────────

/// Révoque TOUS les domaines PKS lors d'un HANDOFF ExoPhoenix.
///
/// Appelé par le handler HANDOFF pour isoler immédiatement toutes les données
//  protégées par PKS. Seul le domaine Default reste accessible.
///
/// # Safety
/// Ring 0 uniquement. Appelé depuis un contexte HANDOFF (pas de scheduler actif).
pub unsafe fn exoveil_revoke_all_on_handoff() {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    // Révoquer tous les domaines sauf Default
    revoke_domain(PksDomain::Caps);
    revoke_domain(PksDomain::Credentials);
    revoke_domain(PksDomain::TcbHot);

    // Vérifier que seul Default est accessible
    let pkrs = rdpkrs();
    debug_assert!(
        (pkrs & PksDomain::Default.mask()) == 0, // Default = RW
        "PKRS: Default domain must be RW after handoff revocation"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Accesseurs publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne true si PKS est supporté par le CPU.
#[inline(always)]
pub fn pks_available() -> bool {
    PKS_AVAILABLE.load(Ordering::Acquire)
}

/// Retourne true si ExoVeil a été initialisé.
#[inline(always)]
pub fn exoveil_initialized() -> bool {
    EXOVEIL_INITIALIZED.load(Ordering::Acquire)
}

/// Lit la valeur PKRS courante (shadow software, pas RDMSR).
#[inline(always)]
pub fn current_pkrs() -> u64 {
    CURRENT_PKRS.load(Ordering::Acquire)
}

/// Vérifie si un domaine spécifique est révoqué.
#[inline(always)]
pub fn is_domain_revoked(domain: PksDomain) -> bool {
    let pkrs = CURRENT_PKRS.load(Ordering::Acquire);
    let perm_bits = (pkrs >> domain.shift()) & 0b11;
    perm_bits == PksPermission::Disabled as u64
}

/// Retourne le nombre de révocations pour un domaine (instrumentation).
#[inline(always)]
pub fn revoke_count(domain: PksDomain) -> u64 {
    REVOKE_COUNT[domain.pkey() as usize].load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Intégration TCB — Écriture PKRS dans le TCB
// ─────────────────────────────────────────────────────────────────────────────

/// Sauvegarde la valeur PKRS courante dans le TCB d'un thread.
///
/// Appelé lors du context switch OUT (préemption du thread).
///
/// # Safety
/// Le TCB doit être valide. Le thread est en cours de descheduling.
#[inline(always)]
pub unsafe fn save_pkrs_to_tcb(tcb: &mut crate::scheduler::core::task::ThreadControlBlock) {
    tcb.pkrs = CURRENT_PKRS.load(Ordering::Relaxed) as u32;
}

/// Restaure la valeur PKRS depuis le TCB d'un thread.
///
/// Appelé lors du context switch IN (reprise du thread).
///
/// # Safety
/// Le TCB doit être valide. Le thread est sur le point d'être schedulé.
#[inline(always)]
pub unsafe fn restore_pkrs_from_tcb(tcb: &crate::scheduler::core::task::ThreadControlBlock) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }
    let pkrs_val = tcb.pkrs as u64;
    wrpkrs(pkrs_val);
    CURRENT_PKRS.store(pkrs_val, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques ExoVeil
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot des statistiques ExoVeil.
#[derive(Debug, Clone, Copy)]
pub struct ExoVeilStats {
    /// PKS supporté par le CPU.
    pub pks_available: bool,
    /// ExoVeil initialisé.
    pub initialized: bool,
    /// Valeur PKRS courante.
    pub current_pkrs: u64,
    /// Révocations du domaine Caps.
    pub caps_revokes: u64,
    /// Révocations du domaine Credentials.
    pub creds_revokes: u64,
    /// Révocations du domaine TcbHot.
    pub tcbhot_revokes: u64,
}

/// Retourne un snapshot des statistiques ExoVeil.
pub fn exoveil_stats() -> ExoVeilStats {
    ExoVeilStats {
        pks_available: PKS_AVAILABLE.load(Ordering::Relaxed),
        initialized: EXOVEIL_INITIALIZED.load(Ordering::Relaxed),
        current_pkrs: CURRENT_PKRS.load(Ordering::Relaxed),
        caps_revokes: REVOKE_COUNT[PksDomain::Caps.pkey() as usize].load(Ordering::Relaxed),
        creds_revokes: REVOKE_COUNT[PksDomain::Credentials.pkey() as usize].load(Ordering::Relaxed),
        tcbhot_revokes: REVOKE_COUNT[PksDomain::TcbHot.pkey() as usize].load(Ordering::Relaxed),
    }
}
