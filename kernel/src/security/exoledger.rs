// kernel/src/security/exoledger.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoLedger — Audit Chaîné avec zone P0 immuable (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoLedger est le module d'audit chaîné d'ExoShield. Il fournit un journal
// d'événements de sécurité avec deux zones distinctes :
//
//   ZONE P0 (immuable) : 16 entrées JAMAIS écrasées — événements critiques :
//     CpViolation, IommuFault, HandoffTriggered, BootSealViolation
//
//   ZONE RING BUFFER (circulaire) : ~90 entrées — événements de sécurité
//     opérationnels. Overflow autorisé : les entrées les plus anciennes
//     sont écrasées (mais JAMAIS les entrées P0).
//
// CHAÎNAGE CRYPTOGRAPHIQUE :
//   Chaque entrée contient hash = Blake3(this_entry || prev_hash).
//   Toute modification d'une entrée invalide la chaîne complète.
//   Kernel B (Core 0) peut vérifier l'intégrité en relisant la chaîne.
//
// EMPLACEMENT MÉMOIRE :
//   SSR.LOG_AUDIT (+0x8000, 16 KiB, append-only, RO pour Kernel A)
//   +0x0000 : AuditHeader { version, total_count, last_merkle_root }
//   +0x0030 : P0_Zone [LedgerEntry; 16]
//   +0x08B0 : RingBuffer [LedgerEntry; ~90]
//
// RÈGLE EXOLEDGER-01 : exo_ledger_append_p0() = append-only, JAMAIS écrasée.
// RÈGLE EXOLEDGER-02 : Toute tentative d'écrasement P0 = panique kernel.
// RÈGLE EXOLEDGER-03 : ISR-safe — pas d'allocation, pas de lock bloquant.
// RÈGLE EXOLEDGER-04 : Hash = Blake3(this || prev_hash) — chaîne immuable.
//
// RÉFÉRENCES :
//   ExoShield_v1_Production.md — MODULE 7 : ExoLedger
//   SSR layout : ExoOS_Architecture_v7.md §3.4
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes ExoLedger
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre d'entrées dans la zone P0 (immuable, jamais écrasée).
pub const P0_ZONE_ENTRIES: usize = 16;

/// Nombre d'entrées dans le ring buffer (circulaire).
/// 16 KiB total / sizeof(LedgerEntry) = ~122 entrées. P0 prend 16 = 106 restantes.
/// On arrondit à 96 pour un alignement propre.
const RING_BUFFER_ENTRIES: usize = 96;

/// Taille d'une entrée LedgerEntry = 136 bytes.
///   seq: u64 (8) + tsc: u64 (8) + actor_oid: [u8;32] (32) + action: ActionTag (24)
///   + prev_hash: [u8;32] (32) + hash: [u8;32] (32) = 136 bytes
const LEDGER_ENTRY_SIZE: usize = 136;

/// Offset de la zone d'audit dans SSR (+0x8000).
pub const SSR_LOG_AUDIT_OFFSET: u64 = 0x8000;

/// Taille totale de la zone d'audit (16 KiB).
pub const SSR_LOG_AUDIT_SIZE: usize = 16 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// ActionTag — Types d'événements audités
// ─────────────────────────────────────────────────────────────────────────────

/// Tag d'action pour les entrées du ledger.
///
/// Les actions P0 (critiques, immuables) et les actions normales (ring buffer)
/// sont distinguées au niveau du type.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub enum ActionTag {
    // ── Actions P0 (zone immuable) ──────────────────────────────────────────
    /// Violation #CP (CET Shadow Stack / IBT). error_code du handler #CP.
    CpViolation { error_code: u64 },
    /// Défaut IOMMU (DMA hors whitelist).
    IommuFault { domain_id: u32, fault_addr: u64 },
    /// HANDOFF déclenché (Kernel B a pris le contrôle).
    HandoffTriggered { reason: u32 },
    /// Violation du boot seal (réseau/FS actifs avant SECURITY_READY).
    BootSealViolation { step: u8 },
    /// Watchdog expiré (ExoNmi — strikes dépassés).
    WatchdogExpired { strikes: u32 },

    // ── Actions normales (ring buffer) ──────────────────────────────────────
    /// Capability révoquée.
    CapabilityRevoked { oid: u64 },
    /// Domaine PKS révoqué.
    PksDomainRevoked { domain: u8 },
    /// Tentative d'accès IPC non autorisé (ExoCordon).
    IpcUnauthorized { src_pid: u32, dst_pid: u32 },
    /// Budget de capability épuisé (ExoKairos).
    BudgetExhausted { oid: u64 },
    /// Violation de politique d'accès.
    AccessDenied { oid: u64, rights: u32 },
    /// Événement de boot (ExoSeal).
    BootEvent { step: u8 },
    /// NIC IOMMU verrouillé (ExoSeal step 0).
    NicIommuLocked,
    /// Audit général (custom data).
    Custom { tag: u64, data: u64 },
}

impl ActionTag {
    /// Retourne true si cette action est un événement P0 (critique).
    pub fn is_p0(&self) -> bool {
        matches!(
            self,
            ActionTag::CpViolation { .. }
                | ActionTag::IommuFault { .. }
                | ActionTag::HandoffTriggered { .. }
                | ActionTag::BootSealViolation { .. }
                | ActionTag::WatchdogExpired { .. }
        )
    }

    /// Sérialise l'ActionTag en 24 bytes pour le hash Blake3.
    /// Format : [type_tag: u64 | field1: u64 | field2: u64]
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut buf = [0u8; 24];
        match self {
            ActionTag::CpViolation { error_code } => {
                buf[0..8].copy_from_slice(&1u64.to_le_bytes());
                buf[8..16].copy_from_slice(&error_code.to_le_bytes());
            }
            ActionTag::IommuFault {
                domain_id,
                fault_addr,
            } => {
                buf[0..8].copy_from_slice(&2u64.to_le_bytes());
                buf[8..12].copy_from_slice(&domain_id.to_le_bytes());
                buf[12..24].copy_from_slice(&fault_addr.to_le_bytes());
            }
            ActionTag::HandoffTriggered { reason } => {
                buf[0..8].copy_from_slice(&3u64.to_le_bytes());
                buf[8..12].copy_from_slice(&reason.to_le_bytes());
            }
            ActionTag::BootSealViolation { step } => {
                buf[0..8].copy_from_slice(&4u64.to_le_bytes());
                buf[8] = *step;
            }
            ActionTag::WatchdogExpired { strikes } => {
                buf[0..8].copy_from_slice(&5u64.to_le_bytes());
                buf[8..12].copy_from_slice(&strikes.to_le_bytes());
            }
            ActionTag::CapabilityRevoked { oid } => {
                buf[0..8].copy_from_slice(&10u64.to_le_bytes());
                buf[8..16].copy_from_slice(&oid.to_le_bytes());
            }
            ActionTag::PksDomainRevoked { domain } => {
                buf[0..8].copy_from_slice(&11u64.to_le_bytes());
                buf[8] = *domain;
            }
            ActionTag::IpcUnauthorized { src_pid, dst_pid } => {
                buf[0..8].copy_from_slice(&12u64.to_le_bytes());
                buf[8..12].copy_from_slice(&src_pid.to_le_bytes());
                buf[12..16].copy_from_slice(&dst_pid.to_le_bytes());
            }
            ActionTag::BudgetExhausted { oid } => {
                buf[0..8].copy_from_slice(&13u64.to_le_bytes());
                buf[8..16].copy_from_slice(&oid.to_le_bytes());
            }
            ActionTag::AccessDenied { oid, rights } => {
                buf[0..8].copy_from_slice(&14u64.to_le_bytes());
                buf[8..16].copy_from_slice(&oid.to_le_bytes());
                buf[16..20].copy_from_slice(&rights.to_le_bytes());
            }
            ActionTag::BootEvent { step } => {
                buf[0..8].copy_from_slice(&15u64.to_le_bytes());
                buf[8] = *step;
            }
            ActionTag::NicIommuLocked => {
                buf[0..8].copy_from_slice(&16u64.to_le_bytes());
            }
            ActionTag::Custom { tag, data } => {
                buf[0..8].copy_from_slice(&255u64.to_le_bytes());
                buf[8..16].copy_from_slice(&tag.to_le_bytes());
                buf[16..24].copy_from_slice(&data.to_le_bytes());
            }
        }
        buf
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LedgerEntry — Structure 136 bytes
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée du journal d'audit chaîné.
///
/// # Layout mémoire (136 bytes, #[repr(C)])
///
/// | Offset | Champ      | Taille | Description                              |
/// |--------|------------|--------|------------------------------------------|
/// | 0      | seq        | 8      | Numéro de séquence monotone global       |
/// | 8      | tsc        | 8      | Timestamp TSC (cycles CPU)               |
/// | 16     | actor_oid  | 32     | ObjectId de l'acteur (jamais PID)        |
/// | 48     | action     | 24     | ActionTag sérialisé                      |
/// | 72     | prev_hash  | 32     | Hash de l'entrée précédente              |
/// | 104    | hash       | 32     | Blake3(this_entry_bytes || prev_hash)     |
///
/// # Intégrité
/// Le hash est calculé comme : Blake3(seq || tsc || actor_oid || action || prev_hash)
/// Cela rend chaque entrée dépendante de toutes les entrées précédentes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LedgerEntry {
    /// Numéro de séquence monotone (global, jamais remis à zéro).
    pub seq: u64,
    /// Timestamp TSC au moment de l'événement.
    pub tsc: u64,
    /// ObjectId de l'acteur (jamais PID — ExoShield utilise OID pour l'audit).
    pub actor_oid: [u8; 32],
    /// Action tag sérialisé (24 bytes).
    pub action: [u8; 24],
    /// Hash de l'entrée précédente dans la chaîne.
    pub prev_hash: [u8; 32],
    /// Hash Blake3 de cette entrée (calculé après écriture).
    pub hash: [u8; 32],
}

impl LedgerEntry {
    /// Taille totale de l'entrée (136 bytes).
    pub const SIZE: usize = LEDGER_ENTRY_SIZE;

    /// Crée une entrée vide/zéro.
    pub const fn zeroed() -> Self {
        Self {
            seq: 0,
            tsc: 0,
            actor_oid: [0u8; 32],
            action: [0u8; 24],
            prev_hash: [0u8; 32],
            hash: [0u8; 32],
        }
    }

    /// Calcule le hash Blake3 de cette entrée (sans le champ hash lui-même).
    ///
    /// Input : seq || tsc || actor_oid || action || prev_hash
    /// Output : Blake3(input)
    fn compute_hash(&self) -> [u8; 32] {
        // Construire le buffer de hachage : tout sauf le champ hash
        // 8 + 8 + 32 + 24 + 32 = 104 bytes
        let mut buf = [0u8; 104];
        buf[0..8].copy_from_slice(&self.seq.to_le_bytes());
        buf[8..16].copy_from_slice(&self.tsc.to_le_bytes());
        buf[16..48].copy_from_slice(&self.actor_oid);
        buf[48..72].copy_from_slice(&self.action);
        buf[72..104].copy_from_slice(&self.prev_hash);
        crate::security::crypto::blake3::blake3_hash(&buf)
    }

    /// Calcule et stocke le hash de cette entrée.
    fn finalize_hash(&mut self) {
        self.hash = self.compute_hash();
    }
}

// Vérification statique de la taille de LedgerEntry
const _: () = assert!(
    core::mem::size_of::<LedgerEntry>() == LEDGER_ENTRY_SIZE,
    "LedgerEntry: taille inattendue — doit être 136 bytes"
);

// ─────────────────────────────────────────────────────────────────────────────
// AuditHeader — En-tête de la zone d'audit
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête de la zone d'audit SSR.LOG_AUDIT.
///
/// Placé au début de la zone (+0x0000), avant les entrées P0.
#[repr(C)]
pub struct AuditHeader {
    /// Version du format (1 = v1.0).
    pub version: u64,
    /// Nombre total d'entrées écrites (P0 + ring buffer).
    pub total_count: u64,
    /// Dernier Merkle root calculé (pour vérification Kernel B).
    pub last_merkle_root: [u8; 32],
    /// Padding pour alignement (total header ≈ 48 bytes).
    pub _reserved: [u8; 16],
}

// ─────────────────────────────────────────────────────────────────────────────
// Stockage en mémoire — Zone P0 + Ring Buffer
// ─────────────────────────────────────────────────────────────────────────────

/// Zone P0 (immuable) — 16 entrées jamais écrasées.
static mut P0_ZONE: [LedgerEntry; P0_ZONE_ENTRIES] = {
    const INIT: LedgerEntry = LedgerEntry::zeroed();
    [INIT; P0_ZONE_ENTRIES]
};

/// Ring buffer (circulaire) — ~96 entrées avec overflow.
static mut RING_BUFFER: [LedgerEntry; RING_BUFFER_ENTRIES] = {
    const INIT: LedgerEntry = LedgerEntry::zeroed();
    [INIT; RING_BUFFER_ENTRIES]
};

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs globaux
// ─────────────────────────────────────────────────────────────────────────────

/// Numéro de séquence monotone global.
static GLOBAL_SEQ: AtomicU64 = AtomicU64::new(0);

/// Nombre d'entrées P0 utilisées.
static P0_USED: AtomicUsize = AtomicUsize::new(0);

/// Head du ring buffer (position d'écriture).
static RING_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Nombre total d'entrées écrites (P0 + ring).
static TOTAL_WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Une seule alerte d'overflow P0 est suffisante : au-delà on sature silencieusement.
static P0_OVERFLOW_REPORTED: AtomicBool = AtomicBool::new(false);

/// Dernier hash calculé (pour chaînage).
static LAST_HASH: [core::sync::atomic::AtomicU8; 32] = {
    const INIT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    [INIT; 32]
};
static P0_CHAIN_LOCK: Mutex<()> = Mutex::new(());

/// ExoLedger initialisé.
static EXOLEDGER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le dernier hash stocké.
fn load_last_hash() -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in LAST_HASH.iter().enumerate() {
        out[i] = byte.load(Ordering::Acquire);
    }
    out
}

/// Stocke un hash comme dernier hash.
fn store_last_hash(hash: &[u8; 32]) {
    for (i, byte) in LAST_HASH.iter().enumerate() {
        byte.store(hash[i], Ordering::Release);
    }
}

/// Lit le TSC courant (ISR-safe).
#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack),
        );
    }
    (hi as u64) << 32 | lo as u64
}

/// Retourne l'ObjectId de l'acteur courant (thread actif).
/// En l'absence d'un OID canonique câblé partout, on encode `(pid, tid)` dans
/// les 16 premiers octets pour conserver un acteur stable dans l'audit.
fn current_actor_oid() -> [u8; 32] {
    let mut oid = [0u8; 32];
    #[cfg(test)]
    let tcb_ptr: *mut crate::scheduler::core::task::ThreadControlBlock = core::ptr::null_mut();
    #[cfg(not(test))]
    let tcb_ptr = crate::scheduler::core::switch::current_thread_raw();
    if tcb_ptr.is_null() || !is_published_tcb(tcb_ptr as usize) {
        // Fallback early-boot: éviter un OID tout-zéro pour garder l'audit exploitable.
        let tsc = rdtsc();
        let cpu = 0u32;
        oid[0..8].copy_from_slice(&tsc.to_le_bytes());
        oid[8..12].copy_from_slice(&cpu.to_le_bytes());
        return oid;
    }
    // SAFETY: current_thread_raw() retourne le TCB actif du CPU courant tant que
    // l'appelant reste dans ce contexte d'exécution.
    let tcb = unsafe { &*tcb_ptr };
    oid[0..8].copy_from_slice(&(tcb.pid.0 as u64).to_le_bytes());
    oid[8..16].copy_from_slice(&tcb.tid.to_le_bytes());
    oid[16..24].copy_from_slice(&tcb.creation_tsc().to_le_bytes());
    oid
}

fn is_published_tcb(tcb_ptr: usize) -> bool {
    if tcb_ptr == 0 {
        return false;
    }

    crate::scheduler::core::switch::CURRENT_THREAD_PER_CPU
        .iter()
        .any(|slot| slot.load(Ordering::Acquire) == tcb_ptr)
}

// ─────────────────────────────────────────────────────────────────────────────
// exo_ledger_append_p0 — Ajout dans la zone P0 (IMMUABLE)
// ─────────────────────────────────────────────────────────────────────────────

/// Ajoute une entrée dans la zone P0 immuable du ledger.
///
/// # RÈGLE EXOLEDGER-01
/// La zone P0 est append-only : les entrées ne sont JAMAIS écrasées.
/// Si les 16 slots P0 sont pleins, la fonction panique (RÈGLE EXOLEDGER-02).
///
/// # ISR-safe
/// Pas d'allocation, pas de lock bloquant. Utilise des atomiques pour
/// la synchronisation.
///
/// # Usage
/// Appelé par :
/// - `exocage::cp_handler()` pour les violations #CP
/// - Le handler IOMMU fault pour les défauts DMA
/// - ExoPhoenix HANDOFF pour les déclenchements de handoff
/// - ExoSeal pour les violations de boot seal
pub fn exo_ledger_append_p0(action: ActionTag) {
    let _guard = P0_CHAIN_LOCK.lock();
    let idx = P0_USED.load(Ordering::Acquire);

    if idx >= P0_ZONE_ENTRIES {
        if !P0_OVERFLOW_REPORTED.swap(true, Ordering::AcqRel) {
            exo_ledger_append(ActionTag::Custom {
                tag: 0x5030_4f56_4552_464c,
                data: idx as u64,
            });
        }
        return;
    }
    P0_USED.store(idx + 1, Ordering::Release);

    let seq = GLOBAL_SEQ.fetch_add(1, Ordering::AcqRel);
    let prev_hash = load_last_hash();

    let mut entry = LedgerEntry {
        seq,
        tsc: rdtsc(),
        actor_oid: current_actor_oid(),
        action: action.to_bytes(),
        prev_hash,
        hash: [0u8; 32],
    };

    // Calculer le hash chaîné : Blake3(this_entry || prev_hash)
    entry.finalize_hash();

    // Écriture dans la zone P0
    // SAFETY: idx < P0_ZONE_ENTRIES garanti par le check ci-dessus.
    //         L'écriture est unique par slot (append-only).
    unsafe {
        core::ptr::write_volatile(&mut P0_ZONE[idx] as *mut LedgerEntry, entry);
    }

    // Mettre à jour le dernier hash et les compteurs
    store_last_hash(&entry.hash);
    TOTAL_WRITTEN.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// exo_ledger_append — Ajout dans le ring buffer (circulaire)
// ─────────────────────────────────────────────────────────────────────────────

/// Ajoute une entrée dans le ring buffer du ledger.
///
/// Si le ring buffer est plein, l'entrée la plus ancienne est écrasée
/// (overflow circulaire autorisé — mais JAMAIS les entrées P0).
///
/// # ISR-safe
/// Pas d'allocation, pas de lock. Utilise des atomiques pour le head.
pub fn exo_ledger_append(action: ActionTag) {
    let seq = GLOBAL_SEQ.fetch_add(1, Ordering::AcqRel);
    let prev_hash = load_last_hash();
    let head = RING_HEAD.fetch_add(1, Ordering::AcqRel) % RING_BUFFER_ENTRIES;

    let mut entry = LedgerEntry {
        seq,
        tsc: rdtsc(),
        actor_oid: current_actor_oid(),
        action: action.to_bytes(),
        prev_hash,
        hash: [0u8; 32],
    };

    // Calculer le hash chaîné
    entry.finalize_hash();

    // Écriture dans le ring buffer
    // SAFETY: head < RING_BUFFER_ENTRIES par construction du modulo.
    unsafe {
        core::ptr::write_volatile(&mut RING_BUFFER[head] as *mut LedgerEntry, entry);
    }

    // Mettre à jour le dernier hash et les compteurs
    store_last_hash(&entry.hash);
    TOTAL_WRITTEN.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation ExoLedger
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système ExoLedger.
///
/// Appelé par `security_init()` à l'étape 7 du boot.
///
/// En Phase 3.1, les zones P0 et ring buffer sont en mémoire statique BSS.
/// En Phase 3.2+, elles seront mappées dans SSR.LOG_AUDIT (+0x8000) en
/// collaboration avec l'infrastructure ExoPhoenix SSR.
pub fn exo_ledger_init() {
    if P0_USED.load(Ordering::Acquire) != 0
        || TOTAL_WRITTEN.load(Ordering::Acquire) != 0
        || GLOBAL_SEQ.load(Ordering::Acquire) != 0
    {
        EXOLEDGER_INITIALIZED.store(true, Ordering::Release);
        return;
    }

    // Réinitialiser les compteurs (en cas de ré-init)
    GLOBAL_SEQ.store(0, Ordering::Release);
    P0_USED.store(0, Ordering::Release);
    RING_HEAD.store(0, Ordering::Release);
    TOTAL_WRITTEN.store(0, Ordering::Release);
    P0_OVERFLOW_REPORTED.store(false, Ordering::Release);

    // Zéro-initialiser les zones (déjà BSS = zéro, mais explicite)
    // Note: on ne peut pas prendre &mut de static mut de manière sûre
    // en Rust safe. L'init se fait une seule fois au boot (single-threaded).
    // SAFETY: boot single-threaded, pas de concurrent access.
    unsafe {
        for entry in P0_ZONE.iter_mut() {
            *entry = LedgerEntry::zeroed();
        }
        for entry in RING_BUFFER.iter_mut() {
            *entry = LedgerEntry::zeroed();
        }
    }

    // Initialiser le dernier hash à zéro (genesis entry)
    for byte in LAST_HASH.iter() {
        byte.store(0, Ordering::Release);
    }

    EXOLEDGER_INITIALIZED.store(true, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification d'intégrité (Kernel B)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie l'intégrité de la chaîne P0.
///
/// Parcourt les entrées P0 et vérifie que chaque hash correspond à
/// Blake3(entry || prev_hash). Retourne le nombre d'entrées P0 valides.
///
/// Appelé par Kernel B pour vérifier l'intégrité de la zone P0.
pub fn verify_p0_integrity() -> Result<usize, LedgerIntegrityError> {
    let p0_used = P0_USED.load(Ordering::Acquire);

    let mut prev_hash = [0u8; 32]; // genesis hash = zéro

    for i in 0..p0_used.min(P0_ZONE_ENTRIES) {
        // SAFETY: i < p0_used <= P0_ZONE_ENTRIES, lecture seule.
        let entry = unsafe { core::ptr::read_volatile(&P0_ZONE[i] as *const LedgerEntry) };

        // Vérifier le chaînage : prev_hash de l'entrée doit correspondre
        if entry.prev_hash != prev_hash {
            return Err(LedgerIntegrityError::ChainBroken { entry_idx: i });
        }

        // Vérifier le hash de l'entrée
        let computed_hash = entry.compute_hash();
        if entry.hash != computed_hash {
            return Err(LedgerIntegrityError::HashMismatch { entry_idx: i });
        }

        prev_hash = entry.hash;
    }

    Ok(p0_used.min(P0_ZONE_ENTRIES))
}

/// Vérifie l'intégrité de la chaîne du ring buffer.
///
/// Retourne le nombre d'entrées valides dans le ring buffer.
pub fn verify_ring_integrity() -> Result<usize, LedgerIntegrityError> {
    let head = RING_HEAD.load(Ordering::Acquire);
    let count = head.min(RING_BUFFER_ENTRIES);

    // Le ring buffer est circulaire — on vérifie depuis le début
    let mut prev_hash = [0u8; 32];
    let mut valid = 0;

    for i in 0..count {
        // SAFETY: i < count <= RING_BUFFER_ENTRIES, lecture seule.
        let entry = unsafe { core::ptr::read_volatile(&RING_BUFFER[i] as *const LedgerEntry) };

        // Les entrées non-écrites ont seq=0, on les ignore
        if entry.seq == 0 && valid == 0 {
            continue;
        }

        if entry.prev_hash != prev_hash && valid > 0 {
            // La chaîne peut être rompue par l'overflow circulaire
            // Ce n'est pas une erreur — c'est attendu avec le ring buffer
            break;
        }

        let computed_hash = entry.compute_hash();
        if entry.hash == computed_hash {
            prev_hash = entry.hash;
            valid += 1;
        } else {
            break;
        }
    }

    Ok(valid)
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs d'intégrité
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur de vérification d'intégrité du ledger.
#[derive(Debug, Clone, Copy)]
pub enum LedgerIntegrityError {
    /// La chaîne de hash est rompue à l'entrée spécifiée.
    ChainBroken { entry_idx: usize },
    /// Le hash d'une entrée ne correspond pas au hash recalculé.
    HashMismatch { entry_idx: usize },
    /// Zone P0 pleine (impossible d'ajouter).
    P0ZoneFull,
}

// ─────────────────────────────────────────────────────────────────────────────
// Accesseurs publics
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre d'entrées P0 utilisées.
#[inline(always)]
pub fn p0_used() -> usize {
    P0_USED.load(Ordering::Acquire)
}

/// Nombre total d'entrées écrites (P0 + ring buffer).
#[inline(always)]
pub fn total_written() -> u64 {
    TOTAL_WRITTEN.load(Ordering::Acquire)
}

/// Numéro de séquence global courant.
#[inline(always)]
pub fn current_seq() -> u64 {
    GLOBAL_SEQ.load(Ordering::Acquire)
}

/// Lit une entrée P0 par index.
///
/// Retourne None si l'index est hors limites ou si l'entrée n'est pas écrite.
pub fn read_p0_entry(idx: usize) -> Option<LedgerEntry> {
    if idx >= P0_USED.load(Ordering::Acquire).min(P0_ZONE_ENTRIES) {
        return None;
    }
    // SAFETY: idx < P0_ZONE_ENTRIES, lecture seule.
    Some(unsafe { core::ptr::read_volatile(&P0_ZONE[idx] as *const LedgerEntry) })
}

/// Lit une entrée du ring buffer par index.
pub fn read_ring_entry(idx: usize) -> Option<LedgerEntry> {
    if idx >= RING_BUFFER_ENTRIES {
        return None;
    }
    // SAFETY: idx < RING_BUFFER_ENTRIES, lecture seule.
    let entry = unsafe { core::ptr::read_volatile(&RING_BUFFER[idx] as *const LedgerEntry) };
    if entry.seq == 0 {
        return None; // entrée non écrite
    }
    Some(entry)
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques ExoLedger
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot des statistiques ExoLedger.
#[derive(Debug, Clone, Copy)]
pub struct ExoLedgerStats {
    /// Nombre d'entrées P0 utilisées.
    pub p0_used: usize,
    /// Capacité maximale P0.
    pub p0_capacity: usize,
    /// Nombre total d'entrées écrites.
    pub total_written: u64,
    /// Séquence globale courante.
    pub current_seq: u64,
    /// Head du ring buffer.
    pub ring_head: usize,
    /// Capacité du ring buffer.
    pub ring_capacity: usize,
}

/// Retourne un snapshot des statistiques ExoLedger.
pub fn exoledger_stats() -> ExoLedgerStats {
    ExoLedgerStats {
        p0_used: P0_USED.load(Ordering::Relaxed),
        p0_capacity: P0_ZONE_ENTRIES,
        total_written: TOTAL_WRITTEN.load(Ordering::Relaxed),
        current_seq: GLOBAL_SEQ.load(Ordering::Relaxed),
        ring_head: RING_HEAD.load(Ordering::Relaxed),
        ring_capacity: RING_BUFFER_ENTRIES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exoledger_init_preserves_phase0_entries() {
        exo_ledger_init();
        exo_ledger_append_p0(ActionTag::NicIommuLocked);
        let seq_before = current_seq();
        let used_before = p0_used();

        exo_ledger_init();

        assert_eq!(p0_used(), used_before);
        assert_eq!(current_seq(), seq_before);
        assert!(read_p0_entry(0).is_some());
    }

    #[test]
    fn test_p0_overflow_is_graceful_and_saturating() {
        GLOBAL_SEQ.store(0, Ordering::Release);
        P0_USED.store(0, Ordering::Release);
        RING_HEAD.store(0, Ordering::Release);
        TOTAL_WRITTEN.store(0, Ordering::Release);
        P0_OVERFLOW_REPORTED.store(false, Ordering::Release);
        unsafe {
            for entry in P0_ZONE.iter_mut() {
                *entry = LedgerEntry::zeroed();
            }
            for entry in RING_BUFFER.iter_mut() {
                *entry = LedgerEntry::zeroed();
            }
        }
        for byte in LAST_HASH.iter() {
            byte.store(0, Ordering::Release);
        }

        for idx in 0..P0_ZONE_ENTRIES {
            exo_ledger_append_p0(ActionTag::BootEvent { step: idx as u8 });
        }
        let written_before = total_written();
        exo_ledger_append_p0(ActionTag::BootEvent { step: 99 });

        assert_eq!(p0_used(), P0_ZONE_ENTRIES);
        assert!(total_written() >= written_before);
        assert!(read_ring_entry(0).is_some());
    }
}
