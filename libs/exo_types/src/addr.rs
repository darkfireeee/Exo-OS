// libs/exo-types/src/addr.rs
//
// Fichier : libs/exo_types/src/addr.rs
// Rôle    : Types d'adresses mémoire — GI-01 Étape 2.
//
// INVARIANTS :
//   - PhysAddr   → CPU uniquement, JAMAIS dans un registre DMA device.
//   - IoVirtAddr → Seule adresse autorisée dans les registres DMA (via IOMMU).
//   - VirtAddr   → Espace d'adressage d'un processus Ring 1/3.
//
// SÉCURITÉ ISR : Ces types sont Copy → utilisables en ISR sans allocation.
//
// SOURCE DE VÉRITÉ : ExoOS_Kernel_Types_v10.md §1, GI-01_Types_TCB_SSR.md §3

/// Adresse physique DRAM — visible CPU uniquement.
///
/// ❌ ERREUR SILENCIEUSE CRITIQUE :
///    Programmer une PhysAddr dans un registre DMA de device
///    → Le device accède à la mauvaise mémoire physique (bypass IOMMU).
///    → Corruption mémoire silencieuse. Détectable uniquement par IOMMU fault.
///
/// ✅ Pour le DMA : convertir via SYS_DMA_MAP → obtenir IoVirtAddr.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Additionne un offset — panique en debug si overflow.
    #[inline(always)]
    pub fn offset(self, off: u64) -> Self {
        PhysAddr(self.0.checked_add(off).expect("PhysAddr overflow"))
    }

    /// Aligne vers le haut sur la puissance de deux donnée.
    #[inline(always)]
    pub fn align_up(self, align: u64) -> Self {
        debug_assert!(align.is_power_of_two(), "align doit être puissance de 2");
        let mask = align - 1;
        PhysAddr((self.0 + mask) & !mask)
    }

    /// Vérifie l'alignement.
    #[inline(always)]
    pub fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }
}

/// Adresse IO virtuelle — visible par le device via l'IOMMU.
///
/// C'est la **seule** adresse autorisée dans les registres DMA d'un device.
/// Obtenue UNIQUEMENT via `SYS_DMA_ALLOC` ou `SYS_DMA_MAP`.
///
/// ❌ ERREUR SILENCIEUSE CRITIQUE :
///    `IoVirtAddr(phys_addr.0)` → bypass de l'IOMMU.
///    → Attaque DMA possible (DMA remapping absent).
///    → Fonctionne sur machine sans IOMMU, crash/exploit avec IOMMU activé.
///
/// ❌ PRÉVENTION : Pas d'impl From<PhysAddr> for IoVirtAddr.
///    Le seul chemin légitime passe par les syscalls DMA kernel.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct IoVirtAddr(pub u64);

impl IoVirtAddr {
    /// Constructeur réservé au module `iommu/` kernel.
    ///
    /// ❌ `pub(crate)` : empêche toute construction depuis un driver Ring 1.
    ///    Un driver Ring 1 ne peut obtenir un IoVirtAddr QUE via SYS_DMA_MAP.
    pub(crate) fn from_raw(v: u64) -> Self {
        IoVirtAddr(v)
    }

    /// Additionne un offset (pour navigation dans une région DMA allouée).
    #[inline(always)]
    pub fn offset(self, off: u64) -> Self {
        IoVirtAddr(self.0.checked_add(off).expect("IoVirtAddr overflow"))
    }
}

/// Adresse virtuelle dans l'espace d'adressage d'un processus Ring 1/3.
///
/// Ne jamais confondre avec `PhysAddr` (CPU) ou `IoVirtAddr` (device DMA).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct VirtAddr(pub usize);

impl VirtAddr {
    /// Construit depuis un pointeur brut — usage exclusif Ring 0 kernel.
    ///
    /// # Safety
    /// Le pointeur doit être dans l'espace d'adressage valide du processus.
    #[inline(always)]
    pub unsafe fn from_ptr<T>(p: *const T) -> Self {
        VirtAddr(p as usize)
    }

    /// Vérifie que l'adresse est dans les plages Ring 3 canoniques x86_64.
    /// Limite userspace : 0x0000_7FFF_FFFF_FFFF (canonical form).
    #[inline(always)]
    pub fn is_userspace(self) -> bool {
        self.0 <= 0x0000_7FFF_FFFF_FFFF
    }
}
