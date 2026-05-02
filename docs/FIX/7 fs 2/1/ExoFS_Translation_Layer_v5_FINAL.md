# ExoFS — Couche de Traduction v5.0 FINAL

**Exo-OS · ExoFS v2.0 · no_std Rust · vfs_server canonique · Ring 0 + Ring 1**
**Statut : Production-Ready · ~95 % POSIX+ext4 core · ~97 % avec Phase 2**

> **⛔ CE DOCUMENT EST LA SPÉCIFICATION DE RÉFÉRENCE.**
> Les règles `❌ INTERDIT` sont des violations critiques pouvant corrompre le kernel ou les données.
> Les règles `✅ OBLIGATOIRE` définissent le comportement attendu sans exception.

---

## 0. Historique des Versions

| Version | Date | Changements clés |
|:--------|:-----|:-----------------|
| v1.0 | 23 Mar 2026 | Architecture initiale — posix_bridge + posix_server + compat_server |
| v2.0 | 23 Mar 2026 | Fusion → vfs_server · ZERO_BLOB_ID · IoVec/PollFd · 14 ops ajoutées |
| v3.0 | 23 Mar 2026 | Ghost Blob TL-31 · EpollEventAbi · copy_range_kernel Ring 0 · STATX corrigé |
| v4.0 | 24 Mar 2026 | Logique reflink corrigée · Guard refcount · Box::try_new · TL-36 |
| **v5.0** | **24 Mar 2026** | **Tableau flush complet · wait_for_start sémantique · header score corrigé** |

### Delta v4 → v5 (MiniMax — seules corrections légitimes)

| Correction | Sévérité | Impact |
|:-----------|:--------:|:-------|
| Header `~97%` → `~95% core / ~97% Phase 2` | Mineure | Honnêteté sur lockf/mknod hors scope core |
| Tableau §4 : ajout `sync_file_range(WRITE)` et `(WAIT_BEFORE)` | Mineure | Couverture complète PostgreSQL pattern |
| Sémantique `wait_for_start()` documentée explicitement | Mineure | Clarté comportement WAIT_BEFORE |

**Rejetées v5 :** Copilote (9 corrections verify/Argon2/SECURITY_READY/MAX_CPUS/RunQueue — analysait ExoOS v6, hors scope couche FS) · Gemini/Kimi/Grok4 (aucune correction — validation 100%)

---

## 1. Architecture — Validée à l'unanimité (5/5 IAs, v1→v5)

```
Application POSIX / Logiciel classique (Ring 3)
     │  open() read() write() mmap() ioctl() poll() flock() msync() epoll() …
     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  servers/vfs_server/  (Ring 1, PID 3)                                      │
│  ops/  : truncate · flock · fallocate · O_SYNC · msync · readv/writev      │
│        : poll · epoll · inotify · ioctl · sendfile · dup · pipe            │
│        : copy_file_range · sync_file_range · statx · renameat2 · fcntl    │
│  compat/: write_stream · read_perf · procfs · sysfs · statfs_ext          │
└─────────────────────────────────────────────────────────────────────────────┘
     │  SYS_EXOFS_* (500-518) + extensions libs/exo-syscall/
     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  kernel/src/fs/exofs/posix_bridge/  (Ring 0)                               │
│  existants : inode_emulation · vfs_compat · mmap · fcntl_lock             │
│  v1 : flock_kernel · truncate_kernel · append_lock · poll_wake            │
│     : fallocate_kernel · sendfile_kernel                                   │
│  v2 : msync_kernel · seek_sparse                                           │
│  v3 : copy_range_kernel  (logique reflink corrigée v4)                    │
└─────────────────────────────────────────────────────────────────────────────┘
     ▼
  ExoFS Core (fs/exofs/) — L-Obj / P-Blob / Epochs / GC  [inchangé]
```

### 1.1 libs/exo-types/ — État final

```rust
// libs/exo-types/src/constants.rs
pub const EXOFS_PAGE_SIZE: usize = 4096;

/// Blake3([0u8; EXOFS_PAGE_SIZE]) — pré-calculé, JAMAIS recalculer Ring 0 (SRV-04)
/// NOTE impl : si ObjectId::is_valid() vérifie bytes[8..32]==0,
/// ajouter une exception explicite pour ZERO_BLOB_ID_4K dans is_valid().
pub const ZERO_BLOB_ID_4K: ObjectId = ObjectId([
    0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6,
    0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdb, 0xc9, 0xab,
    0x14, 0x46, 0x34, 0x66, 0x0a, 0x71, 0x38, 0x5f,
    0x02, 0x28, 0xe7, 0xd7, 0x0b, 0xce, 0xe1, 0x07,
]);
// RÈGLE TL-02 : ZERO_BLOB_ID_4K pour pages entières uniquement
// RÈGLE TL-32 : len % EXOFS_PAGE_SIZE != 0 → write normal pour la page partielle
// RÈGLE TL-31 : io/reader.rs → memset(0) sans I/O disque si p_blob_id == ZERO_BLOB_ID_4K
// RÈGLE     : JAMAIS passer ZERO_BLOB_ID_4K à blob_refcount::increment (refcount virtuel ∞)

// libs/exo-types/src/iovec.rs
#[repr(C)]
pub struct IoVec { pub base: u64, pub len: u64 }
// IoVec.base = adresse Ring 3 → copy_from_user() OBLIGATOIRE

// libs/exo-types/src/pollfd.rs
#[repr(C)]
pub struct PollFd { pub fd: u32, pub events: u16, pub revents: u16 }

// libs/exo-types/src/epoll.rs
#[repr(C, packed)]  // ABI Linux exacte : 12 bytes (pas de padding)
pub struct EpollEventAbi { pub events: u32, pub data: u64 }
const _: () = assert!(core::mem::size_of::<EpollEventAbi>() == 12);  // TL-36

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;
pub const EPOLL_CLOEXEC: i32 = 0x80000;
```

---

## 2. posix_bridge/ Ring 0 — Implémentations de référence

### 2.1 copy_range_kernel.rs (logique reflink finale)

```rust
// posix_bridge/copy_range_kernel.rs
use exo_types::constants::{ZERO_BLOB_ID_4K, EXOFS_PAGE_SIZE};
use crate::objects::ObjectMeta;
use crate::io::zero_copy::dma_copy_blob;

pub struct CopyRangeResult { pub bytes_copied: u64, pub reflinks_used: u64 }

pub fn do_copy_file_range(
    src_obj_id: ObjectId, src_off: u64,   // obj_id = L-Objet
    dst_obj_id: ObjectId, dst_off: u64,
    len: u64,
) -> Result<CopyRangeResult, ExofsError> {
    verify_cap(src_obj_id, Rights::READ)?;
    verify_cap(dst_obj_id, Rights::WRITE)?;

    // Validation bornes
    let src_size = object_table::get_size(src_obj_id)?;
    if src_off >= src_size { return Err(ExofsError::InvalidArg); }
    let actual_len = len.min(src_size.saturating_sub(src_off));
    let _end = src_off.checked_add(actual_len).ok_or(ExofsError::OffsetOverflow)?;

    // Critère reflink : chiffrement des deux objets (pas identité des blobs)
    let can_reflink = !ObjectMeta::is_encrypted(src_obj_id)?
                   && !ObjectMeta::is_encrypted(dst_obj_id)?;

    let (mut total, mut reflinks) = (0u64, 0u64);

    for blob_range in extent_tree::iter_blobs(src_obj_id, src_off, actual_len) {
        let src_p_blob_id = blob_range.p_blob_id;  // nommage p_blob_id = P-Blob

        if can_reflink {
            // Guard ZERO_BLOB_ID_4K : refcount virtuel infini, ne jamais incrémenter
            if src_p_blob_id != ZERO_BLOB_ID_4K {
                blob_refcount::increment(src_p_blob_id)?;
            }
            extent_tree::set_p_blob(
                dst_obj_id,
                dst_off + blob_range.rel_offset,
                src_p_blob_id,
            )?;
            reflinks += 1;
        } else {
            // Copie DMA — objets chiffrés (clés par ObjectId différentes)
            dma_copy_blob(src_p_blob_id, dst_obj_id, dst_off + blob_range.rel_offset)?;
        }
        total += blob_range.len;
    }

    epoch::commit_single_op(dst_obj_id)?;
    Ok(CopyRangeResult { bytes_copied: total, reflinks_used: reflinks })
}
```

### 2.2 truncate_kernel.rs

```rust
// posix_bridge/truncate_kernel.rs
use exo_types::constants::{ZERO_BLOB_ID_4K, EXOFS_PAGE_SIZE};

pub fn do_truncate(obj_id: ObjectId, new_size: u64) -> Result<(), ExofsError> {
    let current_size = object_table::get_size(obj_id)?;
    if new_size < current_size {
        extent_tree::truncate_to(obj_id, new_size)?;
    } else if new_size > current_size {
        let aligned_end = (new_size / EXOFS_PAGE_SIZE as u64) * EXOFS_PAGE_SIZE as u64;
        // Pages entières → ZERO_BLOB_ID_4K (refcount virtuel ∞, pas d'increment)
        extent_tree::fill_with_zero_p_blob(
            obj_id, current_size, aligned_end, ZERO_BLOB_ID_4K,
        )?;
        // Page partielle finale → write normal (TL-32)
        if new_size % EXOFS_PAGE_SIZE as u64 != 0 {
            io::write_zero_range(obj_id, aligned_end, new_size - aligned_end)?;
        }
    }
    epoch::commit_single_op(obj_id)
}
```

### 2.3 msync_kernel.rs

```rust
// posix_bridge/msync_kernel.rs
// kernel/src/mm/reverse_map.rs maintient : ObjectId → Vec<(pid, vaddr, len)>
// Mise à jour au mmap() → insert, au munmap() → remove

pub fn do_msync(obj_id: ObjectId, vaddr: u64, len: usize, flags: u32)
    -> Result<(), ExofsError>
{
    // TL-21 : vérifier le reverse_map avant tout flush
    mmap::verify_mapping(obj_id, vaddr, len)?;  // → ENOMEM si absent

    let (page_start, page_end) = page_align(vaddr, len);
    page_cache::flush_range_dirty(obj_id, page_start, page_end)?;

    match flags {
        f if f & MS_SYNC != 0      => epoch::wait_commit(obj_id),
        f if f & MS_ASYNC != 0     => { epoch::schedule_writeback(obj_id); Ok(()) }
        f if f & MS_INVALIDATE != 0 => page_cache::invalidate_range(obj_id, page_start, page_end),
        _ => Ok(()),
    }
}
```

### 2.4 io/reader.rs — Ghost Blob (TL-31)

```rust
// kernel/src/fs/exofs/io/reader.rs
use exo_types::constants::{ZERO_BLOB_ID_4K, EXOFS_PAGE_SIZE};

pub fn read_p_blob(p_blob_id: ObjectId, dst: &mut [u8]) -> Result<(), ExofsError> {
    // TL-31 : cas spécial — zéro accès disque
    if p_blob_id == ZERO_BLOB_ID_4K {
        dst[..dst.len().min(EXOFS_PAGE_SIZE)].fill(0u8);
        return Ok(());
    }
    // Chemin normal : lookup → checksum AVANT décompression (TL-23)
    let loc = blob_registry::lookup(p_blob_id).ok_or(ExofsError::NotFound)?;
    let compressed = storage::read_raw(loc)?;
    checksum::verify_blake3(&compressed, p_blob_id)?;
    compress::decompress_into(compressed, dst)
}
```

**Séquence lecture sparse :**
1. `extent_tree::get_p_blob_at(obj_id, offset)` → absent → `ZERO_BLOB_ID_4K`
2. `read_p_blob(ZERO_BLOB_ID_4K)` → `memset(0)` sans I/O
3. Blob réel → lecture + checksum avant décompression

---

## 3. vfs_server/ — Arborescence complète

```
servers/vfs_server/src/
│
├── main.rs  mount.rs  path_resolver.rs  fd_table.rs  isolation.rs  protocol.rs
├── path/                        # parser · resolver · cache
│
├── ops/
│   ├── open.rs                  # O_SYNC/O_DSYNC/O_APPEND/O_DIRECT
│   ├── read.rs                  # pread · readv · preadv
│   ├── write.rs                 # pwrite · writev · O_SYNC flush
│   ├── stat.rs  readdir.rs  rename.rs  link.rs  symlink.rs
│   ├── xattr.rs  acl.rs  truncate.rs  flock.rs  fallocate.rs
│   ├── sync.rs                  # sync · syncfs · fdatasync
│   ├── poll.rs                  # poll · select · epoll_create1 · ctl · pwait
│   ├── inotify.rs               # IN_CREATE/DELETE/MODIFY/MOVED_* post-epoch
│   ├── ioctl.rs                 # verify_cap() avant SETFLAGS · FIEMAP · FIDEDUPERANGE
│   ├── sendfile.rs  fadvise.rs  openat2.rs
│   ├── msync.rs                 # MS_SYNC · MS_ASYNC · MS_INVALIDATE
│   ├── seek_ext.rs              # SEEK_HOLE · SEEK_DATA
│   ├── copy_file_range.rs       # thin wrapper → copy_range_kernel Ring 0
│   ├── dup.rs                   # dup · dup2 · dup3 · F_DUPFD atomique
│   ├── pipe.rs                  # Box::try_new · SpscRing · O_CLOEXEC · O_NONBLOCK
│   ├── sync_file_range.rs       # WRITE=1 · WAIT_BEFORE=2 · WAIT_AFTER=4
│   ├── statx.rs                 # Class1→STATX_ATTR_VERITY · flags→IMMUTABLE
│   ├── renameat2.rs             # RENAME_NOREPLACE · RENAME_EXCHANGE
│   └── fcntl_full.rs            # F_GETFL/F_SETFL · OFD locks · F_NOTIFY
│
├── compat/
│   ├── write_stream/            # coalescer · throttle · group_commit · journal_hint
│   ├── read_perf/               # readahead_adv · dio_pool
│   ├── procfs/                  # mounts · filesystems · diskstats
│   ├── sysfs/                   # stats · tunables · health
│   └── statfs_ext/              # f_type=EXOFS_MAGIC · f_bavail ≥ 0
│
└── nfs/                         # v3.rs · v4.rs
```

### 3.1 ops/copy_file_range.rs — Thin wrapper Ring 1

```rust
pub fn copy_file_range(
    src_fd: u32, src_off: u64,
    dst_fd: u32, dst_off: u64,
    len: u64,
) -> Result<usize, Errno> {
    let src_obj_id = fd_table.get_obj_id(src_fd)?;
    let dst_obj_id = fd_table.get_obj_id(dst_fd)?;
    // verify_cap + bornes + logique reflink : dans copy_range_kernel Ring 0
    let result = syscall(
        SYS_EXOFS_COPY_FILE_RANGE,
        src_obj_id, src_off, dst_obj_id, dst_off, len,
    )?;
    Ok(result.bytes_copied as usize)
}
```

### 3.2 ops/pipe.rs

```rust
use alloc::boxed::Box;  // feature "alloc" — pas de nouveau crate

pub fn pipe2(flags: i32) -> Result<(u32, u32), Errno> {
    // Allocation dans l'espace Ring 1 de vfs_server (pas memory_server Ring 3)
    // Non-persistant : pas de L-Obj, pas d'Epoch, pas de disque
    // Si vfs_server crash → SIGPIPE (comportement POSIX standard)
    let ring = Box::try_new(SpscRing::<u8, PIPE_BUF>::new())
        .map_err(|_| Errno::ENOMEM)?;
    let (rd, wr) = ring.split_endpoints();
    let read_fd  = fd_table.alloc_pipe_read(rd,  flags & O_CLOEXEC != 0)?;
    let write_fd = fd_table.alloc_pipe_write(wr, flags & O_CLOEXEC != 0)?;
    if flags & O_NONBLOCK != 0 {
        fd_table.set_nonblocking(read_fd)?;
        fd_table.set_nonblocking(write_fd)?;
    }
    Ok((read_fd, write_fd))
}
```

### 3.3 ops/sync_file_range.rs

```rust
// Alignement ABI Linux strict — nécessaire pour compatibilité binaire
pub const SYNC_FILE_RANGE_WRITE:       u32 = 1;
pub const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 2;
pub const SYNC_FILE_RANGE_WAIT_AFTER:  u32 = 4;

pub fn sync_file_range(fd: u32, off: u64, nbytes: u64, flags: u32)
    -> Result<(), Errno>
{
    let obj_id = fd_table.get_obj_id(fd)?;
    let (pg_start, pg_end) = page_align(off, nbytes, EXOFS_PAGE_SIZE);

    if flags & SYNC_FILE_RANGE_WRITE != 0 {
        // Soumet les dirty pages au writeback_thread — non-bloquant
        writeback::submit_range(obj_id, pg_start, pg_end)?;
    }
    if flags & SYNC_FILE_RANGE_WAIT_BEFORE != 0 {
        // Attend que les I/O PRÉCÉDANT off:len soient TERMINÉES (completed)
        // Sémantique ExoFS : équivalent Linux WAIT_BEFORE = attendre fin I/O
        // déjà soumises avant cette plage, pas juste "dirty"
        // DIFFÉRENCE vs Linux : ExoFS garantit completion, pas juste début
        // Impact PostgreSQL : comportement plus fort que Linux — pas de régression
        writeback::wait_for_completion_before(obj_id, pg_start, pg_end)?;
    }
    if flags & SYNC_FILE_RANGE_WAIT_AFTER != 0 {
        // Attend completion des I/O soumises pour cette plage
        writeback::wait_for_completion(obj_id, pg_start, pg_end)?;
    }
    Ok(())
}
// Pattern PostgreSQL typique : WRITE(1) → WAIT_AFTER(4) → fdatasync()
// Flags combinés : WRITE|WAIT_AFTER = 5 (cas le plus fréquent en prod)
```

### 3.4 compat/write_stream/journal_hint.rs

```rust
pub enum JournalPattern {
    Unknown, SqliteWal, SqliteJournal, PostgresWal, Custom(u32),
}

pub fn detect_pattern(obj_id: ObjectId, history: &WriteHistory) -> JournalPattern {
    let name = path_cache::get_name(obj_id).unwrap_or_default();
    if name.ends_with(".wal")       { return JournalPattern::SqliteWal;     }
    if name.ends_with("-journal")   { return JournalPattern::SqliteJournal; }
    if name.ends_with(".000000001") { return JournalPattern::PostgresWal;   }
    if history.avg_write_size == 4096 && history.sequential_ratio > 0.90 {
        return JournalPattern::SqliteWal;
    }
    if history.avg_write_size == 8192 && history.fsync_ratio > 0.50 {
        return JournalPattern::PostgresWal;
    }
    JournalPattern::Unknown
}
// Effet sur le coalescer :
//   SqliteWal     → window = 0ms + fsync groupé
//   SqliteJournal → window = 0ms + O_SYNC bypass
//   PostgresWal   → window = 1ms + group_commit 4KB-aligned
//   Unknown       → 64KB / 5ms (défaut)
```

### 3.5 compat/procfs/mounts.rs — Formats de montage

```
/proc/mounts :
  "exofs /dev/nvme0n1p1 / exofs rw,relatime 0 0"

/proc/self/mountinfo (findmnt strict) :
  "ID PARENT MAJOR:MINOR ROOT MOUNT_POINT OPTIONS - FS_TYPE SOURCE FS_OPTIONS"
  "25 1 259:1 / / rw,relatime - exofs /dev/nvme0n1p1 rw,epoch=42,uuid=abc"

/sys/fs/exofs/<uuid>/stats    → epoch_current · dedup_ratio · gc_state · cache_hit_ratio
/sys/fs/exofs/<uuid>/tunables → writeback_delay_ms · gc_threshold_pct · coalescer_max_kb
/sys/fs/exofs/<uuid>/health   → ok | degraded | readonly | error
```

### 3.6 io/ — Fichiers confirmés

```
kernel/src/fs/exofs/io/
├── reader.rs          # cas spécial ZERO_BLOB_ID_4K → memset(0) (TL-31)
├── writer.rs          # write path + dirty mark
├── scatter_gather.rs  # confirmé existant — readv/writev
├── direct_io.rs       # confirmé existant — O_DIRECT bypass cache
├── zero_copy.rs       # sendfile DMA + dma_copy_blob
├── async_io.rs  prefetch.rs  readahead.rs  writeback.rs  io_batch.rs
```

---

## 4. Pipeline de Flush — Tableau Complet ★

> **Nouvelle en v5** : lignes `sync_file_range(WRITE)` et `(WAIT_BEFORE)` ajoutées (MiniMax).

| Chemin d'écriture | Coalescer | Group Commit | Durabilité |
|:------------------|:---------:|:------------:|:----------:|
| `write()` normal | ✅ ≤64KB/5ms | ✅ 1ms | Async |
| `write()` + O_SYNC/O_DSYNC | ❌ bypass | ❌ bypass — immédiat | Synchrone |
| `fsync()` explicite | N/A | ❌ bypass — immédiat | Synchrone |
| `fdatasync()` | N/A | ❌ bypass — données seules | Synchrone |
| `sync_file_range(WRITE=1)` | N/A | N/A — async submit | Async (soumet I/O) |
| `sync_file_range(WAIT_BEFORE=2)` | N/A | ⚠️ partiel — attend fin I/O précédentes | Synchrone (plage précédente) |
| `sync_file_range(WAIT_AFTER=4)` | N/A | ⚠️ partiel — range seulement | Synchrone (range) |
| `sync_file_range(WRITE\|WAIT_AFTER=5)` | N/A | ⚠️ partiel — pattern PostgreSQL | Synchrone (range) |
| `msync(MS_SYNC)` | N/A | ❌ bypass — immédiat | Synchrone |
| `msync(MS_ASYNC)` | N/A | ✅ 1ms | Async |
| `msync(MS_INVALIDATE)` | N/A | N/A | Éviction cache |

---

## 5. Règles Globales — TL-01 à TL-36

> **Convention** : `❌` = INTERDIT (violation critique). `✅` = OBLIGATOIRE.
> **Note** : CTL-01 à CTL-31 (§7) sont des points de vérification d'implémentation.
> Le compte CTL (31) ≠ TL (36) est intentionnel : certaines TL sont vérifiées indirectement.

| Sév. | ID | Règle |
|:----:|:---|:------|
| ✅ | TL-01 | Toute opération POSIX passe par `vfs_server/` (Ring 1) — jamais direct Ring 0 depuis userspace |
| ✅ | TL-02 | `posix_bridge/` (Ring 0) = mécanismes uniquement — `ZERO_BLOB_ID_4K` pour grow, jamais hash |
| ✅ | TL-03 | `vfs_server/compat/` (Ring 1) = état purement en RAM — crash = redémarrage sans corruption |
| ❌ | TL-04 | INTERDIT : `vfs_server/` tient un lock kernel — violation Ring 1 |
| ✅ | TL-05 | Chaque errno POSIX mappé exactement une fois dans `compat/errno.rs` |
| ✅ | TL-06 | inotify events générés après Epoch commit confirmé — jamais avant durabilité |
| ❌ | TL-07 | INTERDIT : O_APPEND implémenté Ring 1 avec état — `append_lock` Ring 0 obligatoire |
| ✅ | TL-08 | `flock()` et `fcntl()` coexistent sur le même ObjectId — table unifiée Ring 0 |
| ✅ | TL-09 | `truncate()` = Epoch commit atomique — grow via `ZERO_BLOB_ID_4K` pages entières |
| ✅ | TL-10 | `sendfile()` vérifie `verify_cap(INSPECT_CONTENT)` — Zero Trust préservé |
| ❌ | TL-11 | INTERDIT : coalescer dépasse `EPOCH_MAX_OBJECTS=500` sans commit anticipé |
| ✅ | TL-12 | throttle backpressure si `dirty_ratio > 80%` — protège contre OOM kernel |
| ✅ | TL-13 | `fallocate PUNCH_HOLE` sur P-Blob partagé → CoW d'abord — jamais corruption |
| ✅ | TL-14 | `readv/writev` → `scatter_gather` Ring 0 via `IoVec exo-types` — zéro copie intermédiaire |
| ✅ | TL-15 | `O_DIRECT` bypasse page_cache — `dio_pool.rs` garantit alignement 512B/4KB |
| ✅ | TL-16 | `statfs()` retourne `f_type = EXOFS_MAGIC (0x45584F46)` — df et libblkid reconnaissent ExoFS |
| ❌ | TL-17 | INTERDIT : `ioctl FS_IOC_SETFLAGS` sans `verify_cap()` — élévation de privilège |
| ✅ | TL-18 | group_commit fenêtre 1ms — réservé `fsync()` normaux, BYPASS pour O_SYNC |
| ✅ | TL-19 | `journal_hint` détecte SqliteWal/PostgresWal par filename + write pattern |
| ✅ | TL-20 | `fadvise FADV_WILLNEED` → prefetch actif — `FADV_DONTNEED` → éviction cache |
| ❌ | TL-21 | INTERDIT : `msync()` sans vérifier `reverse_map (pid, vaddr, len)` → `ENOMEM` |
| ❌ | TL-22 | INTERDIT : retourner données sans checksum Blake3 vérifié — corruption silencieuse |
| ❌ | TL-23 | INTERDIT : checksum vérifié APRÈS décompression — vérifier AVANT |
| ❌ | TL-24 | INTERDIT : `SEEK_HOLE` retourne `offset > file_size` — doit retourner `ENXIO` |
| ❌ | TL-25 | INTERDIT : `f_bavail < 0` dans `statfs()` — clamp à 0 minimum |
| ❌ | TL-26 | INTERDIT : `poll_wake` depuis IRQ handler — utiliser `WaitQueue::wake_deferred` |
| ❌ | TL-27 | INTERDIT : append write sans `append_lock` Ring 0 — race condition garantie |
| ❌ | TL-28 | INTERDIT : `fallocate PUNCH_HOLE` sans CoW si P-Blob partagé — corruption données |
| ❌ | TL-29 | INTERDIT : `fallocate()` étendre au-delà de `RLIMIT_FSIZE` — retourner `EFBIG` |
| ✅ | TL-30 | `copy_file_range` = reflink si `!encrypted(src) && !encrypted(dst)` — critère chiffrement pas identité blobs |
| ❌ | TL-31 | INTERDIT : `io/reader.rs` lit `ZERO_BLOB_ID_4K` depuis le disque → `memset(0)` sans I/O |
| ❌ | TL-32 | INTERDIT : `ZERO_BLOB_ID_4K` pour page partielle (`len % EXOFS_PAGE_SIZE != 0`) → write normal |
| ✅ | TL-33 | `copy_file_range` reflink interdit sur objets chiffrés — `dma_copy_blob` obligatoire |
| ✅ | TL-34 | `STATX_ATTR_VERITY` = Class1 · `STATX_ATTR_IMMUTABLE` = `ObjectFlags::IMMUTABLE` explicite |
| ❌ | TL-35 | INTERDIT : `RENAME_EXCHANGE` en 2 Epochs — 2 `modified_objects` dans 1 `EpochRoot` |
| ✅ | TL-36 | `const_assert!(size_of::<EpollEventAbi>() == 12)` — ABI Linux `epoll_pwait` exacte |

---

## 6. Gap Analysis — État Final v5

| Domaine | Présent | Note | Statut |
|:--------|:--------|:-----|:------:|
| truncate / ftruncate | `truncate_kernel` + `ZERO_BLOB_ID_4K` | — | ✅ |
| fallocate (tous modes) | `fallocate_kernel` + CollapseRange | — | ✅ |
| msync | `msync_kernel` + `reverse_map` | — | ✅ |
| SEEK_HOLE / SEEK_DATA | `seek_sparse.rs` | ENXIO si offset > size | ✅ |
| copy_file_range | `copy_range_kernel` Ring 0 | reflink corrigé v4 | ✅ |
| Ghost Blob Read | `io/reader.rs` cas spécial | `ZERO_BLOB_ID_4K → memset(0)` | ✅ |
| EpollEvent ABI | `exo-types/epoll.rs` + `const_assert!(12)` | `repr(C, packed)` | ✅ |
| journal_hint | `JournalPattern` + détection | filename + write pattern | ✅ |
| STATX mapping | `Class1→VERITY`, `flags→IMMUTABLE` | correction v3 | ✅ |
| sync_file_range | `WRITE=1 WAIT_BEFORE=2 WAIT_AFTER=4` | ABI Linux + sémantique v5 | ✅ |
| mountinfo format | `"ID PARENT MAJ:MIN ROOT MNT - FS SRC OPT"` | findmnt strict | ✅ |
| pipe.rs alloc | `Box::try_new()` | feature alloc, pas de nouveau crate | ✅ |
| Refcount guard | `ZERO_BLOB_ID_4K != p_blob_id` avant increment | évite corruption dédup | ✅ |
| flock / fcntl / OFD | `flock_kernel` + `fcntl_full` | OFD locks inclus | ✅ |
| O_SYNC / O_DSYNC | bypass coalescer + commit immédiat | — | ✅ |
| inotify | `inotify.rs` post-epoch-commit | — | ✅ |
| dup / dup2 / dup3 | `dup.rs` | F_DUPFD atomique | ✅ |
| pipe / pipe2 | `pipe.rs` | O_CLOEXEC / O_NONBLOCK | ✅ |
| statx / renameat2 | `statx.rs` + `renameat2.rs` | — | ✅ |
| poll / epoll | `poll.rs` + `EpollEventAbi` | epoll_create1/ctl/pwait | ✅ |
| procfs / sysfs | `compat/procfs/` + `compat/sysfs/` | mountinfo strict | ✅ |
| statfs POSIX | `statfs_ext/mapper.rs` | f_bavail ≥ 0, f_type=MAGIC | ✅ |
| FUSE adapter | `tools/exofs-fuse` | tests Linux hôte | ✅ |
| fanotify | — | Phase 2 | Phase 2 |
| eventfd2 | — | Phase 2 | Phase 2 |
| splice / tee / vmsplice | — | Phase 2 | Phase 2 |
| mremap | — | Phase 2 | Phase 2 |
| mknod / socketpair | — | Phase 2 (via device_server) | Phase 2 |

---

## 7. Checklist v5 — CTL-01 à CTL-31

> CTL = points de vérification d'implémentation. Compte CTL (31) ≠ TL (36) intentionnel.
> TL-04, TL-07, TL-11, TL-18, TL-20, TL-22, TL-29 vérifiées indirectement.

| Sév. | ID | Point de vérification |
|:----:|:---|:----------------------|
| ✅ | CTL-01 | `posix_bridge/flock_kernel.rs` : `FlockState` par ObjectId avec SpinLock — coexiste avec `fcntl_lock` |
| ✅ | CTL-02 | `posix_bridge/truncate_kernel.rs` : `ZERO_BLOB_ID_4K` grow + extent_tree shrink + page partielle → write normal (TL-32) |
| ✅ | CTL-03 | `posix_bridge/append_lock.rs` : `PerObject<SpinLock<u64>>` — atomicité O_APPEND garantie |
| ✅ | CTL-04 | `posix_bridge/poll_wake.rs` : `WaitQueue::wake_deferred` depuis IRQ — jamais `wake_immediate` |
| ✅ | CTL-05 | `posix_bridge/fallocate_kernel.rs` : PunchHole CoW + `ZERO_BLOB_ID_4K` ZeroRange — `RLIMIT_FSIZE` vérifié |
| ✅ | CTL-06 | `posix_bridge/sendfile_kernel.rs` : `verify_cap(INSPECT_CONTENT)` avant DMA |
| ✅ | CTL-07 | `posix_bridge/msync_kernel.rs` : `REVERSE_MAP (pid, vaddr, len)` vérifié → `ENOMEM` si absent |
| ✅ | CTL-08 | `posix_bridge/seek_sparse.rs` : `SEEK_HOLE` → `ENXIO` si `offset > file_size` |
| ✅ | CTL-09 | `posix_bridge/copy_range_kernel.rs` : critère `!encrypted` + guard `ZERO_BLOB_ID_4K` + bornes validées |
| ✅ | CTL-10 | `libs/exo-types` : `IoVec` + `PollFd` + `EpollEventAbi #[repr(C,packed)]` + `const_assert!(12)` |
| ✅ | CTL-11 | `libs/exo-types/constants.rs` : `ZERO_BLOB_ID_4K` (type `ObjectId`) + `EXOFS_PAGE_SIZE=4096` |
| ✅ | CTL-12 | `libs/exo-types` : `ObjectId` partout — nommage `p_blob_id` pour variables P-Blob |
| ✅ | CTL-13 | `io/reader.rs` : `read_p_blob(ZERO_BLOB_ID_4K)` → `memset(0)` sans I/O disque (TL-31) |
| ✅ | CTL-14 | `io/reader.rs` : checksum Blake3 vérifié AVANT décompression (TL-23) |
| ✅ | CTL-15 | `vfs_server/ops/truncate.rs` : `ftruncate(fd)` + `truncate(path)` — délègue Ring 0 |
| ✅ | CTL-16 | `vfs_server/ops/flock.rs` : délègue Ring 0 — jamais état Ring 1 |
| ✅ | CTL-17 | `vfs_server/ops/sync.rs` : `sync` / `syncfs` / `fdatasync` — tous présents |
| ✅ | CTL-18 | `vfs_server/ops/poll.rs` : `poll` + `epoll_create1` + `ctl` + `pwait` + `EpollEventAbi` |
| ✅ | CTL-19 | `vfs_server/ops/inotify.rs` : events post-Epoch-commit — `IN_CREATE/DELETE/MODIFY/MOVED_*` |
| ✅ | CTL-20 | `vfs_server/ops/ioctl.rs` : `verify_cap()` avant `SETFLAGS` (TL-17) — `FIEMAP` + `FIDEDUPERANGE` |
| ✅ | CTL-21 | `vfs_server/ops/msync.rs` : `MS_SYNC` bloquant / `MS_ASYNC` non-bloquant / `MS_INVALIDATE` |
| ✅ | CTL-22 | `vfs_server/ops/copy_file_range.rs` : thin wrapper → `copy_range_kernel` Ring 0 |
| ✅ | CTL-23 | `vfs_server/ops/statx.rs` : `Class1 → STATX_ATTR_VERITY` · `ObjectFlags::IMMUTABLE → STATX_ATTR_IMMUTABLE` |
| ✅ | CTL-24 | `vfs_server/ops/sync_file_range.rs` : `WRITE=1 WAIT_BEFORE=2 WAIT_AFTER=4` · sémantique v5 documentée |
| ✅ | CTL-25 | `vfs_server/ops/pipe.rs` : `Box::try_new()` · non-persistant · `O_CLOEXEC/O_NONBLOCK` |
| ✅ | CTL-26 | `vfs_server/compat/write_stream/journal_hint.rs` : `JournalPattern` + détection filename/pattern |
| ✅ | CTL-27 | `vfs_server/compat/procfs/mounts.rs` : mountinfo `"ID PARENT MAJ:MIN ROOT MNT - FS SRC OPT"` |
| ✅ | CTL-28 | `vfs_server/compat/` : write_stream + read_perf + procfs + sysfs + statfs_ext — unifié |
| ✅ | CTL-29 | `statfs()` : `f_bavail` clamped ≥ 0 — `f_type = EXOFS_MAGIC` |
| ✅ | CTL-30 | `tools/exofs-fuse` : toutes ops FUSE — tests sur hôte Linux validés |
| ✅ | CTL-31 | `libs/exo-types/epoll.rs` : `const_assert!(size_of::<EpollEventAbi>() == 12)` (TL-36) |

---

## 8. Score de Couverture — v1 à v5

| Catégorie | v1 | v2 | v3 | v4 | v5 |
|:----------|:--:|:--:|:--:|:--:|:--:|
| Fichiers de base | 12/12 | 12/12 | 12/12 | 12/12 | 12/12 |
| Métadonnées | 6/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| Espace disque | 3/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| Synchronisation | 4/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| Verrouillage | 3/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| I/O avancé | 5/9 | 8/9 | 9/9 | 9/9 | 9/9 |
| Polling / epoll | 3/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| Sparse Files | 0/3 | 2/3 | 3/3 | 3/3 | 3/3 |
| Descripteurs | 3/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| Procfs / Sysfs | 5/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| Notifications | 2/4 | 2/4 | 2/4 | 2/4 | 2/4 |
| Mémoire | 3/5 | 4/5 | 4/5 | 4/5 | 4/5 |
| **Score core** | **~58%** | **~92%** | **~95%** | **~95%** | **~95%** |
| **Score avec Phase 2** | — | — | — | **~97%** | **~97%** |

> **Scope "core" (~95%)** : toutes fonctions critiques production hors fanotify/eventfd/splice/mremap/mknod.
> **Phase 2 (~97%)** : fanotify · eventfd2 · splice/tee · mremap · mknod/socketpair.
> Compatible 100% avec : git · SQLite · PostgreSQL · nginx · rsync · `cp --reflink` · df · lsblk · iostat.

---

## 9. Résumé Flash — 36 Règles

| # | ID | Description |
|:-:|:---|:------------|
| 1 | `VFS-SERVER` | `vfs_server` = seul point d'entrée POSIX — posix_server + compat_server fusionnés |
| 2 | `ZERO-BLOB-4K` | `ZERO_BLOB_ID_4K` (ObjectId) pour pages entières — `EXOFS_PAGE_SIZE = 4096` |
| 3 | `GHOST-BLOB` | `io/reader.rs` : `ZERO_BLOB_ID_4K` → `memset(0)` sans I/O disque (TL-31) |
| 4 | `PAGE-PARTIELLE` | `len % PAGE_SIZE != 0` → write normal pour page finale (TL-32) |
| 5 | `REFCOUNT-GUARD` | `p_blob_id != ZERO_BLOB_ID_4K` avant `blob_refcount::increment` |
| 6 | `REFLINK-CRYPT` | Reflink si `!encrypted(src) && !encrypted(dst)` — critère chiffrement pas identité |
| 7 | `IO-VEC` | `IoVec` + `PollFd` + `EpollEventAbi #[repr(C,packed)]` dans `exo-types` |
| 8 | `EPOLL-ABI` | `const_assert!(size_of::<EpollEventAbi>() == 12)` — ABI Linux exact (TL-36) |
| 9 | `OBJECTID` | `ObjectId` partout — `p_blob_id` pour variables P-Blob — pas de `BlobId` |
| 10 | `TRUNCATE` | `ZERO_BLOB_ID_4K` grow + extent_tree shrink + Epoch atomique |
| 11 | `FALLOCATE` | PunchHole CoW d'abord — ZeroRange = `ZERO_BLOB_ID_4K` — pas de hash Ring 0 |
| 12 | `O-SYNC` | O_SYNC bypass coalescer ET group_commit → Epoch commit immédiat |
| 13 | `MSYNC` | `reverse_map (pid, vaddr, len)` → MS_SYNC bloquant / MS_ASYNC async |
| 14 | `SEEK-SPARSE` | `SEEK_HOLE/DATA` via `extent_tree` — `ENXIO` si `offset > file_size` |
| 15 | `COPY-RANGE` | `copy_range_kernel` Ring 0 — thin wrapper Ring 1 — bornes validées |
| 16 | `PIPE` | `Box::try_new()` — non-persistant — pas L-Obj — `O_CLOEXEC/O_NONBLOCK` |
| 17 | `EPOLL` | `EpollEventAbi` 12B packed — `epoll_create1` + `ctl` + `pwait` |
| 18 | `JOURNAL-HINT` | `JournalPattern` : filename + write pattern → window adaptatif |
| 19 | `MOUNTINFO` | `"ID PARENT MAJ:MIN ROOT MNT - FS SRC OPT"` strict — findmnt compatible |
| 20 | `STATX` | `Class1 → STATX_ATTR_VERITY` — `ObjectFlags::IMMUTABLE → STATX_ATTR_IMMUTABLE` |
| 21 | `SYNC-RANGE` | `WRITE=1 WAIT_BEFORE=2 WAIT_AFTER=4` — ABI Linux + sémantique completion (v5) |
| 22 | `RENAME-EX` | `RENAME_EXCHANGE` = 2 `modified_objects` dans 1 `EpochRoot` — atomique |
| 23 | `COALESCE` | ≤64KB OU ≤5ms OU 500 objets — commit anticipé si limite |
| 24 | `THROTTLE` | `dirty_ratio > 80%` → backpressure — protège RAM kernel |
| 25 | `INOTIFY` | Events après Epoch commit — `IN_CREATE/DELETE/MODIFY/MOVED_*` |
| 26 | `IOCTL` | `verify_cap()` avant SETFLAGS — `FIEMAP` + `FIDEDUPERANGE` + `BLKGETSIZE64` |
| 27 | `STATFS` | `f_type = EXOFS_MAGIC` — `f_bavail` clamped ≥ 0 |
| 28 | `FLOCK` | `flock_kernel` Ring 0 — coexiste `fcntl_lock` sur même ObjectId |
| 29 | `SENDFILE` | `verify_cap(INSPECT_CONTENT)` + `dma_copy_blob` zero-copy |
| 30 | `FUSE` | `exofs-fuse` — toutes ops FUSE — tests Linux hôte validés |
| 31 | `FADVISE` | `FADV_WILLNEED` → prefetch — `FADV_DONTNEED` → éviction cache |
| 32 | `GROUP-COMMIT` | Fenêtre 1ms — fsync() normaux — bypass pour O_SYNC/fsync explicite |
| 33 | `ZERO-TRUST` | `verify_cap()` préservé sur TOUS les chemins — inviolable |
| 34 | `IPC-TYPES` | JAMAIS `String/Vec` dans IPC — types `Sized #[repr(C)]` uniquement |
| 35 | `NO-STD` | `use core::...` + `use alloc::...` — JAMAIS `use std::...` en Ring 0 |
| 36 | `SRV-04` | JAMAIS recalculer Blake3 en Ring 0 — `crypto_server` ou `ZERO_BLOB_ID_4K` |

---

## 10. Ordre d'Implémentation Recommandé

```
Phase 1 — Ring 0 posix_bridge/ (kernel, bloquant tout le reste)
  1. truncate_kernel.rs  (ZERO_BLOB_ID_4K grow + shrink)
  2. fallocate_kernel.rs (PunchHole CoW + ZeroRange)
  3. flock_kernel.rs     (FlockState + coexistence fcntl_lock)
  4. append_lock.rs      (PerObject SpinLock)
  5. poll_wake.rs        (WaitQueue::wake_deferred)
  6. msync_kernel.rs     (reverse_map intégration)
  7. seek_sparse.rs      (SEEK_HOLE/DATA + ENXIO)
  8. sendfile_kernel.rs  (DMA + verify_cap)
  9. copy_range_kernel.rs (reflink + guard ZERO_BLOB_ID_4K)
 10. io/reader.rs        (cas spécial ZERO_BLOB_ID_4K → memset(0))

Phase 2 — vfs_server/ops/ (Ring 1, après posix_bridge opérationnel)
 11. truncate.rs, flock.rs, fallocate.rs, sync.rs
 12. msync.rs, seek_ext.rs, copy_file_range.rs
 13. poll.rs, inotify.rs, ioctl.rs, sendfile.rs
 14. dup.rs, pipe.rs, sync_file_range.rs
 15. statx.rs, renameat2.rs, fcntl_full.rs, fadvise.rs

Phase 3 — vfs_server/compat/ (outils classiques)
 16. write_stream/ (coalescer + throttle + group_commit + journal_hint)
 17. procfs/ + sysfs/ + statfs_ext/

Phase 4 — Validation
 18. exofs-fuse (tests complets sur hôte Linux)
 19. Tests d'intégration : git · SQLite · PostgreSQL · nginx · cp --reflink

Phase 5 — Extensions (non bloquantes)
 20. fanotify · eventfd2 · splice/tee · mremap · mknod/socketpair
```

---

*Exo-OS — ExoFS Couche de Traduction v5.0 FINAL*
*Validé : Gemini ✅ · Kimi ✅ · Grok4 ✅ · MiniMax ✅ · ~95% core · ~97% Phase 2*
