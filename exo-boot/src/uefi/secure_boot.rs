//! secure_boot.rs — Vérification Secure Boot UEFI pour exo-boot.
//!
//! RÈGLE BOOT-02 (DOC10) :
//!   "Ne charge JAMAIS un kernel non signé si Secure Boot actif."
//!
//! Deux niveaux de vérification :
//!
//!   1. UEFI Secure Boot natif :
//!      Le firmware vérifie la signature du bootloader lui-même via la db (Signature Database).
//!      Exo-boot est signé avec une clé incluse dans db du firmware.
//!      → Géré par le firmware, transparent pour le code Rust du bootloader.
//!
//!   2. Chaîne de confiance bootloader → kernel (ED25519) :
//!      Exo-boot vérifie la signature Ed25519 du kernel ELF avant tout chargement.
//!      La clé publique est EMBARQUÉE dans le binaire du bootloader au compile-time.
//!      → Implémenté dans kernel_loader/verify.rs, orchestré depuis ici.
//!
//! Variables EFI utilisées :
//!   - `SecureBoot` (8be4df61-…) : 1 = Secure Boot actif
//!   - `SetupMode`  (8be4df61-…) : 1 = mode Setup (pas de vérification)

use uefi::table::{Boot, SystemTable};
use uefi::table::runtime::VariableVendor;
use uefi::cstr16;

/// Statut Secure Boot lu depuis les variables EFI.
#[derive(Debug, Clone, Copy, Default)]
pub struct SecureBootStatus {
    /// `true` si la variable UEFI `SecureBoot` = 1.
    pub enabled:    bool,
    /// `true` si la variable UEFI `SetupMode` = 1.
    /// Setup Mode = les clés ne sont pas encore configurées, PK absent.
    pub setup_mode: bool,
    /// `true` si la variable `AuditMode` = 1 (log seulement, pas de blocage).
    pub audit_mode: bool,
    /// `true` si la variable `DeployedMode` = 1 (verrouillage renforcé).
    pub deployed_mode: bool,
}

impl SecureBootStatus {
    /// Secure Boot est effectivement actif ET les clés sont configurées.
    /// C'est la condition pour enforcer la vérification du kernel.
    #[inline]
    pub fn is_enforcing(&self) -> bool {
        self.enabled && !self.setup_mode && !self.audit_mode
    }
}

/// Interroge le firmware UEFI pour déterminer l'état de Secure Boot.
///
/// Lit les variables EFI `SecureBoot`, `SetupMode`, `AuditMode`, `DeployedMode`.
/// Ne panique jamais : si une variable est illisible, elle est supposée absente (false).
pub fn query_secure_boot_status(st: &SystemTable<Boot>) -> SecureBootStatus {
    let rt = st.runtime_services();
    let mut status = SecureBootStatus::default();

    // Lecture de la variable `SecureBoot`
    let mut buf = [0u8; 1];
    if let Ok((data, _)) = rt.get_variable(
        cstr16!("SecureBoot"),
        &VariableVendor::GLOBAL_VARIABLE,
        &mut buf,
    ) {
        status.enabled = data.first().copied().unwrap_or(0) == 1;
    }

    // Lecture de `SetupMode`
    if let Ok((data, _)) = rt.get_variable(
        cstr16!("SetupMode"),
        &VariableVendor::GLOBAL_VARIABLE,
        &mut buf,
    ) {
        status.setup_mode = data.first().copied().unwrap_or(0) == 1;
    }

    // Lecture de `AuditMode` (UEFI 2.6+)
    if let Ok((data, _)) = rt.get_variable(
        cstr16!("AuditMode"),
        &VariableVendor::GLOBAL_VARIABLE,
        &mut buf,
    ) {
        status.audit_mode = data.first().copied().unwrap_or(0) == 1;
    }

    // Lecture de `DeployedMode` (UEFI 2.6+)
    if let Ok((data, _)) = rt.get_variable(
        cstr16!("DeployedMode"),
        &VariableVendor::GLOBAL_VARIABLE,
        &mut buf,
    ) {
        status.deployed_mode = data.first().copied().unwrap_or(0) == 1;
    }

    status
}

/// Enforce la politique Secure Boot pour exo-boot.
///
/// Si `secure_boot_required = true` dans la config :
///   → Vérifie que le kernel est signé QUEL QUE SOIT l'état UEFI Secure Boot.
///
/// Si `secure_boot_required = false` MAIS UEFI Secure Boot est actif en mode enforcing :
///   → Avertit l'utilisateur mais ne bloque pas (defensive security).
///
/// RÈGLE BOOT-02 : la vérification Ed25519 est implémentée dans `kernel_loader/verify.rs`.
/// Ce module n'en refait pas une copie — délégation stricte.
pub fn enforce_secure_boot_policy(
    sb_status:              &SecureBootStatus,
    kernel_sig_valid:       bool,
    config_requires_signed: bool,
) -> Result<(), SecureBootError> {
    match (sb_status.is_enforcing(), kernel_sig_valid, config_requires_signed) {
        // Cas 1 : Signature valide — toujours OK
        (_, true, _) => Ok(()),

        // Cas 2 : Signature invalide + Secure Boot enforcing → BLOCAGE
        (true, false, _) => Err(SecureBootError::KernelSignatureInvalidSecureBootActive),

        // Cas 3 : Signature invalide + config force vérification → BLOCAGE
        (false, false, true) => Err(SecureBootError::KernelSignatureInvalidConfigRequired),

        // Cas 4 : Signature invalide + Secure Boot inactif + config permissive → WARNING seul
        (false, false, false) => {
            // On ne retourne pas d'erreur mais le caller doit logger l'avertissement
            Ok(())
        }
    }
}

/// Erreurs Secure Boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureBootError {
    /// Signature kernel invalide alors que UEFI Secure Boot est en mode enforcing.
    KernelSignatureInvalidSecureBootActive,
    /// Signature kernel invalide alors que la config exo-boot.cfg requiert une signature valide.
    KernelSignatureInvalidConfigRequired,
}

impl core::fmt::Display for SecureBootError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::KernelSignatureInvalidSecureBootActive =>
                write!(f,
                    "BOOT-02 VIOLATION : Kernel non signé/signature invalide, \
                     UEFI Secure Boot actif en mode enforcing — DÉMARRAGE REFUSÉ"),
            Self::KernelSignatureInvalidConfigRequired =>
                write!(f,
                    "BOOT-02 VIOLATION : Kernel non signé/signature invalide, \
                     secure_boot_required=true dans exo-boot.cfg — DÉMARRAGE REFUSÉ"),
        }
    }
}
