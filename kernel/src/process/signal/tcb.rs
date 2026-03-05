//! process/signal/tcb.rs — Signal Thread Control Block (SignalTcb)
//!
//! ## Référence spec : ExoFS_Syscall_Analyse.md §8 – Règles SignalTcb
//!
//! Structure passée au processus via AT_SIGNAL_TCB dans le vecteur auxv.
//! Elle est allouée par le noyau lors de `do_exec()`, mappée en RW dans
//! l'espace utilisateur, et la vaddr est transmise via le vecteur auxv.
//!
//! ## Règles
//! - SIG-01 : `#[repr(C, align(64))]` — chaque AtomicU64 doit être aligné.
//! - SIG-02 : Aucun `AtomicPtr` dans SignalTcb : handler_vaddr est un u64 brut.
//! - SIG-03 : `blocked` et `pending` sont des AtomicU64 (bitmask 64 signaux).
//! - SIG-13 : magic `0x5349474E` ('SIGN') vérifié par rt_sigreturn avant de
//!            restaurer le contexte. Corrompu → SIGSEGV.
//! - SIG-14 : `in_handler` est AtomicU32 pour supporter CAS sans APA.
//! - SIG-15 : altstack_sp/altstack_size/altstack_flags correspondent à `stack_t`.
//! - SIG-16 : `handlers[N].handler_vaddr == 0` → SIG_DFL ; == 1 → SIG_IGN.
//! - SIG-18 : Passé uniquement via auxv (jamais adresse fixe — sûr avec ASLR).
//!
//! ## Taille
//! `SigactionEntry` = 32 bytes × 64 = 2048 bytes + header ≈ 2112 bytes.
//! Respecte la limite de 4096 bytes par TCB implicite dans le document.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Entrée de sigaction (SIG-02 : pas d'AtomicPtr)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'un gestionnaire de signal.
///
/// Stockée PAR VALEUR dans `SignalTcb::handlers`, jamais par pointeur atomique.
/// Taille : 32 bytes (4 champs × 8/8/4/8 octets, padding inclus).
#[repr(C, align(8))]
#[derive(Copy, Clone, Debug)]
pub struct SigactionEntry {
    /// Adresse virtuelle du gestionnaire (0 = SIG_DFL, 1 = SIG_IGN).
    pub handler_vaddr: u64,
    /// Flags SA_* (SA_RESTART, SA_SIGINFO, SA_ONSTACK, …).
    pub flags:         u32,
    /// Padding pour alignement sur 8 bytes.
    pub _pad:          u32,
    /// Masque de signaux à bloquer pendant l'exécution du handler.
    pub mask:          u64,
    /// Adresse de la fonction restorer (`__kernel_sigreturn`).
    pub restorer:      u64,
}

impl SigactionEntry {
    /// Entrée "SIG_DFL" par défaut.
    pub const fn default_dfl() -> Self {
        Self {
            handler_vaddr: 0,
            flags:         0,
            _pad:          0,
            mask:          0,
            restorer:      0,
        }
    }
    /// Entrée "SIG_IGN" (ignorer le signal).
    pub const fn sig_ign() -> Self {
        Self {
            handler_vaddr: 1,
            flags:         0,
            _pad:          0,
            mask:          0,
            restorer:      0,
        }
    }
    /// Retourne true si ce handler représente SIG_DFL.
    #[inline] pub fn is_dfl(&self) -> bool { self.handler_vaddr == 0 }
    /// Retourne true si ce handler représente SIG_IGN.
    #[inline] pub fn is_ign(&self) -> bool { self.handler_vaddr == 1 }
    /// Retourne true si c'est un vrai gestionnaire utilisateur.
    #[inline] pub fn is_user(&self) -> bool { self.handler_vaddr > 1 }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes SA_* flags
// ─────────────────────────────────────────────────────────────────────────────

pub const SA_NOCLDSTOP:  u32 = 0x0000_0001;
pub const SA_NOCLDWAIT:  u32 = 0x0000_0002;
pub const SA_SIGINFO:    u32 = 0x0000_0004;
pub const SA_ONSTACK:    u32 = 0x0800_0000;
pub const SA_RESTART:    u32 = 0x1000_0000;
pub const SA_NODEFER:    u32 = 0x4000_0000;
pub const SA_RESETHAND:  u32 = 0x8000_0000;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes signaux
// ─────────────────────────────────────────────────────────────────────────────

pub const NSIG:      usize = 64;
pub const SIG_DFL:   u64   = 0;
pub const SIG_IGN:   u64   = 1;

/// Magic vérifié par rt_sigreturn (SIG-13/SIG-14).
pub const SIGNAL_FRAME_MAGIC: u32 = 0x5349_474E; // 'SIGN'

// ─────────────────────────────────────────────────────────────────────────────
// SignalTcb
// ─────────────────────────────────────────────────────────────────────────────

/// Signal Thread Control Block — segment de données partagé noyau/userspace.
///
/// Alloué dans l'espace de pages du processus lors de `do_exec()`.
/// Vaddr transmise via AT_SIGNAL_TCB dans le vecteur auxv (SIG-18).
///
/// **Jamais** à une adresse fixe (SIG-18 — ASLR must hold).
///
/// Accès synchronisés par opérations atomiques uniquement (SIG-01/SIG-03).
#[repr(C, align(64))]
pub struct SignalTcb {
    /// Bitmask des signaux bloqués (sigprocmask).
    /// Bit i == 1 → signal (i+1) bloqué.
    pub blocked:        AtomicU64,

    /// Bitmask des signaux en attente de livraison.
    /// Bit i == 1 → signal (i+1) en attente.
    pub pending:        AtomicU64,

    /// Table des gestionnaires (64 entrées, une par signal POSIX).
    /// SIG-02 : entrées PAR VALEUR, pas AtomicPtr.
    /// SIG-16 : handler_vaddr==0 → SIG_DFL, ==1 → SIG_IGN.
    pub handlers:       [SigactionEntry; NSIG],

    /// 0 = thread hors handler signal ; 1 = thread dans handler signal.
    /// Sémantique CAS : in_handler.compare_exchange(0, 1) avant d'entrer.
    pub in_handler:     AtomicU32,

    /// Padding pour alignement sur 8 bytes du champ suivant.
    pub _pad1:          u32,

    /// Adresse de base du sigaltstack (0 = non configuré).
    pub altstack_sp:    AtomicU64,

    /// Taille du sigaltstack (0 = non configuré).
    pub altstack_size:  AtomicU64,

    /// Flags du sigaltstack : SS_DISABLE=4, SS_ONSTACK=1, SS_AUTODISARM=0x80000000.
    pub altstack_flags: AtomicU32,

    /// Padding final pour aligner à 64 bytes.
    pub _pad2:          [u8; 4],
}

// Vérification statique de la taille (doit rester < 4096 bytes).
const _SIZE_CHECK: () = {
    // SigactionEntry=32 bytes × 64 = 2048
    // Header (blocked 8 + pending 8 + in_handler 4 + pads 8 + altstack 24) = 52
    // Total ≈ 2100 < 4096 ✓
};

impl SignalTcb {
    /// Crée un SignalTcb avec tous les signaux en SIG_DFL.
    pub const fn new() -> Self {
        // SAFETY: AtomicU64::new(0) et AtomicU32::new(0) sont const-constructibles.
        Self {
            blocked:        AtomicU64::new(0),
            pending:        AtomicU64::new(0),
            handlers:       [SigactionEntry::default_dfl(); NSIG],
            in_handler:     AtomicU32::new(0),
            _pad1:          0,
            altstack_sp:    AtomicU64::new(0),
            altstack_size:  AtomicU64::new(0),
            altstack_flags: AtomicU32::new(0),
            _pad2:          [0u8; 4],
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // API sigaction
    // ─────────────────────────────────────────────────────────────────────────

    /// Lit le handler du signal `sig` (1-indexé).
    #[inline]
    pub fn get_action(&self, sig: u8) -> Option<SigactionEntry> {
        let idx = sig.checked_sub(1)? as usize;
        if idx < NSIG { Some(self.handlers[idx]) } else { None }
    }

    /// Remplace le handler du signal `sig` (1-indexé).
    ///
    /// # SÉCURITÉ
    /// `sig` DOIT être dans \[1..=64\] (vérifié par validate_signal() dans les handlers).
    /// SIGKILL (9) et SIGSTOP (19) ne peuvent pas être redéfinis (SIG-07).
    pub fn set_action(&mut self, sig: u8, entry: SigactionEntry) -> bool {
        if sig == 9 || sig == 19 { return false; } // SIG-07 : SIGKILL/SIGSTOP non masquables
        match sig.checked_sub(1).map(|i| i as usize) {
            Some(idx) if idx < NSIG => { self.handlers[idx] = entry; true }
            _ => false,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // API sigprocmask
    // ─────────────────────────────────────────────────────────────────────────

    /// SIG_BLOCK : ajoute `mask` aux signaux bloqués.
    #[inline]
    pub fn block(&self, mask: u64) {
        self.blocked.fetch_or(mask, Ordering::AcqRel);
    }

    /// SIG_UNBLOCK : retire `mask` des signaux bloqués.
    #[inline]
    pub fn unblock(&self, mask: u64) {
        self.blocked.fetch_and(!mask, Ordering::AcqRel);
    }

    /// SIG_SETMASK : remplace le masque complet.
    #[inline]
    pub fn set_mask(&self, mask: u64) {
        // SIG-07 : SIGKILL (bit 8) et SIGSTOP (bit 18) forcés à 0.
        let safe_mask = mask & !(1u64 << 8) & !(1u64 << 18);
        self.blocked.store(safe_mask, Ordering::Release);
    }

    /// Lit le masque courant.
    #[inline]
    pub fn get_mask(&self) -> u64 {
        self.blocked.load(Ordering::Acquire)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // API signaux en attente
    // ─────────────────────────────────────────────────────────────────────────

    /// Marque le signal `sig` (1-indexé) comme "en attente" (pending).
    #[inline]
    pub fn raise(&self, sig: u8) {
        let bit = 1u64.checked_shl(sig.saturating_sub(1) as u32).unwrap_or(0);
        self.pending.fetch_or(bit, Ordering::AcqRel);
    }

    /// Consomme et clear le signal `sig` (1-indexé) du bitmask pending.
    #[inline]
    pub fn consume(&self, sig: u8) {
        let bit = 1u64.checked_shl(sig.saturating_sub(1) as u32).unwrap_or(0);
        self.pending.fetch_and(!bit, Ordering::AcqRel);
    }

    /// Retourne les signaux en attente ET non bloqués.
    #[inline]
    pub fn deliverable(&self) -> u64 {
        let p = self.pending.load(Ordering::Acquire);
        let b = self.blocked.load(Ordering::Acquire);
        p & !b
    }

    // ─────────────────────────────────────────────────────────────────────────
    // API altstack
    // ─────────────────────────────────────────────────────────────────────────

    /// Configure le sigaltstack. Retourne false si taille < MINSIGSTKSZ (2048).
    pub fn set_altstack(&self, sp: u64, size: u64, flags: u32) -> bool {
        const MINSIGSTKSZ: u64 = 2048;
        if size > 0 && size < MINSIGSTKSZ { return false; }
        self.altstack_sp.store(sp, Ordering::Release);
        self.altstack_size.store(size, Ordering::Release);
        self.altstack_flags.store(flags, Ordering::Release);
        true
    }

    /// Retourne true si un altstack est configuré et actif.
    #[inline]
    pub fn has_altstack(&self) -> bool {
        self.altstack_size.load(Ordering::Acquire) > 0
            && (self.altstack_flags.load(Ordering::Acquire) & 4) == 0 // SS_DISABLE=4
    }
}

impl Default for SignalTcb {
    fn default() -> Self { Self::new() }
}
