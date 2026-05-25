# Corrections Profondes ExoFS — Storage · Epoch · Recovery · Syscall

**Auteur :** claude iota  
**Date :** 2026-05-21  
**Référence :** `AUDIT_PROFOND_EXOFS_CLAUDE_IOTA.md`  
**Priorité :** P0 ABSOLU — ces corrections définissent si ExoFS existe réellement

---

## Découverte Majeure — Le Driver VirtIO-blk est un Ramdisk

L'audit révèle une incohérence architecturale fondamentale, plus grave que toutes les autres :

**`drivers/storage/virtio_blk/src/lib.rs` — `ExoVirtioBlkDevice` est entièrement en mémoire :**

```rust
pub struct ExoVirtioBlkDevice {
    pub base_address: usize,           // ← stocké mais JAMAIS utilisé pour l'I/O
    capacity_bytes: usize,
    block_size: usize,
    internal_storage: Mutex<BTreeMap<u64, Box<[u8]>>>, // ← RAMDISK BTREE
}

pub fn read_block(&self, block_id: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    // Lit depuis le BTreeMap en mémoire — AUCUN accès MMIO
    if let Some(block) = storage.get(&block_id) { buf.copy_from_slice(block); }
    else { buf.fill(0); }
    Ok(())
}

pub fn write_block(&self, block_id: u64, buf: &[u8]) -> Result<(), &'static str> {
    // Écrit dans le BTreeMap en mémoire — AUCUN accès MMIO
    storage.entry(block_id).or_insert_with(|| ...).copy_from_slice(buf);
    Ok(())
}

pub fn flush(&self) -> Result<(), &'static str> {
    Ok(())  // ← No-op complet — aucune barrière NVMe réelle
}
```

`virtqueue.rs` et `hal.rs` contiennent les structures VirtIO correctes (descripteurs, available/used rings, `mmio_phys_to_virt`), mais **aucun code dans `lib.rs` ne les utilise**. L'implémentation réelle du protocole VirtIO-blk — envoi de requêtes via le virtqueue, notification MMIO, attente de complétion — est **absente**.

**Conséquence :** Tout ce qui semble fonctionner avec ExoFS (lecture, écriture, GC, cache) opère sur un ramdisk volatile. Au reboot, 100% des données sont perdues, indépendamment de toute autre correction.

---

## CORR-IOTA-FS01 — Implémenter le Protocole VirtIO-blk Réel

**Fichier :** `drivers/storage/virtio_blk/src/lib.rs`

Le code existant dans `virtqueue.rs` et `hal.rs` fournit toute l'infrastructure nécessaire. Il faut relier `lib.rs` à ce scaffolding.

```rust
// ── Structure réelle avec virtqueues MMIO ────────────────────────────────
pub struct ExoVirtioBlkDevice {
    pub base_address: usize,
    capacity_bytes:   usize,
    block_size:       usize,
    /// Pointeur vers la région MMIO virtio (CommonCfg + Notify)
    mmio_virt:        usize,
    /// Virtqueue 0 : requêtes blk (READ / WRITE / FLUSH)
    queue:            Mutex<VirtqueueState>,
}

/// État d'une virtqueue (descripteurs, available, used rings)
struct VirtqueueState {
    descs:      [VirtqDesc; 128],
    avail_idx:  u16,
    used_idx:   u16,
    // Adresses physiques pour le device
    desc_phys:  u64,
    avail_phys: u64,
    used_phys:  u64,
    free_list:  DescriptorFreeList<128>,
}

impl ExoVirtioBlkDevice {
    pub fn new(base_address: usize, disk_capacity_bytes: usize) -> Self {
        let hal_virt = unsafe {
            Hal::mmio_phys_to_virt(
                PhysAddr::new(base_address as u64), 0x1000
            ).as_ptr() as usize
        };

        let mut dev = Self {
            base_address,
            capacity_bytes: disk_capacity_bytes,
            block_size: 512,  // VirtIO blk : secteurs de 512 B
            mmio_virt: hal_virt,
            queue: Mutex::new(VirtqueueState::new()),
        };
        dev.virtio_init();
        dev
    }

    /// Initialise le périphérique VirtIO selon la spec 1.2 §5.2
    fn virtio_init(&mut self) {
        unsafe {
            let base = self.mmio_virt;
            // 1. Reset device
            self.write_reg(DEVICE_STATUS_REG, 0x00);
            // 2. Acknowledge + Driver
            self.write_reg(DEVICE_STATUS_REG, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
            // 3. Lire feature bits (on accepte VIRTIO_BLK_F_FLUSH = bit 9)
            let features = self.read_reg(DEVICE_FEATURES_REG);
            let accepted = features & (VIRTIO_BLK_F_FLUSH | VIRTIO_BLK_F_BLK_SIZE);
            self.write_reg(DRIVER_FEATURES_REG, accepted);
            // 4. Features OK
            self.write_reg(DEVICE_STATUS_REG,
                STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
            // 5. Configurer la virtqueue 0
            let q = self.queue.lock();
            self.write_reg(QUEUE_SEL_REG, 0);
            self.write_reg64(QUEUE_DESC_LOW_REG, q.desc_phys);
            self.write_reg64(QUEUE_AVAIL_LOW_REG, q.avail_phys);
            self.write_reg64(QUEUE_USED_LOW_REG, q.used_phys);
            self.write_reg(QUEUE_SIZE_REG, 128);
            self.write_reg(QUEUE_ENABLE_REG, 1);
            // 6. Driver OK
            self.write_reg(DEVICE_STATUS_REG,
                STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
        }
    }

    /// Envoie une requête READ sur la virtqueue et attend la complétion.
    pub fn read_block(&self, block_id: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() != self.block_size { return Err("Buffer size mismatch"); }
        self.submit_request(VIRTIO_BLK_T_IN, block_id, buf, false)
    }

    /// Envoie une requête WRITE sur la virtqueue et attend la complétion.
    pub fn write_block(&self, block_id: u64, buf: &[u8]) -> Result<(), &'static str> {
        if buf.len() != self.block_size { return Err("Buffer size mismatch"); }
        // Safety: write ne modifie pas buf, mais l'API virtio exige un pointeur.
        let buf_mut = unsafe {
            core::slice::from_raw_parts_mut(buf.as_ptr() as *mut u8, buf.len())
        };
        self.submit_request(VIRTIO_BLK_T_OUT, block_id, buf_mut, true)
    }

    /// Envoie une requête FLUSH (VIRTQ_BLK_T_FLUSH = 4) pour garantir
    /// la persistance des écritures précédentes sur le media.
    pub fn flush(&self) -> Result<(), &'static str> {
        let mut dummy = [0u8; 512]; // Pas de données pour FLUSH
        self.submit_request(VIRTIO_BLK_T_FLUSH, 0, &mut dummy, false)
    }

    /// Soumet une requête VirtIO-blk et attend sa complétion (polling).
    fn submit_request(
        &self,
        req_type: u32,
        sector:   u64,
        data:     &mut [u8],
        is_write: bool,
    ) -> Result<(), &'static str> {
        // En-tête de requête VirtIO-blk (§5.2.6)
        let hdr = VirtioBlkReqHeader {
            req_type,
            reserved: 0,
            sector,
        };
        let mut status = [0u8; 1]; // status byte retourné par le device

        let mut q = self.queue.lock();

        // Allouer 3 descripteurs : header, data, status
        let d_hdr    = q.free_list.alloc().map_err(|_| "Queue full")?;
        let d_data   = q.free_list.alloc().map_err(|_| "Queue full")?;
        let d_status = q.free_list.alloc().map_err(|_| "Queue full")?;

        // Remplir les descripteurs
        q.descs[d_hdr as usize] = VirtqDesc {
            addr:  &hdr as *const _ as u64,
            len:   core::mem::size_of::<VirtioBlkReqHeader>() as u32,
            flags: VIRTQ_DESC_F_NEXT,
            next:  d_data,
        };
        q.descs[d_data as usize] = VirtqDesc {
            addr:  data.as_ptr() as u64,
            len:   data.len() as u32,
            flags: if is_write { VIRTQ_DESC_F_NEXT } else { VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT },
            next:  d_status,
        };
        q.descs[d_status as usize] = VirtqDesc {
            addr:  status.as_ptr() as u64,
            len:   1,
            flags: VIRTQ_DESC_F_WRITE,
            next:  0,
        };

        // Publier dans le available ring
        let avail_idx = q.avail_idx as usize;
        unsafe {
            let avail_ptr = q.avail_phys as *mut VirtqAvailHeader;
            let ring_ptr  = (avail_ptr as usize + 4 + avail_idx * 2) as *mut u16;
            core::ptr::write_volatile(ring_ptr, d_hdr);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            (*avail_ptr).idx = q.avail_idx.wrapping_add(1);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }
        q.avail_idx = q.avail_idx.wrapping_add(1);

        // Notifier le device (QUEUE_NOTIFY_REG)
        unsafe { self.write_reg(QUEUE_NOTIFY_REG, 0); }

        // Attendre la complétion (polling — no interrupts in Ring0)
        self.poll_completion(&mut *q, d_hdr)?;

        // Libérer les descripteurs
        q.free_list.free(d_hdr);
        q.free_list.free(d_data);
        q.free_list.free(d_status);

        if status[0] == VIRTIO_BLK_S_OK { Ok(()) } else { Err("VirtIO I/O error") }
    }

    fn poll_completion(&self, q: &mut VirtqueueState, head: u16) -> Result<(), &'static str> {
        let timeout = 1_000_000usize; // ~1s en polling tight
        let mut i = 0;
        loop {
            let used_idx = unsafe {
                let used_ptr = q.used_phys as *const VirtqUsedElem;
                // Lire le idx du used ring
                let idx_ptr = (q.used_phys as usize - 4) as *const u16;
                core::ptr::read_volatile(idx_ptr)
            };
            if used_idx != q.used_idx {
                q.used_idx = q.used_idx.wrapping_add(1);
                return Ok(());
            }
            i += 1;
            if i >= timeout { return Err("VirtIO timeout"); }
            core::hint::spin_loop();
        }
    }

    #[inline] unsafe fn write_reg(&self, offset: usize, val: u32) {
        core::ptr::write_volatile((self.mmio_virt + offset) as *mut u32, val);
    }
    #[inline] unsafe fn write_reg64(&self, offset: usize, val: u64) {
        core::ptr::write_volatile((self.mmio_virt + offset) as *mut u32, val as u32);
        core::ptr::write_volatile((self.mmio_virt + offset + 4) as *mut u32, (val >> 32) as u32);
    }
    #[inline] unsafe fn read_reg(&self, offset: usize) -> u32 {
        core::ptr::read_volatile((self.mmio_virt + offset) as *const u32)
    }
}

// ── Constantes VirtIO MMIO (spec 1.2 §4.2) ──────────────────────────────────
const DEVICE_FEATURES_REG: usize = 0x010;
const DRIVER_FEATURES_REG: usize = 0x020;
const QUEUE_SEL_REG:        usize = 0x030;
const QUEUE_SIZE_REG:       usize = 0x038;
const QUEUE_ENABLE_REG:     usize = 0x044;
const QUEUE_NOTIFY_REG:     usize = 0x050;
const DEVICE_STATUS_REG:    usize = 0x070;
const QUEUE_DESC_LOW_REG:   usize = 0x080;
const QUEUE_AVAIL_LOW_REG:  usize = 0x090;
const QUEUE_USED_LOW_REG:   usize = 0x0A0;

const STATUS_ACKNOWLEDGE:   u32 = 0x01;
const STATUS_DRIVER:        u32 = 0x02;
const STATUS_DRIVER_OK:     u32 = 0x04;
const STATUS_FEATURES_OK:   u32 = 0x08;

const VIRTIO_BLK_T_IN:      u32 = 0;  // READ
const VIRTIO_BLK_T_OUT:     u32 = 1;  // WRITE
const VIRTIO_BLK_T_FLUSH:   u32 = 4;  // FLUSH (nécessite VIRTIO_BLK_F_FLUSH)
const VIRTIO_BLK_S_OK:      u8  = 0;
const VIRTIO_BLK_F_FLUSH:   u32 = 1 << 9;
const VIRTIO_BLK_F_BLK_SIZE: u32 = 1 << 6;

#[repr(C)]
struct VirtioBlkReqHeader { req_type: u32, reserved: u32, sector: u64 }
```

---

## CORR-IOTA-FS02 — `do_commit()` : Relier au Protocole 3 Barrières

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`

Le `do_commit()` doit appeler `epoch::epoch_commit::commit_epoch()`. Voici le refactoring complet :

```rust
use crate::fs::exofs::epoch::epoch_commit::{commit_epoch, CommitCallbacks, CommitInput};
use crate::fs::exofs::epoch::epoch_root::EpochRootInMemory;
use crate::fs::exofs::storage::superblock_backup::SUPERBLOCK_MANAGER;

fn do_commit(args: &EpochCommitArgs) -> ExofsResult<EpochCommitResult> {
    let cur = current_epoch();
    let target = resolve_target_epoch(args, cur)?;

    // ── 1. Construire l'EpochRoot depuis les blobs modifiés ──────────────
    let entries = collect_epoch_blobs_by_metadata(target); // ← CORR-IOTA-FS04
    let root = EpochRootInMemory::from_entries(&entries)?;

    // ── 2. Écrire le payload sur disque (Phase 1 prépare la barrière) ────
    write_epoch_payload(&entries)?;

    // ── 3. Écrire l'EpochRoot sérialisé sur disque ───────────────────────
    let root_offset = SUPERBLOCK_MANAGER.allocate_epoch_root_slot(target)?;
    let root_bytes  = root.serialize()?;
    write_at(root_offset, &root_bytes)?;

    // ── 4. Appeler le protocole de commit à 3 barrières ─────────────────
    let slot_offset = SUPERBLOCK_MANAGER.next_slot_offset(target)?;
    let result = commit_epoch(CommitInput {
        root: &root,
        callbacks: CommitCallbacks {
            get_current_epoch: &|| EpochId(current_epoch()),
            advance_epoch:     &|e| SUPERBLOCK_MANAGER.advance_epoch(e),
            get_tsc:           &|| crate::arch::x86_64::tsc::read_tsc_ns(),
            write_fn:          &|data, off| write_at(off, data).map(|_| data.len()),
        },
        root_disk_offset: root_offset,
        slot_offset,
        extra_flags: EpochFlags::from_args(args),
    })?;

    // ── 5. Synchroniser CURRENT_EPOCH ────────────────────────────────────
    CURRENT_EPOCH.store(result.epoch_id.0, Ordering::Release);

    // ── 6. Marquer les blobs comme clean (écrits, pas dirty) ─────────────
    mark_epoch_blobs_clean(&entries); // ← CORR-IOTA-FS03

    Ok(EpochCommitResult {
        epoch_id: result.epoch_id.0,
        slot_offset: result.slot_offset.0,
        object_count: result.object_count,
        duration_cycles: result.duration_cycles,
    })
}

/// Écrit le payload de l'epoch sur disque via persist_blob_data_if_disk (sync).
fn write_epoch_payload(entries: &[EpochJournalEntry]) -> ExofsResult<()> {
    for entry in entries {
        let bid = BlobId(entry.blob_id);
        if let Some(data) = BLOB_CACHE.get(&bid) {
            object_store::persist_blob_data_if_disk(bid, data.as_ref(), true)?;
        }
    }
    Ok(())
}

/// Marque les blobs de l'epoch comme clean après commit réussi.
fn mark_epoch_blobs_clean(entries: &[EpochJournalEntry]) {
    for entry in entries {
        let bid = BlobId(entry.blob_id);
        let _ = BLOB_CACHE.mark_clean(&bid);
    }
}
```

---

## CORR-IOTA-FS03 — `flush_dirty_blobs()` : Flusher Réellement

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`

```rust
// AVANT (incorrecte) :
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    for entry in entries {
        BLOB_CACHE.mark_dirty(&BlobId(entry.blob_id)).ok();
    }
}

// APRÈS (CORR-IOTA-FS03) — appelé UNIQUEMENT si do_commit() n'est pas encore
// refactorisé (solution intermédiaire avant CORR-IOTA-FS02) :
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    for entry in entries {
        let bid = BlobId(entry.blob_id);
        if let Some(data) = BLOB_CACHE.get(&bid) {
            // sync=true : écriture bloquante, retourne après complétion I/O
            match object_store::persist_blob_data_if_disk(bid, data.as_ref(), true) {
                Ok(_)  => { let _ = BLOB_CACHE.mark_clean(&bid); }
                Err(e) => log::error!("[exofs] flush_dirty bid={:?} err={:?}", bid, e),
            }
        }
    }
}
```

---

## CORR-IOTA-FS04 — Epoch ID dans les Métadonnées, pas dans les Données

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs` + `objects/logical_object.rs`

La racine du problème est que `blob_epoch_id()` lit les 8 premiers octets du contenu. Il faut stocker l'epoch d'un blob dans ses métadonnées.

**Étape 1 — Ajouter `epoch_id` dans `ObjectMeta` :**

```rust
// kernel/src/fs/exofs/objects/object_meta.rs :
pub struct ObjectMeta {
    // ... champs existants ...
    /// Epoch lors duquel cet objet a été modifié pour la dernière fois.
    /// Mis à jour par object_write() et object_create().
    pub dirty_epoch: u64,
}
```

**Étape 2 — Mettre à jour `dirty_epoch` dans `object_write()` :**

```rust
// kernel/src/fs/exofs/syscall/object_write.rs :
fn write_blob(blob_id: BlobId, ...) -> ExofsResult<WriteResult> {
    // ... vérification is_immutable, quota, ACL ...

    // Après écriture réussie :
    let cur_epoch = crate::fs::exofs::syscall::epoch_commit::current_epoch();
    OBJECT_TABLE.update_dirty_epoch(&blob_id, cur_epoch);
    BLOB_CACHE.mark_dirty(&blob_id)?;
    Ok(result)
}
```

**Étape 3 — `collect_epoch_blobs_by_metadata()` :**

```rust
// kernel/src/fs/exofs/syscall/epoch_commit.rs :
fn collect_epoch_blobs_by_metadata(epoch_id: u64) -> heapless::Vec<EpochJournalEntry, 512> {
    let mut entries: heapless::Vec<EpochJournalEntry, 512> = heapless::Vec::new();
    // Itérer sur l'OBJECT_TABLE, sélectionner les objets avec dirty_epoch == epoch_id
    OBJECT_TABLE.for_each(|blob_id, meta| {
        if meta.dirty_epoch == epoch_id {
            let _ = entries.push(EpochJournalEntry {
                blob_id: blob_id.0,
                flags: meta.flags,
                ..EpochJournalEntry::default()
            });
        }
    });
    entries
}
```

---

## CORR-IOTA-FS05 — Persister le LBA Mapping sur Disque

**Fichier :** `kernel/src/fs/exofs/syscall/object_store.rs`

```rust
// ── Constantes de layout de l'index ──────────────────────────────────────────
/// LBA de début de l'index BlobId→LBA (après les superblocks + slots epoch).
/// Réservation : LBA 64 → LBA 127 (64 × 4096 = 256 KiB pour l'index).
pub const OBJECT_INDEX_START_LBA: u64 = 64;
pub const OBJECT_INDEX_LBA_COUNT: u64 = 64;
/// Magic d'identification de l'index
const OBJECT_INDEX_MAGIC: u64 = 0xE4_0B_1D_EX_0F_53_49_44; // ExoFS_ID

/// En-tête du bloc d'index
#[repr(C, packed)]
struct ObjectIndexHeader {
    magic:       u64,
    entry_count: u32,
    next_lba:    u64,   // prochain LBA libre pour allocation
    checksum:    u32,   // CRC32 du reste du bloc
    _pad:        [u8; 8],
}

/// Une entrée de l'index (24 octets)
#[repr(C, packed)]
struct ObjectIndexEntry {
    blob_id_hash: u64,   // hash 64 bits du BlobId (CollisionID)
    base_lba:     u64,   // premier LBA alloué
    block_count:  u32,   // nombre de blocs (block_size chacun)
    flags:        u32,   // immutable, deleted, etc.
}

impl ObjectStore {
    /// Sauvegarde l'index complet sur disque.
    /// Appelé après chaque `epoch_commit` réussi.
    pub fn persist_index(&self) -> ExofsResult<()> {
        use crate::fs::exofs::storage::virtio_adapter::with_global_disk;

        let inner = self.inner.lock();
        let entry_count = inner.map.len() as u32;

        // Sérialiser en page de 4096 octets (max 170 entrées par page)
        let mut page = [0u8; 4096];
        let header = ObjectIndexHeader {
            magic: OBJECT_INDEX_MAGIC,
            entry_count,
            next_lba: inner.next_lba,
            checksum: 0, // calculé après
            _pad: [0u8; 8],
        };
        // Copier header
        page[..core::mem::size_of::<ObjectIndexHeader>()]
            .copy_from_slice(unsafe {
                core::slice::from_raw_parts(
                    &header as *const _ as *const u8,
                    core::mem::size_of::<ObjectIndexHeader>(),
                )
            });
        // Copier entrées
        let entry_size  = core::mem::size_of::<ObjectIndexEntry>();
        let header_size = core::mem::size_of::<ObjectIndexHeader>();
        let mut offset  = header_size;
        for (blob_id, mapping) in inner.map.iter() {
            if offset + entry_size > 4096 { break; } // TODO : pagination multi-blocs
            let entry = ObjectIndexEntry {
                blob_id_hash: blob_id.as_u64_hash(),
                base_lba:     mapping.base_lba,
                block_count:  mapping.allocated_blocks as u32,
                flags:        0,
            };
            page[offset..offset + entry_size].copy_from_slice(unsafe {
                core::slice::from_raw_parts(
                    &entry as *const _ as *const u8,
                    entry_size,
                )
            });
            offset += entry_size;
        }
        // Écrire sur disque au LBA OBJECT_INDEX_START_LBA
        with_global_disk(|dev| dev.write_block(OBJECT_INDEX_START_LBA, &page))
            .map_err(|_| ExofsError::IoError)
    }

    /// Charge l'index depuis disque au boot.
    /// Appelé par `boot_recovery_sequence()` avant tout autre accès.
    pub fn load_index(&self) -> ExofsResult<()> {
        use crate::fs::exofs::storage::virtio_adapter::with_global_disk;

        let mut page = [0u8; 4096];
        with_global_disk(|dev| dev.read_block(OBJECT_INDEX_START_LBA, &mut page))
            .map_err(|_| ExofsError::IoError)?;

        // Vérifier le magic
        let magic = u64::from_le_bytes(page[..8].try_into().unwrap_or_default());
        if magic != OBJECT_INDEX_MAGIC {
            // Pas encore d'index — premier boot, tout est vide
            return Ok(());
        }

        let header_size = core::mem::size_of::<ObjectIndexHeader>();
        let entry_size  = core::mem::size_of::<ObjectIndexEntry>();
        let entry_count = u32::from_le_bytes(page[8..12].try_into().unwrap_or_default());
        let next_lba    = u64::from_le_bytes(page[12..20].try_into().unwrap_or_default());

        let mut inner = self.inner.lock();
        inner.next_lba = next_lba;

        let mut offset = header_size;
        let mut loaded = 0u32;
        while loaded < entry_count && offset + entry_size <= 4096 {
            let entry_bytes = &page[offset..offset + entry_size];
            let base_lba    = u64::from_le_bytes(entry_bytes[8..16].try_into().unwrap_or_default());
            let block_count = u32::from_le_bytes(entry_bytes[16..20].try_into().unwrap_or_default());
            let hash        = u64::from_le_bytes(entry_bytes[..8].try_into().unwrap_or_default());
            // Reconstruire un BlobId depuis le hash (approximation — voir TODO)
            // TODO : stocker le BlobId complet (32 octets) par entrée
            // Pour l'instant, utiliser le hash comme clé proxy
            let blob_id = BlobId::from_u64_hash(hash);
            inner.map.insert(blob_id, PersistedBlobMapping {
                base_lba,
                allocated_blocks: block_count as u64,
            });
            offset  += entry_size;
            loaded  += 1;
        }
        log::info!("[exofs] index chargé : {} blobs, next_lba={}", loaded, next_lba);
        Ok(())
    }
}
```

**Note critique sur l'entrée de 24 octets :** Stocker 8 octets de hash au lieu des 32 octets du `BlobId` complet crée des collisions potentielles. La v0.2.0 doit utiliser des entrées de **40 octets** (32 octets BlobId + 8 octets métadonnées), ce qui permet 97 entrées par bloc de 4 KiB. Pour un volume avec > 97 blobs, implémenter la pagination multi-blocs (LBA 64..127).

---

## CORR-IOTA-FS06 — `boot_recovery_sequence()` : Déstubbifier

**Fichier :** `kernel/src/fs/exofs/recovery/boot_recovery.rs`

```rust
pub fn boot_recovery_sequence(disk_size_bytes: u64) -> ExofsResult<()> {
    RECOVERY_LOG.log_boot_start();
    RECOVERY_AUDIT.record_recovery_started(EpochId(0));

    // ── Étape 1 : Charger l'index LBA depuis disque ───────────────────────
    crate::fs::exofs::syscall::object_store::OBJECT_STORE
        .load_index()
        .map_err(|e| {
            log::warn!("[exofs recovery] index illisible, démarrage à vide: {:?}", e);
            // Ne pas échouer si c'est le premier boot (index absent)
        })
        .ok();

    // ── Étape 2 : Lire et sélectionner le meilleur miroir superblock ──────
    let opts = BootRecoveryOptions {
        force_fsck:   false,
        dry_run:      false,
        max_fsck_errors: 128,
        allow_replay: true,
    };

    let recovery_result = crate::fs::exofs::storage::virtio_adapter::with_global_disk(|device| {
        let mut recovery = BootRecovery::new(device, disk_size_bytes, opts);
        recovery.run()
    });

    match recovery_result {
        Ok(r) => {
            // ── Étape 3 : Synchroniser CURRENT_EPOCH ───────────────────────
            crate::fs::exofs::syscall::epoch_commit::set_current_epoch(r.recovered_epoch.0);

            RECOVERY_AUDIT.record_recovery_completed(r.recovered_epoch, r.total_errors);
            RECOVERY_LOG.log_boot_done();

            log::info!(
                "[exofs] recovery OK — epoch={} erreurs={} dirty={}",
                r.recovered_epoch.0,
                r.total_errors,
                r.had_dirty_flag,
            );
            Ok(())
        }
        Err(ExofsError::NoDiskAvailable) => {
            // Pas de disque → démarrage en mode mémoire volatile
            log::warn!("[exofs] aucun disque disponible — mode volatile (données non persistées)");
            RECOVERY_LOG.log_boot_done();
            Ok(())
        }
        Err(e) => {
            RECOVERY_LOG.log_boot_error();
            log::error!("[exofs] recovery FAILED: {:?}", e);
            Err(ExofsError::RecoveryFailed)
        }
    }
}
```

---

## CORR-IOTA-FS07 — GC Sweeper : Guard `is_immutable()`

**Fichier :** `kernel/src/fs/exofs/gc/sweeper.rs`

```rust
fn sweep_batch(&self, batch: &[BlobId], current_epoch: EpochId) -> ExofsResult<BatchSweepResult> {
    let mut br = BatchSweepResult::default();

    for &blob_id in batch {
        // ... lecture refcount et create_epoch existants ...

        if is_epoch_pinned(create_epoch) {
            br.pinned_skipped = br.pinned_skipped.saturating_add(1);
            continue;
        }
        if rc > 0 {
            br.live_skipped = br.live_skipped.saturating_add(1);
            continue;
        }

        // ── CORR-IOTA-FS07 : ne jamais supprimer un blob immutable ────────
        if self.is_immutable_blob(&blob_id) {
            br.pinned_skipped = br.pinned_skipped.saturating_add(1);
            STORAGE_STATS.inc_gc_immutable_skipped();
            continue;
        }
        // ─────────────────────────────────────────────────────────────────

        BLOB_REFCOUNT.queue_zero(&blob_id, current_epoch)?;
        br.queued_count = br.queued_count.saturating_add(1);
    }

    Ok(br)
}

fn is_immutable_blob(&self, blob_id: &BlobId) -> bool {
    // Chercher l'objet dans l'OBJECT_TABLE et vérifier son flag immutable
    crate::fs::exofs::objects::logical_object::is_immutable_by_blob_id(blob_id)
}
```

---

## CORR-IOTA-FS08 — Quota Appliqué sur Write et Create

Voir `CORRECTIONS_BLOCS_0_2_11_CLAUDE_IOTA.md` CORR-IOTA-12 pour le détail. Résumé :

```rust
// object_write.rs — avant l'écriture :
crate::fs::exofs::syscall::quota_query::check_quota(owner_uid, data_len as u64, 0)?;

// object_create.rs — avant la création :
crate::fs::exofs::syscall::quota_query::check_quota(owner_uid, initial_size, 1)?;
```

---

## Ordre de Correction Strict

```
1. CORR-IOTA-FS01   Driver VirtIO MMIO réel
   ↓
2. CORR-IOTA-FS05   Index LBA persisté (persist_index + load_index)
   ↓
3. CORR-IOTA-FS04   Epoch ID dans métadonnées (dirty_epoch dans ObjectMeta)
   ↓
4. CORR-IOTA-FS02   do_commit() → commit_epoch() 3 barrières
5. CORR-IOTA-FS03   flush_dirty_blobs() — persistance synchrone
   ↓
6. CORR-IOTA-FS06   boot_recovery_sequence() déstubbifié
   ↓
7. CORR-IOTA-FS07   GC respecte is_immutable()
8. CORR-IOTA-FS08   Quota sur write/create
9. FS-11             incompat_flags vérifié au montage
```

---

## Tests de Validation ExoFS Post-Correction

```bash
# ── Test 1 : I/O MMIO réelle ──────────────────────────────────────────────
# Vérifier que les compteurs MMIO augmentent
exosh> exofs_stats | grep -E "reads|writes|flushes"
# reads=N writes=N flushes=N  (N > 0 après quelques opérations)

# ── Test 2 : Persistance de base ─────────────────────────────────────────
echo "claude_iota_persistence_test" > /data/probe.txt
sync
reboot
cat /data/probe.txt   # DOIT afficher "claude_iota_persistence_test"

# ── Test 3 : Epoch correcte après reboot ─────────────────────────────────
exosh> exofs_stats | grep epoch
# [ExoFS] epoch=47 (chargé depuis superblock, pas depuis CURRENT_EPOCH=1)

# ── Test 4 : Index LBA rechargé ──────────────────────────────────────────
echo "file_a" > /data/file_a.txt
echo "file_b" > /data/file_b.txt
reboot
exosh> ls /data/
# file_a.txt  file_b.txt  (les deux fichiers visibles)

# ── Test 5 : GC ne supprime pas les immutables ───────────────────────────
exofs_setimmutable /audit/entry.log
exofs_gc_run --force
cat /audit/entry.log    # DOIT toujours être lisible

# ── Test 6 : 3 barrières NVMe générées par commit ───────────────────────
exosh> exofs_barrier_stats
# barriers_data=N  barriers_root=N  barriers_record=N  (N > 0 et = après commit)
# unhook_flush_count=0  (le hook VirtIO est bien enregistré)

# ── Test 7 : Quota sur écriture ───────────────────────────────────────────
exofs_quota_set uid=1000 max_bytes=512K
dd if=/dev/zero of=/data/bigfile bs=1K count=600
# ENOSPC après ~512K
```

---

*claude iota — CORRECTIONS_EXOFS_PROFONDES_CLAUDE_IOTA.md — 2026-05-21*
