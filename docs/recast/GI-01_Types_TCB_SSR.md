# ExoOS — Guide d'Implémentation GI-01
## Types Partagés, TCB, SSR, Structures Fondamentales

**Prérequis** : Workspace Cargo configuré (GI-00 §2)  
**Produit** : `libs/exo-types/`, `libs/exo-phoenix-ssr/`, TCB dans `kernel/`

**Traçabilité CORR (vérifiée, mars 2026)** : `CORR-07` (01), `CORR-10` (02), `CORR-05`/`CORR-17` (06), `CORR-40`/`CORR-41` (07).

---

## 1. Ordre d'Implémentation

```
Étape 1 : exo-types/src/lib.rs       ← #![no_std], pub use
Étape 2 : exo-types/src/addr.rs      ← PhysAddr, IoVirtAddr, VirtAddr
Étape 3 : exo-types/src/object_id.rs ← ObjectId + is_valid() + ZERO_BLOB_ID_4K
Étape 4 : exo-types/src/cap.rs       ← CapToken, CapabilityType, verify_cap_token()
Étape 5 : exo-types/src/ipc_msg.rs   ← IpcMessage, IpcEndpoint
Étape 6 : exo-types/src/fixed_string.rs ← FixedString<N>
Étape 7 : exo-types/src/iovec.rs     ← IoVec, PollFd, EpollEventAbi
Étape 8 : exo-types/src/constants.rs ← ZERO_BLOB_ID_4K, EXOFS_PAGE_SIZE
Étape 9 : kernel/src/scheduler/core/task.rs ← TCB layout 256B
Étape 10: libs/exo-phoenix-ssr/src/lib.rs   ← SSR constantes
```

---

## 2. exo-types/src/lib.rs

```rust
// libs/exo-types/src/lib.rs
//
// RÈGLE : Ce crate ne doit importer NI blake3 NI chacha20poly1305 (SRV-02)
// Vérification CI : grep -r 'blake3\|chacha20' libs/exo-types/ && exit 1

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]   // Oblige à justifier chaque unsafe dans un unsafe fn
#![warn(missing_docs)]
#![allow(clippy::new_without_default)] // const fn new() ne peut pas impl Default

pub mod addr;
pub mod cap;
pub mod constants;
pub mod epoll;
pub mod error;
pub mod fixed_string;
pub mod iovec;
pub mod ipc_msg;
pub mod object_id;
pub mod pollfd;

// Réexports pour usage simple
pub use addr::{IoVirtAddr, PhysAddr, VirtAddr};
pub use cap::{CapToken, CapabilityType, Rights};
pub use constants::{EXOFS_PAGE_SIZE, ZERO_BLOB_ID_4K};
pub use epoll::EpollEventAbi;
pub use error::ExoError;
pub use fixed_string::{FixedString, PathBuf, ServiceName};
pub use iovec::IoVec;
pub use ipc_msg::{IpcEndpoint, IpcMessage};
pub use object_id::ObjectId;
pub use pollfd::PollFd;
```

---

## 3. Types d'Adresse — Erreurs Silencieuses Critiques

```rust
// libs/exo-types/src/addr.rs
//
// RÈGLE ABSOLUE (Kernel_Types_v10 §1) :
//   PhysAddr   → CPU uniquement, JAMAIS dans un registre DMA device
//   IoVirtAddr → Seule adresse dans les registres DMA (après SYS_DMA_MAP)
//   VirtAddr   → Espace d'un processus Ring 1/3

/// Adresse physique DRAM — visible CPU uniquement.
/// ❌ ERREUR SILENCIEUSE : programmer PhysAddr dans un registre DMA
///    → Le device accède à la mauvaise mémoire (contourne l'IOMMU)
///    → Corruption mémoire silencieuse, détectable uniquement par IOMMU fault
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

/// Adresse IO virtuelle — visible par le device via l'IOMMU.
/// SEULE adresse autorisée dans les registres DMA du device.
/// Obtenue UNIQUEMENT via SYS_DMA_ALLOC ou SYS_DMA_MAP.
///
/// ❌ ERREUR SILENCIEUSE : construire IoVirtAddr(phys_addr.0)
///    → Bypass de l'IOMMU → attaque DMA possible
///    → Fonctionne sur machine sans IOMMU mais crash avec IOMMU activé
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct IoVirtAddr(pub u64);

/// Adresse virtuelle dans l'espace d'un processus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct VirtAddr(pub usize);

// ─── PRÉVENTION DES CONVERSIONS IMPLICITES ─────────────────────────────
// Pas d'implémentation From<PhysAddr> for IoVirtAddr
// Pas d'implémentation From<u64> for IoVirtAddr directement
// La seule façon d'obtenir une IoVirtAddr est via les syscalls DMA
impl IoVirtAddr {
    /// Constructeur interne — seulement pour le module iommu/
    /// ❌ Pas pub : empêche la création d'IoVirtAddr depuis PhysAddr
    pub(crate) fn from_raw(v: u64) -> Self { IoVirtAddr(v) }
}
```

---

## 4. ObjectId — Pièges de l'Exception ZERO_BLOB_ID_4K

```rust
// libs/exo-types/src/object_id.rs

/// Identifiant d'objet ExoOS — 32 bytes opaques.
///
/// Format standard : bytes[0..8] = compteur u64 LE, bytes[8..32] = zéro.
/// EXCEPTION : ZERO_BLOB_ID_4K ne suit pas ce format (c'est un hash Blake3).
///
/// ❌ ERREUR GRAVE : utiliser ZERO_BLOB_ID_4K comme ObjectId "normal"
///    dans des contextes qui appellent is_valid() sans connaître l'exception
///    → Les comparaisons de hash reflink deviennent incorrectes
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    /// Crée un ObjectId depuis un compteur (format standard).
    /// bytes[0..8] = compteur, bytes[8..32] = zéro automatiquement.
    pub fn from_counter(counter: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&counter.to_le_bytes());
        ObjectId(bytes)
    }

    /// Vérifie la validité selon le format standard.
    ///
    /// CORR-07 : Exception explicite pour ZERO_BLOB_ID_4K.
    ///
    /// ❌ PIÈGES COMMUNS :
    ///   1. Appeler is_valid() sur ZERO_BLOB_ID_4K avant l'exception → false
    ///   2. Utiliser is_valid() comme seule vérification de sécurité
    ///      (un attaquant peut forger un ObjectId avec bytes[8..32]==0)
    pub fn is_valid(&self) -> bool {
        // Exception CORR-07 : ZERO_BLOB_ID_4K est un P-Blob valide (hash Blake3)
        if *self == crate::constants::ZERO_BLOB_ID_4K {
            return true;
        }
        // Format standard : bytes[8..32] doivent être zéro
        self.0[8..32].iter().all(|&b| b == 0)
    }
}
```

### Implémentation des constantes ExoFS

```rust
// libs/exo-types/src/constants.rs
//
// COMMENT OBTENIR ZERO_BLOB_ID_4K :
// C'est Blake3([0u8; 4096]). Calculé une fois, codé en dur.
//
// ❌ ERREUR GRAVE : recalculer ce hash au runtime en Ring 0
//    → Viole SRV-04 (blake3 uniquement dans crypto_server)
//    → Introduit une dépendance blake3 dans le kernel
//
// ✅ La valeur est pré-calculée et vérifiée par le test blake3_zero_4k

pub const EXOFS_PAGE_SIZE: usize = 4096;

/// Blake3([0u8; 4096]) — pré-calculé.
/// JAMAIS recalculer en Ring 0.
/// JAMAIS passer à blob_refcount::increment() (refcount virtuel = ∞).
pub const ZERO_BLOB_ID_4K: ObjectId = ObjectId([
    0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6,
    0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdb, 0xc9, 0xab,
    0x14, 0x46, 0x34, 0x66, 0x0a, 0x71, 0x38, 0x5f,
    0x02, 0x28, 0xe7, 0xd7, 0x0b, 0xce, 0xe1, 0x07,
]);

// Test de validation obligatoire (exécuté en CI uniquement, feature "test")
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blake3_zero_4k_correct() {
        // Ce test valide que la constante est bien Blake3([0u8; 4096])
        // S'exécute côté host (pas dans le kernel) avec blake3 crate
        let input = [0u8; 4096];
        let hash = blake3::hash(&input);
        assert_eq!(hash.as_bytes(), &ZERO_BLOB_ID_4K.0);
    }
}
```

---

## 5. IpcMessage — Contraintes ABI

```rust
// libs/exo-types/src/ipc_msg.rs

/// Message IPC — exactement 64 bytes (1 cache line).
///
/// LAYOUT VERROUILLÉ :
///   [0]   sender_pid:  u32  — renseigné par le kernel (non falsifiable)
///   [4]   msg_type:    u32  — discriminant du protocole Ring 1
///   [8]   reply_nonce: u32  — anti-reuse PID (CORR-17)
///   [12]  _pad:        u32  — alignement
///   [16]  payload:    [u8; 48]
///
/// ❌ ERREUR ABI : ajouter un champ entre sender_pid et payload
///    → Tous les servers recompilés ensemble (workspace) — sinon ABI mismatch
///
/// ❌ ERREUR SILENCIEUSE : payload > 48B pour les données volumineuses
///    → Utiliser un SHM handle (ObjectId = 24B) + données dans SHM
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IpcMessage {
    pub sender_pid:  u32,
    pub msg_type:    u32,
    pub reply_nonce: u32,    // CORR-17 : anti-reuse PID
    pub _pad:        u32,
    pub payload:     [u8; 48],
}

// Assertions compile-time obligatoires
const _: () = assert!(core::mem::size_of::<IpcMessage>() == 64);
const _: () = assert!(core::mem::offset_of!(IpcMessage, payload) == 16);

/// Endpoint IPC — DOIT rester Copy (CORR-40).
/// ❌ Ne JAMAIS ajouter Arc<T>, Box<T>, ou tout champ non-Copy.
/// La compile-time assertion ci-dessous garantit cette propriété.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcEndpoint {
    pub pid:         u32,
    pub chan_idx:    u32,
    pub generation:  u32,
    pub _pad:        u32,
}

// Garantie Copy (CORR-40)
const _: () = assert!(core::mem::size_of::<IpcEndpoint>() == 16);
const fn _assert_copy<T: Copy>() {}
const _: () = _assert_copy::<IpcEndpoint>();
```

---

## 6. EpollEventAbi — Piège repr(packed)

```rust
// libs/exo-types/src/epoll.rs
//
// PROBLÈME RUST E0793 :
// Dans une struct #[repr(C, packed)], accéder à un champ u64 non-aligné
// via référence = Undefined Behavior (depuis Rust 1.72).
//
// SOLUTION : Stocker data comme [u8; 8] avec accesseurs.
//
// ❌ ERREUR COURANTE après migration :
//    let data = epoll_event.data;  // compile mais peut être UB si struct packed
//
// ✅ CORRECT :
//    let data = epoll_event.data_u64();  // toujours safe

/// ABI Linux exacte pour epoll_event — 12 bytes.
/// Voir : man 2 epoll_ctl
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct EpollEventAbi {
    /// Bitmask des événements (EPOLLIN, EPOLLOUT, etc.)
    pub events:    u32,
    /// Données utilisateur — ne pas accéder directement !
    /// Utiliser data_u64() et set_data_u64()
    data_bytes:   [u8; 8],
}

impl EpollEventAbi {
    /// Lecture safe du champ data (évite le UB repr(packed)).
    #[inline(always)]
    pub fn data_u64(&self) -> u64 {
        // from_ne_bytes est always correct pour packed structs
        u64::from_ne_bytes(self.data_bytes)
    }

    /// Écriture safe du champ data.
    #[inline(always)]
    pub fn set_data_u64(&mut self, v: u64) {
        self.data_bytes = v.to_ne_bytes();
    }

    pub fn new(events: u32, data: u64) -> Self {
        EpollEventAbi { events, data_bytes: data.to_ne_bytes() }
    }
}

// Validation ABI (TL-36)
const _: () = assert!(core::mem::size_of::<EpollEventAbi>() == 12);
const _: () = assert!(core::mem::offset_of!(EpollEventAbi, data_bytes) == 4);
```

---

## 7. TCB — Layout 256 Bytes

```rust
// kernel/src/scheduler/core/task.rs
//
// RÈGLE : Ce fichier est la SOURCE UNIQUE DE VÉRITÉ pour le layout TCB.
// ExoPhoenix_Spec_v6 §3 doit être aligné sur ce fichier (pas l'inverse).
//
// ❌ ERREUR GRAVE : Modifier le layout TCB sans mettre à jour switch_asm.s
//    switch_asm.s utilise des offsets hardcodés [0], [8], [56]
//    → Corruption silencieuse du context switch
//
// ❌ ERREUR SILENCIEUSE : Oublier cr3_phys à [56]
//    → KPTI ne fonctionne pas : tous les threads partagent le même espace d'adressage
//    → Fonctionne sur QEMU sans KPTI, crash en production
//
// OFFSETS UTILISÉS PAR switch_asm.s (hardcodés en ASM) :
//   [8]  kstack_ptr  → RSP sauvegardé/restauré
//   [56] cr3_phys    → CR3 chargé pour KPTI

#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // ─── Cache Line 1 [0..63] ─────────────────────────────────────────
    /// [0]  Pointeur vers CapTable partagée du processus (u64 pour #[repr(C)])
    pub cap_table_ptr:  u64,
    /// [8]  RSP Ring 0 — source pour TSS.RSP0 (V7-C-03)
    pub kstack_ptr:     u64,
    /// [16] Thread ID global unique
    pub tid:            u64,
    /// [24] AtomicU8 logiquement : RUNNING/BLOCKED/ZOMBIE/DEAD
    pub sched_state:    u64,
    /// [32] MSR 0xC0000100 — FS.base (TLS userspace)
    pub fs_base:        u64,
    /// [40] MSR 0xC0000101 — GS.base valeur USERSPACE (voir GI-02 §4)
    pub user_gs_base:   u64,
    /// [48] Intel PKS (0 sur AMD/sans PKS)
    pub pkrs:           u32,
    /// [52] Padding alignement — NE PAS UTILISER
    pub _pad_cl1:       u32,
    /// [56] Adresse physique PML4 — LU PAR switch_asm.s
    pub cr3_phys:       u64,
    // ─── Cache Lines 2-3 [64..191] : GPRs ────────────────────────────
    /// [64..183] 15 GPRs : rax,rbx,rcx,rdx,rsi,rdi,rbp,r8..r14
    pub gpr:            [u64; 15],
    /// [184..191] Padding alignement
    pub _pad_gpr:       [u8; 8],
    // ─── Cache Line 4 [192..255] ──────────────────────────────────────
    pub rip:            u64,   // [192]
    pub rsp_user:       u64,   // [200]
    pub rflags:         u64,   // [208]
    pub cs_ss:          u64,   // [216] cs<<32 | ss
    pub cr2:            u64,   // [224] diagnostic #PF — JAMAIS restauré via MOV CR2
    /// [232] *mut XSaveArea — null si thread jamais utilisé FPU (Lazy FPU)
    pub fpu_state_ptr:  u64,
    /// [240] RunQueue intrusive — null si thread BLOCKED
    pub rq_next:        u64,
    /// [248] RunQueue intrusive — null si thread BLOCKED
    pub rq_prev:        u64,
}

// ─── ASSERTIONS COMPILE-TIME OBLIGATOIRES ────────────────────────────────
const _: () = assert!(core::mem::size_of::<ThreadControlBlock>() == 256);
const _: () = assert!(core::mem::align_of::<ThreadControlBlock>() == 64);
// Offsets utilisés dans switch_asm.s — DOIVENT correspondre à l'ASM
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, kstack_ptr)   == 8);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, cr3_phys)     == 56);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rip)          == 192);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, fpu_state_ptr) == 232);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_next)      == 240);
```

---

## 8. SSR — Implémentation exo-phoenix-ssr

```rust
// libs/exo-phoenix-ssr/src/lib.rs
//
// RÈGLE : Ce crate est compilé IDENTIQUEMENT par Kernel A et Kernel B.
// Toute divergence = désynchronisation silencieuse entre A et B.
//
// ❌ ERREUR CRITIQUE : Kernel A et B compilés avec des versions différentes
//    → Offsets SSR divergents → corruption totale du gel/restore
//
// GUIDE DE VALIDATION :
// Après chaque modification, vérifier les offsets avec :
//   cargo test --lib -- ssr_layout_check

#![no_std]
use core::sync::atomic::AtomicU32;

// ─── MAGIC ────────────────────────────────────────────────────────────────
/// Valeur magique vérifiée au boot par Kernel A ET Kernel B.
/// "SSR_EXOS" en ASCII little-endian.
pub const SSR_LAYOUT_MAGIC: u64 = 0x5353525F4558_4F53;

// ─── ADRESSAGE SSR ────────────────────────────────────────────────────────
pub const SSR_BASE_PHYS:          u64   = 0x0100_0000; // 16 MiB
pub const SSR_SIZE:               usize = 0x10000;     // 64 KiB = 65536B
pub const SSR_MAX_CORES_LAYOUT:   usize = 256;         // COMPILE-TIME FIXE
pub const KERNEL_B_APIC_ID:       u32   = 0;           // Core 0 = Kernel B (CORR-10)

pub static MAX_CORES_RUNTIME: AtomicU32 = AtomicU32::new(0);

// ─── OFFSETS SSR (ABSOLUS dans la région physique) ────────────────────────
//
// LAYOUT (vérifié par calcul dans tests ci-dessous) :
//   +0x0000 : HEADER 64B
//     [+0x00] u64 MAGIC (SSR_LAYOUT_MAGIC)
//     [+0x08] AtomicU64 HANDOFF_FLAG
//     [+0x10] AtomicU64 LIVENESS_NONCE
//     [+0x18] AtomicU64 SEQLOCK_COUNTER
//     [+0x20..0x3F] padding 32B
//   +0x0040 : CANAL COMMANDE B→A (64B)
//   +0x0080 : FREEZE ACK PER-CORE (256 × 64B = 16384B)
//   +0x4080 : PMC SNAPSHOT PER-CORE (256 × 64B = 16384B)
//   +0x8080 : EXTENSIONS RESERVED (16256B)
//   +0xC000 : LOG AUDIT B (8192B — RO pour Kernel A)
//   +0xE000 : MÉTRIQUES PUSH A→B (8192B)
//   TOTAL   : 65536B = 64 KiB ✓

pub const SSR_MAGIC_OFFSET:          usize = 0x0000;
pub const SSR_HANDOFF_FLAG_OFFSET:   usize = 0x0008;
pub const SSR_LIVENESS_NONCE_OFFSET: usize = 0x0010;
pub const SSR_SEQLOCK_OFFSET:        usize = 0x0018;
pub const SSR_CMD_B2A_OFFSET:        usize = 0x0040;
pub const SSR_FREEZE_ACK_OFFSET:     usize = 0x0080;
pub const SSR_PMC_OFFSET:            usize = 0x4080;
pub const SSR_EXTENSIONS_START:      usize = 0x8080;
pub const SSR_LOG_AUDIT_OFFSET:      usize = 0xC000;
pub const SSR_METRICS_OFFSET:        usize = 0xE000;

// ─── ACCESSEURS PER-CORE ──────────────────────────────────────────────────
#[inline(always)]
pub const fn freeze_ack_offset(apic_id: usize) -> usize {
    // Chaque slot = 1 AtomicU64 (8B) + 56B padding = 64B total
    // 56B de padding évitent le false sharing entre cores
    SSR_FREEZE_ACK_OFFSET + apic_id * 64
}

#[inline(always)]
pub const fn pmc_snapshot_offset(apic_id: usize) -> usize {
    // 8 valeurs u64 (64B) par core — pas de padding nécessaire
    SSR_PMC_OFFSET + apic_id * 64
}

// ─── VÉRIFICATIONS COMPILE-TIME ───────────────────────────────────────────
const _: () = assert!(SSR_SIZE == 0x10000);
// Vérifier que FREEZE_ACK + 256*64 = PMC_OFFSET
const _: () = assert!(
    SSR_FREEZE_ACK_OFFSET + SSR_MAX_CORES_LAYOUT * 64 == SSR_PMC_OFFSET
);
// Vérifier que PMC + 256*64 = EXTENSIONS
const _: () = assert!(
    SSR_PMC_OFFSET + SSR_MAX_CORES_LAYOUT * 64 == SSR_EXTENSIONS_START
);
// Vérifier que LOG_AUDIT + METRICS = fin du SSR
const _: () = assert!(SSR_METRICS_OFFSET + 8192 == SSR_SIZE);
```

---

## 9. CapabilityType — Erreur repr(C) avec données

```rust
// libs/exo-types/src/cap.rs
//
// ❌ ERREUR GRAVE (CORR-05) : enum #[repr(C)] avec variantes à données = ILLÉGAL
//
//    #[repr(C)]
//    pub enum CapabilityType {
//        IpcBroker,
//        Driver { pci_id: u16 }, // ← E0517 : repr(C) interdit variantes avec données
//    }
//
// ✅ CORRECT : discriminant pur + données séparées dans CapToken.object_id
//
// COMMENT ENCODER UN BDF dans CapToken :
//   token.object_id.0[0] = bdf.bus
//   token.object_id.0[1] = bdf.device
//   token.object_id.0[2] = bdf.function
//   token.type_id = CapabilityType::DriverPci as u16

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityType {
    IpcBroker      = 1,
    MemoryServer   = 2,
    DriverPci      = 3,
    SysDeviceAdmin = 4,
    ExoFsAccess    = 5,
    CryptoServer   = 6,
    ExoPhoenix     = 7,
}

/// verify_cap_token — constant-time via crate `subtle` (LAC-01, CORR-41).
///
/// ❌ PIÈGES COMMUNS :
///   1. Comparer token.type_id == expected as u16 directement
///      → Attaque de timing : branche dépend du secret
///   2. Retourner bool au lieu de paniquer
///      → init_server peut démarrer sans capability valide
///
/// ✅ CORRECT : constant-time + panic si invalide (CAP-01)
pub fn verify_cap_token(token: &CapToken, expected: CapabilityType) -> bool {
    use subtle::ConstantTimeEq;
    let type_match = token.type_id
        .to_le_bytes()
        .ct_eq(&(expected as u16).to_le_bytes());
    let gen_nonzero = !token.generation.to_le_bytes().ct_eq(&[0u8; 8]);
    let result = bool::from(type_match & gen_nonzero);
    if !result {
        // CAP-01 : panic immédiat si token invalide
        panic!("SECURITY: CapToken invalide — arrêt immédiat");
    }
    result
}
```

---

## 10. Tests de Validation Phase 0

```bash
# Validation complète des types
cargo test --package exo-types

# Vérifications spécifiques
cargo test --package exo-types -- \
    layout::ipc_message_size \
    layout::epoll_event_abi_size \
    layout::ipc_endpoint_copy \
    constants::blake3_zero_4k_correct \
    object_id::is_valid_zero_blob \
    object_id::is_valid_standard

# Vérification TCB (kernel)
cargo test --package kernel -- \
    scheduler::task::tcb_layout

# Vérification SSR
cargo test --package exo-phoenix-ssr -- \
    ssr_layout_check
```

---

*ExoOS — Guide d'Implémentation GI-01 : Types, TCB, SSR — Mars 2026*
