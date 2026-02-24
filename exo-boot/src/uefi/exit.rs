//! exit.rs — ExitBootServices — point de non-retour UEFI.
//!
//! RÈGLE BOOT-06 (DOC10) :
//!   "ExitBootServices = point de non-retour.
//!    Aucun accès aux UEFI Boot Services après ce point."
//!
//! Après ExitBootServices :
//!   - Plus d'allocateur pool UEFI → pas de Box, Vec, etc.
//!   - Plus de timer UEFI
//!   - ConOut peut ne plus fonctionner (firmware-dépendant)
//!   - Runtime Services toujours disponibles (SetVirtualAddressMap, etc.)
//!     mais exo-boot N'UTILISE PAS les Runtime Services directement.
//!     Seul arch/acpi/ du kernel les utilise pour SetVirtualAddressMap.
//!
//! Cette contrainte est trackée via BOOT_SERVICES_ACTIVE (AtomicBool).

use core::sync::atomic::{AtomicBool, Ordering};

/// Flag global indiquant si les Boot Services UEFI sont encore actifs.
///
/// `true`  = avant ExitBootServices (démarrage normal)
/// `false` = après  ExitBootServices (plus aucun BS utilisable)
///
/// Utilisé par :
///   - Le panic handler pour décider quel chemin d'affichage utiliser
///   - Les assertions dans les wrappers de Boot Services
static BOOT_SERVICES_ACTIVE: AtomicBool = AtomicBool::new(true);

/// Retourne `true` si les Boot Services sont encore actifs.
///
/// Thread-safe (Relaxed suffisant : lecture de flag mono-transition).
#[inline]
pub fn boot_services_active() -> bool {
    BOOT_SERVICES_ACTIVE.load(Ordering::Acquire)
}

/// Marque les Boot Services comme inactifs.
///
/// Appelé IMMÉDIATEMENT après `system_table.exit_boot_services()` dans main.rs.
/// Cette fonction ne peut être appelée qu'une seule fois.
///
/// Après cet appel :
///   - `boot_services_active()` retourne `false`
///   - Tout appel aux Boot Services → comportement indéfini (UEFI spec §7.4.6)
#[inline]
pub fn mark_boot_services_exited() {
    let prev = BOOT_SERVICES_ACTIVE.swap(false, Ordering::Release);
    debug_assert!(
        prev,
        "mark_boot_services_exited() appelé deux fois — double ExitBootServices interdit"
    );
}

/// Vérifie que les Boot Services sont actifs, avec un message de contexte.
///
/// Utilisé dans les wrappers de Boot Services pour détecter les utilisations
/// incorrectes après ExitBootServices.
///
/// # Panics
/// Panique si les Boot Services ne sont plus actifs.
#[inline]
pub fn assert_boot_services_active(context: &'static str) {
    if !boot_services_active() {
        panic!(
            "Boot Service appelé après ExitBootServices : {} (RÈGLE BOOT-06)",
            context
        );
    }
}

/// Résumé de l'état post-ExitBootServices pour les logs de diagnostic.
#[derive(Debug, Clone, Copy)]
pub struct ExitBootServicesReport {
    /// Nombre total de Memory Descriptors récupérés avant exit.
    pub memory_descriptors_count: usize,
    /// Taille totale de RAM libre fournie au kernel (en octets).
    pub total_free_ram_bytes: u64,
    /// Adresse RSDP ACPI détectée avant exit.
    pub acpi_rsdp: Option<u64>,
    /// Le kernel a été chargé à cette adresse physique.
    pub kernel_physical_base: u64,
}

impl ExitBootServicesReport {
    /// Affiche le rapport via le framebuffer (seul mécanisme disponible post-exit).
    pub fn log_to_framebuffer(&self, _fb: &crate::display::framebuffer::Framebuffer) {
        let mut writer = crate::display::framebuffer::BootWriter;
        let _ = core::fmt::write(
            &mut writer,
            format_args!(
                "[ExitBootServices OK]\n\
                 Memory descriptors : {}\n\
                 RAM libre          : {} MB\n\
                 ACPI RSDP          : {:#018x?}\n\
                 Kernel base        : {:#016x}\n",
                self.memory_descriptors_count,
                self.total_free_ram_bytes / (1024 * 1024),
                self.acpi_rsdp,
                self.kernel_physical_base,
            ),
        );
    }
}
