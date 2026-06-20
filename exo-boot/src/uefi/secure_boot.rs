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

// La POLITIQUE d'enforcement (refuser vs avertir selon le verdict de signature,
// l'état UEFI Secure Boot et `secure_boot_required`) vit désormais dans
// `kernel_loader::verify::decide` — partagée par les chemins UEFI ET BIOS, et
// pilotée par un verdict de signature HONNÊTE (exo-verity), pas par un booléen
// qui pouvait valoir « valide » sans rien vérifier. Ce module n'expose plus que
// la LECTURE de l'état UEFI Secure Boot (`query_secure_boot_status`), consommée
// pour calculer `uefi_sb_enforcing`.
