// kernel/src/security/capability/token.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAP TOKEN — Jeton de capacité inforgeable (Exo-OS Security · Couche 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  PÉRIMÈTRE DE PREUVE FORMELLE — Toute modification ici IMPOSE une mise à
//     jour des preuves Coq/TLA+ dans /proofs/kernel_security/.
//
// PROPRIÉTÉ FONDAMENTALE :
//   Un CapToken est inforgeable : il ne peut être créé qu'via CapTable::grant()
//   qui initialise l'objet { object_id, rights, generation } atomiquement.
//   La vérification compare systématiquement object_id + rights + generation
//   contre la table → un token révoqué retourne Err(Revoked) instantanément.
//
// LAYOUT MÉMOIRE (24 bytes, repr(C), Rights=u32) :
//   Offset  0 :  object_id   u64   — identifiant unique de l'objet protégé
//   Offset  8 :  rights      u32   — bitmask des droits accordés (Rights)
//   Offset 12 :  generation  u32   — compteur de révocation (copiée depuis table)
//   Offset 16 :  type_tag    u16   — type d'objet (CapObjectType)
//   Offset 18 :  _pad        u16   — alignement ABI explicite
//   Total      : 24 bytes (repr(C) : 20 data + 4 padding, u64 align) — vérifiée statiquement
//
// RÈGLE CAP-01 : security/capability/ est UNIQUE source de vérité dans tout l'OS.
// RÈGLE CAP-02 : Un CapToken Copy — jamais de référence mutable partagée.
// RÈGLE CAP-03 : La génération est toujours copiée au moment de grant(),
//                jamais recalculée après. La vérification compare.
// ═══════════════════════════════════════════════════════════════════════════════


use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::rights::Rights;

// ─────────────────────────────────────────────────────────────────────────────
// Compteur global de tokens créés — instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre total de tokens émis depuis le démarrage.
static TOKENS_ISSUED: AtomicU64 = AtomicU64::new(0);

/// Nombre de tentatives de vérification totales.
static TOKENS_VERIFIED: AtomicU64 = AtomicU64::new(0);

/// Nombre de vérifications ayant échoué (toutes causes confondues).
static TOKENS_DENIED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// ObjectId — identifiant d'objet système
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un objet du système (endpoint IPC, VMA, fichier, device…).
/// Généré par CapTable — jamais construit directement hors de ce module.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ObjectId(pub(super) u64);

impl ObjectId {
    /// Valeur invalide — jamais émise par CapTable.
    pub const INVALID: Self = Self(u64::MAX);

    /// Crée un ObjectId depuis une valeur brute — réservé aux tests et au boot.
    #[inline(always)]
    pub const fn from_raw(v: u64) -> Self {
        Self(v)
    }

    #[inline(always)]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    #[inline(always)]
    pub fn is_valid(self) -> bool {
        self.0 != u64::MAX
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId(0x{:016x})", self.0)
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "oid:{:x}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CapObjectType — type sémantique de l'objet référencé
// ─────────────────────────────────────────────────────────────────────────────

/// Catégorisation de l'objet référencé par un CapToken.
/// Permet des politiques de vérification type-spécifiques.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u16)]
pub enum CapObjectType {
    /// Non initialisé — invalide.
    Invalid      = 0,
    /// Endpoint IPC (canal de communication).
    IpcEndpoint  = 1,
    /// Région mémoire virtuelle (VMA).
    MemoryRegion = 2,
    /// Inode de fichier.
    FileInode    = 3,
    /// Périphérique (driver Ring 1).
    Device       = 4,
    /// Thread (pour délégation de signaux, kill, etc.).
    Thread       = 5,
    /// Processus entier.
    Process      = 6,
    /// Namespace (PID, net, mount…).
    Namespace    = 7,
    /// Backend DMA.
    DmaChannel   = 8,
    /// Domaine IOMMU.
    IommuDomain  = 9,
    /// Crypto key slot.
    CryptoKey    = 10,
    /// Capability elle-même (délégation de caps).
    Capability   = 11,
}

impl CapObjectType {
    /// Reconvertit depuis u16 — retourne Invalid si inconnu.
    #[inline(always)]
    pub fn from_u16(v: u16) -> Self {
        match v {
            1  => Self::IpcEndpoint,
            2  => Self::MemoryRegion,
            3  => Self::FileInode,
            4  => Self::Device,
            5  => Self::Thread,
            6  => Self::Process,
            7  => Self::Namespace,
            8  => Self::DmaChannel,
            9  => Self::IommuDomain,
            10 => Self::CryptoKey,
            11 => Self::Capability,
            _  => Self::Invalid,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CapToken — jeton principal (PÉRIMÈTRE DE PREUVE FORMELLE)
// ─────────────────────────────────────────────────────────────────────────────

/// Jeton de capacité inforgeable — 128 bits logiques, layout 20 bytes ABI.
///
/// # Invariants (prouvés Coq)
/// * `I1` : ∀ t créé par `CapTable::grant`, `t.generation == table.entry[t.object_id].generation`
/// * `I2` : ∀ t révoqué, `table.entry[t.object_id].generation != t.generation`
/// * `I3` : Les droits d'un token ne peuvent jamais excéder les droits de la source de délégation.
///
/// # Sécurité
/// Le token est `Copy` car il est passé par valeur dans les syscalls (registres).
/// Il NE doit jamais être mutable depuis l'espace utilisateur.
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct CapToken {
    /// Identifiant de l'objet protégé.
    pub(super) object_id:  ObjectId,
    /// Droits accordés — sous-ensemble des droits de la source de délégation.
    pub(super) rights:     Rights,
    /// Copie de la génération au moment de l'émission — utilisée pour détecter révocation.
    pub(super) generation: u32,
    /// Type sémantique — accélère la vérification sans consulter la table.
    pub(super) type_tag:   CapObjectType,
    /// Padding explicite — garantit zéro byte indéfini (size_of == 24 par alignement u64).
    pub(super) _pad:       u16,
}

// Vérification statique de la taille — alignment ABI Rust pour #[repr(C)]:
// u64(8) + u32(4) + u32(4) + u16(2) + u16(2) = 20 bytes data
// mais avec repr(C) et align=8, la taille est arrondie à 24 bytes.
const _SIZE_CHECK: () = assert!(
    core::mem::size_of::<CapToken>() == 24,
    "CapToken: taille inattendue — aligner avec PROOF_SCOPE.md"
);

const _ALIGN_CHECK: () = assert!(
    core::mem::align_of::<CapToken>() == 8,
    "CapToken: alignement inattendu"
);

impl CapToken {
    /// Token invalide — ne passe jamais la vérification.
    pub const INVALID: Self = Self {
        object_id:  ObjectId::INVALID,
        rights:     Rights::NONE,
        generation: u32::MAX,
        type_tag:   CapObjectType::Invalid,
        _pad:       0,
    };

    /// Construit un token — RÉSERVÉ à `CapTable::grant()`.
    /// Appelé uniquement depuis ce module (visibility `pub(super)`).
    #[inline(always)]
    pub(crate) fn new(
        object_id:  ObjectId,
        rights:     Rights,
        generation: u32,
        type_tag:   CapObjectType,
    ) -> Self {
        TOKENS_ISSUED.fetch_add(1, Ordering::Relaxed);
        Self {
            object_id,
            rights,
            generation,
            type_tag,
            _pad: 0,
        }
    }

    /// Retourne l'ObjectId — lecture seule.
    #[inline(always)]
    pub fn object_id(self) -> ObjectId {
        self.object_id
    }

    /// Retourne les droits accordés.
    #[inline(always)]
    pub fn rights(self) -> Rights {
        self.rights
    }

    /// Retourne la génération capturée au moment de l'émission.
    #[inline(always)]
    pub fn generation(self) -> u32 {
        self.generation
    }

    /// Retourne le type d'objet.
    #[inline(always)]
    pub fn object_type(self) -> CapObjectType {
        self.type_tag
    }

    /// Vrai si le token possède au moins les droits demandés.
    #[inline(always)]
    pub fn has_rights(self, required: Rights) -> bool {
        self.rights.contains(required)
    }

    /// Vrai si le token est manifestement invalide (ObjectId::INVALID).
    #[inline(always)]
    pub fn is_invalid(self) -> bool {
        self.object_id == ObjectId::INVALID
    }

    /// Convertit en tableau de 20 bytes (pour stockage en espace utilisateur via copy_to_user).
    #[inline(always)]
    pub fn to_bytes(self) -> [u8; 20] {
        let mut buf = [0u8; 20];
        buf[0..8].copy_from_slice(&self.object_id.0.to_ne_bytes());
        buf[8..12].copy_from_slice(&self.rights.bits().to_ne_bytes());
        buf[12..16].copy_from_slice(&self.generation.to_ne_bytes());
        buf[16..18].copy_from_slice(&(self.type_tag as u16).to_ne_bytes());
        // _pad reste à zéro
        buf
    }

    /// Reconstruit depuis des bytes (utilisé par le syscall handler côté kernel).
    /// Retourne None si le type_tag est inconnu.
    #[inline(always)]
    pub fn from_bytes(b: &[u8; 20]) -> Option<Self> {
        let oid = u64::from_ne_bytes(b[0..8].try_into().ok()?);
        let r   = u32::from_ne_bytes(b[8..12].try_into().ok()?);
        let gen = u32::from_ne_bytes(b[12..16].try_into().ok()?);
        let tt  = u16::from_ne_bytes(b[16..18].try_into().ok()?);
        let obj_type = CapObjectType::from_u16(tt);
        Some(Self {
            object_id:  ObjectId(oid),
            rights:     Rights::from_bits_truncate(r),
            generation: gen,
            type_tag:   obj_type,
            _pad:       0,
        })
    }
}

impl fmt::Debug for CapToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapToken")
            .field("object_id",  &self.object_id)
            .field("rights",     &self.rights)
            .field("generation", &self.generation)
            .field("type_tag",   &self.type_tag)
            .finish()
    }
}

impl Default for CapToken {
    fn default() -> Self {
        Self::INVALID
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales — instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot de statistiques de tokens.
#[derive(Debug, Clone, Copy)]
pub struct TokenStats {
    pub issued:   u64,
    pub verified: u64,
    pub denied:   u64,
}

/// Incrémente le compteur de vérification — appelé depuis revocation::verify().
#[inline(always)]
pub(super) fn stat_verified() {
    TOKENS_VERIFIED.fetch_add(1, Ordering::Relaxed);
}

/// Incrémente le compteur de refus — appelé depuis revocation::verify().
#[inline(always)]
pub(super) fn stat_denied() {
    TOKENS_DENIED.fetch_add(1, Ordering::Relaxed);
}

/// Lit un snapshot cohérent (chaque compteur lu indépendamment — acceptable pour perf monitoring).
pub fn read_stats() -> TokenStats {
    TokenStats {
        issued:   TOKENS_ISSUED.load(Ordering::Relaxed),
        verified: TOKENS_VERIFIED.load(Ordering::Relaxed),
        denied:   TOKENS_DENIED.load(Ordering::Relaxed),
    }
}
