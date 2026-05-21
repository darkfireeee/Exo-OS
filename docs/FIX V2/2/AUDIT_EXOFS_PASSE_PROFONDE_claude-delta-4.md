# ExoOS — Audit Passe Profonde ExoFS — Snapshot 2026-05-20
## Rapport de stabilisation — Itération 4

**Rédigé par** : Claude Delta  
**Date** : 2026-05-20  
**Périmètre** : Audit exhaustif d'ExoFS — storage, epoch, cache, syscalls, crypto, déduplication, GC, chemins  
**Précédents rapports** : Itérations 1, 2, 3 (voir `docs/FIX V2/`)

---

## Préambule — Méthode

Cette passe couvre l'intégralité du sous-arbre `kernel/src/fs/exofs/` (130+ fichiers) et tous les syscalls ExoFS (`SYS_EXOFS_*`). Chaque module a été lu dans l'ordre de la chaîne de données : syscall → cache → storage → epoch → superblock → crypto.

### Ce que cette passe confirme comme correct

| Module | Verdict |
|--------|---------|
| `superblock.rs` — structure disque, checksum Blake3, 3 miroirs, const_assert 512B | ✅ Solide |
| `epoch_barriers.rs` — injection de dépendance propre via `CommitCallbacks` | ✅ Architecture correcte |
| `block_allocator.rs` — ExtentHandle lifecycle (Reserved → Committed → Freed) | ✅ Correct |
| `heap_allocator.rs` — first-fit + coalescence, bitmap | ✅ Correct |
| `xchacha20.rs` — nonces randomisés (RDRAND + compteur), zéroïsation à Drop | ✅ Correct |
| `relation_cycle.rs` — DFS itératif, profondeur max 128, pas de récursion | ✅ Correct |
| `snapshot_gc.rs` — respect du flag `PROTECTED`, cascade optionnelle | ✅ Correct |
| `path_cache.rs` — LRU eviction avec TTL, taille bornée | ✅ Correct |
| `decompress_reader.rs` — limite `MAX_DECOMPRESSED_BLOCK_SIZE = 256 MiB` | ✅ Correct |
| `object_fd.rs` — alloc/free linéaire sûr, alloc retourne `Err` quand plein | ✅ Correct |

---

## Sommaire des gravités — Passe ExoFS

| Gravité | Nombre | Nature |
|---------|--------|--------|
| **P0 — Bloquant** | 3 | Durabilité nulle, perte de données garantie après reboot, cache corruptible |
| **P1 — Majeur** | 4 | Protocole epoch déconnecté, TOCTOU sur delete, métadonnées fictives, LBA non réclamé |
| **P2 — Mineur** | 2 | Allocation statique excessive, chaîne epoch incomplète |

---

## P0 — Incohérences Bloquantes

### FS-P0-1 · `register_nvme_flush_fn()` jamais appelé — les 3 barrières NVMe sont des no-ops

**Fichiers concernés** :
- `kernel/src/fs/exofs/epoch/epoch_barriers.rs` — stub par défaut
- `kernel/src/fs/exofs/epoch/mod.rs:58` — exporte `register_nvme_flush_fn`
- **Aucun fichier** dans `kernel/`, `drivers/` ou `servers/` n'appelle `register_nvme_flush_fn`

**Constat** :

La fonction de flush NVMe est une injection de dépendance : le block layer doit appeler `register_nvme_flush_fn(ma_fonction_flush)` au démarrage. Tant qu'il ne le fait pas, le stub par défaut est actif :

```rust
// epoch_barriers.rs — stub actif en production
fn default_flush_stub() -> ExofsResult<()> {
    UNHOOK_FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())   // ← retourne Ok() SANS flush physique
}
```

Le commentaire du stub est explicite :

> *"En production : si appelé, cela indique un boot incomplet. Les écritures sont en mémoire volatile — pas de durabilité."*

Grep exhaustif sur tout le projet :

```
register_nvme_flush_fn → 1 occurrence : epoch/mod.rs (export)
                        → 0 occurrence dans drivers/, kernel/lib.rs, init
```

**Conséquence** : les 3 barrières NVMe du protocole EPOCH-01 (`nvme_barrier_after_data`, `nvme_barrier_after_root`, `nvme_barrier_after_record`) sont **toutes des no-ops**. L'ordre d'écriture n'est jamais garanti au niveau matériel. Sur toute séquence d'écriture suivie d'une coupure de courant, l'état sur disque est indéfini.

**Impact** : Durabilité ExoFS = zéro, même si le BAR VirtIO était corrigé (FS-P0-2). La RÈGLE EPOCH-02 (*"INTERDIT d'omettre une barrière NVMe — reordering = corruption"*) est violée en permanence.

**Correction** : Dans `kernel/src/fs/exofs/storage/virtio_adapter.rs`, après `init_global_disk_with_mmio()`, enregistrer le hook de flush :

```rust
// virtio_adapter.rs — après init réussie du disque
pub fn init_global_disk_with_mmio(base: usize, capacity: usize) {
    // ... init device ...
    crate::fs::exofs::epoch::epoch_barriers::register_nvme_flush_fn(
        virtio_blk_flush  // fn() -> ExofsResult<()>
    );
}

fn virtio_blk_flush() -> ExofsResult<()> {
    with_global_disk(|dev| {
        dev.flush().map_err(|_| ExofsError::NvmeFlushFailed)
    })
}
```

---

### FS-P0-2 · `OBJECT_STORE` (table LBA) est purement en mémoire — toutes les données sont illisibles après reboot

**Fichiers concernés** :
- `kernel/src/fs/exofs/syscall/object_store.rs` — `static OBJECT_STORE: ObjectStore`
- `kernel/src/fs/exofs/syscall/object_store.rs:60` — `reserve_for_write()` : allocateur linéaire sans free-list
- Aucun mécanisme de sérialisation/chargement de `OBJECT_STORE` depuis le disque

**Constat** :

`OBJECT_STORE` est une `SpinLock<ObjectStoreInner>` initialisée à zéro au boot. Elle contient :

```rust
struct ObjectStoreInner {
    map: BTreeMap<BlobId, PersistedBlobMapping>,  // BlobId → (base_lba, size)
    next_lba: u64,                                // compteur d'allocation
}
```

À chaque `persist_blob_data_if_disk()`, un bloc de LBAs est alloué linéairement (`next_lba += needed_blocks`) et la correspondance `BlobId → LBA` est stockée dans `map`. C'est l'unique endroit où cette correspondance existe.

**Cette structure n'est jamais écrite sur disque.** Elle ne contient aucune fonction `serialize()`, `save()` ou `persist()`. Elle n'est jamais lue depuis le disque au montage. Le superblock n'a pas de champ `object_store_lba`.

**Scénario de perte de données** :

```
Session 1 : exo compat install calendar
  → persist_blob_data_if_disk("calendar.wasm") → LBA 1024
  → OBJECT_STORE.map["calendar.wasm"] = {base_lba: 1024, size: 512KB}
  → Données physiquement sur disque à LBA 1024 ✓

Reboot

Session 2 : exo compat run calendar
  → OBJECT_STORE.map est vide (réinitialisé)
  → load_blob_data_if_available("calendar.wasm") → Ok(None) [introuvable]
  → "application non trouvée"
```

Les données sont sur le disque mais ExoFS ne sait plus où elles sont. **Toute donnée écrite en session N est illisible en session N+1.** C'est un bug de conception fondamental, indépendant de la correction du BAR VirtIO (P0-1, itération 2).

**Correction** : Sérialiser `OBJECT_STORE.map` sur disque à chaque epoch commit et le recharger au montage. L'emplacement naturel est le superblock (champ `object_catalog_lba`) ou un bloc dédié après le heap start :

```rust
// Au commit epoch
fn commit_epoch(...) {
    // ... 3 barrières ...
    object_store::persist_catalog(&superblock)?;  // sérialise la BTreeMap
}

// Au montage
fn mount(...) {
    let sb = SuperblockManager::mount(...)?;
    object_store::load_catalog(&sb)?;  // restaure la BTreeMap
}
```

---

### FS-P0-3 · `evict_to_fit_except()` évince des blobs dirty sans flush — perte de données silencieuse

**Fichier concerné** : `kernel/src/fs/exofs/cache/blob_cache.rs:295–318`

**Constat** :

`BlobEntry` possède un champ `dirty: bool` indiquant que les données ont été modifiées en cache mais pas encore persistées sur disque. La fonction d'éviction sous pression mémoire ne vérifie **jamais** ce flag :

```rust
// blob_cache.rs — evict_to_fit_except()
for v in &victims {
    if protected == Some(*v) { continue; }
    if let Some(e) = self.map.remove(v) {   // ← AUCUNE vérification de e.dirty
        let sz = e.len();
        self.eviction.remove(v);
        self.used = self.used.saturating_sub(sz);
        CACHE_STATS.record_eviction(sz);    // ← comptabilisé comme éviction normale
        removed_any = true;
    }
}
```

Un blob marqué `dirty` (modifié via `SYS_EXOFS_OBJECT_WRITE` mais pas encore commité via `SYS_EXOFS_EPOCH_COMMIT`) peut être expulsé silencieusement si le cache est sous pression. La donnée modifiée est **perdue sans erreur** : ni le processus appelant ni le journal d'audit ne sont notifiés.

**Impact** : Sur un système avec peu de mémoire (QEMU `-m 256M`), toute écriture suivie d'une pression mémoire peut perdre les données sans que `SYS_EXOFS_EPOCH_COMMIT` retourne une erreur. Viole l'invariant EPOCH-01 (les données committées sont durables).

**Correction** :

```rust
for v in &victims {
    if protected == Some(*v) { continue; }
    if let Some(e) = self.map.get(v) {
        if e.dirty {
            // Ne pas évincer un blob dirty — tenter de le flusher d'abord
            if let Err(err) = flush_dirty_blob(v) {
                // Si le flush échoue, skip ce candidat
                CACHE_STATS.record_eviction_skip_dirty();
                continue;
            }
        }
        // Maintenant safe à évincer
        self.map.remove(v);
        // ...
    }
}
```

---

## P1 — Incohérences Majeures

### FS-P1-1 · `SYS_EXOFS_EPOCH_COMMIT` opère entièrement en mémoire, jamais sur disque

**Fichier concerné** : `kernel/src/fs/exofs/syscall/epoch_commit.rs`

**Constat** :

Le syscall `SYS_EXOFS_EPOCH_COMMIT` (518) est censé sceller une epoch et garantir la durabilité des données. Son implémentation dans `syscall/epoch_commit.rs` effectue :

1. `collect_epoch_blobs()` — liste les blobs de l'epoch dans `BLOB_CACHE`
2. `save_journal()` — écrit le journal dans `BLOB_CACHE` (en mémoire)
3. `flush_dirty_blobs()` — marque les blobs comme non-dirty dans `BLOB_CACHE`
4. `CURRENT_EPOCH.store(new_epoch)` — incrémente l'epoch en mémoire

**À aucun moment** le syscall n'appelle :
- `epoch/epoch_commit.rs::commit_epoch()` (le protocole 3 barrières NVMe)
- `storage/superblock.rs::SuperblockManager::commit()` (mise à jour du superblock disque)
- `persist_blob_data_if_disk()` (écriture physique des blobs)

Le protocole 3-barrières est architecturalement correct et complet dans `epoch/epoch_commit.rs`, mais il n'est **jamais invoqué depuis le syscall**. Les deux implémentations coexistent sans se connecter :

```
syscall/epoch_commit.rs  ←── AppelantSyscall ─── [utilisé]
epoch/epoch_commit.rs    ←── [implémentation correcte mais jamais appelée]
```

**Impact** : `SYS_EXOFS_EPOCH_COMMIT` ne garantit aucune durabilité. Les données restent exclusivement dans `BLOB_CACHE` (RAM). Aggrave FS-P0-1 et FS-P0-2 : même si le hook NVMe et l'OBJECT_STORE étaient corrigés, le syscall de commit n'en profiterait pas.

**Correction** : `do_commit()` dans `syscall/epoch_commit.rs` doit appeler le protocole réel :

```rust
fn do_commit(args: &EpochCommitArgs) -> ExofsResult<EpochCommitResult> {
    // ... validations ...
    let entries = collect_epoch_blobs(epoch_to_commit)?;

    // 1. Flusher les blobs dirty vers le disque (persist_blob_data_if_disk)
    for entry in &entries {
        let blob_id = BlobId(entry.blob_id);
        if let Some(data) = BLOB_CACHE.get(&blob_id) {
            object_store::persist_blob_data_if_disk(blob_id, &data, false)?;
        }
    }

    // 2. Appeler le protocole 3-barrières
    let input = CommitInput { /* ... */ };
    epoch::epoch_commit::commit_epoch(input)?;

    // 3. Mettre à jour le superblock
    SUPERBLOCK_MANAGER.commit(tsc_now(), |off, data| {
        virtio_adapter::write_at(off, data)
    })?;
    // ...
}
```

---

### FS-P1-2 · `object_delete` : race TOCTOU entre `open_count_for()` et `invalidate()`

**Fichier concerné** : `kernel/src/fs/exofs/syscall/object_delete.rs:88–100`

**Constat** :

La séquence de suppression d'un blob est :

```rust
fn delete_blob(blob_id: BlobId, flags: u32) -> ExofsResult<DeleteResult> {
    // 1. Vérifier l'existence
    let existing = BLOB_CACHE.get(&blob_id);
    drop(existing);                                    // verrou relâché

    // 2. Vérifier les fds ouverts
    if OBJECT_TABLE.open_count_for(&blob_id) > 0 {   // ← verrou OBJECT_TABLE
        return Err(ExofsError::PermissionDenied);
    }
    // ← ICI : fenêtre de race entre étapes 2 et 3

    // 3. Supprimer du cache
    BLOB_CACHE.invalidate(&blob_id);                  // ← verrou BLOB_CACHE
}
```

Entre l'étape 2 (`open_count_for()`) et l'étape 3 (`invalidate()`), un autre thread peut ouvrir le blob via `SYS_EXOFS_OBJECT_OPEN`. Le blob sera supprimé du cache pendant qu'un fd valide le référence.

**Impact** : Un processus peut se retrouver avec un fd ouvert pointant vers un blob invalidé. Les lectures/écritures ultérieures retourneront `BlobNotFound` ou des données corrompues si le blob est réalloué avec un autre contenu.

**Correction** : Acquérir un verrou combiné OBJECT_TABLE + BLOB_CACHE ou utiliser une opération atomique qui vérifie et supprime en une seule étape :

```rust
// API atomique à ajouter dans BLOB_CACHE
pub fn invalidate_if_not_open(
    &self,
    blob_id: &BlobId,
    open_table: &ObjectFdTable,
) -> ExofsResult<bool> {
    let mut inner = self.inner.lock();          // verrou BLOB_CACHE tenu
    if open_table.open_count_for_locked(blob_id) > 0 {
        return Ok(false);                      // toujours sous verrou
    }
    inner.map.remove(blob_id);
    Ok(true)
}
```

---

### FS-P1-3 · `path_resolve` retourne toujours `object_kind=0`, `size_bytes=0` — `stat()` est non fonctionnel

**Fichier concerné** : `kernel/src/fs/exofs/syscall/path_resolve.rs:151–230`

**Constat** :

`resolve_path_to_blob()` calcule `BlobId = Blake3(chemin_canonique)` et construit un `PathResolveResult` avec des champs statiques :

```rust
Ok(PathResolveResult {
    blob_id: *bid_bytes,
    object_id: obj_bytes,
    object_kind: 0,         // ← TOUJOURS 0 = Blob générique
    _pad: [0u8; 7],
    size_bytes: 0,          // ← TOUJOURS 0
    epoch_id: 0,            // ← TOUJOURS 0
    link_count: 1,          // ← TOUJOURS 1
    flags: 0,               // ← TOUJOURS 0
    _reserved: [0u8; 8],
})
```

La fonction ne consulte **pas** `BLOB_CACHE`, `OBJECT_TABLE` ni aucune métadonnée stockée. Elle dérive uniquement un `BlobId` depuis le chemin, sans vérifier si le blob existe ou lire ses propriétés.

**Conséquences** :
- `SYS_EXOFS_PATH_RESOLVE` ne peut pas distinguer un fichier existant d'un fichier inexistant.
- La taille retournée est toujours 0 — `ls -l` ou tout équivalent affiche des tailles nulles.
- `object_kind` est toujours `Blob (0)` — les objets `Code`, `Config`, `PathIndex` et `Relation` sont indiscernables d'un fichier ordinaire.
- Les permissions et le type POSIX (`st_mode`) ne peuvent pas être correctement dérivés.

**Correction** : Après le calcul du `BlobId`, consulter le cache pour enrichir le résultat :

```rust
let blob_id = BlobId::from_bytes_blake3(&canonical);

// Consulter les métadonnées réelles
let (kind, size, epoch, flags) = if let Some(meta) = blob_meta_cache_get(&blob_id) {
    (meta.kind as u8, meta.size_bytes, meta.epoch_id, meta.flags)
} else {
    return Err(ExofsError::BlobNotFound);   // chemin inexistant
};

Ok(PathResolveResult {
    blob_id: *blob_id.as_bytes(),
    object_kind: kind,
    size_bytes: size,
    epoch_id: epoch,
    flags,
    // ...
})
```

---

### FS-P1-4 · `OBJECT_STORE` : allocateur linéaire sans free-list — l'espace disque n'est jamais récupéré

**Fichier concerné** : `kernel/src/fs/exofs/syscall/object_store.rs:60–110`

**Constat** :

L'allocateur LBA de `reserve_for_write()` est purement linéaire :

```rust
inner.next_lba = end_lba;          // avance toujours vers l'avant
inner.map.insert(blob_id, mapping);
```

Il n'existe aucune fonction `free_lba()` ou `reclaim_extent()`. Quand un blob est supprimé via `object_delete()` → `BLOB_CACHE.invalidate()`, son entrée dans `OBJECT_STORE.map` reste — et si elle était retirée, les LBAs correspondants ne seraient pas non plus récupérés.

**Impact chiffré** sur un disque de 512 MiB (config par défaut) :
- Taille moyenne d'un blob WASM installé : ~2 MiB
- Chaque `install` + `uninstall` d'application gaspille 2 MiB de LBAs définitivement
- Après ~256 cycles install/uninstall, le disque est plein malgré 0 application installée
- `exo compat install` retourne `ExofsError::NoSpace` sur un disque "vide"

**Correction** : Maintenir une free-list d'extents LBA récupérés :

```rust
struct ObjectStoreInner {
    map: BTreeMap<BlobId, PersistedBlobMapping>,
    next_lba: u64,
    free_extents: Vec<(u64, u64)>,  // (base_lba, size_blocks) récupérés
}

pub fn free_lba(&self, blob_id: &BlobId) {
    let mut inner = self.inner.lock();
    if let Some(mapping) = inner.map.remove(blob_id) {
        inner.free_extents.push((mapping.base_lba, mapping.allocated_blocks));
    }
}
```

---

## P2 — Incohérences Mineures

### FS-P2-1 · `[ObjectFdEntry; 65532]` : 5.24 MiB de BSS statique

**Fichier concerné** : `kernel/src/fs/exofs/syscall/object_fd.rs:43,156`

**Constat** :

```rust
pub const FD_MAX: u32 = 65_535;
pub const MAX_FDS: usize = (FD_MAX - FD_RESERVED + 1) as usize;  // 65532
// ...
slots: [ObjectFdEntry; MAX_FDS],   // 65532 × 80 bytes = 5 242 560 octets ≈ 5.24 MiB
```

Ce tableau est alloué statiquement en BSS (`pub static OBJECT_TABLE`). Cela représente 5.24 MiB de mémoire kernel réservée en permanence, même si aucun fichier n'est ouvert.

Un système avec `-m 256M` consacre ~2% de sa RAM kernel à cette table vide. Dans un contexte embarqué ou avec KPTI activé (surcoût mémoire supplémentaire), cette allocation est significative.

**Correction** : Réduire `FD_MAX` à 4096 (raisonnable pour v0.2.0) ou utiliser une `BTreeMap` allouée dynamiquement avec une borne maximale :

```rust
pub const MAX_FDS: usize = 4096;   // 4096 × 80B = 320 KiB vs 5.24 MiB
```

---

### FS-P2-2 · `EpochRecord.prev_slot` toujours `DiskOffset(0)` — chaîne de recovery brisée

**Fichier concerné** : `kernel/src/fs/exofs/epoch/epoch_commit.rs:177`

**Constat** :

```rust
let record = EpochRecord::new(
    next_epoch,
    flags,
    tsc_now,
    root_oid,
    input.root_disk_offset,
    DiskOffset(0),  // prev_slot : rempli par le slot selector avant cet appel
);
```

Le commentaire dit *"rempli par le slot selector avant cet appel"* mais le `CommitInput` reçu a `slot_offset` correctement renseigné, pas `prev_slot`. Aucun code ne remplit `prev_slot` avec l'offset du slot précédent avant d'appeler `commit_epoch()`.

`prev_slot` est utilisé par `recovery/ondisk.rs` pour remonter la chaîne d'epochs lors d'une récupération (`BACKUP-02`). Avec `prev_slot = 0` sur tous les records, la recovery walk ne peut pas reconstruire l'historique des epochs — elle s'arrête immédiatement.

**Impact** : Si le miroir primaire du superblock est corrompu et que la recovery doit lire les slots d'epoch pour déterminer le dernier état valide, elle ne trouvera aucune chaîne utilisable. La récupération se limite au miroir superblock le plus récent sans possibilité de rejouer les epochs intermédiaires.

**Correction** : Dans le code qui construit `CommitInput`, passer l'offset du dernier slot commité :

```rust
let prev_slot_offset = LAST_COMMITTED_SLOT.load(Ordering::Acquire);
let input = CommitInput {
    // ...
    slot_offset: next_slot_offset,
    // prev_slot est maintenant dans EpochRecord::new() — le passer explicitement :
};
let record = EpochRecord::new(
    next_epoch, flags, tsc_now, root_oid,
    input.root_disk_offset,
    DiskOffset(prev_slot_offset),   // ← LBA du slot précédent
);
LAST_COMMITTED_SLOT.store(input.slot_offset.0, Ordering::Release);
```

---

## Synthèse — Carte de la durabilité ExoFS

```
Chemin d'une écriture ExoFS aujourd'hui (snapshot 2026-05-20) :

SYS_EXOFS_OBJECT_WRITE
  → object_write.rs::write_blob()
    → BLOB_CACHE.write_at()                    [RAM ✓]
    → persist_cached_blob_if_disk()
      → virtio_adapter::has_global_disk()      [BAR 0x1000_0000 erroné → FALSE]
        → return Ok(false)                     [écriture disque IGNORÉE]

SYS_EXOFS_EPOCH_COMMIT
  → syscall/epoch_commit.rs::do_commit()
    → collect_epoch_blobs()                    [BLOB_CACHE → RAM]
    → save_journal()                           [BLOB_CACHE → RAM]
    → CURRENT_EPOCH.store()                    [RAM]
    → epoch/epoch_commit.rs::commit_epoch()    [JAMAIS APPELÉ]
    → SuperblockManager::commit()              [JAMAIS APPELÉ]
    → register_nvme_flush_fn                   [JAMAIS ENREGISTRÉ]

Résultat : 100% des données ExoFS sont exclusivement en RAM.
           Toutes disparaissent à chaque reboot.
```

### Ordre de correction recommandé

```
1. Corriger BAR VirtIO (P0-1, itération 2) — prérequis pour tout le reste
2. Enregistrer register_nvme_flush_fn() dans virtio_adapter.rs  (FS-P0-1)
3. Relier syscall/epoch_commit.rs → epoch/epoch_commit.rs       (FS-P1-1)
4. Persister OBJECT_STORE sur disque au commit + charger au montage (FS-P0-2)
5. Protéger blobs dirty dans evict_to_fit_except()              (FS-P0-3)
6. Enrichir path_resolve avec métadonnées réelles               (FS-P1-3)
7. Corriger TOCTOU dans object_delete                           (FS-P1-2)
8. Ajouter free-list dans OBJECT_STORE                          (FS-P1-4)
9. Passer prev_slot réel dans commit_epoch                      (FS-P2-2)
10. Réduire FD_MAX à 4096                                       (FS-P2-1)
```

---

## Table de concordance — Règles spec vs état du code

| Règle spec | Description | État |
|-----------|-------------|------|
| EPOCH-01 | 3 barrières NVMe obligatoires | ❌ No-ops — `register_nvme_flush_fn` non appelé |
| EPOCH-02 | Interdit d'omettre une barrière | ❌ Omises en permanence |
| EPOCH-03 | EPOCH_COMMIT_LOCK — un seul commit | ✅ `COMMIT_STATE` CAS correct |
| EPOCH-05 | Commit anticipé si EpochRoot > 500 | ✅ `should_force_commit()` implémenté |
| BACKUP-01 | 3 miroirs superblock à chaque commit | ❌ `SuperblockManager::commit()` jamais appelé depuis syscall |
| BACKUP-02 | Recovery sélectionne epoch le plus élevé | ⚠️ Logique correcte mais `prev_slot=0` empêche le replay |
| WRITE-02 | Vérification `bytes_written` | ✅ Vérifié dans `persist_blob_data_if_disk()` |
| HDR-03 | Vérifier magic + checksum avant accès | ✅ `superblock.rs::verify()` conforme |
| ONDISK-03 | Pas d'AtomicXxx dans `#[repr(C)]` | ✅ Respecté — AtomicXxx uniquement en mémoire |
| FS-10 | `EXO_BLAKE3\|EXO_DELAYED\|EXO_REFLINK` obligatoires | ✅ `incompat_flags::REQUIRED` dans `new_volume()` |
| GC-04 | Ne collecter jamais un epoch épinglé | ✅ `epoch_gc.rs` respecte les pins snapshots |
| DEAD-01 | GC ne jamais acquérir EPOCH_COMMIT_LOCK | ✅ Respecté — GC lit uniquement des atomiques |
| DAG-01 | `epoch/` n'importe pas `storage/` | ✅ Callbacks injectés via `CommitCallbacks` |
| SEC-07 | BlobId des Secrets jamais exposé | ✅ `ObjectKind::Secret` vérifié dans `object_stat.rs` |

---

*— Claude Delta, passe profonde ExoFS — snapshot kernel.zip 2026-05-20.*  
*Itération 4 — fait suite aux rapports des 2026-05-14 et 2026-05-20 (×3).*
