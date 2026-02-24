//! services.rs — Boot Services UEFI — wrappers ergonomiques et sûrs.
//!
//! Ce module encapsule les Boot Services UEFI fréquemment utilisés dans
//! exo-boot et ajoute :
//!   - Des vérifications systématiques "boot services actifs" (RÈGLE BOOT-06)
//!   - Des logs de diagnostic intégrés
//!   - Des conversions vers les types internes d'exo-boot
//!
//! RÈGLE BOOT-06 : tous les appels passent par `assert_boot_services_active()`.

use uefi::prelude::*;
use uefi::table::boot::{
    AllocateType, MemoryDescriptor, MemoryType, OpenProtocolAttributes, OpenProtocolParams,
    ScopedProtocol,
};
use uefi::proto::Protocol;
use crate::uefi::exit::assert_boot_services_active;

// ─── Allocation de pages physiques ────────────────────────────────────────────

/// Alloue `page_count` pages physiques consécutives.
///
/// Type d'allocation :
///   - `AllocateType::AnyPages` pour laisser UEFI choisir l'adresse
///   - `AllocateType::Address(addr)` pour forcer une adresse (KASLR)
///
/// Mémoire allouée avec `MemoryType::LoaderData` — sera marquée
/// `KernelCode` ou `KernelData` dans BootInfo selon l'usage.
///
/// RÈGLE BOOT-06 : panique si appelé après ExitBootServices.
pub fn allocate_pages(
    bt:             &BootServices,
    allocate_type:  AllocateType,
    page_count:     usize,
) -> Result<*mut u8, UefiServiceError> {
    assert_boot_services_active("allocate_pages");

    let phys_addr = bt
        .allocate_pages(allocate_type, MemoryType::LOADER_DATA, page_count)
        .map_err(|e| UefiServiceError::AllocationFailed {
            page_count,
            status: e.status(),
        })?;

    // SAFETY : UEFI garantit que cette adresse est une page valide et mappée.
    let ptr = phys_addr as *mut u8;
    // Zéroïse la mémoire allouée — RÈGLE BOOT-03 (BootInfo sans champs non initialisés)
    unsafe { core::ptr::write_bytes(ptr, 0, page_count * 4096) };

    Ok(ptr)
}

/// Libère des pages précédemment allouées via `allocate_pages`.
///
/// SAFETY : `ptr` doit être une adresse retournée par `allocate_pages`,
/// et `page_count` doit correspondre.
pub unsafe fn free_pages(
    bt:          &BootServices,
    ptr:         *mut u8,
    page_count:  usize,
) -> Result<(), UefiServiceError> {
    assert_boot_services_active("free_pages");
    // SAFETY : ptr est une adresse valide retournée par allocate_pages.
    unsafe {
        bt.free_pages(ptr as u64, page_count)
            .map_err(|e| UefiServiceError::FreeFailed { status: e.status() })
    }
}

// ─── Récupération de la Memory Map ────────────────────────────────────────────

/// Taille de la mémoire tampon pour la Memory Map UEFI.
/// La spec recommande : GetMemoryMap() + extra pour tenir compte de l'allocation
/// du buffer lui-même qui peut ajouter des entrées.
const MEMORY_MAP_EXTRA_BYTES: usize = 8 * 1024; // 8 KB de marge

/// Récupère la Memory Map UEFI complète avec retry si le buffer est trop petit.
///
/// Retourne le buffer brut + le MemoryMapKey (nécessaire pour ExitBootServices).
///
/// IMPORTANT : Appeler cette fonction JUSTE AVANT ExitBootServices pour obtenir
/// une clé à jour. Tout AllocatePages/FreePages entre cet appel et ExitBootServices
/// invalide la clé et nécessite un nouvel appel.
pub fn get_memory_map_raw(
    bt: &BootServices,
) -> Result<RawMemoryMapBuffer, UefiServiceError> {
    assert_boot_services_active("get_memory_map_raw");

    // Récupère d'abord la taille nécessaire
    let map_size_hint = bt.memory_map_size();
    let buffer_size = map_size_hint.map_size + MEMORY_MAP_EXTRA_BYTES;

    // Alloue le buffer (taille variable → pool UEFI)
    // On utilise allocate_pool plutôt que allocate_pages pour les petites allocations
    let buffer_ptr = bt
        .allocate_pool(MemoryType::LOADER_DATA, buffer_size)
        .map_err(|e| UefiServiceError::AllocationFailed {
            page_count: buffer_size / 4096 + 1,
            status:     e.status(),
        })?;

    // SAFETY : buffer_ptr est valide, taille buffer_size, alloué par UEFI.
    let buffer = unsafe {
        core::slice::from_raw_parts_mut(buffer_ptr, buffer_size)
    };

    // Remplit la Memory Map
    let memory_map = bt
        .memory_map(buffer)
        .map_err(|e| UefiServiceError::MemoryMapFailed { status: e.status() })?;

    let key = memory_map.key();

    // Collecte tous les descripteurs dans un tableau statique
    // (on ne peut pas utiliser Vec — serait alloué dans le pool UEFI
    //  qui sera libéré après ExitBootServices)
    let mut entries = arrayvec::ArrayVec::<UefiMemoryDescriptorCompact, 1024>::new();
    for desc in memory_map.entries() {
        if entries.is_full() {
            return Err(UefiServiceError::TooManyMemoryDescriptors {
                count: entries.len(),
                max:   1024,
            });
        }
        entries.push(UefiMemoryDescriptorCompact::from(desc));
    }

    // Libère le buffer pool UEFI (données déjà copiées dans `entries`)
    // SAFETY : buffer_ptr valide et le buffer n'est plus utilisé.
    unsafe { bt.free_pool(buffer_ptr).ok(); }

    Ok(RawMemoryMapBuffer {
        // SAFETY : MemoryMapKey est repr(C) wrapping usize — transmute est sound.
        key:     unsafe { core::mem::transmute::<uefi::table::boot::MemoryMapKey, usize>(key) },
        entries,
    })
}

// ─── Ouverture de protocoles ───────────────────────────────────────────────────

/// Ouvre un protocole UEFI de manière sûre.
///
/// SAFETY : Le handle doit être valide pour le protocole demandé.
pub unsafe fn open_protocol_safe<'bt, P: Protocol>(
    bt:           &'bt BootServices,
    handle:       Handle,
    agent:        Handle,
) -> Result<uefi::table::boot::ScopedProtocol<'bt, P>, UefiServiceError> {
    assert_boot_services_active("open_protocol");
    // SAFETY : handle et agent sont valides pour le protocole demandé.
    unsafe {
        bt.open_protocol::<P>(
            OpenProtocolParams {
                handle,
                agent,
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
        .map_err(|e| UefiServiceError::ProtocolNotFound { status: e.status() })
    }
}

/// Localise un protocole UEFI par GUID (recherche dans tous les handles).
pub fn locate_protocol<'bt, P: uefi::proto::ProtocolPointer + ?Sized>(
    bt: &'bt BootServices,
) -> Result<ScopedProtocol<'bt, P>, UefiServiceError> {
    assert_boot_services_active("locate_protocol");
    let handle = bt
        .get_handle_for_protocol::<P>()
        .map_err(|e| UefiServiceError::ProtocolNotFound { status: e.status() })?;
    bt.open_protocol_exclusive::<P>(handle)
        .map_err(|e| UefiServiceError::ProtocolNotFound { status: e.status() })
}

// ─── Types ────────────────────────────────────────────────────────────────────

/// Memory Map brute récupérée avant ExitBootServices.
/// Survit à ExitBootServices car entièrement copiée sur pile/BSS.
pub struct RawMemoryMapBuffer {
    /// Clé à passer à ExitBootServices.
    pub key:     usize,
    /// Entrées de la Memory Map (compactées pour économiser de la mémoire).
    pub entries: arrayvec::ArrayVec<UefiMemoryDescriptorCompact, 1024>,
}

/// Version compacte d'un MemoryDescriptor UEFI.
/// `MemoryDescriptor` UEFI fait 40 bytes — version compacte = 24 bytes.
#[derive(Clone, Copy, Debug)]
pub struct UefiMemoryDescriptorCompact {
    pub memory_type:       u32,
    pub physical_start:    u64,
    pub number_of_pages:   u64,
    pub attribute:         u64,
}

impl From<&MemoryDescriptor> for UefiMemoryDescriptorCompact {
    fn from(d: &MemoryDescriptor) -> Self {
        Self {
            memory_type:    d.ty.0,
            physical_start: d.phys_start,
            number_of_pages: d.page_count,
            attribute:      d.att.bits(),
        }
    }
}

/// Erreurs des Boot Services wrappers.
#[derive(Debug)]
pub enum UefiServiceError {
    AllocationFailed { page_count: usize, status: uefi::Status },
    FreeFailed       { status: uefi::Status },
    MemoryMapFailed  { status: uefi::Status },
    ProtocolNotFound { status: uefi::Status },
    TooManyMemoryDescriptors { count: usize, max: usize },
}

impl core::fmt::Display for UefiServiceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AllocationFailed { page_count, status } =>
                write!(f, "AllocatePages({} pages) échoué : {:?}", page_count, status),
            Self::FreeFailed { status } =>
                write!(f, "FreePages échoué : {:?}", status),
            Self::MemoryMapFailed { status } =>
                write!(f, "GetMemoryMap échoué : {:?}", status),
            Self::ProtocolNotFound { status } =>
                write!(f, "Protocole UEFI introuvable : {:?}", status),
            Self::TooManyMemoryDescriptors { count, max } =>
                write!(f, "Trop de Memory Descriptors : {} (max {})", count, max),
        }
    }
}
