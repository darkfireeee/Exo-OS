# ExoOS — Corrections ExoFS & Types Partagés
**Couvre : CORR-06, CORR-20, CORR-22**  
**Sources IAs : ChatGPT5 (§1.3 EpollEvent), Kimi (TL-37), Claude**

---

## CORR-06 🔴 — `EpollEventAbi #[repr(C, packed)]` : accès `u64 data` = UB en Rust

### Problème
`libs/exo-types/src/epoll.rs` définit :
```rust
#[repr(C, packed)]
pub struct EpollEventAbi {
    pub events: u32,
    pub data:   u64,  // ← 4 octets après events → non aligné si struct à adresse 4n
}
```

**En Rust, tout accès par référence à un champ non aligné dans un `repr(packed)` est Undefined Behavior** (E0793 — hard error depuis Rust 1.72).

Concrètement : `let d = &event.data` ou `event.data.as_ref()` = UB.  
Même `event.data` (copie par valeur) génère une instruction unaligned load que le compilateur peut mal optimiser sur ARM.

**Vérification** : `size_of::<EpollEventAbi>() == 12` ✓ (TL-36 correct)  
**Problème** : accès safe au champ `data: u64` ← non portable.

**Sources** : ChatGPT5 Hard Stress §1.3, Rust Nomicon, Rust E0793

### Correction — `libs/exo-types/src/epoll.rs`

```rust
// libs/exo-types/src/epoll.rs — CORR-06

/// Représentation ABI exacte de Linux epoll_event (12 bytes, non aligné sur u64).
///
/// SÉCURITÉ : Le champ `data` est à l'offset 4 → non aligné si la struct
/// est à une adresse 4n (et non 8n). En Rust, prendre une référence à `data`
/// dans une struct `repr(packed)` = UB (E0793).
///
/// Solution : stocker `data` comme `[u8; 8]` et fournir des accesseurs sûrs.
/// L'ABI Linux est préservée identiquement.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct EpollEventAbi {
    pub events: u32,
    /// Données utilisateur stockées en bytes pour éviter l'accès non aligné.
    /// Utiliser `data_u64()` et `set_data_u64()` pour accéder à la valeur.
    data_bytes: [u8; 8],
}

impl EpollEventAbi {
    /// Lit la valeur u64 du champ `data` via unaligned read (sûr).
    #[inline(always)]
    pub fn data_u64(&self) -> u64 {
        // Lecture unaligned explicite — ABI Linux epoll_event exacte
        u64::from_ne_bytes(self.data_bytes)
    }

    /// Écrit la valeur u64 du champ `data`.
    #[inline(always)]
    pub fn set_data_u64(&mut self, v: u64) {
        self.data_bytes = v.to_ne_bytes();
    }

    /// Constructeur complet.
    pub fn new(events: u32, data: u64) -> Self {
        EpollEventAbi {
            events,
            data_bytes: data.to_ne_bytes(),
        }
    }
}

// Vérifications compile-time obligatoires (TL-36)
const _: () = assert!(core::mem::size_of::<EpollEventAbi>() == 12);
// Vérification de l'offset de data_bytes (doit être 4)
const _: () = assert!(core::mem::offset_of!(EpollEventAbi, data_bytes) == 4);

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;
pub const EPOLL_CLOEXEC: i32 = 0x80000;
```

### Impact sur vfs_server

Tous les endroits qui accédaient à `event.data` directement :
```rust
// AVANT (UB potentiel)
let user_data = event.data;

// APRÈS (sûr)
let user_data = event.data_u64();
```

**Migration automatisable** :
```bash
# Rechercher tous les accès directs à .data dans les fichiers epoll
grep -rn "\.data\b" servers/vfs_server/src/ops/poll.rs \
                    servers/vfs_server/src/ops/epoll.rs \
  | grep -v "data_u64\|data_bytes" \
  | grep "EpollEvent\|epoll_event"
```

---

## CORR-20 ⚠️ — Mapping SYS_EXOFS_* 500-518 : spécification manquante

### Problème
Tous les documents mentionnent "syscalls 500-518 ExoFS" mais aucun ne liste le mapping numéro→opération. La couche `exo-syscall/src/exofs.rs` ne peut pas être implémentée sans cette table.

### Mapping canonique proposé — à verrouiller avant implémentation

```rust
// exo-syscall/src/exofs.rs — Table des syscalls ExoFS
// Mapping canonique — VERROUILLÉ — toute modification = bump de version ABI

pub const SYS_EXOFS_OPEN:             u32 = 500; // (path: ObjectId, flags: u32) → fd: u32
pub const SYS_EXOFS_CLOSE:            u32 = 501; // (fd: u32) → ()
pub const SYS_EXOFS_READ:             u32 = 502; // (fd, buf, len, off) → bytes_read
pub const SYS_EXOFS_WRITE:            u32 = 503; // (fd, buf, len, off) → bytes_written
pub const SYS_EXOFS_STAT:             u32 = 504; // (path: ObjectId) → ObjectStat
pub const SYS_EXOFS_CREATE:           u32 = 505; // (parent: ObjectId, name: FixedString<256>) → ObjectId
pub const SYS_EXOFS_DELETE:           u32 = 506; // (obj: ObjectId) → ()
pub const SYS_EXOFS_READDIR:          u32 = 507; // (dir: ObjectId, offset: u64) → DirEntry
pub const SYS_EXOFS_TRUNCATE:         u32 = 508; // (obj: ObjectId, new_size: u64) → ()
pub const SYS_EXOFS_FALLOCATE:        u32 = 509; // (obj, mode, off, len) → ()
pub const SYS_EXOFS_MMAP:             u32 = 510; // (obj, off, len, prot) → vaddr
pub const SYS_EXOFS_MSYNC:            u32 = 511; // (obj, vaddr, len, flags) → ()
pub const SYS_EXOFS_SEEK_SPARSE:      u32 = 512; // (fd, off, whence) → new_off
pub const SYS_EXOFS_COPY_FILE_RANGE:  u32 = 513; // (src, src_off, dst, dst_off, len) → CopyRangeResult
pub const SYS_EXOFS_SYNC:             u32 = 514; // (obj: ObjectId) → ()
pub const SYS_EXOFS_GET_CONTENT_HASH: u32 = 515; // (obj: ObjectId) → ObjectId (audité S-15)
pub const SYS_EXOFS_PATH_RESOLVE:     u32 = 516; // (path: PathBuf) → ObjectId
pub const SYS_EXOFS_EPOCH_META:       u32 = 517; // (obj: ObjectId) → EpochMeta (TODO)
pub const SYS_EXOFS_QUOTA:            u32 = 518; // (obj: ObjectId) → QuotaInfo
pub const SYS_EXOFS_RESERVED:         u32 = 519; // sys_ni_syscall → ENOSYS

// Plages adjacentes (pour référence)
// 520-529 : SYS_PHOENIX_* (phoenix_query, phoenix_notify, ...)
// 530-546 : SYS_DRIVER_* (IRQ, DMA, PCI — Driver Framework v10 §2.2)
```

**Note** : Ce mapping est une proposition fondée sur les opérations documentées dans ExoFS TL v5 et Architecture v7. Il doit être validé et verrouillé avant implémentation de `exo-syscall/src/exofs.rs`.

### Règle TL-37 à ajouter — `ExoFS_Translation_Layer_v5_FINAL.md §5`

```markdown
| ✅ | TL-37 | Lors de l'écriture de blobs via virtio-block (driver Ring 1), le driver DOIT
              utiliser SYS_DMA_MAP (syscall 541) qui retourne IoVirtAddr.
              Ne jamais programmer directement PhysAddr dans les registres DMA.
              IoVirtAddr est la seule adresse valide dans les registres DMA du device
              (Kernel_Types_v10 §1 — règle PhysAddr/IoVirtAddr). |
```

---

## CORR-22 ⚠️ — BlobId : concept uniquement, aucun type Rust

### Problème
Architecture v7 §1.2 définit :
```
BlobId : Blake3([u8;32]) — hash contenu — déduplication
```
ExoFS TL v5 règle 9 et CTL-12 imposent :
```
ObjectId partout — p_blob_id pour variables P-Blob — pas de BlobId
```

Si un développeur crée un type Rust `BlobId`, il violera ces règles.

### Correction — Architecture v7 §1.2 (texte à modifier)

```markdown
<!-- AVANT -->
| `BlobId` | `Blake3([u8;32])` | Hash contenu — déduplication | Calculé par `crypto_server/hash.rs` |

<!-- APRÈS -->
| ~~`BlobId`~~ | *Concept documentaire uniquement* | Hash contenu (P-Blob) | NE PAS créer un type Rust BlobId. |

> **Règle BlobId → ObjectId** : En Rust, les P-Blobs sont représentés par `ObjectId`.
> La convention de nommage `p_blob_id` (préfixe `p_`) identifie une variable contenant
> un hash de contenu plutôt qu'un identifiant d'objet.
> Exemple : `let p_blob_id: ObjectId = blob_registry::lookup(...)`.
> Référence : ExoFS TL v5 §9 règle 9, CTL-12.
```

---

*ExoOS — Corrections ExoFS — Mars 2026*
