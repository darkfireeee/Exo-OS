// SPDX-License-Identifier: MIT
// ExoFS — object_kind/code.rs
// CodeDescriptor — objets exécutables (ELF vérifiés par BlobId + signature).
//
// Règles :
//   HASH-01  : BlobId = Blake3(données brutes)
//   SEC-04   : contenu jamais loggué
//   ONDISK-01: CodeDescriptorDisk #[repr(C, packed)]
//   ARITH-02 : checked_add / saturating_* partout

#![allow(dead_code)]

use core::fmt;
use core::mem;

use crate::fs::exofs::core::{
    BlobId, ObjectId, EpochId, DiskOffset,
    ExofsError, ExofsResult, blake3_hash, compute_blob_id,
};

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Magic ELF (\x7fELF).
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// Magic d'un CodeDescriptorDisk.
pub const CODE_DESCRIPTOR_MAGIC: u32 = 0xC0DE_0100;

/// Version du format CodeDescriptorDisk.
pub const CODE_DESCRIPTOR_VERSION: u8 = 1;

/// Taille maximale d'un exécutable (256 Mio).
pub const CODE_MAX_SIZE: u64 = 256 * 1024 * 1024;

/// Longueur de la signature de code (Ed25519 = 64 octets, slot max).
pub const CODE_SIGNATURE_LEN: usize = 64;

/// Longueur de la clé publique stockée.
pub const CODE_PUBKEY_LEN: usize = 32;

// ── Flags Code ─────────────────────────────────────────────────────────────────

pub const CODE_FLAG_ELF_VERIFIED:    u16 = 1 << 0; // Headers ELF vérifiés
pub const CODE_FLAG_SIGNATURE_VALID: u16 = 1 << 1; // Signature Ed25519 vérifiée
pub const CODE_FLAG_PRIVILEGED:      u16 = 1 << 2; // Peut s'exécuter en Ring 0
pub const CODE_FLAG_STRIPPED:        u16 = 1 << 3; // Sections debug retirées
pub const CODE_FLAG_PIE:             u16 = 1 << 4; // Position-Independent Exec
pub const CODE_FLAG_SEALED:          u16 = 1 << 5; // Immuable, jamais recompilé
pub const CODE_FLAG_TRUSTED:         u16 = 1 << 6; // Approuvé par la Trusted CA

// ── ELF class & machine ────────────────────────────────────────────────────────

/// Classe ELF (EI_CLASS).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ElfClass {
    Class32 = 1,
    Class64 = 2,
    Unknown = 0xFF,
}

impl ElfClass {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Class32,
            2 => Self::Class64,
            _ => Self::Unknown,
        }
    }
}

/// Architecture machine ELF (e_machine, 16 bits).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ElfMachine {
    X86    = 0x0003,
    X86_64 = 0x003E,
    Arm    = 0x0028,
    Arm64  = 0x00B7,
    RiscV  = 0x00F3,
    Unknown = 0xFFFF,
}

impl ElfMachine {
    pub fn from_u16(v: u16) -> Self {
        match v {
            0x0003 => Self::X86,
            0x003E => Self::X86_64,
            0x0028 => Self::Arm,
            0x00B7 => Self::Arm64,
            0x00F3 => Self::RiscV,
            _      => Self::Unknown,
        }
    }
}

// ── CodeDescriptorDisk ─────────────────────────────────────────────────────────

/// Représentation on-disk d'un descripteur de code exécutable (160 octets).
///
/// Layout :
/// ```text
///   0..  3  magic        u32
///   4.. 35  blob_id      [u8;32]
///  36.. 67  object_id    [u8;32]
///  68.. 75  disk_offset  u64
///  76.. 83  size         u64
///  84.. 91  epoch_create u64
///  92.. 93  flags        u16
///  94       elf_class    u8       (EI_CLASS)
///  95       version      u8
///  96.. 97  elf_machine  u16
///  98.. 99  _pad0        [u8;2]
/// 100..131  signature    [u8;32]  (Blake3 de la clé publique Ed25519)
/// 132..163  pubkey_hash  [u8;32]  (Blake3 de la clé publique)
/// 164..175  _pad1        [u8;12]
/// 176..191  checksum     [u8;16]  (Blake3 des 176 premiers octets, tronqué)
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct CodeDescriptorDisk {
    pub magic:       u32,
    pub blob_id:     [u8; 32],
    pub object_id:   [u8; 32],
    pub disk_offset: u64,
    pub size:        u64,
    pub epoch_create:u64,
    pub flags:       u16,
    pub elf_class:   u8,
    pub version:     u8,
    pub elf_machine: u16,
    pub _pad0:       [u8; 2],
    pub signature:   [u8; 32], // hash de la signature Ed25519
    pub pubkey_hash: [u8; 32],
    pub _pad1:       [u8; 12],
    pub checksum:    [u8; 16],
}

const _: () = assert!(
    mem::size_of::<CodeDescriptorDisk>() == 192,
    "CodeDescriptorDisk doit être 192 octets (ONDISK-01)"
);

impl CodeDescriptorDisk {
    pub fn compute_checksum(&self) -> [u8; 16] {
        let raw: &[u8; 192] =
            unsafe { &*(self as *const CodeDescriptorDisk as *const [u8; 192]) };
        let full = blake3_hash(&raw[..176]);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }

    pub fn verify(&self) -> ExofsResult<()> {
        if { self.magic } != CODE_DESCRIPTOR_MAGIC {
            return Err(ExofsError::Corrupt);
        }
        if { self.version } != CODE_DESCRIPTOR_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        let computed = self.compute_checksum();
        if { self.checksum } != computed {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }
}

impl fmt::Debug for CodeDescriptorDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CodeDescriptorDisk {{ size: {}, flags: {:#x}, elf_class: {} }}",
            { self.size }, { self.flags }, { self.elf_class },
        )
    }
}

// ── CodeValidationResult ───────────────────────────────────────────────────────

/// Résultat de la validation d'un objet Code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeValidationResult {
    /// Validation complète réussie.
    Valid,
    /// Magic ELF absent ou incorrect.
    BadElfMagic,
    /// Classe ELF non supportée.
    UnsupportedElfClass,
    /// Architecture machine non reconnue.
    UnknownMachine,
    /// Trop grand (> CODE_MAX_SIZE).
    TooLarge,
    /// BlobId ne correspond pas aux données.
    BlobIdMismatch,
    /// Signature invalide ou absente.
    SignatureInvalid,
    /// Objet marqué comme partiel (upload incomplet).
    Partial,
}

impl fmt::Display for CodeValidationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Valid                => write!(f, "valid"),
            Self::BadElfMagic         => write!(f, "bad ELF magic"),
            Self::UnsupportedElfClass => write!(f, "unsupported ELF class"),
            Self::UnknownMachine      => write!(f, "unknown ELF machine"),
            Self::TooLarge            => write!(f, "code too large"),
            Self::BlobIdMismatch      => write!(f, "blob_id mismatch"),
            Self::SignatureInvalid    => write!(f, "signature invalid"),
            Self::Partial             => write!(f, "partial upload"),
        }
    }
}

// ── CodeDescriptor in-memory ───────────────────────────────────────────────────

/// Descripteur in-memory d'un objet Code ExoFS.
pub struct CodeDescriptor {
    /// BlobId du code (Blake3 brut).
    pub blob_id:      BlobId,
    /// Objet propriétaire.
    pub object_id:    ObjectId,
    /// Offset disque du payload.
    pub disk_offset:  DiskOffset,
    /// Taille en octets.
    pub size:         u64,
    /// Epoch de création.
    pub epoch_create: EpochId,
    /// Flags (CODE_FLAG_*).
    pub flags:        u16,
    /// Classe ELF.
    pub elf_class:    ElfClass,
    /// Architecture machine.
    pub elf_machine:  ElfMachine,
    /// Hash de la clé publique de signature.
    pub pubkey_hash:  [u8; 32],
    /// Hash de la signature Ed25519.
    pub signature:    [u8; 32],
}

impl CodeDescriptor {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    /// Crée un nouveau CodeDescriptor après validation ELF de base.
    ///
    /// Ne fait PAS la vérification de signature (doit être faite séparément).
    pub fn new(
        data:        &[u8],
        object_id:   ObjectId,
        disk_offset: DiskOffset,
        epoch:       EpochId,
    ) -> ExofsResult<Self> {
        if data.len() as u64 > CODE_MAX_SIZE {
            return Err(ExofsError::Overflow);
        }
        // Validation ELF minimale.
        let vr = validate_elf_header(data);
        if vr != CodeValidationResult::Valid {
            return Err(ExofsError::InvalidArgument);
        }
        // HASH-01 : BlobId sur données brutes.
        let blob_id = compute_blob_id(data);
        let elf_class   = if data.len() > 4 { ElfClass::from_u8(data[4]) } else { ElfClass::Unknown };
        let elf_machine = if data.len() > 19 {
            let m = u16::from_le_bytes([data[18], data[19]]);
            ElfMachine::from_u16(m)
        } else {
            ElfMachine::Unknown
        };
        Ok(Self {
            blob_id,
            object_id,
            disk_offset,
            size:         data.len() as u64,
            epoch_create: epoch,
            flags:        CODE_FLAG_ELF_VERIFIED,
            elf_class,
            elf_machine,
            pubkey_hash:  [0u8; 32],
            signature:    [0u8; 32],
        })
    }

    /// Reconstruit depuis on-disk (HDR-03 : verify() en premier).
    pub fn from_disk(d: &CodeDescriptorDisk) -> ExofsResult<Self> {
        d.verify()?;
        Ok(Self {
            blob_id:      BlobId(d.blob_id),
            object_id:    ObjectId(d.object_id),
            disk_offset:  DiskOffset(d.disk_offset),
            size:         d.size,
            epoch_create: EpochId(d.epoch_create),
            flags:        d.flags,
            elf_class:    ElfClass::from_u8(d.elf_class),
            elf_machine:  ElfMachine::from_u16(d.elf_machine),
            pubkey_hash:  d.pubkey_hash,
            signature:    d.signature,
        })
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk(&self) -> CodeDescriptorDisk {
        let mut d = CodeDescriptorDisk {
            magic:        CODE_DESCRIPTOR_MAGIC,
            blob_id:      self.blob_id.0,
            object_id:    self.object_id.0,
            disk_offset:  self.disk_offset.0,
            size:         self.size,
            epoch_create: self.epoch_create.0,
            flags:        self.flags,
            elf_class:    self.elf_class as u8,
            version:      CODE_DESCRIPTOR_VERSION,
            elf_machine:  self.elf_machine as u16,
            _pad0:        [0; 2],
            signature:    self.signature,
            pubkey_hash:  self.pubkey_hash,
            _pad1:        [0; 12],
            checksum:     [0; 16],
        };
        d.checksum = d.compute_checksum();
        d
    }

    // ── Vérification ──────────────────────────────────────────────────────────

    /// Vérifie que `data` correspond au BlobId et valide les headers ELF.
    ///
    /// SEC-04 : les données ne sont pas loguées en cas d'erreur.
    pub fn verify_content(&self, data: &[u8]) -> ExofsResult<CodeValidationResult> {
        if data.len() as u64 > CODE_MAX_SIZE {
            return Ok(CodeValidationResult::TooLarge);
        }
        let computed = compute_blob_id(data);
        if computed != self.blob_id {
            return Ok(CodeValidationResult::BlobIdMismatch);
        }
        let vr = validate_elf_header(data);
        if vr != CodeValidationResult::Valid {
            return Ok(vr);
        }
        Ok(CodeValidationResult::Valid)
    }

    /// Enregistre un hash de clé publique et le hash de signature Ed25519.
    ///
    /// La vérification Ed25519 est effectuée AVANT cet appel par le
    /// scheduler/security, ici on stocke seulement les hashes.
    pub fn record_signature(
        &mut self,
        pubkey_hash: [u8; 32],
        sig_hash:    [u8; 32],
    ) {
        self.pubkey_hash = pubkey_hash;
        self.signature   = sig_hash;
        self.flags      |= CODE_FLAG_SIGNATURE_VALID;
    }

    /// Marque ce code comme approuvé (CODE_FLAG_TRUSTED).
    pub fn mark_trusted(&mut self) {
        self.flags |= CODE_FLAG_TRUSTED;
    }

    /// Vrai si la signature est marquée valide.
    #[inline]
    pub fn has_valid_signature(&self) -> bool {
        self.flags & CODE_FLAG_SIGNATURE_VALID != 0
    }

    /// Vrai si ce code peut s'exécuter en Ring 0 (noyau).
    #[inline]
    pub fn is_privileged(&self) -> bool {
        self.flags & CODE_FLAG_PRIVILEGED != 0
    }

    // ── Validation ────────────────────────────────────────────────────────────

    pub fn validate(&self) -> ExofsResult<()> {
        if self.size == 0 || self.size > CODE_MAX_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        if matches!(self.elf_class, ElfClass::Unknown) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

impl fmt::Display for CodeDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CodeDescriptor {{ size: {}, machine: {:?}, class: {:?}, \
             flags: {:#x}, trusted: {} }}",
            self.size, self.elf_machine, self.elf_class,
            self.flags,
            self.flags & CODE_FLAG_TRUSTED != 0,
        )
    }
}

impl fmt::Debug for CodeDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── Fonctions utilitaires ELF ──────────────────────────────────────────────────

/// Valide les headers ELF minimaux d'un buffer.
///
/// Retourne `CodeValidationResult::Valid` si OK, sinon la raison du rejet.
pub fn validate_elf_header(data: &[u8]) -> CodeValidationResult {
    if data.len() < 64 {
        return CodeValidationResult::BadElfMagic;
    }
    if data[0..4] != ELF_MAGIC {
        return CodeValidationResult::BadElfMagic;
    }
    match ElfClass::from_u8(data[4]) {
        ElfClass::Unknown => return CodeValidationResult::UnsupportedElfClass,
        _ => {}
    }
    if (data.len() as u64) > CODE_MAX_SIZE {
        return CodeValidationResult::TooLarge;
    }
    CodeValidationResult::Valid
}

/// Vérifie que `data` est un ELF valide et correspond au `blob_id`.
///
/// Utilisé par l'object_loader avant mapping mémoire.
/// SEC-04 : données non loguées.
pub fn code_is_valid(data: &[u8], blob_id: &BlobId) -> bool {
    if validate_elf_header(data) != CodeValidationResult::Valid {
        return false;
    }
    let computed = compute_blob_id(data);
    &computed == blob_id
}

// ── CodeStats ──────────────────────────────────────────────────────────────────

/// Statistiques agrégées des objets Code.
#[derive(Default, Debug)]
pub struct CodeStats {
    pub total:          u64,
    pub privileged:     u64,
    pub trusted:        u64,
    pub signed:         u64,
    pub pie_count:      u64,
    pub x86_64_count:   u64,
    pub arm64_count:    u64,
    pub total_bytes:    u64,
}

impl CodeStats {
    pub fn new() -> Self { Self::default() }

    pub fn record(&mut self, c: &CodeDescriptor) {
        self.total          = self.total.saturating_add(1);
        self.total_bytes    = self.total_bytes.saturating_add(c.size);
        if c.is_privileged()         { self.privileged  = self.privileged.saturating_add(1); }
        if c.flags & CODE_FLAG_TRUSTED != 0  { self.trusted    = self.trusted.saturating_add(1); }
        if c.has_valid_signature()   { self.signed      = self.signed.saturating_add(1); }
        if c.flags & CODE_FLAG_PIE != 0      { self.pie_count  = self.pie_count.saturating_add(1); }
        if matches!(c.elf_machine, ElfMachine::X86_64) {
            self.x86_64_count = self.x86_64_count.saturating_add(1);
        }
        if matches!(c.elf_machine, ElfMachine::Arm64) {
            self.arm64_count = self.arm64_count.saturating_add(1);
        }
    }
}

impl fmt::Display for CodeStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CodeStats {{ total: {}, privileged: {}, trusted: {}, \
             signed: {}, PIE: {}, bytes: {} }}",
            self.total, self.privileged, self.trusted,
            self.signed, self.pie_count, self.total_bytes,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fake_elf() -> alloc::vec::Vec<u8> {
        let mut elf = alloc::vec![0u8; 64];
        elf[0..4].copy_from_slice(&ELF_MAGIC);
        elf[4] = 2; // ELF64
        elf[18] = 0x3E; elf[19] = 0x00; // x86_64
        elf
    }

    #[test]
    fn test_validate_elf_too_short() {
        assert_ne!(validate_elf_header(b"ELF"), CodeValidationResult::Valid);
    }

    #[test]
    fn test_validate_elf_bad_magic() {
        let data = [0u8; 64];
        assert_eq!(validate_elf_header(&data), CodeValidationResult::BadElfMagic);
    }

    #[test]
    fn test_validate_elf_ok() {
        let elf = make_fake_elf();
        assert_eq!(validate_elf_header(&elf), CodeValidationResult::Valid);
    }

    #[test]
    fn test_code_descriptor_disk_size() {
        assert_eq!(mem::size_of::<CodeDescriptorDisk>(), 192);
    }

    #[test]
    fn test_code_is_valid_tampered() {
        let elf  = make_fake_elf();
        let id   = compute_blob_id(&elf);
        let mut tampered = elf.clone();
        tampered[63] ^= 0xFF;
        assert!(!code_is_valid(&tampered, &id));
    }
}
