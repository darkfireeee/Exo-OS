// kernel/src/memory/core/constants.rs
//
// Constantes fondamentales de gestion mémoire — Exo-OS Couche 0
// Aucune dépendance externe. Toutes les valeurs sont des constantes
// de compilation vérifiées statiquement.

// ─────────────────────────────────────────────────────────────────────────────
// TAILLES DE PAGES
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une page standard (4 KiB)
pub const PAGE_SIZE: usize = 4096;

/// Décalage (shift) d'une page standard
pub const PAGE_SHIFT: usize = 12;

/// Masque pour extraire l'offset dans une page
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

/// Taille d'une huge page (2 MiB)
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Décalage d'une huge page (21 bits)
pub const HUGE_PAGE_SHIFT: usize = 21;

/// Masque pour extraire l'offset dans une huge page
pub const HUGE_PAGE_MASK: usize = HUGE_PAGE_SIZE - 1;

/// Taille d'une gigapage (1 GiB)
pub const GIGA_PAGE_SIZE: usize = 1024 * 1024 * 1024;

/// Décalage d'une gigapage (30 bits)
pub const GIGA_PAGE_SHIFT: usize = 30;

// ─────────────────────────────────────────────────────────────────────────────
// LIGNE DE CACHE
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une ligne de cache x86_64 (64 octets)
pub const CACHE_LINE_SIZE: usize = 64;

/// Décalage d'une ligne de cache
pub const CACHE_LINE_SHIFT: usize = 6;

/// Masque d'alignement cache-line
pub const CACHE_LINE_MASK: usize = CACHE_LINE_SIZE - 1;

// ─────────────────────────────────────────────────────────────────────────────
// BUDDY ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Ordre maximum du buddy allocator (2^11 = 2048 pages = 8 MiB)
pub const BUDDY_MAX_ORDER: usize = 12;

/// Nombre de niveaux du buddy allocator
pub const BUDDY_ORDER_COUNT: usize = BUDDY_MAX_ORDER + 1;

/// Taille maximale allouée par le buddy (en pages)
pub const BUDDY_MAX_PAGES: usize = 1 << BUDDY_MAX_ORDER;

/// Taille maximale allouée par le buddy (en octets)
pub const BUDDY_MAX_BYTES: usize = BUDDY_MAX_PAGES * PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// SLAB ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de classes de taille du slab allocator
pub const SLAB_NR_SIZE_CLASSES: usize = 16;

/// Taille minimale d'un objet slab (alignée sur 8 octets)
pub const SLAB_MIN_OBJ_SIZE: usize = 8;

/// Taille maximale d'un objet slab (2 KiB — au-delà → buddy)
pub const SLAB_MAX_OBJ_SIZE: usize = 2048;

/// Nombre maximum d'objets par slab page
pub const SLAB_MAX_OBJS_PER_PAGE: usize = PAGE_SIZE / SLAB_MIN_OBJ_SIZE;

/// Seuil : au-dessus de cette taille, le slab dispatch vers buddy direct
pub const SLAB_LARGE_THRESHOLD: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU POOLS
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de frames dans chaque per-CPU pool
pub const PER_CPU_POOL_SIZE: usize = 512;

/// Seuil de réapprovisionnement du per-CPU pool (25%)
pub const PER_CPU_REFILL_THRESHOLD: usize = PER_CPU_POOL_SIZE / 4;

/// Seuil de vidange du per-CPU pool (75%)
pub const PER_CPU_DRAIN_THRESHOLD: usize = (PER_CPU_POOL_SIZE * 3) / 4;

// ─────────────────────────────────────────────────────────────────────────────
// EMERGENCY POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de WaitNodes pré-alloués au boot dans l'EmergencyPool
/// DOIT être initialisé AVANT tout autre module noyau (RÈGLE EMERGENCY-01)
pub const EMERGENCY_POOL_SIZE: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// TLB SHOOTDOWN
// ─────────────────────────────────────────────────────────────────────────────

/// Timeout d'un TLB shootdown IPI (cycles ~100µs à 3 GHz)
pub const TLB_SHOOTDOWN_TIMEOUT_CYCLES: u64 = 300_000;

/// Nombre maximum de CPUs supportés
pub const MAX_CPUS: usize = 256;

/// Nombre maximum de nœuds NUMA supportés
pub const MAX_NUMA_NODES: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// ZONES MÉMOIRE
// ─────────────────────────────────────────────────────────────────────────────

/// Fin de la zone DMA (<16 MiB — devices 32 bits legacy)
pub const ZONE_DMA_END: usize = 16 * 1024 * 1024;

/// Fin de la zone DMA32 (<4 GiB — devices PCIe 32 bits)
pub const ZONE_DMA32_END: usize = 4 * 1024 * 1024 * 1024;

/// Début de la zone NORMAL (>= 4 GiB sur systèmes 64 bits)
pub const ZONE_NORMAL_START: usize = ZONE_DMA32_END;

// ─────────────────────────────────────────────────────────────────────────────
// DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'un DMA ring (512 descripteurs, puissance de 2 obligatoire)
pub const DMA_RING_SIZE: usize = 512;

/// Masque du DMA ring
pub const DMA_RING_MASK: usize = DMA_RING_SIZE - 1;

/// Timeout de completion DMA (cycles ~500ns à 3 GHz)
pub const DMA_COMPLETION_TIMEOUT_CYCLES: u64 = 1_500;

/// Taille maximale d'un transfert DMA continu
pub const DMA_MAX_TRANSFER_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

// ─────────────────────────────────────────────────────────────────────────────
// IPC RING (partagé avec ipc/ via memory/utils/)
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un message IPC petit (inline dans le ring)
pub const IPC_MAX_SMALL_MSG: usize = 4080;

/// Taille d'un ring IPC (puissance de 2 obligatoire)
pub const IPC_RING_SIZE: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// HEAP KERNEL
// ─────────────────────────────────────────────────────────────────────────────

/// Taille du TLS cache du heap kernel (hot path <25 cycles)
pub const HEAP_TLS_CACHE_SIZE: usize = 64;

/// Taille d'un magazine (batch alloc/free)
pub const HEAP_MAGAZINE_SIZE: usize = 128;

/// Seuil de vidange du TLS cache vers pool global
pub const HEAP_TLS_DRAIN_COUNT: usize = HEAP_TLS_CACHE_SIZE / 2;

// ─────────────────────────────────────────────────────────────────────────────
// STACK CANARY
// ─────────────────────────────────────────────────────────────────────────────

/// Valeur initiale du canary de pile noyau (pattern aléatoire compilée)
/// Remplacée au boot par une valeur lue depuis RDRAND ou RDSEED
pub const STACK_CANARY_INITIAL: u64 = 0x_DEAD_BEEF_CAFE_BABE;

// ─────────────────────────────────────────────────────────────────────────────
// FUTEX
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de buckets dans la futex hash table (doit être puissance de 2)
pub const FUTEX_HASH_BUCKETS: usize = 256;

/// Masque pour le hash de la futex table
pub const FUTEX_HASH_MASK: usize = FUTEX_HASH_BUCKETS - 1;

// ─────────────────────────────────────────────────────────────────────────────
// VÉRIFICATIONS STATIQUES
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(PAGE_SIZE.is_power_of_two(), "PAGE_SIZE doit être une puissance de 2");
const _: () = assert!(HUGE_PAGE_SIZE.is_power_of_two(), "HUGE_PAGE_SIZE doit être une puissance de 2");
const _: () = assert!(GIGA_PAGE_SIZE.is_power_of_two(), "GIGA_PAGE_SIZE doit être une puissance de 2");
const _: () = assert!(CACHE_LINE_SIZE.is_power_of_two(), "CACHE_LINE_SIZE doit être une puissance de 2");
const _: () = assert!(DMA_RING_SIZE.is_power_of_two(), "DMA_RING_SIZE doit être une puissance de 2");
const _: () = assert!(IPC_RING_SIZE.is_power_of_two(), "IPC_RING_SIZE doit être une puissance de 2");
const _: () = assert!(FUTEX_HASH_BUCKETS.is_power_of_two(), "FUTEX_HASH_BUCKETS doit être une puissance de 2");
const _: () = assert!(PAGE_SHIFT == 12, "PAGE_SHIFT doit être 12 pour x86_64");
const _: () = assert!(HUGE_PAGE_SHIFT == 21, "HUGE_PAGE_SHIFT doit être 21 (2MiB)");
const _: () = assert!(EMERGENCY_POOL_SIZE >= 64, "EmergencyPool trop petit — risque deadlock");
const _: () = assert!(BUDDY_MAX_ORDER <= 20, "BUDDY_MAX_ORDER excessif");
const _: () = assert!(MAX_CPUS <= 4096, "MAX_CPUS dépasse la limite x2APIC");
const _: () = assert!(ZONE_DMA_END == 16 * 1024 * 1024, "Zone DMA doit se terminer à 16MiB");
const _: () = assert!(1 << PAGE_SHIFT == PAGE_SIZE, "PAGE_SHIFT incohérent avec PAGE_SIZE");
const _: () = assert!(1 << HUGE_PAGE_SHIFT == HUGE_PAGE_SIZE, "HUGE_PAGE_SHIFT incohérent");
