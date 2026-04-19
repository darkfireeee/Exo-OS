# Corrections ExoFS — Priorité P2 / Qualité
## Commit de référence : 93616537 · Audit croisé Claude + Kimi (19 avril 2026)

---

## P2-01 · `Ordering::Relaxed` insuffisant dans `core/config.rs`

**Fichier :** `kernel/src/fs/exofs/core/config.rs`  
**Sévérité :** P2 — Valeurs de configuration potentiellement obsolètes sur ARM/RISC-V  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

Les getters de `ExofsConfig` utilisent `Ordering::Relaxed` tandis que les
setters (dans `apply_update()`) utilisent `Ordering::Release`. Sans
`Ordering::Acquire` sur les lectures, le modèle mémoire Rust ne garantit
pas que les écriture release soient visibles — critique sur architectures
faiblement ordonnées (ARM, RISC-V, théoriquement PowerPC).

```rust
// Getters — Relaxed ne garantit pas la visibilité après un Release
pub fn gc_min_epoch_delay(&self) -> u64 {
    self.gc_min_epoch_delay.load(Ordering::Relaxed)  // ← devrait être Acquire
}
```

### Correction

Remplacer `Ordering::Relaxed` par `Ordering::Acquire` sur tous les getters
qui lisent des valeurs pouvant être mises à jour par `apply_update()` :

```rust
// Principe : si le setter fait store(v, Release), le getter doit faire load(Acquire).
// Les statistiques pures (compteurs incrémentaux lus en monitoring) peuvent
// rester Relaxed, mais les valeurs de config influençant le comportement
// doivent être Acquire.

pub fn gc_min_epoch_delay(&self) -> u64 {
    self.gc_min_epoch_delay.load(Ordering::Acquire)
}
pub fn gc_timer_secs(&self) -> u64 {
    self.gc_timer_secs.load(Ordering::Acquire)
}
pub fn gc_free_threshold_pct(&self) -> u64 {
    self.gc_free_threshold_pct.load(Ordering::Acquire)
}
pub fn writeback_interval_ms(&self) -> u64 {
    self.writeback_interval_ms.load(Ordering::Acquire)
}
// ... appliquer à tous les getters de valeurs de configuration.

// Les compteurs purement statistiques (hits, misses) peuvent rester Relaxed
// car une sous-comptation temporaire n'affecte pas le comportement correct.
```

---

## P2-02 · `PathCache::new_const()` — `max` hardcodé à 16384 au lieu de `PATH_CACHE_CAPACITY`

**Fichier :** `kernel/src/fs/exofs/cache/path_cache.rs`  
**Sévérité :** P2 — Valeur de limite divergente de la constante canonique  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

```rust
pub const fn new_const() -> Self {
    Self {
        inner: SpinLock::new(PathCacheInner::new(16384)),  // ← hardcodé
        // ...
    }
}
```

`PATH_CACHE_CAPACITY = 10_000` est défini dans `constants.rs` mais non
utilisé ici — la limite effective est `16384` (1.64× la valeur documentée).

### Correction

```rust
use crate::fs::exofs::core::constants::PATH_CACHE_CAPACITY;

pub const fn new_const() -> Self {
    Self {
        inner: SpinLock::new(PathCacheInner::new(PATH_CACHE_CAPACITY)),
        // ...
    }
}
```

---

## P2-03 · `vfs_rename()` — Perte du lien `object_id → blob` lors du rename

**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`  
**Sévérité :** P2 — Fichier renommé inaccessible via BLOB_CACHE  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`vfs_rename()` appelle `INODE_EMULATION.release(old_oid)` puis
`get_or_alloc_flags(new_oid, ...)`. Cela crée un **nouvel** `object_id` (le
hash du nouveau nom) sans transférer le contenu de l'ancien blob. Après un
rename, la lecture du fichier renommé retourne un blob vide.

```rust
INODE_EMULATION.release(old_oid);
INODE_EMULATION.get_or_alloc_flags(new_oid, flags, size, uid)?;
// ↑ new_oid est un nouveau hash — l'ancien contenu est perdu.
```

### Correction

```rust
pub fn vfs_rename(
    old_parent: ObjectIno, old_name: &[u8],
    new_parent: ObjectIno, new_name: &[u8],
) -> ExofsResult<()> {
    // ... validations ...

    let old_oid = hash_name(old_parent, old_name);
    let src     = INODE_EMULATION.get_entry_by_oid(old_oid).ok_or(ExofsError::ObjectNotFound)?;
    let new_oid = hash_name(new_parent, new_name);

    if INODE_EMULATION.contains_oid(new_oid) { return Err(ExofsError::ObjectAlreadyExists); }

    // Transférer le contenu du blob : copier sous le nouveau BlobId.
    let old_blob_id = BlobId::from_u64(old_oid);
    let new_blob_id = BlobId::from_u64(new_oid);
    if let Some(data) = BLOB_CACHE.get(&old_blob_id) {
        BLOB_CACHE.insert(new_blob_id, data.to_vec()).map_err(|_| ExofsError::NoSpace)?;
    }

    // Recréer l'entrée inode sous le nouvel oid, en conservant l'ino stable.
    INODE_EMULATION.release(old_oid);
    INODE_EMULATION.get_or_alloc_flags(new_oid, src.flags, src.size, src.uid)?;
    Ok(())
}
```

---

## P2-04 · `EpochJournalHeader` — `repr(C)` et sérialisation manuelle coexistent sans assertion

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`  
**Sévérité :** P2 — Surface de divergence future  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`EpochJournalHeader` est une struct `#[repr(C)]` avec son propre layout (40
octets avec padding). `save_journal()` sérialise manuellement le même format
sans tenir compte du padding. Les deux représentations divergent actuellement
(voir P0-04). Pour éviter toute récidive, ajouter une assertion qui lie les
deux.

### Correction

Après la correction P0-04, ajouter une assertion statique qui vérifie que les
offsets manuels correspondent à la struct :

```rust
// À placer après les définitions de constantes dans epoch_commit.rs
const _: () = assert!(
    core::mem::offset_of!(EpochJournalHeader, entry_count) == 16,
    "Offset entry_count doit être 16"
);
const _: () = assert!(
    core::mem::offset_of!(EpochJournalHeader, checksum) == 24,
    "Offset checksum doit être 24 — mettre à jour HDR_OFF_CHECKSUM"
);
// Si offset_of! n'est pas disponible (stabilisé Rust 1.77),
// utiliser un test d'intégration qui vérifie le layout.
```

---

## P2-05 · `cache/mod.rs` — `reclaim_bytes()` ne tient pas compte des données dirty

**Fichier :** `kernel/src/fs/exofs/cache/mod.rs`  
**Sévérité :** P2 — Logique d'éviction incorrecte (complète la correction P0-05)  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`reclaim_bytes()` appelle `flush_all()` sur PATH_CACHE et METADATA_CACHE (qui
détruit le contenu) mais estime avoir libéré `targets[0]` et `targets[1]`
octets **quelles que soient les vraies tailles évincées**. Si ces caches sont
vides, `freed` est surestimé et les caches blob/extent ne sont pas vidés alors
qu'ils le devraient.

### Correction

```rust
pub fn reclaim_bytes(bytes: u64) -> u64 {
    let mut freed = 0u64;

    // Étape 1 : path cache (pas de dirty — sûr à dropper).
    let path_freed = PATH_CACHE.estimated_bytes();
    PATH_CACHE.drop_all(); // Utiliser drop_all() après correction P0-05
    freed = freed.saturating_add(path_freed);
    if freed >= bytes { return freed; }

    // Étape 2 : metadata cache.
    let meta_freed = METADATA_CACHE.estimated_bytes();
    METADATA_CACHE.drop_all();
    freed = freed.saturating_add(meta_freed);
    if freed >= bytes { return freed; }

    // Étape 3 : extent cache — éviction sélective.
    let e = EXTENT_CACHE.evict_n(64);
    freed = freed.saturating_add(e);
    if freed >= bytes { return freed; }

    // Étape 4 : blob cache — éviction sélective (respecte dirty).
    let b = BLOB_CACHE.evict_n(64);
    freed = freed.saturating_add(b);
    freed
}
```

---

## P2-06 · Rapport Kimi invalidé : `FS-CRIT-03` (InodeNumber type)

**Fichier :** `kernel/src/fs/exofs/posix_bridge/inode_emulation.rs`  
**Sévérité :** N/A — Finding Kimi incorrect  
**Vérifié sur :** code réel commit 93616537

### Clarification

Kimi prétend que `InodeNumber = [u8; 32]`. **C'est faux.** Le code déclare :

```rust
pub type ObjectIno = u64;
```

`ObjectIno` est bien un `u64`, compatible POSIX `ino_t`. La table `InodeEntry`
contient `ino: ObjectIno` (u64) et `object_id: u64` (hash FNV-1a de l'ObjectId
réel). La bijection FNV-64 introduit un risque théorique de collision mais ne
constitue pas d'incompatibilité de type.

**Aucune correction nécessaire sur le type.** La seule lacune réelle est
l'absence de table de reverse-mapping `u64 → BlobId` pleine résolution (voir
P0-01, P0-02).

---

## P2-07 · Rapport Kimi invalidé : `FS-CRIT-04` (AtomicU64 on-disk dans epoch_record.rs)

**Fichier :** `kernel/src/fs/exofs/epoch/epoch_record.rs`  
**Sévérité :** N/A — Finding Kimi incorrect  
**Vérifié sur :** code réel commit 93616537

### Clarification

`epoch_record.rs` contient explicitement en en-tête :

```
// RÈGLE ONDISK-01 : #[repr(C, packed)] + types plain uniquement (pas d'AtomicU64).
```

La structure `EpochRecord` est `#[repr(C, packed)]` sans aucun type atomique.
Le finding de Kimi (`cache_stats.rs` et `io_stats.rs`) concerne des structures
**in-memory** de statistiques, non des structures on-disk. Ces types atomiques
sont appropriés pour leur usage.

**Aucune correction nécessaire** sur ces structures.

---

## P2-08 · Rapport Kimi invalidé : `FS-HIGH-03` (format() sans MIN_DISK_SIZE)

**Fichier :** `kernel/src/fs/exofs/storage/superblock.rs`  
**Sévérité :** N/A — Finding Kimi partiellement incorrect  
**Vérifié sur :** code réel commit 93616537

### Clarification

`format()` **vérifie bien** `disk_size < MIN_DISK_SIZE` (ligne 374). Kimi a
tort sur ce point. En revanche, `mount()` n'effectue pas cette vérification —
c'est le vrai bug, documenté en P1-06 ci-dessus.

---

## P2-09 · Rapport Kimi invalidé : `FS-HIGH-04` (align_up overflow)

**Fichier :** `kernel/src/fs/exofs/storage/layout.rs`  
**Sévérité :** N/A — Finding Kimi incorrect  
**Vérifié sur :** code réel commit 93616537

### Clarification

`align_up()` et `align_down()` utilisent `checked_add`, `checked_sub` et
retournent `Err(ExofsError::OffsetOverflow)`. Le finding de Kimi décrit une
version non protégée qui n'existe pas dans ce commit.

---

## P2-10 · Rapport Kimi invalidé : `FS-HIGH-05` (PathCache sans limite)

**Fichier :** `kernel/src/fs/exofs/cache/path_cache.rs`  
**Sévérité :** N/A — Finding Kimi incorrect  
**Vérifié sur :** code réel commit 93616537

### Clarification

`PathCacheInner` a un champ `max: usize` et `new_const()` l'initialise à
`16384`. La méthode d'insertion vérifie :

```rust
if inner.map.len() >= inner.max { inner.evict_one_lru(); }
```

Le cache est bien borné. Le vrai problème est que la limite est `16384` au
lieu de `PATH_CACHE_CAPACITY = 10_000` — corrigé en P2-02.

---

## P2-11 · `FS-CRIT-02` Kimi — Reclassification : POSIX layer partiellement stub

**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`  
**Sévérité :** P0/P1 — Non « totalement absente » comme le dit Kimi  
**Vérifié sur :** code réel commit 93616537

### Clarification

Kimi déclare la POSIX layer « entièrement absente ». C'est inexact. Les
fonctions suivantes sont **structurellement implémentées** avec logique de
validation, gestion des erreurs et table de FDs :
`vfs_lookup`, `vfs_create`, `vfs_open`, `vfs_close`, `vfs_mkdir`,
`vfs_unlink`, `vfs_rename`, `vfs_getattr`, `vfs_truncate`, `vfs_symlink`,
`vfs_readdir`, `vfs_close_all_pid`.

Les deux fonctions **réellement stub** sont :
- `vfs_read()` → retourne des zéros (P0-01)
- `vfs_write()` → ne persiste pas (P0-02)
- `vfs_readdir()` → ne liste que `.` et `..` (P1-02)

La couche POSIX est **incomplète** (3–4 semaines d'effort restant pour
connecter BLOB_CACHE, ajouter les tables parent→enfants, etc.) mais n'est
pas vide. L'évaluation de Kimi « 3-4 semaines » reste correcte sur
l'effort total.

