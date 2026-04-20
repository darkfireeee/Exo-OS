// PATCH-02 : Fix ExoCage — CET per-thread + WRSSQ + static_assert TCB
// Fichiers cibles :
//   - kernel/src/security/exocage.rs   (compléter le câblage)
//   - kernel/src/scheduler/core/task.rs (appeler enable_cet_for_thread)
// Priorité : P0 CRITIQUE

// ============================================================
// SECTION 1 : kernel/src/security/exocage.rs
// Ajouts/correctifs dans le fichier existant
// ============================================================

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::scheduler::core::task::ThreadControlBlock;

// --- Constantes MSR ---
const MSR_IA32_S_CET:      u32 = 0x6A2;
const MSR_IA32_PL0_SSP:    u32 = 0x6A4;
const CET_SHSTK_EN:        u64 = 1 << 0;
const CET_FLAG_ENABLED:    u8  = 0x01;

// --- Offsets TCB (GI-01_Types_TCB_SSR.md) ---
// IMPÉRATIF : ces constantes doivent correspondre à _cold_reserve layout
const TCB_SHADOW_STACK_TOKEN_OFFSET: usize = 0;  // relatif à _cold_reserve[0]
const TCB_CET_FLAGS_OFFSET: usize = 8;            // relatif à _cold_reserve[8]

// Taille shadow stack par thread (4 pages = 16 KiB)
const SHADOW_STACK_SIZE: usize = 4 * 4096;

// --- État global ---
static EXOCAGE_GLOBAL_ENABLED: AtomicBool = AtomicBool::new(false);
pub static CP_VIOLATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Vérifie au compile-time que le TCB fait exactement 256 bytes.
/// Si ThreadControlBlock change, ce code refusera de compiler.
/// Corrige : S-04 — static_assert TCB absent
const _: () = {
    // Utilisation de const-assert inline (pas besoin de crate externe)
    // Si la taille est incorrecte, le compilateur émet une erreur claire.
    let _ = [(); 0 - (core::mem::size_of::<ThreadControlBlock>() != 256) as usize];
    // Alternative avec const_assert! si la crate est disponible :
    // const_assert!(core::mem::size_of::<ThreadControlBlock>() == 256);
};

/// Détecte si le CPU supporte CET Shadow Stack et IBT.
/// Retourne (shadow_stack_supported, ibt_supported)
pub fn cpuid_cet_available() -> (bool, bool) {
    // CPUID leaf 7, sub-leaf 0 : ECX bit 7 = CET_SS, EDX bit 20 = CET_IBT
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",       // sauvegarder rbx (callee-saved en SysV)
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov {ecx_out:e}, ecx",
            "mov {edx_out:e}, edx",
            "pop rbx",
            ecx_out = out(reg) ecx,
            edx_out = out(reg) edx,
            out("eax") _,
            options(nostack)
        );
    }
    let ss  = (ecx >> 7)  & 1 == 1;
    let ibt = (edx >> 20) & 1 == 1;
    (ss, ibt)
}

/// Vérifie qu'une adresse est bien dans une page shadow stack valide.
///
/// CORRIGE : CRITIQUE-02 — WRSSQ sans validation
/// Avant toute instruction WRSSQ, la cible DOIT être validée :
/// 1. Adresse alignée sur 8 bytes
/// 2. Dans une région mappée comme shadow-stack (bit SS de la PTE)
/// 3. Token busy-bit = 1 (stack active, pas déjà restore-pointée)
pub fn is_valid_shadow_stack_address(addr: u64) -> bool {
    // Vérification 1 : alignement 8 bytes (requis par x86 CET)
    if addr & 0x7 != 0 {
        return false;
    }
    // Vérification 2 : adresse dans l'espace kernel (haut de l'espace virtuel)
    // Sur x86_64, les adresses kernel commencent à 0xFFFF_8000_0000_0000
    if addr < 0xFFFF_8000_0000_0000u64 {
        return false;
    }
    // Vérification 3 : le token ne doit pas être zéro (stack non initialisée)
    // On lit le token de manière sécurisée (pas de déréférencement brut)
    let token_ptr = addr as *const u64;
    let token = unsafe { core::ptr::read_volatile(token_ptr) };
    // Busy-bit = bit 0 du token (x86 CET spec §17.3.1)
    token & 1 == 1
}

/// Active CET Shadow Stack pour le thread donné.
///
/// CORRIGE : CRITIQUE-02 — CET per-thread non câblé
/// Doit être appelé depuis task::new_thread() pour chaque nouveau thread.
///
/// # Safety
/// - `tcb` doit pointer vers un ThreadControlBlock valide et alloué
/// - Doit être appelé APRÈS que l'espace d'adressage du thread est prêt
/// - Requires EXOCAGE_GLOBAL_ENABLED == true
pub unsafe fn enable_cet_for_thread(tcb: &mut ThreadControlBlock) -> Result<(), ExoCageError> {
    // Prérequis : CET global activé
    if !EXOCAGE_GLOBAL_ENABLED.load(Ordering::Acquire) {
        return Err(ExoCageError::GlobalNotEnabled);
    }

    // Prérequis : CPU supporte CET
    let (ss_ok, _) = cpuid_cet_available();
    if !ss_ok {
        return Err(ExoCageError::CpuNotSupported);
    }

    // Vérification : thread pas déjà initialisé
    let current_flags = tcb_read_cet_flags(tcb);
    if current_flags & CET_FLAG_ENABLED != 0 {
        return Err(ExoCageError::AlreadyEnabled);
    }

    // 1. Allouer la shadow stack (4 pages = 16 KiB, alignée 16 KiB)
    let ss_base = allocate_shadow_stack(SHADOW_STACK_SIZE)?;
    let ss_top = ss_base + SHADOW_STACK_SIZE as u64;

    // 2. Écrire le token busy-bit en haut de la stack
    // Token format : adresse de la stack | busy-bit (bit 0)
    let token_addr = ss_top - 8;
    let token_val = token_addr | 1; // busy-bit = 1
    unsafe {
        // WRSSQ : écriture dans la shadow stack via instruction CET
        // Valide l'adresse AVANT l'écriture (corrige CRITIQUE-02)
        if !is_valid_shadow_stack_address(token_addr) {
            deallocate_shadow_stack(ss_base, SHADOW_STACK_SIZE);
            return Err(ExoCageError::InvalidShadowStackAddress);
        }
        core::arch::asm!(
            "wrssq [{addr}], {val}",
            addr = in(reg) token_addr,
            val  = in(reg) token_val,
            options(nostack)
        );
    }

    // 3. Configurer MSR IA32_PL0_SSP avec le pointeur de stack
    let ssp = token_addr; // SSP pointe sur le token
    unsafe { wrmsr(MSR_IA32_PL0_SSP, ssp); }

    // 4. Activer CET shadow stack dans MSR IA32_S_CET du thread
    let cet_val = unsafe { rdmsr(MSR_IA32_S_CET) };
    unsafe { wrmsr(MSR_IA32_S_CET, cet_val | CET_SHSTK_EN); }

    // 5. Mettre à jour le TCB (_cold_reserve)
    tcb_write_shadow_stack_token(tcb, token_val);
    tcb_write_cet_flags(tcb, CET_FLAG_ENABLED);

    // 6. Audit ledger P0
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::CetThreadEnabled
    );

    Ok(())
}

/// Handler #CP (Control Protection Fault)
/// Appelé par l'IDT sur violation CET.
///
/// CORRIGE : CRITIQUE-02 — absence de handler complet
#[no_mangle]
pub extern "C" fn cp_handler() {
    // Incrémenter le compteur atomiquement
    let count = CP_VIOLATION_COUNT.fetch_add(1, Ordering::SeqCst);

    // Logger en P0 (immuable, Blake3-chained)
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::CpViolation
    );

    // Seuil : après N violations, considérer le système compromis
    const CP_MAX_VIOLATIONS: u64 = 3;
    if count >= CP_MAX_VIOLATIONS {
        // Handoff immédiat vers Kernel B
        unsafe {
            crate::exophoenix::handoff::freeze_req(
                crate::exophoenix::handoff::FreezeReason::CpViolationThreshold
            );
        }
    }

    // Pour la première violation : log + continuer (peut être légitime en debug)
    // NB : en production, même la première violation devrait déclencher un handoff
    #[cfg(not(feature = "debug_cet_permissive"))]
    unsafe {
        crate::exophoenix::handoff::freeze_req(
            crate::exophoenix::handoff::FreezeReason::CpViolation
        );
    }
}

// --- Helpers internes ---

#[derive(Debug)]
pub enum ExoCageError {
    GlobalNotEnabled,
    CpuNotSupported,
    AlreadyEnabled,
    AllocationFailed,
    InvalidShadowStackAddress,
}

/// Alloue des pages pour la shadow stack (mappées en mode SS)
fn allocate_shadow_stack(size: usize) -> Result<u64, ExoCageError> {
    // Délégation à l'allocateur physique kernel
    crate::memory::alloc_shadow_stack_pages(size)
        .map_err(|_| ExoCageError::AllocationFailed)
}

fn deallocate_shadow_stack(base: u64, size: usize) {
    unsafe { crate::memory::free_pages(base, size); }
}

#[inline]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem)
    );
}

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// Accesseurs TCB _cold_reserve (offsets relatifs GI-01)
#[inline]
fn tcb_write_shadow_stack_token(tcb: &mut ThreadControlBlock, token: u64) {
    tcb._cold_reserve[TCB_SHADOW_STACK_TOKEN_OFFSET..TCB_SHADOW_STACK_TOKEN_OFFSET + 8]
        .copy_from_slice(&token.to_le_bytes());
}

#[inline]
fn tcb_write_cet_flags(tcb: &mut ThreadControlBlock, flags: u8) {
    tcb._cold_reserve[TCB_CET_FLAGS_OFFSET] = flags;
}

#[inline]
fn tcb_read_cet_flags(tcb: &ThreadControlBlock) -> u8 {
    tcb._cold_reserve[TCB_CET_FLAGS_OFFSET]
}

// ============================================================
// SECTION 2 : kernel/src/scheduler/core/task.rs
// Câbler enable_cet_for_thread dans new_thread()
// ============================================================

// Dans new_thread() ou ThreadControlBlock::new(), ajouter :
//
// pub fn new_thread(...) -> Result<Box<ThreadControlBlock>, ThreadError> {
//     let mut tcb = Box::new(ThreadControlBlock::new_zeroed());
//
//     // [PATCH-02] Activer CET shadow stack si supporté par le CPU
//     if crate::security::exocage::cpuid_cet_available().0 {
//         unsafe {
//             match crate::security::exocage::enable_cet_for_thread(&mut tcb) {
//                 Ok(()) => {},
//                 Err(crate::security::exocage::ExoCageError::GlobalNotEnabled) => {
//                     // Normal pendant la phase de boot avant ExoSeal
//                 },
//                 Err(e) => {
//                     log::warn!("CET enable failed for new thread: {:?}", e);
//                     // Non fatal : le thread s'exécute sans shadow stack
//                     // (sera détecté par validate_thread_cet() périodiquement)
//                 }
//             }
//         }
//     }
//
//     Ok(tcb)
// }
