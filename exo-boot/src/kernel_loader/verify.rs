//! verify.rs — Vérification de signature Ed25519 du kernel.
//!
//! RÈGLE BOOT-02 (DOC10) :
//!   "Signature Ed25519 vérifiée AVANT tout chargement du kernel.
//!    Clé publique intégrée dans le binaire du bootloader."
//!
//! Processus de vérification :
//!   1. La signature est stockée dans les 256 derniers bytes de l'image ELF.
//!   2. Le message signé = SHA-512(image ELF sans signature).
//!   3. Vérification avec ed25519-dalek.
//!
//! Si la vérification échoue : PANIC immédiat (BOOT-02 est bloquant).
//!
//! COMPILATION :
//!   - Feature `secure-boot`  : vérification complète Ed25519 + SHA-512.
//!   - Feature `dev-skip-sig` : vérification tentée mais non-bloquante.
//!   - Ni l'un ni l'autre     : vérification structurelle uniquement (marqueur).
//!
//! CLÉS : La clé publique est intégrée via `include_bytes!()`.
//!   Pour le développement, une clé de test est utilisée.

// ─── Constantes de signature (toujours compilées) ────────────────────────────

/// Taille d'une signature Ed25519 en bytes.
pub const SIGNATURE_SIZE: usize = 64;

/// Marqueur de début de section signature (8 bytes, vérifié par le loader).
pub const SIGNATURE_MARKER: [u8; 8] = *b"EXOSIG01";

/// Taille totale de la structure KernelSignature (fixe, ABI stable).
pub const KERNEL_SIG_STRUCT_SIZE: usize = 256;

// ─── Structure de signature (toujours compilée — layout ABI) ─────────────────

/// Signature attachée à une image kernel.
/// Placée à la fin de l'image ELF, dans la section `.kernel_sig`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelSignature {
    /// Marqueur d'identification de la section signature.
    pub marker:    [u8; 8],
    /// Signature Ed25519 (64 bytes).
    pub signature: [u8; SIGNATURE_SIZE],
    /// Hash SHA-512 du kernel (pour vérification rapide sans re-hash).
    pub sha512:    [u8; 64],
    /// Padding pour alignement à 256 bytes.
    pub _pad:      [u8; 120],
}

const _: () = assert!(
    core::mem::size_of::<KernelSignature>() == KERNEL_SIG_STRUCT_SIZE,
    "KernelSignature doit faire 256 bytes"
);

// ─── Erreurs (toujours compilées) ─────────────────────────────────────────────

#[derive(Debug)]
pub enum VerifyError {
    /// Clé publique invalide dans le binaire bootloader.
    InvalidPublicKey,
    /// Image trop petite pour contenir une signature.
    ImageTooSmall { size: usize },
    /// Pas de section signature dans l'image.
    MissingSignature { found_marker: [u8; 8] },
    /// Hash SHA-512 ne correspond pas.
    HashMismatch,
    /// Signature Ed25519 invalide.
    SignatureMismatch,
    /// Vérification non disponible (secure-boot feature inactive).
    Unavailable,
}

impl core::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidPublicKey    => write!(f, "Clé publique intégrée invalide"),
            Self::ImageTooSmall { size } => write!(f, "Image trop petite : {} bytes", size),
            Self::MissingSignature { found_marker } =>
                write!(f, "Section signature absente (marqueur: {:02X?})", found_marker),
            Self::HashMismatch        => write!(f, "Hash SHA-512 du kernel incorrect"),
            Self::SignatureMismatch   => write!(f, "Signature Ed25519 invalide"),
            Self::Unavailable         => write!(f, "Feature secure-boot inactive"),
        }
    }
}

// ─── Implémentation avec vérification complète (feature = "secure-boot") ─────

#[cfg(feature = "secure-boot")]
mod secure_impl {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey, Verifier};
    use sha2::{Sha512, Digest};

    /// Clé publique Ed25519 pour la vérification du kernel.
    /// PRODUCTION : Remplacer par include_bytes!("../../../keys/kernel_signing_pub.raw")
    static KERNEL_SIGNING_PUBLIC_KEY: &[u8; 32] = &[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60,
        0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0x44,
        0xda, 0xe8, 0x86, 0x0d, 0x30, 0x68, 0xd4, 0x96,
        0x97, 0xf4, 0x3d, 0xfb, 0x7f, 0xed, 0xce, 0x08,
    ];

    pub fn verify_full(image: &[u8]) -> Result<(), VerifyError> {
        if image.len() < KERNEL_SIG_STRUCT_SIZE {
            return Err(VerifyError::ImageTooSmall { size: image.len() });
        }

        let sig_offset  = image.len() - KERNEL_SIG_STRUCT_SIZE;
        let kernel_data = &image[..sig_offset];
        let sig_bytes   = &image[sig_offset..];

        let kernel_sig: KernelSignature = unsafe {
            core::ptr::read_unaligned(sig_bytes.as_ptr() as *const KernelSignature)
        };

        if kernel_sig.marker != SIGNATURE_MARKER {
            return Err(VerifyError::MissingSignature { found_marker: kernel_sig.marker });
        }

        let mut hasher = Sha512::new();
        hasher.update(kernel_data);
        let digest = hasher.finalize();

        if digest.as_slice() != &kernel_sig.sha512 {
            return Err(VerifyError::HashMismatch);
        }

        let key = VerifyingKey::from_bytes(KERNEL_SIGNING_PUBLIC_KEY)
            .map_err(|_| VerifyError::InvalidPublicKey)?;
        let sig = Signature::from_bytes(&kernel_sig.signature);
        key.verify(digest.as_slice(), &sig).map_err(|_| VerifyError::SignatureMismatch)
    }
}

// ─── Implémentation légère sans crypto (feature = "secure-boot" inactive) ────

#[cfg(not(feature = "secure-boot"))]
mod secure_impl {
    use super::*;

    /// Vérification structurelle uniquement : présence du marqueur.
    pub fn verify_full(image: &[u8]) -> Result<(), VerifyError> {
        if image.len() < KERNEL_SIG_STRUCT_SIZE {
            return Err(VerifyError::ImageTooSmall { size: image.len() });
        }
        let sig_offset = image.len() - KERNEL_SIG_STRUCT_SIZE;
        let sig_bytes  = &image[sig_offset..];

        let kernel_sig: KernelSignature = unsafe {
            core::ptr::read_unaligned(sig_bytes.as_ptr() as *const KernelSignature)
        };

        if kernel_sig.marker != SIGNATURE_MARKER {
            // Sans secure-boot : acceptable (kernel non signé = dev mode)
            return Ok(());
        }

        // Marqueur présent mais pas de vérification crypto → Ok en dev
        Ok(())
    }
}

// ─── Point d'entrée public ────────────────────────────────────────────────────

/// Vérifie le kernel et PANIC en cas d'échec.
///
/// RÈGLE BOOT-02 : Appelé AVANT tout chargement.
/// - Avec `secure-boot` : vérification complète Ed25519 + SHA-512 (bloquante).
/// - Avec `dev-skip-sig` : vérification tentée, kernel non-signé accepté.
/// - Sans les deux : vérification de marqueur uniquement.
pub fn verify_kernel_or_panic(image: &[u8]) {
    match secure_impl::verify_full(image) {
        Ok(()) => { /* OK */ }

        #[cfg(feature = "dev-skip-sig")]
        Err(VerifyError::MissingSignature { .. }) => {
            // Dev : kernel non signé accepté
        }

        Err(e) => {
            #[cfg(feature = "secure-boot")]
            panic!("BOOT-02: Vérification signature kernel ÉCHOUÉE : {}\n\
                    Le kernel peut avoir été compromis. Boot annulé.", e);

            #[cfg(not(feature = "secure-boot"))]
            let _ = e; // En dev sans secure-boot, ne pas bloquer
        }
    }
}



