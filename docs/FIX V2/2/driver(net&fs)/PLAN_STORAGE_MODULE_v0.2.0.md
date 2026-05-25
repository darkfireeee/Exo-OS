# ExoOS v0.2.0 — Plan de création du module Storage
**Auteur:** claude-alpha  
**Date:** 2026-05-21  
**Périmètre:** `drivers/storage/` complet + `servers/storage_server/` (nouveau) + kernel `virtio_adapter` corrigé  
**Référence architecture:** ExoOS Architecture v7, Driver Framework v10, Ring1 startup V4 canonique

---

## 1. Diagnostic de l'état actuel

### Architecture de stockage existante (schéma de données)

```
Ring3 app
  ↓ SYS_READ / SYS_WRITE / SYS_OPEN
kernel/src/syscall/table.rs
  ↓
kernel/src/syscall/fs_bridge.rs          ← pont POSIX → ExoFS
  ↓
kernel/src/fs/exofs/ (ExoFS TL v5)      ← VFS complet (~95% POSIX)
  ↓
kernel/src/fs/exofs/storage/virtio_adapter.rs   ← BlockDevice trait
  ↓
drivers/storage/virtio_blk/src/lib.rs   ← ExoVirtioBlkDevice
  [⚠️  STUB IN-MEMORY — aucun accès hardware]
```

### État précis de chaque composant storage

| Composant | État | Problème |
|---|---|---|
| `drivers/storage/virtio_blk/src/lib.rs` | ⚠️ Stub | BTreeMap in-memory, `base_address` jamais utilisé |
| `drivers/storage/virtio_blk/src/hal.rs` | ⚠️ Mock | DMA par `alloc::alloc` — pas de pages physiques réelles |
| `drivers/storage/virtio_blk/src/virtqueue.rs` | ⚠️ Incomplet | Structures ring présentes, mais pas d'allocation DMA ni notify MMIO |
| `drivers/storage/virtio_blk/Cargo.toml` | ⚠️ Mal configuré | Dépend de `virtio-drivers` crate (inutilisé), pas de `[bin]` |
| `kernel/src/fs/exofs/storage/virtio_adapter.rs` | ✅ Correct structurellement | Wraps `ExoVirtioBlkDevice` proprement, mais hérite du stub |
| `kernel/src/drivers/pci_cfg.rs::find_virtio_blk_mmio_bar()` | ✅ Complet | Scan PCI réel, retourne BAR0 physique |
| `kernel/src/drivers/mod.rs::init()` | ✅ Complet | Appelle `iommu_init + dma::init + device_server_ipc::init` |
| `servers/virtio_drivers/src/main.rs` | ✅ Stub lifecycle | Endpoint statut uniquement — prévu par design |
| `drivers/storage/ahci/src/` | ❌ Vide | Aucun fichier |
| `drivers/storage/nvme/src/` | ❌ Vide | Aucun fichier |
| `servers/storage_server/` | ❌ Inexistant | À créer intégralement |

### Décision architecturale v0.2.0

**`virtio_blk` reste un `rlib` lié au kernel** (pas de Ring1 userspace driver). Raison : ExoFS opère en espace kernel et appelle `BlockDevice` synchronement. La migration vers IPC est Phase3+ (Architecture v7 Roadmap Phase3 Ring1 complet).

**AHCI et NVMe** sont des **Ring1 userspace drivers** avec IPC vers un nouveau `storage_server`, lui-même broker vers ExoFS via les syscalls existants. Ce pattern respecte DRV-ARCH-01.

---

## 2. Arborescence complète du module Storage

```
drivers/storage/
├── Cargo.toml                              ← workspace member declarations
│
├── virtio_blk/
│   ├── Cargo.toml                          ← rlib, dépendances: spin, log (PAS virtio-drivers crate)
│   └── src/
│       ├── lib.rs                          ← [RÉÉCRITURE] ExoVirtioBlkDevice avec MMIO réel
│       ├── hal.rs                          ← [RÉÉCRITURE] ExoHal avec allocateur kernel réel
│       ├── virtqueue.rs                    ← [RÉÉCRITURE] VirtqueueBlk avec DMA + notify MMIO
│       ├── config.rs                       ← [NOUVEAU] constantes registres VirtIO-blk
│       ├── init.rs                         ← [NOUVEAU] séquence init VirtIO 1.2 §3.1 pour blk
│       └── request.rs                      ← [NOUVEAU] struct VirtioBlkReq + types opérations
│
├── ahci/
│   ├── Cargo.toml                          ← binary Ring1, dépendances: exo-syscall-abi, spin
│   └── src/
│       ├── main.rs                         ← _start: init PCI + HBA + IPC loop
│       ├── hba.rs                          ← HBA registers MMIO (GHC, PI, IS, CAP)
│       ├── port.rs                         ← Port registers (PxCLB, PxFB, PxCMD, PxCI, PxIS)
│       ├── fis.rs                          ← FIS types (H2D, D2H, DMA Setup, PIO Setup)
│       ├── command.rs                      ← Command Table + Command Header layout
│       ├── pci.rs                          ← découverte PCI class 0x01:0x06, BAR5 ABAR
│       ├── interrupt.rs                    ← ISR: PxIS poll, completion, SYS_IRQ_REGISTER
│       ├── protocol.rs                     ← StorageMsg/StorageReply + opcodes BLK_OP_*
│       └── identify.rs                     ← IDENTIFY DEVICE: capacité, secteurs, modèle
│
├── nvme/
│   ├── Cargo.toml                          ← binary Ring1, dépendances: exo-syscall-abi, spin
│   └── src/
│       ├── main.rs                         ← _start: init PCI + controller + IPC loop
│       ├── controller.rs                   ← CC, CSTS, AQA, ASQ, ACQ registers
│       ├── queue.rs                        ← SQ (Submission Queue) + CQ (Completion Queue)
│       ├── command.rs                      ← NVMe commands: Identify, Read, Write, Flush
│       ├── pci.rs                          ← découverte PCI class 0x01:0x08, BAR0 MBAR
│       ├── interrupt.rs                    ← MSI-X ou pin IRQ, CQ polling
│       ├── protocol.rs                     ← StorageMsg/StorageReply (shared avec ahci)
│       └── namespace.rs                    ← LBAF, namespace size, format queries
│
└── common/
    ├── Cargo.toml                          ← rlib partagée (no_std, no alloc)
    └── src/
        ├── lib.rs                          ← re-exports
        ├── protocol.rs                     ← StorageMsg/StorageReply (48B chacun)
        ├── pci_scan.rs                     ← itérateur PCI class/subclass
        └── block.rs                        ← BlockError, LBA types, secteur 512/4096

servers/storage_server/
├── Cargo.toml                              ← binary Ring1: exo-syscall-abi, spin, storage-common
├── src/
│   ├── main.rs                             ← _start: bootstrap, IPC loop, dispatch
│   ├── protocol.rs                         ← BLK_OP_READ/WRITE/FLUSH/IDENTIFY opcodes
│   ├── device_table.rs                     ← table des block devices enregistrés (max 8)
│   ├── driver_link.rs                      ← IPC handshake avec ahci/nvme drivers
│   ├── request_queue.rs                    ← file d'attente des requêtes pending (ring 64)
│   ├── partition.rs                        ← lecture GPT + MBR, table de partitions
│   ├── cache.rs                            ← write-back cache de blocs (128 entrées LRU)
│   ├── stats.rs                            ← compteurs atomiques I/O
│   └── isolation.rs                        ← ExoPhoenix drain/restore pour storage_server
└── tests/
    ├── storage_stress.rs                   ← stress RW 1000 blocs
    ├── partition_gpt.rs                    ← parse image GPT test
    └── cache_lru.rs                        ← test LRU eviction

kernel/src/fs/exofs/storage/
├── virtio_adapter.rs                       ← [MODIFICATION] init_global_disk → MMIO réel
└── storage_server_bridge.rs               ← [NOUVEAU] pont kernel → storage_server IPC (Phase3+)
```

---

## 3. Instructions de création par composant

### 3.1 `drivers/storage/virtio_blk/src/config.rs` [NOUVEAU]

Définir toutes les constantes de registres VirtIO-blk 1.2 pour accès MMIO.

**Contenu requis :**
```
// ─── Offsets registres MMIO communs VirtIO ───────────────────────────────────
VIRTIO_MMIO_MAGIC_VALUE         = 0x000   // "virt" = 0x74726976
VIRTIO_MMIO_VERSION             = 0x004   // 1 = legacy MMIO, 2 = modern
VIRTIO_MMIO_DEVICE_ID           = 0x008   // 2 = blk
VIRTIO_MMIO_VENDOR_ID           = 0x00C   // 0x554D4551 QEMU
VIRTIO_MMIO_DEVICE_FEATURES     = 0x010
VIRTIO_MMIO_DEVICE_FEATURES_SEL = 0x014
VIRTIO_MMIO_DRIVER_FEATURES     = 0x020
VIRTIO_MMIO_DRIVER_FEATURES_SEL = 0x024
VIRTIO_MMIO_QUEUE_SEL           = 0x030
VIRTIO_MMIO_QUEUE_NUM_MAX       = 0x034
VIRTIO_MMIO_QUEUE_NUM           = 0x038
VIRTIO_MMIO_QUEUE_ALIGN         = 0x03C   // legacy: alignement du used ring
VIRTIO_MMIO_QUEUE_PFN           = 0x040   // legacy: phys_addr >> PAGE_SHIFT
VIRTIO_MMIO_QUEUE_READY         = 0x044   // moderne: 1 = queue activée
VIRTIO_MMIO_QUEUE_NOTIFY        = 0x050   // écrire queue_idx pour notifier
VIRTIO_MMIO_INTERRUPT_STATUS    = 0x060   // bit0: used ring update, bit1: config changed
VIRTIO_MMIO_INTERRUPT_ACK       = 0x064   // écrire valeur lue pour acquitter
VIRTIO_MMIO_STATUS              = 0x070   // Device Status Register
VIRTIO_MMIO_CONFIG              = 0x100   // Device-specific config space

// ─── Device Status bits ────────────────────────────────────────────────────
VIRTIO_STATUS_ACKNOWLEDGE       = 0x01
VIRTIO_STATUS_DRIVER            = 0x02
VIRTIO_STATUS_DRIVER_OK         = 0x04
VIRTIO_STATUS_FEATURES_OK       = 0x08
VIRTIO_STATUS_DEVICE_NEEDS_RESET= 0x40
VIRTIO_STATUS_FAILED            = 0x80

// ─── Feature bits VirtIO-blk ───────────────────────────────────────────────
VIRTIO_BLK_F_SIZE_MAX           = 1u64 << 1   // taille max d'une requête
VIRTIO_BLK_F_SEG_MAX            = 1u64 << 2   // nb max de segments
VIRTIO_BLK_F_GEOMETRY          = 1u64 << 4   // CHS geometry
VIRTIO_BLK_F_RO                 = 1u64 << 5   // read only
VIRTIO_BLK_F_BLK_SIZE          = 1u64 << 6   // block size dans config
VIRTIO_BLK_F_FLUSH             = 1u64 << 9   // cache flush command
VIRTIO_BLK_F_TOPOLOGY          = 1u64 << 10  // optimal I/O alignment
VIRTIO_BLK_F_CONFIG_WCE        = 1u64 << 11  // writeback cache
VIRTIO_BLK_F_DISCARD           = 1u64 << 13  // discard/trim support
VIRTIO_BLK_F_WRITE_ZEROES      = 1u64 << 14  // write-zeroes command

// ─── Config Space VirtIO-blk (offset relatif à VIRTIO_MMIO_CONFIG) ─────────
VIRTIO_BLK_CFG_CAPACITY         = 0x00  // u64: nb secteurs 512B
VIRTIO_BLK_CFG_SIZE_MAX         = 0x08  // u32: taille max requête octets
VIRTIO_BLK_CFG_SEG_MAX          = 0x0C  // u32: nb max de segments scatter
VIRTIO_BLK_CFG_CYL              = 0x10  // u16: cylindres
VIRTIO_BLK_CFG_HEADS            = 0x12  // u8: têtes
VIRTIO_BLK_CFG_SECTORS          = 0x13  // u8: secteurs par piste
VIRTIO_BLK_CFG_BLK_SIZE        = 0x14  // u32: taille bloc logique
VIRTIO_BLK_CFG_PHYS_BLOCK_EXP  = 0x1A  // u8: log2(phys/logical)
VIRTIO_BLK_CFG_WRITEBACK        = 0x1B  // u8: 0=writethrough, 1=writeback

// ─── PCI IDs VirtIO-blk ────────────────────────────────────────────────────
VIRTIO_PCI_VENDOR               = 0x1AF4
VIRTIO_BLK_PCI_LEGACY           = 0x1001  // legacy interface
VIRTIO_BLK_PCI_MODERN           = 0x1042  // modern PCIe

// ─── Queue et ring ─────────────────────────────────────────────────────────
VRING_BLK_QUEUE_SIZE            = 128     // puissance de 2, ≤ QUEUE_NUM_MAX
PAGE_SIZE                       = 4096

// ─── Types de requêtes VirtIO-blk ──────────────────────────────────────────
VIRTIO_BLK_T_IN                 = 0      // READ (device → driver)
VIRTIO_BLK_T_OUT                = 1      // WRITE (driver → device)
VIRTIO_BLK_T_FLUSH              = 4      // FLUSH cache
VIRTIO_BLK_T_DISCARD            = 11     // DISCARD/TRIM
VIRTIO_BLK_T_WRITE_ZEROES       = 13     // WRITE ZEROES

// ─── Status de fin de requête ──────────────────────────────────────────────
VIRTIO_BLK_S_OK                 = 0      // succès
VIRTIO_BLK_S_IOERR              = 1      // erreur I/O
VIRTIO_BLK_S_UNSUPP             = 2      // opération non supportée
```

---

### 3.2 `drivers/storage/virtio_blk/src/request.rs` [NOUVEAU]

Définir les structures C des requêtes VirtIO-blk (§5.2.6 spec VirtIO 1.2).

**Structures obligatoires :**

```rust
/// En-tête de requête VirtIO-blk — toujours le premier descriptor (read-only pour le device)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtioBlkReqHeader {
    pub req_type: u32,   // VIRTIO_BLK_T_IN / T_OUT / T_FLUSH ...
    pub reserved: u32,   // toujours 0
    pub sector: u64,     // offset LBA (en secteurs 512B) pour READ/WRITE
}
const _: () = assert!(core::mem::size_of::<VirtioBlkReqHeader>() == 16);

/// Status de fin de requête — toujours le dernier descriptor (write-only par le device)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtioBlkReqStatus {
    pub status: u8,      // VIRTIO_BLK_S_OK | S_IOERR | S_UNSUPP
}

/// Requête complète en mémoire (3 descriptors chaînés)
/// Layout en mémoire DMA :
///   [VirtioBlkReqHeader][data buffer][VirtioBlkReqStatus]
///   ↑ read-only (driver→device) ↑   ↑ write-only (device→driver)
pub struct BlkRequest {
    pub header: VirtioBlkReqHeader,   // 16 bytes — premier descriptor
    pub data_phys: u64,               // adresse physique DMA du buffer data
    pub data_len: u32,                // longueur en bytes (multiple de 512)
    pub status_phys: u64,             // adresse physique du byte status
    pub in_flight: bool,              // requête soumise, pas encore complétée
    pub callback_tag: u64,            // tag opaque retourné à l'appelant
}
```

**Contrainte de layout DMA :** Les 3 régions (header, data, status) doivent être en mémoire physiquement contiguë OU dans des pages séparées. La solution la plus simple et correcte : allouer une seule page de 4096 bytes par requête, layout :
- offset 0 : `VirtioBlkReqHeader` (16 bytes)
- offset 512 : data buffer (jusqu'à 3568 bytes pour un secteur, ou taille variable)
- offset 4095 : `VirtioBlkReqStatus` (1 byte, dernier byte de la page)

Cela garantit l'alignement requis et simplifie la gestion DMA.

---

### 3.3 `drivers/storage/virtio_blk/src/virtqueue.rs` [RÉÉCRITURE]

Remplacer le fichier existant (structures sans DMA) par une implémentation complète.

**Les structures `VirtqDesc`, `VirtqAvailHeader`, `VirtqUsedElem` existantes sont correctes — les conserver.**

**Ajouter la structure `VirtqueueBlk` (gestion DMA réelle) :**

```rust
/// Virtqueue DMA split-ring pour VirtIO-blk.
/// Alloue via l'allocateur kernel (alloc_pages / palloc) et écrit le PFN dans MMIO.
pub struct VirtqueueBlk {
    // Layout mémoire DMA (1 allocation contiguë de 2 pages pour N=128)
    phys_base: u64,          // adresse physique page 0 (descriptor table)
    virt_base: *mut u8,      // adresse virtuelle correspondante (HHDM ou kernel virt)
    queue_size: u16,         // N = 128

    // Pointeurs calculés (offsets dans virt_base)
    desc:  *mut VirtqDesc,           // offset 0
    avail: *mut VirtqAvailRing,      // offset N * 16
    used:  *mut VirtqUsedRing,       // offset PAGE_SIZE (aligné page)

    // État de la free list
    free_head: u16,          // tête de la liste chaînée des descs libres
    free_count: u16,         // nombre de descs disponibles

    // Suivi des completions
    last_used_idx: u16,      // dernier index used ring consommé

    // Table de demandes en vol (max N/3 = 42 pour 3 descs par requête)
    inflight: [Option<BlkInFlightSlot>; 64],
}

struct BlkInFlightSlot {
    head_desc: u16,      // index du premier descriptor de la chaîne
    callback_tag: u64,   // tag opaque pour identifier la requête
    completion_status: u8, // rempli par le device
}
```

**Opérations obligatoires :**

1. `init(queue_size: u16, mmio_base: *mut u8) -> Result<Self, BlkError>`
   - Calculer la taille totale du vring : `DESC_TABLE_SIZE + AVAIL_RING_SIZE + PAGE_ALIGN + USED_RING_SIZE`
   - Allouer les pages physiques via `crate::memory::alloc_pages(n_pages, AllocFlags::DMA_COHERENT)`
   - Obtenir phys_addr et virt_addr (via HHDM : `phys_to_virt(phys)`)
   - Initialiser la free list : `desc[i].next = i+1` pour i < N-1, `desc[N-1].next = u16::MAX`
   - Écrire dans les registres MMIO : `QUEUE_SEL=0, QUEUE_NUM=N, QUEUE_ALIGN=PAGE_SIZE, QUEUE_PFN=phys>>12`
   - Retourner la struct initialisée

2. `submit_read(sector: u64, data_phys: u64, sectors: u32, tag: u64) -> Result<(), BlkError>`
   - Allouer 3 descs via free list (header_desc, data_desc, status_desc)
   - desc[header] = { addr: &header_phys, len: 16, flags: NEXT, next: data_desc_idx }
   - desc[data]   = { addr: data_phys, len: sectors*512, flags: NEXT|WRITE, next: status_desc_idx }
   - desc[status] = { addr: status_phys, len: 1, flags: WRITE, next: 0 }
   - Écrire avail.ring[avail.idx % N] = header_desc_idx
   - `avail.idx += 1` (avec fence Release)
   - Écrire `VIRTIO_MMIO_QUEUE_NOTIFY = 0` (queue 0)
   - Enregistrer dans `inflight[slot] = { head: header_desc_idx, tag, status: 0 }`

3. `submit_write(sector: u64, data_phys: u64, sectors: u32, tag: u64) -> Result<(), BlkError>`
   - Identique à submit_read mais desc[data].flags = NEXT (pas WRITE — le device lit les données)
   - desc[data] est read-only pour le device

4. `submit_flush(tag: u64) -> Result<(), BlkError>`
   - 2 descs : header (T_FLUSH, sector=0) + status
   - Pas de data descriptor

5. `poll_completions() -> CompletionIter`
   - Fence Acquire
   - Lire `used.idx` (écrit par le device)
   - Pour chaque `used.ring[last_used_idx % N]`:
     - Récupérer `id` (head_desc) et `len` (bytes écrits par device)
     - Lire `inflight[slot].completion_status`
     - Libérer la chaîne de descs via `recycle_chain(head)`
     - Retourner `(tag, status)` à l'appelant

6. `recycle_chain(head: u16)`
   - Suivre les liens `next` jusqu'à `flags & NEXT == 0`
   - Remettre chaque desc dans la free list

**Règle mémoire critique :** Entre `avail.idx += 1` et `notify`, utiliser `core::sync::atomic::fence(Ordering::Release)` pour garantir que le device voit les descripteurs avant la notification. Après lecture de `used.idx`, utiliser `fence(Ordering::Acquire)`.

---

### 3.4 `drivers/storage/virtio_blk/src/init.rs` [NOUVEAU]

Séquence d'initialisation VirtIO 1.2 §3.1 pour blk, appelée depuis `ExoVirtioBlkDevice::new()`.

**Séquence obligatoire (ordre strict) :**

```
1. Vérifier le magic MMIO :
   - Lire VIRTIO_MMIO_MAGIC_VALUE → doit être 0x74726976 ("virt")
   - Lire VIRTIO_MMIO_DEVICE_ID → doit être 2 (blk)
   - Si échec : retourner BlkError::DeviceNotFound

2. Reset du device :
   - Écrire 0 dans VIRTIO_MMIO_STATUS
   - Attendre que STATUS lise 0 (spin jusqu'à 10 μs max)

3. Acknowledge :
   - Écrire STATUS |= VIRTIO_STATUS_ACKNOWLEDGE

4. Driver ready :
   - Écrire STATUS |= VIRTIO_STATUS_DRIVER

5. Négociation des features :
   Feature selection bits 0-31 :
     - Écrire DEVICE_FEATURES_SEL = 0
     - Lire DEVICE_FEATURES (bits 0-31)
   Feature selection bits 32-63 :
     - Écrire DEVICE_FEATURES_SEL = 1
     - Lire DEVICE_FEATURES (bits 32-63)
   Features négociées :
     - Garder : VIRTIO_BLK_F_BLK_SIZE | VIRTIO_BLK_F_FLUSH | VIRTIO_BLK_F_TOPOLOGY
     - Ignorer : VIRTIO_BLK_F_RO seulement si en lecture seule
   Écrire DRIVER_FEATURES_SEL = 0, puis DRIVER_FEATURES = features_bits_0_31
   Écrire DRIVER_FEATURES_SEL = 1, puis DRIVER_FEATURES = features_bits_32_63

6. Features OK :
   - Écrire STATUS |= VIRTIO_STATUS_FEATURES_OK
   - Relire STATUS → vérifier que FEATURES_OK est toujours set
   - Si non : device refuse nos features → erreur fatale

7. Lire la configuration device :
   - Lire VIRTIO_BLK_CFG_CAPACITY (u64) → nb de secteurs 512B
   - Si VIRTIO_BLK_F_BLK_SIZE négocié : lire VIRTIO_BLK_CFG_BLK_SIZE

8. Configurer la virtqueue (queue 0) :
   a. Écrire QUEUE_SEL = 0
   b. Lire QUEUE_NUM_MAX → vérifier > 0
   c. Calculer queue_size = min(QUEUE_NUM_MAX, VRING_BLK_QUEUE_SIZE)
   d. Écrire QUEUE_NUM = queue_size
   e. Allouer DMA pour le vring (VirtqueueBlk::init)
   f. Écrire QUEUE_ALIGN = PAGE_SIZE
   g. Écrire QUEUE_PFN = phys_base >> 12

9. Driver OK :
   - Écrire STATUS |= VIRTIO_STATUS_DRIVER_OK

10. Stocker la capacité et le block size dans la struct ExoVirtioBlkDevice
```

**Retour :** `Result<InitResult, BlkError>` où `InitResult` contient `capacity_sectors: u64` et `block_size: u32`.

---

### 3.5 `drivers/storage/virtio_blk/src/hal.rs` [RÉÉCRITURE]

Remplacer le mock DMA par une implémentation réelle utilisant l'allocateur kernel.

**Problème du mock actuel :**
```rust
// ACTUEL (faux) :
fn dma_alloc(pages: usize, _: BufferDirection) -> (PhysAddr, NonNull<u8>) {
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    let paddr = ptr as usize; // ← FAUX : adresse virtuelle ≠ physique
    (paddr, NonNull::new(ptr).unwrap())
}
```

**Implémentation correcte :**
```rust
unsafe impl Hal for ExoHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        // Appeler l'allocateur DMA kernel (identique à ce que fait SYS_DMA_ALLOC pour userspace)
        let phys = crate::memory::alloc_pages(pages, AllocFlags::DMA_COHERENT)
            .expect("DMA alloc failed");
        // HHDM : physique → virtuelle (direct map dans le haut de l'espace kernel)
        let virt = crate::memory::phys_to_virt(phys);
        // Zéroiser
        unsafe {
            core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, pages * PAGE_SIZE);
        }
        (phys.as_u64() as usize, NonNull::new(virt.as_mut_ptr()).unwrap())
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let phys = crate::memory::virt_to_phys(VirtAddr::from_ptr(vaddr.as_ptr()));
        crate::memory::free_pages(phys, pages);
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        // HHDM direct map : phys + PHYS_MAP_BASE
        let virt = crate::memory::phys_to_virt(PhysAddr::new(paddr as u64));
        NonNull::new(virt.as_mut_ptr()).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        // Traduire virt → phys via le page walker
        let vaddr = buffer.as_ptr() as *mut u8 as usize;
        crate::memory::virt_to_phys(VirtAddr::new(vaddr as u64)).as_u64() as usize
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Aucune action nécessaire — les pages DMA_COHERENT sont toujours mappées
    }
}
```

**Note :** `crate::memory::alloc_pages`, `phys_to_virt`, `virt_to_phys` sont les fonctions du kernel ExoOS déjà présentes. `AllocFlags::DMA_COHERENT` garantit que les pages sont dans un IOMMU domain accessible par le device.

---

### 3.6 `drivers/storage/virtio_blk/src/lib.rs` [RÉÉCRITURE]

Remplacer intégralement `ExoVirtioBlkDevice` pour qu'il parle au hardware réel.

**Nouvelle structure :**
```rust
pub struct ExoVirtioBlkDevice {
    mmio_base: *mut u8,         // adresse virtuelle BAR0 (HHDM ou mapped)
    capacity_sectors: u64,      // capacité totale en secteurs 512B
    block_size: u32,            // taille bloc logique (souvent 512 ou 4096)
    queue: Mutex<VirtqueueBlk>, // virtqueue 0 (unique pour blk)
    next_tag: AtomicU64,        // compteur de tags pour les requêtes
}
```

**Méthodes publiques (conservées — `BlockDevice` trait unchanged) :**

```rust
impl ExoVirtioBlkDevice {
    /// Crée et initialise le device depuis l'adresse physique BAR0.
    /// Appelé depuis virtio_adapter::init_global_disk() → find_virtio_blk_mmio_bar().
    pub fn new(bar0_phys: usize, _capacity_hint: usize) -> Self {
        // 1. Obtenir l'adresse virtuelle via HHDM
        let mmio_base = phys_to_virt(PhysAddr::new(bar0_phys as u64)).as_mut_ptr();
        // 2. Exécuter la séquence init VirtIO §3.1 (init.rs)
        let result = unsafe { virtio_blk_init(mmio_base) }
            .expect("VirtIO-blk init failed");
        // 3. Créer la virtqueue
        let queue = VirtqueueBlk::init(result.queue_size, mmio_base)
            .expect("VirtQueue init failed");
        Self {
            mmio_base,
            capacity_sectors: result.capacity_sectors,
            block_size: result.block_size,
            queue: Mutex::new(queue),
            next_tag: AtomicU64::new(1),
        }
    }

    pub fn block_size(&self) -> u32 { self.block_size }
    pub fn total_blocks(&self) -> u64 { self.capacity_sectors }
}

impl BlockDevice for ExoVirtioBlkDevice {
    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> ExofsResult<()> {
        // 1. Allouer une page DMA pour la requête (header + data + status)
        // 2. Écrire VirtioBlkReqHeader { T_IN, sector: block_id }
        // 3. queue.submit_read(block_id, data_phys, 1, tag)
        // 4. Polling actif jusqu'à completion (avec timeout 100ms max)
        // 5. Vérifier status == VIRTIO_BLK_S_OK
        // 6. Copier data DMA → buf
        // 7. Libérer la page DMA
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> ExofsResult<()> {
        // 1. Allouer DMA
        // 2. Copier buf → data DMA
        // 3. queue.submit_write(block_id, data_phys, 1, tag)
        // 4. Polling jusqu'à completion
        // 5. Vérifier status
    }

    fn flush(&self) -> ExofsResult<()> {
        // queue.submit_flush(tag) + polling
    }
}
```

**Polling vs interrupt :** En mode kernel (rlib), le polling actif (spin loop sur `used.idx`) est acceptable pour l'initialisation. Pour la production, l'interruption virtio-blk (signalée par le IOAPIC) appellera `queue.poll_completions()` depuis le handler d'interruption kernel — à câbler en Phase2.

---

### 3.7 `drivers/storage/ahci/src/hba.rs` [NOUVEAU]

Registres MMIO du Host Bus Adapter AHCI (Generic Host Control).

**Offsets ABAR (BAR5 des devices PCI class 0x01/0x06/0x01) :**
```
// Generic Host Control (offset 0x00)
HBA_CAP       = 0x0000   // Host Capabilities
HBA_GHC       = 0x0004   // Global Host Control
HBA_IS        = 0x0008   // Interrupt Status (port bitmap)
HBA_PI        = 0x000C   // Ports Implemented (bitmap des ports actifs)
HBA_VS        = 0x0010   // AHCI Version (1.00 = 0x00010000, 1.30 = 0x00010300)
HBA_CAP2      = 0x0024   // Host Capabilities Extended

// GHC bits
GHC_AE        = 1 << 31  // AHCI Enable
GHC_MRSM      = 1 << 2   // MSI Revert to Single Message
GHC_IE        = 1 << 1   // Interrupt Enable
GHC_HR        = 1 << 0   // HBA Reset

// Port registers (offset 0x100 + port_num * 0x80)
PORT_BASE     = 0x0100
PORT_SIZE     = 0x0080

// Per-port register offsets (relatifs à PORT_BASE + port * PORT_SIZE)
PORT_CLB      = 0x00     // Command List Base (physique, 1KB aligned)
PORT_CLBU     = 0x04     // Command List Base Upper 32 bits
PORT_FB       = 0x08     // FIS Base (physique, 256B aligned)
PORT_FBU      = 0x0C     // FIS Base Upper 32 bits
PORT_IS       = 0x10     // Interrupt Status
PORT_IE       = 0x14     // Interrupt Enable
PORT_CMD      = 0x18     // Command and Status
PORT_TFD      = 0x20     // Task File Data
PORT_SIG      = 0x24     // Signature
PORT_SSTS     = 0x28     // Serial ATA Status (DET bits 0-3)
PORT_SCTL     = 0x2C     // Serial ATA Control
PORT_SERR     = 0x30     // Serial ATA Error
PORT_SACT     = 0x34     // Serial ATA Active (NCQ)
PORT_CI       = 0x38     // Command Issue

// PORT_CMD bits
CMD_ST        = 1 << 0   // Start (DMA engine)
CMD_FRE       = 1 << 4   // FIS Receive Enable
CMD_FR        = 1 << 14  // FIS Receive Running
CMD_CR        = 1 << 15  // Command List Running

// PORT_SSTS DET field (bits 0-3)
DET_PRESENT_OK = 0x3     // Device present and PHY communication established
SIG_ATA        = 0x0101_0101  // SATA device (hard disk)
SIG_ATAPI      = 0xEB14_0101  // ATAPI device (CD/DVD)
```

---

### 3.8 `drivers/storage/ahci/src/fis.rs` [NOUVEAU]

Structures FIS (Frame Information Structure) AHCI.

**Types de FIS obligatoires :**

```rust
/// Register Host to Device FIS (type 0x27) — commandes ATA
#[repr(C, packed)]
pub struct FisH2D {
    pub fis_type: u8,    // 0x27
    pub flags: u8,       // bit7=1 : Command, bit7=0 : Control
    pub command: u8,     // ATA command (READ_DMA_EXT=0x25, WRITE_DMA_EXT=0x35, FLUSH=0xE7)
    pub feature_lo: u8,
    pub lba0: u8,        // LBA bits 0-7
    pub lba1: u8,        // LBA bits 8-15
    pub lba2: u8,        // LBA bits 16-23
    pub device: u8,      // bit6=LBA mode (toujours 1 pour LBA48)
    pub lba3: u8,        // LBA bits 24-31
    pub lba4: u8,        // LBA bits 32-39
    pub lba5: u8,        // LBA bits 40-47
    pub feature_hi: u8,
    pub count_lo: u8,    // sector count lo
    pub count_hi: u8,    // sector count hi (LBA48)
    pub icc: u8,         // Isochronous Command Completion
    pub control: u8,
    pub _reserved: [u8; 4],
}
const _: () = assert!(core::mem::size_of::<FisH2D>() == 20);

/// Register Device to Host FIS (type 0x34) — résultats
#[repr(C, packed)]
pub struct FisD2H {
    pub fis_type: u8,  // 0x34
    pub flags: u8,     // bit6=interrupt
    pub status: u8,    // ATA Status register
    pub error: u8,     // ATA Error register
    pub lba0: u8, pub lba1: u8, pub lba2: u8, pub device: u8,
    pub lba3: u8, pub lba4: u8, pub lba5: u8, pub _reserved0: u8,
    pub count_lo: u8, pub count_hi: u8, pub _reserved1: [u8; 6],
}
const _: () = assert!(core::mem::size_of::<FisD2H>() == 20);

/// DMA Setup FIS (type 0x41)
#[repr(C, packed)]
pub struct FisDmaSetup {
    pub fis_type: u8,   // 0x41
    pub flags: u8,
    pub _reserved0: [u8; 2],
    pub dma_buffer_id: u64,
    pub _reserved1: u32,
    pub dma_buffer_offset: u32,
    pub transfer_count: u32,
    pub _reserved2: u32,
}
```

**FIS Received Area (256 bytes par port, alignée 256B) :**
```rust
#[repr(C, align(256))]
pub struct ReceivedFis {
    pub dsfis: FisDmaSetup,     // +0x00 DMA Setup FIS
    pub _pad0: [u8; 4],
    pub psfis: [u8; 20],        // +0x20 PIO Setup FIS
    pub _pad1: [u8; 12],
    pub rfis: FisD2H,           // +0x40 D2H Register FIS
    pub _pad2: [u8; 4],
    pub sdbfis: [u8; 8],        // +0x58 Set Device Bits FIS
    pub ufis: [u8; 64],         // +0x60 Unknown FIS
    pub _reserved: [u8; 96],
}
const _: () = assert!(core::mem::size_of::<ReceivedFis>() == 256);
```

---

### 3.9 `drivers/storage/ahci/src/command.rs` [NOUVEAU]

Command Header et Command Table AHCI.

**Command List (32 × Command Header, 1KB par port, alignée 1KB) :**
```rust
#[repr(C)]
pub struct CommandHeader {
    pub dw0: u32,      // [4:0]=CFL (longueur FIS en DW), [6]=ATAPI, [7]=WRITE, [16:31]=PRDTL
    pub dw1: u32,      // PRDBC: Physical Region Descriptor Byte Count (rempli par device)
    pub ctba: u32,     // Command Table Base Address (physique, 128B aligné)
    pub ctbau: u32,    // Command Table Base Address Upper 32 bits
    pub _reserved: [u32; 4],
}
const _: () = assert!(core::mem::size_of::<CommandHeader>() == 32);
```

**Command Table (variable, minimum 128B, alignée 128B) :**
```rust
/// Entrée PRDT (Physical Region Descriptor Table)
#[repr(C)]
pub struct PrdtEntry {
    pub dba: u32,       // Data Base Address (physique)
    pub dbau: u32,      // Data Base Address Upper
    pub _reserved: u32,
    pub dbc: u32,       // Bit 0-21: byte count - 1 (max 4MB-1 par entrée), bit31: interrupt on completion
}

#[repr(C)]
pub struct CommandTable {
    pub cfis: [u8; 64],          // Command FIS (max 64 bytes = 16 DW)
    pub acmd: [u8; 16],          // ATAPI Command (12 ou 16 bytes)
    pub _reserved: [u8; 48],
    pub prdt: [PrdtEntry; MAX_PRDT_ENTRIES], // MAX_PRDT_ENTRIES = 8 pour simplifier
}
```

**Implémentation `command_read_dma_ext(slot: usize, lba: u64, count: u16, data_phys: u64)` :**
1. Construire `FisH2D` avec command = 0x25 (READ DMA EXT), LBA48, count
2. Copier dans `cmd_table[slot].cfis`
3. Remplir 1 entrée PRDT : `dba = data_phys lo32, dbau = data_phys hi32, dbc = count * 512 - 1`
4. `CommandHeader.dw0 = (FIS_LEN_DW << 0) | (PRDTL=1 << 16)` (WRITE bit=0 pour lecture)
5. `PORT_CI |= 1 << slot` — soumettre la commande

---

### 3.10 `drivers/storage/ahci/src/main.rs` [NOUVEAU]

Point d'entrée Ring1 pour le driver AHCI.

**Séquence d'initialisation AHCI :**
```
1. SYS_PCI_CLAIM pour le device SATA (class=0x01, subclass=0x06)
2. SYS_MMIO_MAP(abar_phys, 0x1100) → abar_virt
3. Vérifier HBA_CAP: SS (Staggered Spinup), SAM, etc.
4. Activer GHC_AE (AHCI Enable)
5. Lire PI (Ports Implemented)
6. Pour chaque port actif (bit set dans PI) :
   a. Vérifier SSTS.DET == 3 (device présent et PHY OK)
   b. Vérifier SIG == SIG_ATA
   c. Stop DMA : CMD &= ~(ST|FRE), attendre CR|FR = 0
   d. Allouer DMA pour Command List (1KB) et FIS area (256B)
   e. Écrire CLB/CLBU et FB/FBU
   f. Start : CMD |= FRE, puis CMD |= ST
   g. Envoyer IDENTIFY pour lire capacité + modèle
7. Enregistrer IRQ via SYS_IRQ_REGISTER
8. register_endpoint("ahci_driver", ep=16)
9. Signaler readiness au storage_server (BLK_REGISTER_DEVICE)
10. Boucle IPC
```

**IPC loop — messages traités :**
- `BLK_OP_READ` : submit_command_read → poll PI → répondre BLK_REPLY_OK + data
- `BLK_OP_WRITE` : submit_command_write → poll → répondre
- `BLK_OP_FLUSH` : FLUSH CACHE EXT (0xEA)
- `BLK_OP_IDENTIFY` : retourner modèle + capacité
- `MSG_IRQ_NOTIFY` : lire HBA_IS, PORT_IS, compléter requêtes pending

---

### 3.11 `drivers/storage/nvme/src/controller.rs` [NOUVEAU]

Registres BAR0 du contrôleur NVMe (NVM Express Base Specification 2.0).

**Registres obligatoires (offset depuis BAR0) :**
```
CAP     = 0x0000   // Controller Capabilities (64 bits)
VS      = 0x0008   // Version
INTMS   = 0x000C   // Interrupt Mask Set
INTMC   = 0x0010   // Interrupt Mask Clear
CC      = 0x0014   // Controller Configuration
CSTS    = 0x001C   // Controller Status
NSSR    = 0x0020   // NVM Subsystem Reset
AQA     = 0x0024   // Admin Queue Attributes
ASQ     = 0x0028   // Admin Submission Queue Base (64 bits)
ACQ     = 0x0030   // Admin Completion Queue Base (64 bits)
CMBLOC  = 0x0038   // Controller Memory Buffer Location
CMBSZ   = 0x003C   // Controller Memory Buffer Size

// CC bits
CC_EN   = 1 << 0   // Enable
CC_IOSQES = 6 << 16  // I/O SQ Entry Size = 64B (log2)
CC_IOCQES = 4 << 20  // I/O CQ Entry Size = 16B (log2)
CC_AMS    = 0 << 11  // Round Robin arbitration
CC_MPS    = 0 << 7   // Memory Page Size = 4096

// CSTS bits
CSTS_RDY  = 1 << 0  // Ready (controller prêt après CC_EN=1)
CSTS_CFS  = 1 << 1  // Controller Fatal Status
```

**Séquence d'initialisation NVMe (NVMe spec §3.5.1) :**
```
1. Disable controller : CC &= ~CC_EN, attendre CSTS.RDY = 0 (CAP.TO timeout)
2. Configurer AQA : ASQS = admin_sq_size-1, ACQS = admin_cq_size-1
3. Écrire ASQ = admin_sq_phys, ACQ = admin_cq_phys
4. Configurer CC :
   CC = CC_EN | CC_IOSQES | CC_IOCQES | CC_AMS | CC_MPS
5. Attendre CSTS.RDY = 1 (timeout = CAP.TO × 500ms)
6. Envoyer Identify Controller (Admin cmd 0x06) :
   a. Identifier le modèle, firmware, capacité
7. Envoyer Identify Namespace (Admin cmd 0x06, NSID=1) :
   a. Lire NSZE (namespace size en LBA), NUSE, LBAF
8. Créer 1 I/O SQ et 1 I/O CQ via Create I/O CQ (0x05) et Create I/O SQ (0x01)
9. Enregistrer IRQ (pin ou MSI-X)
10. register_endpoint("nvme_driver", ep=17)
11. Signaler au storage_server
```

---

### 3.12 `drivers/storage/nvme/src/queue.rs` [NOUVEAU]

Submission Queue (SQ) et Completion Queue (CQ) NVMe.

**SQ Entry (64 bytes) :**
```rust
#[repr(C)]
pub struct SqEntry {
    pub cdw0: u32,       // [7:0]=Opcode, [9:8]=FUSE, [15:14]=PSDT, [31:16]=CID
    pub nsid: u32,       // Namespace ID (1 pour le premier namespace)
    pub _reserved: u64,
    pub mptr: u64,       // Metadata Pointer
    pub prp1: u64,       // Physical Region Page 1 (adresse physique data)
    pub prp2: u64,       // Physical Region Page 2 (si data > PAGE_SIZE)
    pub cdw10: u32,      // Command-specific (ex: SLBA low)
    pub cdw11: u32,      // Command-specific (ex: SLBA high ou NLB)
    pub cdw12: u32,      // Command-specific (ex: NLB pour read/write)
    pub cdw13: u32, pub cdw14: u32, pub cdw15: u32,
}
const _: () = assert!(core::mem::size_of::<SqEntry>() == 64);
```

**CQ Entry (16 bytes) :**
```rust
#[repr(C)]
pub struct CqEntry {
    pub dw0: u32,   // Command-specific (souvent 0)
    pub dw1: u32,   // Reserved
    pub dw2: u32,   // [15:0]=SQ Head Pointer, [31:16]=SQ Identifier
    pub dw3: u32,   // [14:1]=Status Field, [0]=Phase Tag (P)
}
const _: () = assert!(core::mem::size_of::<CqEntry>() == 16);
```

**Opérations :**
- `sq_submit(entry: SqEntry)` — écrire à `sq_base + sq_tail * 64`, incrémenter tail, écrire doorbell SQ
- `cq_poll() -> Option<CqEntry>` — vérifier Phase bit, consommer entry, écrire doorbell CQ
- Doorbell SQ : `MMIO[0x1000 + qid * stride * 2]`
- Doorbell CQ : `MMIO[0x1000 + qid * stride * 2 + stride]`
- `stride = 1 << (2 + CAP.DSTRD)`

---

### 3.13 `servers/storage_server/src/main.rs` [NOUVEAU]

Le `storage_server` est un Ring1 broker entre ExoFS et les drivers block physiques (AHCI, NVMe). Il ne gère PAS `virtio_blk` (qui reste in-kernel).

**Responsabilités :**
- Enregistrer les devices AHCI et NVMe qui se connectent au démarrage
- Exposer un endpoint IPC `storage_server` (ep=18) pour les clients
- Broker les requêtes read/write/flush vers le bon driver selon le device_id
- Gérer un write-back cache de 128 blocs (LRU)
- Lire la table de partitions GPT pour exposer des partitions nommées

**Structure `StorageService` :**
```rust
struct StorageService {
    devices: DeviceTable,         // table des block devices (max 8)
    request_queue: RequestQueue,  // file d'attente pending (ring 64)
    cache: BlockCache,            // write-back LRU 128 entrées
    stats: StorageStats,          // compteurs atomiques
    isolation: IsolationState,    // ExoPhoenix drain/restore
    bootstrapped: bool,
}
```

**Protocole IPC `StorageMsg` (48 bytes) :**
```rust
pub const BLK_OP_REGISTER_DEVICE : u32 = 0x5000;  // driver → storage_server
pub const BLK_OP_READ            : u32 = 0x5001;  // client → storage_server
pub const BLK_OP_WRITE           : u32 = 0x5002;
pub const BLK_OP_FLUSH           : u32 = 0x5003;
pub const BLK_OP_IDENTIFY        : u32 = 0x5004;
pub const BLK_OP_PARTITION_LIST  : u32 = 0x5005;

#[repr(C)]
pub struct StorageMsg {
    pub opcode:     u32,
    pub sender_pid: u32,
    pub device_id:  u32,   // index dans DeviceTable (0-7)
    pub _pad:       u32,
    pub lba:        u64,   // LBA de début
    pub count:      u32,   // nb de secteurs (max 128 = 64KB)
    pub buf_iova:   u64,   // adresse DMA du buffer applicatif (ou 0 = shared mem)
    pub tag:        u64,   // tag opaque retourné dans StorageReply
}
const _: () = assert!(core::mem::size_of::<StorageMsg>() == 48);

#[repr(C)]
pub struct StorageReply {
    pub status: i64,
    pub device_id: u32,
    pub sectors_done: u32,
    pub tag: u64,
    pub capacity_sectors: u64,
    pub block_size: u32,
    pub _pad: [u8; 16],
}
const _: () = assert!(core::mem::size_of::<StorageReply>() == 48);
```

**Séquence de boot storage_server :**
```
1. register_endpoint("storage_server", ep=18)
2. Attendre BLK_OP_REGISTER_DEVICE de chaque driver :
   - AHCI driver → device_id=0 (sda), device_id=1 (sdb)...
   - NVMe driver → device_id=4 (nvme0n1)...
3. Pour chaque device enregistré :
   a. Lire les 34 premiers secteurs pour parser GPT header + partition table
   b. Exposer les partitions dans DeviceTable
4. Bootstrap complet → boucle IPC principale
```

---

### 3.14 `servers/storage_server/src/cache.rs` [NOUVEAU]

Write-back cache LRU à 128 entrées pour absorber les écritures répétées sur les mêmes blocs.

**Structure :**
```rust
pub struct BlockCache {
    entries: [CacheEntry; 128],
    lru_order: [u8; 128],       // ordre LRU (index 0 = le plus récent)
    dirty_bitmap: [u64; 2],     // bit set → entrée dirty
}

pub struct CacheEntry {
    pub valid: bool,
    pub device_id: u8,
    pub lba: u64,
    pub data: [u8; 512],
}
```

**Opérations :**
- `lookup(device_id, lba) -> Option<&CacheEntry>` — LRU hit
- `insert(device_id, lba, data: &[u8])` — insérer, évincer LRU si plein
- `mark_dirty(slot: usize)` — marquer pour flush
- `flush_dirty() -> impl Iterator<Item = (u8, u64, &[u8])>` — itérer les dirty entries pour écriture

**Politique :** Le cache est à write-back — les écritures sont d'abord en cache et flushées soit sur BLK_OP_FLUSH soit à l'éviction LRU. Les lectures vérifient d'abord le cache avant de passer au driver.

---

### 3.15 `servers/storage_server/src/partition.rs` [NOUVEAU]

Parse la table de partitions GPT pour exposer les partitions nommées.

**Layout GPT :**
```
LBA 0 : MBR protectif (512B)
LBA 1 : GPT Header (512B) — signature "EFI PART", MyLBA, AlternateLBA, FirstUsableLBA, LastUsableLBA, DiskGUID, PartitionEntryLBA, NumPartitionEntries, SizeOfPartitionEntry, PartitionEntryArrayCRC32
LBA 2-33 : Partition Entries (128 × 128B chacune)
  Chaque entry : TypeGUID (16B), UniqueGUID (16B), StartingLBA (8B), EndingLBA (8B), Attributes (8B), PartitionName (72B UTF-16LE)
```

**Structure de sortie :**
```rust
pub struct Partition {
    pub device_id: u8,
    pub index: u8,
    pub start_lba: u64,
    pub end_lba: u64,
    pub size_sectors: u64,
    pub name: [u8; 36],   // UTF-16LE → ASCII tronqué
    pub type_guid: [u8; 16],
}
```

**Implémentation :**
- Lire LBA 1 via `BLK_OP_READ` vers le driver concerné
- Vérifier signature GPT ("EFI PART" = 0x5452415020494645)
- Valider CRC32 du header
- Lire les partition entries (LBA 2 onwards)
- Filtrer les entries non-nulles (TypeGUID ≠ all-zero)
- Convertir PartitionName UTF-16LE → ASCII approximatif pour affichage

---

## 4. Relation avec le kernel et séquence de boot storage

```
Boot kernel Phase2d:
    drivers::init() → iommu_init + dma_init
    
Boot kernel Phase5 (FS init):
    fs::exofs::storage::virtio_adapter::init_global_disk()
        → find_virtio_blk_mmio_bar()     ← scan PCI pour 0x1AF4:0x1001/0x1042
        → init_global_disk_with_mmio(bar0_phys, capacity)
        → VirtioBlockAdapter::new(bar0_phys, capacity)
        → ExoVirtioBlkDevice::new(bar0_phys)   ← init VirtIO MMIO réel
        → register_global_disk(Arc<VirtioBlockAdapter>)
        → ExoFS peut maintenant lire/écrire le disque

Ring1 boot (après kernel Phase5):
    [PID 4]  vfs_server → monte ExoFS sur /
    [PID 5]  crypto_server
    [PID 6]  device_server → discovery hardware
    [PID 7]  exo-virtio-drivers (lifecycle only, ep=13)
    ...
    [PID 14] exo-ahci-driver :
        → SYS_PCI_CLAIM(0x01, 0x06, BAR5)
        → SYS_MMIO_MAP(abar_phys)
        → Init HBA + ports
        → SYS_IRQ_REGISTER(irq)
        → register_endpoint("ahci_driver", ep=16)
        → Envoyer BLK_OP_REGISTER_DEVICE → storage_server
    [PID 15] exo-nvme-driver :
        → SYS_PCI_CLAIM(0x01, 0x08, BAR0)
        → SYS_MMIO_MAP(mbar_phys)
        → Init contrôleur NVMe
        → register_endpoint("nvme_driver", ep=17)
        → Envoyer BLK_OP_REGISTER_DEVICE → storage_server
    [PID 16] storage_server :
        → register_endpoint("storage_server", ep=18)
        → Attendre AHCI + NVMe
        → Parser GPT
        → Prêt
```

**Note importante :** `virtio_blk` n'est PAS dans cette chaîne Ring1. Il est directement utilisé par le kernel via `GLOBAL_DISK`. Les drivers AHCI et NVMe sont des Ring1 userspace processes avec IPC vers `storage_server`. `storage_server` n'a pas de lien direct avec ExoFS kernel — il est destiné aux applications Ring3 qui voudraient accéder au stockage brut (ex: backup, formatage) sans passer par le VFS.

---

## 5. Mise à jour de `service_table.rs` [MODIFICATION]

Ajouter les nouvelles entrées dans `servers/init_server/src/service_table.rs` :

```rust
// Ajouter dans CANONICAL_SERVICES :
pub const SERVICE_COUNT: usize = 15;  // était 12

pub static AHCI_DRIVER_BIN:    &[u8] = b"/sbin/exo-ahci-driver\0";
pub static NVME_DRIVER_BIN:    &[u8] = b"/sbin/exo-nvme-driver\0";
pub static STORAGE_SERVER_BIN: &[u8] = b"/sbin/exo-storage-server\0";

const DEPS_AHCI:    &[&str] = &["ipc_router", "device_server"];
const DEPS_NVME:    &[&str] = &["ipc_router", "device_server"];
const DEPS_STORAGE: &[&str] = &["ipc_router", "device_server"];
const OPT_DEPS_STORAGE: &[&str] = &["ahci_driver", "nvme_driver"];

// Ajouter 3 ServiceMetadata à la fin du tableau CANONICAL_SERVICES :
ServiceMetadata {
    name: "ahci_driver",
    bin_path: AHCI_DRIVER_BIN,
    requires: DEPS_AHCI,
    requires_optional: NO_DEPS,
    ready_timeout_ms: 8_000,
    critical: false,  // optionnel si pas de disque SATA physique
},
ServiceMetadata {
    name: "nvme_driver",
    bin_path: NVME_DRIVER_BIN,
    requires: DEPS_NVME,
    requires_optional: NO_DEPS,
    ready_timeout_ms: 8_000,
    critical: false,
},
ServiceMetadata {
    name: "storage_server",
    bin_path: STORAGE_SERVER_BIN,
    requires: DEPS_STORAGE,
    requires_optional: OPT_DEPS_STORAGE,
    ready_timeout_ms: 10_000,
    critical: false,
},
```

---

## 6. Corrections critiques virtio_blk Cargo.toml [MODIFICATION]

Le `Cargo.toml` actuel déclare `virtio-drivers` comme dépendance mais ne l'utilise pas (le code ne l'importe pas). Il manque également les imports kernel nécessaires.

**Nouveau `drivers/storage/virtio_blk/Cargo.toml` :**
```toml
[package]
name    = "exo-virtio-blk"
version.workspace = true
edition.workspace = true

[lib]
path = "src/lib.rs"
crate-type = ["rlib"]

[dependencies]
spin  = { version = "0.9.8", default-features = false, features = ["spin_mutex"] }
log   = { version = "0.4", default-features = false }
# NE PAS déclarer virtio-drivers — on implémente directement le protocole VirtIO

[features]
default = []
kernel-link = []  # activé quand lié au kernel (accède à crate::memory::*)
```

**Note :** Quand `kernel-link` est activé, `hal.rs` utilise `crate::memory::alloc_pages()`. Quand non activé (tests standalone), il utilise `alloc::alloc::alloc_zeroed`. Le `Cargo.toml` workspace du kernel ajoute `exo-virtio-blk` avec `features = ["kernel-link"]`.

---

## 7. Contraintes et règles techniques storage

### Règles driver (Driver Framework v10)
- **DRV-ARCH-01** : Zéro logique driver en Ring0 pour AHCI/NVMe. Le kernel ne connaît que `find_virtio_blk_mmio_bar()` + `ExoVirtioBlkDevice` (exception provisoire Phase1).
- **SYS_DMA_ALLOC = 534** : Syscall pour allouer des pages DMA depuis Ring1.
- **SYS_MMIO_MAP = 535** : Syscall pour mapper une région MMIO physique dans l'espace Ring1.
- **SYS_PCI_CLAIM = 540** : Doit être appelé avant tout accès BAR.
- **do_exit() 7 steps** : Lors de la terminaison d'un driver, le kernel exécute bus_master_disable → quiescence → revoke_DMA → revoke_alloc → revoke_MMIO → revoke_IRQ → revoke_claims.

### Règles ISR (FIX-108/109)
- Les ISR AHCI et NVMe ne font PAS d'allocation.
- Ils mettent à jour un flag atomique et retournent.
- La complétion réelle se fait dans la boucle IPC via `MSG_IRQ_NOTIFY`.

### Contrainte DMA (IommuFaultQueue CAS-strong — FIX-104)
- Tout buffer DMA partagé avec un device DOIT être dans le domaine IOMMU du processus driver.
- Après `SYS_DMA_ALLOC`, le kernel ajoute automatiquement le mapping IOMMU.
- Ne jamais passer une adresse virtuelle directement à un descriptor DMA — toujours utiliser l'IOVA retourné par `SYS_DMA_ALLOC`.

### Politique d'accès concurrent
- `ExoVirtioBlkDevice.queue` est protégé par `Mutex<VirtqueueBlk>`. En mode kernel (rlib), les accès depuis l'ISR sont impossibles (ISR ne tient pas de lock). Le polling de complétion se fait depuis le callpath `fs_bridge → virtio_adapter → read_block`.
- `StorageService.devices` dans `storage_server` est modifié uniquement pendant le bootstrap (phase séquentielle). Après `bootstrapped = true`, la table est read-only. Pas de lock nécessaire en lecture.

---

## 8. Tests requis pour valider v0.2.0 Storage

| Test | Fichier | Critère |
|---|---|---|
| VirtIO-blk init séquence | `virtio_blk/tests/init.rs` | STATUS = DRIVER_OK, capacité non nulle |
| Read/Write roundtrip réel | `virtio_blk/tests/rw.rs` | Écrire 4KB, relire, comparer |
| VirtQueue free list | `virtio_blk/tests/queue.rs` | (existant) conserver + ajouter notify test |
| HAL DMA alloc | `virtio_blk/tests/hal.rs` | phys_addr aligné 4096, non nul |
| AHCI port detection | `ahci/tests/port.rs` | DET=3 + SIG=ATA sur port 0 |
| AHCI Read DMA | `ahci/tests/rw.rs` | 1 secteur LBA 0 lisible |
| NVMe controller ready | `nvme/tests/init.rs` | CSTS.RDY=1 après CC.EN |
| NVMe Identify | `nvme/tests/identify.rs` | NSZE non nul |
| GPT parse | `storage_server/tests/partition_gpt.rs` | 3 partitions reconnues |
| Cache LRU | `storage_server/tests/cache_lru.rs` | 128+1 inserts → éviction correcte |
| Storage stress | `storage_server/tests/storage_stress.rs` | 1000 RW sans perte ni corruption |
| ExoFS integration | `kernel/tests/fs_storage.rs` | open/write/read/close via ExoFS sur vrai disque |
