// kernel/src/security/exocage.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoCage — CET Shadow Stack + IBT avec intégration TCB (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoCage est le module Couche 1 d'ExoShield — il garantit l'intégrité du flux
// de contrôle via Intel CET (Shadow Stack hardware + Indirect Branch Tracking).
//
// Architecture :
//   • Shadow Stack : copie hardware des adresses de retour (CET Shadow Stack)
//   • IBT : toute cible de branchement indirect doit commencer par ENDBR64
//   • Token obligatoire au sommet de la shadow stack (Intel CET §3.4)
//   • Handler #CP : toute violation = compromission confirmée = HANDOFF immédiat
//
// INTÉGRATION TCB GI-01 :
//   _cold_reserve[144]   shadow_stack_token : u64   (PKS domain TcbHot)
//   _cold_reserve[152]   cet_flags          : u8    (bit 0 = CET_EN)
//   _cold_reserve[153]   threat_score_u8    : u8    (0..=100)
//   _cold_reserve[160]   pt_buffer_phys     : u64   (Phase 4, LBR/PT futur)
//   _cold_reserve[168..232] réservé
//
// CONTRAINTE ABSOLUE : size_of::<TCB>() == 256 bytes — ZÉRO impact offsets hardcodés.
//
// RÉFÉRENCES :
//   Intel SDM Vol.1 §18.3 (CET Overview)
//   Intel SDM Vol.3 §2.7 (Shadow Stack)
//   ExoShield_v1_Production.md — MODULE 3 : ExoCage
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::cpu::msr;
use crate::scheduler::core::task::ThreadControlBlock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes MSR CET (référence : Intel SDM Vol.4 §2)
// ─────────────────────────────────────────────────────────────────────────────

/// MSR IA32_U_CET — User-mode CET control.
const MSR_IA32_U_CET:   u32 = 0x6A0;
/// MSR IA32_S_CET — Supervisor-mode CET control.
const MSR_IA32_S_CET:   u32 = 0x6A2;
/// MSR IA32_PL0_SSP — Ring 0 Shadow Stack Pointer.
const MSR_IA32_PL0_SSP: u32 = 0x6A4;
/// MSR IA32_PL1_SSP — Ring 1 Shadow Stack Pointer.
const MSR_IA32_PL1_SSP: u32 = 0x6A5;
/// MSR IA32_PL2_SSP — Ring 2 Shadow Stack Pointer.
#[allow(dead_code)]
const MSR_IA32_PL2_SSP: u32 = 0x6A6;
/// MSR IA32_PL3_SSP — Ring 3 Shadow Stack Pointer (userspace).
#[allow(dead_code)]
const MSR_IA32_PL3_SSP: u32 = 0x6A7;
/// MSR IA32_INTERRUPT_SSP_TABLE — SSP table pour interrupt shadow stack.
#[allow(dead_code)]
const MSR_IA32_INTERRUPT_SSP_TABLE: u32 = 0x6A8;

// ─────────────────────────────────────────────────────────────────────────────
// Bits de contrôle CET (MSR IA32_S_CET / IA32_U_CET)
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 0 — Shadow Stack Enable.
const CET_SHSTK_EN:    u64 = 1 << 0;
/// Bit 1 — WRSS instruction enable (required for kernel shadow stack management).
const CET_WR_SHSTK_EN: u64 = 1 << 1;
/// Bit 2 — ENDBRANCH enforcement (IBT).
const CET_ENDBR_EN:    u64 = 1 << 2;
/// Bit 3 — Legacy Shadow Stack compatibility (IBT only when set).
#[allow(dead_code)]
const CET_LEG_IW_EN:   u64 = 1 << 3;
/// Bit 4 — No Track for indirect CALL/JMP (suppress IBT when set).
#[allow(dead_code)]
const CET_NO_TRACK_EN: u64 = 1 << 4;
/// Bit 5 — Suppress Shadow Stack error on WRSS.
#[allow(dead_code)]
const CET_SUPPRESS_DIS: u64 = 1 << 5;

// ─────────────────────────────────────────────────────────────────────────────
// CR4.CET (bit 23) — Feature Enable
// ─────────────────────────────────────────────────────────────────────────────

const CR4_CET_BIT: u64 = 1 << 23;

// ─────────────────────────────────────────────────────────────────────────────
// Flags TCB _cold_reserve — cet_flags (offset [152])
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 0 : CET Shadow Stack activé pour ce thread.
pub const CET_FLAG_ENABLED: u8 = 1 << 0;
/// Bit 1 : IBT activé pour ce thread.
pub const CET_FLAG_IBT:     u8 = 1 << 1;
/// Bit 2 : Token shadow stack validé au setup.
pub const CET_FLAG_TOKEN_VALID: u8 = 1 << 2;

// ─────────────────────────────────────────────────────────────────────────────
// Taille Shadow Stack par thread
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de pages allouées pour la shadow stack de chaque thread (4 pages = 16 KiB).
const SHADOW_STACK_PAGES: usize = 4;
/// Taille d'une page (4 KiB).
const PAGE_SIZE: u64 = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// État global ExoCage
// ─────────────────────────────────────────────────────────────────────────────

/// CET Shadow Stack activé globalement (ExoSeal step 0).
static EXOCAGE_GLOBAL_ENABLED: AtomicBool = AtomicBool::new(false);

/// IBT activé globalement (ExoSeal step 0).
static EXOCAGE_IBT_ENABLED: AtomicBool = AtomicBool::new(false);

/// Compteur de violations #CP (cumulatif).
static CP_VIOLATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Compteur de threads avec CET actif.
static CET_THREAD_COUNT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs ExoCage
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs possibles lors de l'activation CET par thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExoCageError {
    /// CET non supporté par le CPU.
    NotSupported,
    /// CET non activé globalement (ExoSeal step 0 non exécuté).
    NotGloballyEnabled,
    /// Impossible d'allouer les pages shadow stack.
    AllocFailed,
    /// Le token shadow stack est invalide après écriture.
    TokenVerificationFailed,
}

// ─────────────────────────────────────────────────────────────────────────────
// CPUID CET — vérification support matériel
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie le support CET via CPUID.7.0.
///
/// Retourne `(shadow_stack_supported, ibt_supported)`.
///
/// - CPUID.07H.0H:ECX bit 7 = CET Shadow Stack
/// - CPUID.07H.0H:EDX bit 20 = IBT (Indirect Branch Tracking)
#[inline]
pub fn cpuid_cet_available() -> (bool, bool) {
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("edx") edx,
            lateout("eax") _,
        );
    }
    let ss = ecx & (1 << 7) != 0;
    let ibt = edx & (1 << 20) != 0;
    (ss, ibt)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers TCB _cold_reserve — accès sécurisés aux offsets ExoShield
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit un u64 dans _cold_reserve du TCB à l'offset spécifié.
///
/// # Safety
/// L'offset + 8 ne doit pas dépasser la taille de _cold_reserve (88 bytes).
/// Seuls les offsets ExoShield documentés sont autorisés.
#[inline(always)]
unsafe fn tcb_write_cold_u64(tcb: &mut ThreadControlBlock, offset: usize, val: u64) {
    debug_assert!(offset + 8 <= 88, "TCB _cold_reserve write out of bounds");
    let base = tcb._cold_reserve.as_ptr() as *const u8 as *mut u8;
    core::ptr::write_volatile(base.add(offset) as *mut u64, val);
}

/// Lit un u64 depuis _cold_reserve du TCB à l'offset spécifié.
#[inline(always)]
unsafe fn tcb_read_cold_u64(tcb: &ThreadControlBlock, offset: usize) -> u64 {
    debug_assert!(offset + 8 <= 88, "TCB _cold_reserve read out of bounds");
    let base = tcb._cold_reserve.as_ptr() as *const u8;
    core::ptr::read_volatile(base.add(offset) as *const u64)
}

/// Écrit un u8 dans _cold_reserve du TCB à l'offset spécifié.
#[inline(always)]
unsafe fn tcb_write_cold_u8(tcb: &mut ThreadControlBlock, offset: usize, val: u8) {
    debug_assert!(offset < 88, "TCB _cold_reserve write out of bounds");
    let base = tcb._cold_reserve.as_ptr() as *const u8 as *mut u8;
    core::ptr::write_volatile(base.add(offset), val);
}

/// Lit un u8 depuis _cold_reserve du TCB à l'offset spécifié.
#[inline(always)]
unsafe fn tcb_read_cold_u8(tcb: &ThreadControlBlock, offset: usize) -> u8 {
    debug_assert!(offset < 88, "TCB _cold_reserve read out of bounds");
    let base = tcb._cold_reserve.as_ptr() as *const u8;
    core::ptr::read_volatile(base.add(offset))
}

// ─────────────────────────────────────────────────────────────────────────────
// Offsets ExoShield dans _cold_reserve (relatifs à [144])
// ─────────────────────────────────────────────────────────────────────────────

/// Offset de shadow_stack_token dans _cold_reserve (= [144] - [144] = 0).
const OFF_SHADOW_STACK_TOKEN: usize = 0;   // [144] u64
/// Offset de cet_flags dans _cold_reserve (= [152] - [144] = 8).
const OFF_CET_FLAGS:          usize = 8;   // [152] u8
/// Offset de threat_score_u8 dans _cold_reserve (= [153] - [144] = 9).
const OFF_THREAT_SCORE:       usize = 9;   // [153] u8
/// Offset de pt_buffer_phys dans _cold_reserve (= [160] - [144] = 16).
const OFF_PT_BUFFER_PHYS:     usize = 16;  // [160] u64
const HANDOFF_FREEZE_REQ: u64 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Allocation Shadow Stack (placeholder — dépend du memory manager)
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue `count` pages physiques contiguës pour la shadow stack.
///
/// Retourne l'adresse physique de base, ou 0 si l'allocation échoue.
///
/// # Note
/// L'implémentation complète dépend du buddy allocator. En Phase 3.1,
/// cette fonction est un placeholder qui sera relié à `phys_alloc::alloc_pages()`
/// une fois le memory manager initialisé. Pour le boot, les shadow stacks
/// BSP sont allouées statiquement.
#[inline]
fn alloc_shadow_stack_pages(count: usize) -> u64 {
    // Phase 3.1 : allocation statique pour BSP via le memory manager.
    // Le caller (enable_cet_for_thread) fournit l'adresse pré-allouée.
    // Cette fonction est un fallback pour les threads créés dynamiquement.
    let _ = count;
    0 // TODO: brancher sur phys_alloc::alloc_pages(order, AllocFlags::ZEROED)
}

/// Libère les pages shadow stack d'un thread (appelé à la mort du thread).
#[inline]
fn free_shadow_stack_pages(_base: u64, _count: usize) {
    // TODO: brancher sur phys_alloc::free_pages()
}

// ─────────────────────────────────────────────────────────────────────────────
// enable_cet_for_thread — Activation CET par thread
// ─────────────────────────────────────────────────────────────────────────────

/// Active CET Shadow Stack + IBT pour un thread spécifique.
///
/// Cette fonction configure :
/// 1. Les MSR SSP (PL0/PL1) pour pointer vers la shadow stack allouée
/// 2. Le token obligatoire au sommet de la shadow stack (Intel CET §3.4)
/// 3. Les flags dans le TCB `_cold_reserve[144..154]`
///
/// # Séquence exacte (spec ExoShield MODULE 3)
///
/// ```text
/// 1. CPUID check (CET_SS disponible)
/// 2. Allouer 4 pages shadow stack (bit 63 PTE = shadow stack marker)
/// 3. Écrire token obligatoire au sommet (busy bit = 1)
/// 4. WRMSR IA32_PL0_SSP (0x6A4) = token_addr
/// 5. WRMSR IA32_PL1_SSP (0x6A5) = token_addr
/// 6. Sauvegarder token dans TCB _cold_reserve[144]
/// 7. Sauvegarder cet_flags dans TCB _cold_reserve[152]
/// ```
///
/// # Safety
/// - Doit être appelé depuis Ring 0 uniquement
/// - Le TCB doit être correctement aligné (64 bytes, repr(C))
/// - CET doit être activé globalement (CR4.CET + IA32_S_CET) par ExoSeal
/// - Le thread ne doit PAS être en cours d'exécution sur un autre CPU
///
/// # Contraintes
/// - `size_of::<TCB>()` reste 256 bytes — écriture uniquement dans `_cold_reserve[144..232]`
/// - Offsets hardcodés inchangés : kstack@8, cr3@56, fpu@232, rq@240/248
pub unsafe fn enable_cet_for_thread(tcb: &mut ThreadControlBlock) -> Result<(), ExoCageError> {
    // 1. Vérification CPUID
    let (ss_ok, ibt_ok) = cpuid_cet_available();
    if !ss_ok {
        return Err(ExoCageError::NotSupported);
    }

    // 2. Vérifier que CET est activé globalement (ExoSeal step 0)
    if !EXOCAGE_GLOBAL_ENABLED.load(Ordering::Acquire) {
        return Err(ExoCageError::NotGloballyEnabled);
    }

    // 3. Allouer 4 pages pour la shadow stack
    let ss_base = alloc_shadow_stack_pages(SHADOW_STACK_PAGES);
    if ss_base == 0 {
        return Err(ExoCageError::AllocFailed);
    }

    let ss_top = ss_base + (SHADOW_STACK_PAGES as u64) * PAGE_SIZE;

    // 4. Token OBLIGATOIRE au sommet (Intel CET Spec §3.4)
    //    Prévient SROP (Sigreturn-Oriented Programming)
    //    Format : (adresse_token & ~0x7) | busy_bit (bit 0 = 1)
    let token_addr = ss_top - 8;
    let token_val = (token_addr & !0x7_u64) | 0x1; // busy bit = 1

    // Écriture du token via WRSSQ (seule instruction autorisée sur shadow stack)
    // SAFETY: token_addr pointe vers une page shadow stack PTE allouée ci-dessus.
    //         CR4.CET et IA32_S_CET.WR_SHSTK_EN sont actifs (ExoSeal step 0).
    core::arch::asm!(
        ".byte 0xF3, 0x48, 0x0F, 0x01, 0x3E",  // WRSSQ [rdi], rsi
        in("rdi") token_addr as *mut u64,
        in("rsi") token_val,
        options(nostack, preserves_flags),
    );

    // 5. Vérification du token écrit (lecture volatile pour confirmer)
    let readback = core::ptr::read_volatile(token_addr as *const u64);
    if readback != token_val {
        return Err(ExoCageError::TokenVerificationFailed);
    }

    // 6. Configurer les MSR SSP Ring 0 et Ring 1
    //    PL0 = Ring 0 (kernel), PL1 = Ring 1 (servers)
    //    SAFETY: MSR existants, CET globalement activé, ring 0 uniquement.
    msr::write_msr(MSR_IA32_PL0_SSP, token_addr as u64);
    msr::write_msr(MSR_IA32_PL1_SSP, token_addr as u64);

    // 7. Sauvegarder dans le TCB _cold_reserve
    //    [144] shadow_stack_token : u64
    //    [152] cet_flags          : u8
    //    [153] threat_score_u8    : u8 (initialisé à 0)
    tcb_write_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN, token_val);

    let mut cet_flags: u8 = CET_FLAG_ENABLED | CET_FLAG_TOKEN_VALID;
    if ibt_ok {
        cet_flags |= CET_FLAG_IBT;
    }
    tcb_write_cold_u8(tcb, OFF_CET_FLAGS, cet_flags);
    tcb_write_cold_u8(tcb, OFF_THREAT_SCORE, 0); // score initial = 0

    // Phase 4 placeholder : pt_buffer_phys = 0
    tcb_write_cold_u64(tcb, OFF_PT_BUFFER_PHYS, 0);

    CET_THREAD_COUNT.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// disable_cet_for_thread — Désactivation à la mort du thread
// ─────────────────────────────────────────────────────────────────────────────

/// Désactive CET pour un thread et libère ses ressources.
///
/// Appelé depuis `do_exit()` ou le cleanup de thread.
///
/// # Safety
/// Le thread ne doit plus être schedulable (state == Dead).
pub unsafe fn disable_cet_for_thread(tcb: &mut ThreadControlBlock) {
    let cet_flags = tcb_read_cold_u8(tcb, OFF_CET_FLAGS);
    if cet_flags & CET_FLAG_ENABLED == 0 {
        return; // CET jamais activé pour ce thread
    }

    // Récupérer le token pour calculer la base de la shadow stack
    let token_val = tcb_read_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN);
    if token_val != 0 {
        // Le token contient l'adresse (token_val & ~0x7) au sommet de la SS
        let token_addr = token_val & !0x7_u64;
        let ss_base = token_addr + 8 - (SHADOW_STACK_PAGES as u64) * PAGE_SIZE;
        free_shadow_stack_pages(ss_base, SHADOW_STACK_PAGES);
    }

    // Effacer les champs ExoShield dans le TCB
    tcb_write_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN, 0);
    tcb_write_cold_u8(tcb, OFF_CET_FLAGS, 0);
    tcb_write_cold_u8(tcb, OFF_THREAT_SCORE, 0);
    tcb_write_cold_u64(tcb, OFF_PT_BUFFER_PHYS, 0);

    CET_THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler #CP — Control Protection Exception (vecteur 21)
// ─────────────────────────────────────────────────────────────────────────────

/// Handler d'exception #CP (Control Protection).
///
/// Toute violation #CP = compromission confirmée = HANDOFF IMMÉDIAT vers ExoPhoenix.
///
/// # Code d'erreur #CP (error_code)
///
/// | Bits  | Signification                              |
/// |-------|---------------------------------------------|
/// | [5:0] | Type : 1=SHADOW_STACK, 2=IBT, 3=FPES       |
/// | [31:6]| Réservé (0)                                 |
/// | [63:32| Page-adr hint (shadow stack violations)     |
///
/// # ExoShield Spec
/// "Tout #CP déclenché et loggé sur toute violation CET.
///  Handoff immédiat — pas de scoring progressif."
///
/// # Safety
/// Cette fonction est un handler d'interruption — elle ne doit jamais retourner
/// normalement. Elle déclenche un HANDOFF vers Kernel B.
pub extern "C" fn cp_handler(
    _frame: usize,
    error_code: u64,
) {
    // 1. Incrémenter le compteur global de violations
    CP_VIOLATION_COUNT.fetch_add(1, Ordering::Relaxed);

    // 2. Logger en zone P0 ExoLedger (non-écrasable)
    //    Note : exo_ledger_append_p0() est ISR-safe (lock-free, pas d'allocation)
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::CpViolation { error_code },
    );

    // 3. HANDOFF immédiat vers ExoPhoenix Kernel B
    //    Écriture dans SSR.HANDOFF_FLAG — déclenche le freeze de Kernel A
    //    via IPI depuis Kernel B.
    //
    //    SAFETY: SSR est une région mémoire réservée E820, accessible en ring 0.
    //            L'écriture est atomique (single u64 write).
    //
    //    Note: ssr_write_atomic sera branché sur l'infrastructure ExoPhoenix
    //          existante (kernel/src/exophoenix/ssr.rs).
    unsafe {
        crate::exophoenix::ssr::ssr_atomic(crate::exophoenix::ssr::SSR_HANDOFF_FLAG)
            .store(HANDOFF_FREEZE_REQ, Ordering::Release);
    }

    // 4. Ne jamais retourner — le HANDOFF va freezer ce core
    //    En attendant l'IPI de Kernel B, on spin
    loop {
        core::hint::spin_loop();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Activation globale ExoCage (appelée par ExoSeal à l'étape 0)
// ─────────────────────────────────────────────────────────────────────────────

/// Active CET Shadow Stack + IBT globalement (ExoSeal step 0).
///
/// Cette fonction est appelée UNE SEULE FOIS par Kernel B avant que Kernel A
/// ne démarre. Elle configure :
/// - CR4.CET (bit 23) = 1
/// - IA32_S_CET : SHSTK_EN | WR_SHSTK_EN | ENDBR_EN
/// - Positionne les flags globaux ExoCage
///
/// # Safety
/// - Doit être appelé depuis Ring 0 sur Core 0 (Kernel B / ExoSeal)
/// - Doit être appelé AVANT que Kernel A ne démarre (step 0)
/// - Aucun autre CPU ne doit être actif pendant cet appel
pub unsafe fn exocage_global_enable() -> Result<(), ExoCageError> {
    let (ss_ok, ibt_ok) = cpuid_cet_available();
    if !ss_ok {
        return Err(ExoCageError::NotSupported);
    }

    // 1. Activer CR4.CET (bit 23)
    let cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4);
    let cr4_new = cr4 | CR4_CET_BIT;
    core::arch::asm!("mov cr4, {}", in(reg) cr4_new);

    // 2. Configurer IA32_S_CET : Shadow Stack + WRSS + IBT
    let mut s_cet_val = CET_SHSTK_EN | CET_WR_SHSTK_EN;
    if ibt_ok {
        s_cet_val |= CET_ENDBR_EN;
    }
    msr::write_msr(MSR_IA32_S_CET, s_cet_val);

    // 3. Activer IA32_U_CET pour les threads userspace futurs
    let mut u_cet_val = CET_SHSTK_EN;
    if ibt_ok {
        u_cet_val |= CET_ENDBR_EN;
    }
    msr::write_msr(MSR_IA32_U_CET, u_cet_val);

    // 4. Positionner les flags globaux
    EXOCAGE_GLOBAL_ENABLED.store(true, Ordering::Release);
    if ibt_ok {
        EXOCAGE_IBT_ENABLED.store(true, Ordering::Release);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Accesseurs publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne true si CET Shadow Stack est activé globalement.
#[inline(always)]
pub fn is_cet_global_enabled() -> bool {
    EXOCAGE_GLOBAL_ENABLED.load(Ordering::Acquire)
}

/// Retourne true si IBT est activé globalement.
#[inline(always)]
pub fn is_ibt_global_enabled() -> bool {
    EXOCAGE_IBT_ENABLED.load(Ordering::Acquire)
}

/// Lit le shadow_stack_token d'un TCB.
///
/// # Safety
/// Le TCB doit être valide et aligné. Lecture seule, ISR-safe.
#[inline(always)]
pub unsafe fn get_shadow_stack_token(tcb: &ThreadControlBlock) -> u64 {
    tcb_read_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN)
}

/// Lit les cet_flags d'un TCB.
#[inline(always)]
pub unsafe fn get_cet_flags(tcb: &ThreadControlBlock) -> u8 {
    tcb_read_cold_u8(tcb, OFF_CET_FLAGS)
}

/// Lit le threat_score d'un TCB.
#[inline(always)]
pub unsafe fn get_threat_score(tcb: &ThreadControlBlock) -> u8 {
    tcb_read_cold_u8(tcb, OFF_THREAT_SCORE)
}

/// Écrit le threat_score d'un TCB.
///
/// # Safety
/// Le TCB doit être valide. Le thread ne doit pas être en cours d'exécution
/// sur un autre CPU (accès exclusif).
#[inline(always)]
pub unsafe fn set_threat_score(tcb: &mut ThreadControlBlock, score: u8) {
    debug_assert!(score <= 100, "threat_score must be 0..=100");
    tcb_write_cold_u8(tcb, OFF_THREAT_SCORE, score.min(100));
}

/// Nombre total de violations #CP depuis le boot.
#[inline(always)]
pub fn cp_violation_count() -> u64 {
    CP_VIOLATION_COUNT.load(Ordering::Relaxed)
}

/// Nombre de threads avec CET actif.
#[inline(always)]
pub fn cet_thread_count() -> u64 {
    CET_THREAD_COUNT.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques ExoCage
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot des statistiques ExoCage.
#[derive(Debug, Clone, Copy)]
pub struct ExoCageStats {
    /// CET Shadow Stack activé globalement.
    pub global_enabled: bool,
    /// IBT activé globalement.
    pub ibt_enabled: bool,
    /// Nombre de threads avec CET actif.
    pub thread_count: u64,
    /// Nombre total de violations #CP.
    pub cp_violations: u64,
}

/// Retourne un snapshot des statistiques ExoCage.
pub fn exocage_stats() -> ExoCageStats {
    ExoCageStats {
        global_enabled: EXOCAGE_GLOBAL_ENABLED.load(Ordering::Relaxed),
        ibt_enabled: EXOCAGE_IBT_ENABLED.load(Ordering::Relaxed),
        thread_count: CET_THREAD_COUNT.load(Ordering::Relaxed),
        cp_violations: CP_VIOLATION_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation CET pour context_switch
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie que la shadow stack token du TCB est valide avant un context switch.
///
/// Appelé depuis `switch_context()` pour valider que le thread suivant
/// a un token CET valide (prévention ROP/JOP).
///
/// Retourne `true` si le thread a CET activé ET un token valide.
#[inline(always)]
pub fn validate_thread_cet(tcb: &ThreadControlBlock) -> bool {
    // SAFETY: lecture seule du _cold_reserve, pas de modification.
    unsafe {
        let flags = tcb_read_cold_u8(tcb, OFF_CET_FLAGS);
        if flags & CET_FLAG_ENABLED == 0 {
            return true; // CET pas activé pour ce thread — acceptable (kthreads, etc.)
        }
        // CET activé : vérifier que le token est présent
        let token = tcb_read_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN);
        token != 0 && (token & 0x1) != 0 // busy bit doit être 1
    }
}
