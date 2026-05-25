# Audit Profond ExoFS — Incohérences & Corrections

**Auteur :** claude iota  
**Date :** 2026-05-21  
**Périmètre :** `kernel/src/fs/exofs/` — 250+ fichiers, lecture exhaustive des couches critiques  
**Méthode :** lecture du code source, traçage des chemins d'appel, vérification des invariants

---

## Résumé Exécutif

L'audit révèle **15 incohérences** dans ExoFS, dont **5 critiques** qui brisent complètement la persistance et la cohérence des données même avec un VirtIO BAR correct. Les corrections de CORR-IOTA-01 (BAR dynamique) sont nécessaires mais **pas suffisantes** : même avec le bon MMIO, ExoFS ne peut ni relire ses données au reboot, ni appliquer correctement le protocole de commit, ni protéger les objets immutables du GC.

**Sévérités :**

| ID | Titre court | Sévérité | Couche |
|---|---|---|---|
| FS-01 | `boot_recovery_sequence()` est un stub vide | CRITIQUE | Recovery |
| FS-02 | `do_commit()` n'appelle jamais les 3 barrières NVMe | CRITIQUE | Epoch |
| FS-03 | `flush_dirty_blobs()` marque dirty au lieu de flusher | CRITIQUE | Epoch |
| FS-04 | `collect_epoch_blobs()` lit l'epoch ID dans les données user | CRITIQUE | Epoch |
| FS-05 | LBA mapping non persisté → données illisibles après reboot | CRITIQUE | Storage |
| FS-06 | `is_immutable()` absent de `object_write` | ÉLEVÉE | Syscall |
| FS-07 | GC sweeper ignore `is_immutable()` | ÉLEVÉE | GC |
| FS-08 | Quota non appliqué sur write/create | ÉLEVÉE | Syscall |
| FS-09 | `CURRENT_EPOCH` déconnecté du superblock | ÉLEVÉE | Epoch |
| FS-10 | `next_lba` repart de 0 au reboot → écrasement disque | ÉLEVÉE | Storage |
| FS-11 | `incompat_flags::REQUIRED` non vérifié au montage | MOYENNE | Superblock |
| FS-12 | `object_id_from_blob()` dupliquée dans `object_create.rs` | MOYENNE | Syscall |
| FS-13 | Commentaire erroné "multiple de BLOCK_SIZE : 512B" | FAIBLE | Superblock |
| FS-14 | Buffer de lecture superblock 4096 octets pour 512 octets lus | FAIBLE | Superblock |
| FS-15 | `boot_recovery_sequence()` ignore `disk_size_bytes` | FAIBLE | Recovery |

---

## FS-01 — `boot_recovery_sequence()` : Stub Vide (CRITIQUE)

**Fichier :** `kernel/src/fs/exofs/recovery/boot_recovery.rs` ligne 369

```rust
// Code actuel — STUB INOPÉRANT :
pub fn boot_recovery_sequence(_disk_size_bytes: u64) -> ExofsResult<()> {
    RECOVERY_LOG.log_boot_start();
    RECOVERY_AUDIT.record_recovery_started(EpochId(0));
    // Le fsck complet nécessite un handle BlockDevice fourni par le storage
    // driver. Voir BootRecovery::run() pour la séquence complète.
    RECOVERY_LOG.log_boot_done();
    RECOVERY_AUDIT.record_recovery_completed(EpochId(0), 0);
    Ok(())  // ← retourne Ok sans rien faire
}
```

`exofs_init()` appelle `boot_recovery_sequence()` en Phase 1. Cette fonction est un stub qui retourne immédiatement `Ok(())`. La vraie séquence de récupération est dans `BootRecovery::run()` qui :
- Lit et sélectionne le meilleur miroir superblock (BACKUP-02)  
- Identifie le slot A/B/C avec l'epoch la plus récente  
- Rejoue l'epoch incomplète (`epoch_replay`)  
- Lance le fsck conditionnel  

Aucun de ces étapes n'est exécuté. ExoFS démarre toujours avec un état vierge, indépendamment de ce qui est sur le disque.

**Conséquence :** Même si les données sont correctement écrites sur disque (après correction FS-05), elles ne sont jamais relues au reboot.

**Correction requise :**
```rust
pub fn boot_recovery_sequence(disk_size_bytes: u64) -> ExofsResult<()> {
    RECOVERY_LOG.log_boot_start();
    RECOVERY_AUDIT.record_recovery_started(EpochId(0));

    // Récupérer le handle vers le disque global
    let result = crate::fs::exofs::storage::virtio_adapter::with_global_disk(|device| {
        let opts = BootRecoveryOptions::default();
        let mut recovery = BootRecovery::new(device, disk_size_bytes, opts);
        recovery.run()
    });

    match result {
        Ok(r) => {
            RECOVERY_AUDIT.record_recovery_completed(r.recovered_epoch, r.total_errors);
            // Synchroniser CURRENT_EPOCH avec l'epoch récupérée (FS-09)
            crate::fs::exofs::syscall::epoch_commit::set_current_epoch(
                r.recovered_epoch.0
            );
            RECOVERY_LOG.log_boot_done();
            Ok(())
        }
        Err(e) => {
            RECOVERY_LOG.log_boot_error();
            Err(e)
        }
    }
}
```

---

## FS-02 — `do_commit()` N'Appelle Jamais les 3 Barrières NVMe (CRITIQUE)

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs` lignes 296–360

Le protocole de commit à 3 barrières NVMe est implémenté dans `epoch/epoch_commit.rs::commit_epoch()`. Ce module est **orphelin** : aucun chemin d'appel ne le relie au handler syscall `SYS_EXOFS_EPOCH_COMMIT`.

Le `do_commit()` actuel :
1. Collecte les blobs de l'epoch (via `collect_epoch_blobs()` — elle-même cassée, FS-04)
2. Sauvegarde un journal en mémoire cache (`save_journal()` écrit dans BLOB_CACHE)
3. Appelle `flush_dirty_blobs()` qui... marque les blobs dirty (FS-03)
4. Avance `CURRENT_EPOCH` en RAM

Il ne fait **jamais** :
- `nvme_barrier_after_data()`
- `nvme_barrier_after_root()`
- `nvme_barrier_after_record()`
- `epoch::epoch_commit::commit_epoch()`

**Preuve :** Aucun import de `epoch::epoch_commit` dans `syscall/epoch_commit.rs`.

**Correction requise :** Relier `do_commit()` au vrai protocole. La séquence doit être :

```
1. Sérialiser et écrire le payload modifié sur disque
2. nvme_barrier_after_data()
3. Écrire l'EpochRoot sérialisé sur disque
4. nvme_barrier_after_root()
5. Écrire l'EpochRecord dans le slot A/B/C
6. nvme_barrier_after_record()
7. Avancer le superblock (sur disque via SuperblockManager::commit())
```

---

## FS-03 — `flush_dirty_blobs()` Marque Dirty au Lieu de Flusher (CRITIQUE)

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs` lignes 286–296

```rust
// Code actuel — NOM TROMPEUR, comportement inversé :
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    let mut i = 0usize;
    while i < entries.len() {
        let bid = BlobId(entries[i].blob_id);
        BLOB_CACHE.mark_dirty(&bid).ok(); // ← MARQUE dirty, ne flush PAS
        i = i.wrapping_add(1);
    }
}
```

Cette fonction est appelée **après** le commit pour indiquer que les blobs doivent être écrits sur disque. Or elle appelle `mark_dirty()` — le contraire de ce qu'un flush devrait faire. La persistance est entièrement déléguée au kthread `exofs-writeback` qui tourne toutes les 5 secondes.

**Impact :** Entre un `SYS_EXOFS_EPOCH_COMMIT` et le prochain cycle de writeback (jusqu'à 5s), les données engagées existent uniquement en RAM. Une panne secteur dans cette fenêtre détruit les données.

**Correction requise :**
```rust
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    let mut i = 0usize;
    while i < entries.len() {
        let bid = BlobId(entries[i].blob_id);
        // Écrire immédiatement sur disque (sync=true)
        if let Some(data) = BLOB_CACHE.get(&bid) {
            let _ = object_store::persist_blob_data_if_disk(bid, data.as_ref(), true);
            let _ = BLOB_CACHE.mark_clean(&bid); // marquer clean APRÈS écriture
        }
        i = i.wrapping_add(1);
    }
}
```

---

## FS-04 — `collect_epoch_blobs()` Lit l'Epoch ID dans les Données User (CRITIQUE)

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs` lignes 253–285

```rust
fn blob_epoch_id(blob_id: &BlobId) -> u64 {
    match BLOB_CACHE.get(blob_id) {
        Some(d) if d.len() >= 8 => {
            // LIT LES 8 PREMIERS OCTETS DES DONNÉES USER comme epoch ID
            u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
        }
        _ => 0,
    }
}
```

Cette fonction traite les 8 premiers octets du **contenu d'un fichier utilisateur** comme un identifiant d'epoch. Un fichier `hello.txt` dont le contenu commence par `hello\n` sera interprété comme ayant l'epoch `0x0A6F6C6C6568` (valeur hexadécimale de "hello\n").

En pratique, `collect_epoch_blobs(current_epoch)` ne collecte jamais les bons blobs : les fichiers utilisateur dont les 8 premiers octets coïncident accidentellement avec l'epoch ID seront collectés ; tous les autres seront ignorés.

**Correction requise :** L'epoch d'un blob doit être stocké dans ses **métadonnées**, pas dans son contenu.

```rust
// Option 1 : utiliser les métadonnées du fd (OBJECT_TABLE)
fn blob_epoch_id(blob_id: &BlobId) -> u64 {
    OBJECT_TABLE.get_by_blob(blob_id)
        .map(|entry| entry.epoch_id)
        .unwrap_or(0)
}

// Option 2 : utiliser un index séparé BlobId → EpochId
// static BLOB_EPOCH_INDEX: SpinLock<BTreeMap<BlobId, u64>> = ...
fn blob_epoch_id(blob_id: &BlobId) -> u64 {
    BLOB_EPOCH_INDEX.lock().get(blob_id).copied().unwrap_or(0)
}
```

---

## FS-05 — LBA Mapping Non Persisté → Données Illisibles Après Reboot (CRITIQUE)

**Fichier :** `kernel/src/fs/exofs/syscall/object_store.rs`

`ObjectStore` maintient un `BTreeMap<BlobId, PersistedBlobMapping>` en RAM qui associe chaque BlobId à sa plage LBA sur disque :

```rust
struct ObjectStoreInner {
    map: BTreeMap<BlobId, PersistedBlobMapping>,  // ← EN RAM UNIQUEMENT
    next_lba: u64,                                 // ← EN RAM UNIQUEMENT
}
```

Ce mapping **n'est jamais écrit sur disque**. Après un reboot :
- `OBJECT_STORE.map` est vide
- `load_blob_data_if_available()` appelle `OBJECT_STORE.lookup()` → `None` → retourne `Ok(None)`
- Les données sont sur le disque mais introuvables car l'index est perdu

De plus, `next_lba` repart de `DATA_LBA_START = 2048` à chaque reboot. Les nouvelles allocations **écrasent les données précédentes** au même offset (voir aussi FS-10).

**Correction requise :** Persister le LBA mapping dans une structure dédiée sur disque (ex. : un B-tree ou une table de hachage stockée dans les premiers blocs après les superblocks).

Chemin minimal :

```rust
// Sauvegarder l'index dans un bloc dédié à l'offset OBJECT_INDEX_LBA = 64
pub const OBJECT_INDEX_LBA: u64 = 64;

pub fn persist_index(write_fn: impl Fn(u64, &[u8]) -> ExofsResult<()>) -> ExofsResult<()> {
    let inner = self.inner.lock();
    let serialized = serialize_btree_map(&inner.map)?;
    write_fn(OBJECT_INDEX_LBA, &serialized)
}

pub fn load_index(read_fn: impl Fn(u64, &mut [u8]) -> ExofsResult<()>) -> ExofsResult<()> {
    // Lire et désérialiser le mapping au boot
    let map = deserialize_btree_map(read_fn(OBJECT_INDEX_LBA)?)?;
    let mut inner = self.inner.lock();
    inner.map = map;
    inner.next_lba = map.values().map(|m| m.base_lba + m.allocated_blocks).max().unwrap_or(DATA_LBA_START);
    Ok(())
}
```

---

## FS-06 — `is_immutable()` Absent de `object_write` (ÉLEVÉE)

**Fichier :** `kernel/src/fs/exofs/syscall/object_write.rs`

Confirmé dans l'audit BLOC 2 (INC-S19). Aucune vérification de `obj.is_immutable()` avant l'écriture. Voir `CORRECTIONS_BLOCS_0_2_11_CLAUDE_IOTA.md` CORR-IOTA-12 pour la correction.

---

## FS-07 — GC Sweeper Ignore `is_immutable()` (ÉLEVÉE)

**Fichier :** `kernel/src/fs/exofs/gc/sweeper.rs` — `sweep_batch()` lignes 253–295

```rust
fn sweep_batch(&self, batch: &[BlobId], current_epoch: EpochId) -> ExofsResult<BatchSweepResult> {
    for &blob_id in batch {
        // Vérifie : epoch pinnée
        if is_epoch_pinned(create_epoch) { continue; }
        // Vérifie : refcount > 0
        if rc > 0 { continue; }
        // ← MANQUE : if blob_is_immutable(&blob_id) { continue; }
        // Place le blob dans la file de suppression différée
        BLOB_REFCOUNT.queue_zero(&blob_id, current_epoch)?;
    }
}
```

Un blob immutable (ex. : log d'audit ExoLedger, image de référence ExoPhoenix, snapshot protégé) peut avoir un refcount de 0 s'il n'est référencé par aucun objet actif mais doit être conservé pour l'intégrité du système. Le GC le collecterait et l'effacerait définitivement.

**Correction requise :**
```rust
fn sweep_batch(&self, batch: &[BlobId], current_epoch: EpochId) -> ExofsResult<BatchSweepResult> {
    for &blob_id in batch {
        if is_epoch_pinned(create_epoch) { continue; }
        if rc > 0 { continue; }

        // ── CORRECTION FS-07 : ne pas supprimer les blobs immutables ──
        if is_immutable_blob(&blob_id) {
            br.pinned_skipped = br.pinned_skipped.saturating_add(1);
            continue;
        }
        // ─────────────────────────────────────────────────────────────

        BLOB_REFCOUNT.queue_zero(&blob_id, current_epoch)?;
    }
}

/// Vérifie si un blob est marqué immutable dans ses métadonnées.
fn is_immutable_blob(blob_id: &BlobId) -> bool {
    // Via le BLOB_CACHE ou un registre d'immutabilité séparé
    crate::fs::exofs::objects::logical_object::is_immutable_by_blob_id(blob_id)
}
```

---

## FS-08 — Quota Non Appliqué sur Write/Create (ÉLEVÉE)

**Fichiers :** `syscall/object_write.rs` · `syscall/object_create.rs`

La fonction `check_quota()` existe dans `syscall/quota_query.rs` :

```rust
pub fn check_quota(owner_uid: u64, extra_bytes: u64, extra_objects: u64) -> ExofsResult<()> {
    // ... logique complète d'enforcement ...
}
```

Mais elle n'est **jamais appelée** depuis `object_write.rs` ou `object_create.rs`. Les quotas sont définis, configurables, audités — mais jamais appliqués sur les opérations d'écriture.

**Correction requise — `object_write.rs` dans `write_blob()`:**
```rust
fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    // ── CORRECTION FS-08 : vérifier le quota avant écriture ──────────
    let owner = OBJECT_TABLE.get_by_blob(&blob_id)
        .map(|e| e.owner_uid)
        .unwrap_or(0);
    crate::fs::exofs::syscall::quota_query::check_quota(owner, data.len() as u64, 0)?;
    // ─────────────────────────────────────────────────────────────────

    let write_end = offset.checked_add(data.len() as u64)
        .ok_or(ExofsError::OffsetOverflow)?;
    // ... suite inchangée ...
}
```

**Correction requise — `object_create.rs` dans `create_object()`:**
```rust
fn create_object(path_bytes: &[u8], path_len: usize, args: &CreateArgs) -> ExofsResult<CreateResult> {
    // ── CORRECTION FS-08 : vérifier le quota avant création ──────────
    crate::fs::exofs::syscall::quota_query::check_quota(
        args.owner_uid,
        args.initial_size,
        1, // +1 objet
    )?;
    // ─────────────────────────────────────────────────────────────────
    // ... suite inchangée ...
}
```

---

## FS-09 — `CURRENT_EPOCH` Déconnecté du Superblock (ÉLEVÉE)

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs` ligne 31

```rust
// Epoch locale au module syscall — jamais synchronisée avec le superblock :
static CURRENT_EPOCH: AtomicU64 = AtomicU64::new(1);
```

`SuperblockManager` maintient sa propre `epoch: AtomicU64` dans `SuperblockInMemory`. Ces deux compteurs d'epoch vivent en parallèle et ne se parlent jamais :

- `syscall/epoch_commit.rs::current_epoch()` → lit `CURRENT_EPOCH` (toujours 1 au boot)
- `storage/superblock.rs::current_epoch()` → lit `SuperblockInMemory::epoch`

Après un boot avec recovery (FS-01 corrigé), l'epoch lue depuis le superblock serait, par exemple, 47. Mais `CURRENT_EPOCH` reste à 1. Toutes les opérations de commit sur l'epoch 1 écrasent silencieusement les données de l'epoch 47.

**Correction requise :** Exposer une fonction `set_current_epoch(u64)` dans `syscall/epoch_commit.rs` et l'appeler depuis `boot_recovery_sequence()` après recovery :

```rust
// syscall/epoch_commit.rs — ajouter :
pub fn set_current_epoch(epoch: u64) {
    CURRENT_EPOCH.store(epoch, Ordering::SeqCst);
}

// boot_recovery.rs — correction FS-01 appelle :
crate::fs::exofs::syscall::epoch_commit::set_current_epoch(
    r.recovered_epoch.0
);
```

---

## FS-10 — `next_lba` Repart de 0 au Reboot (ÉLEVÉE)

**Fichier :** `kernel/src/fs/exofs/syscall/object_store.rs` lignes 29–36

```rust
impl ObjectStoreInner {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            next_lba: 0,  // ← repart toujours de 0
        }
    }
}
```

À chaque reboot, `reserve_for_write()` recommence les allocations LBA depuis `DATA_LBA_START = 2048`. Les nouveaux blobs écrasent **au même offset** les données écrites lors du boot précédent.

Exemple :
- **Boot 1** : `fichier_a` → LBA 2048..2056, `fichier_b` → LBA 2056..2064
- **Boot 2** : `fichier_c` → LBA 2048..2056 (écrase `fichier_a`)

**Correction :** Cette incohérence est résolue par FS-05 (persister l'index, relire `next_lba` au reboot).

---

## FS-11 — `incompat_flags::REQUIRED` Non Vérifié au Montage (MOYENNE)

**Fichier :** `kernel/src/fs/exofs/storage/superblock.rs` — `ExoSuperblockDisk::verify()`

```rust
pub fn verify(&self) -> ExofsResult<()> {
    if self.magic != EXOFS_MAGIC {
        return Err(ExofsError::BadMagic);        // ✓ magic vérifié
    }
    if self.version_major != FORMAT_VERSION_MAJOR {
        return Err(ExofsError::InvalidArgument); // ✓ version vérifiée
    }
    // Checksum vérifié ✓
    // ...
    // MANQUE : vérification des incompat_flags obligatoires
    Ok(())
}
```

La règle FS-10 (déclarée dans le code) dit que `EXO_BLAKE3 | EXO_DELAYED | EXO_REFLINK` sont obligatoires. Mais `verify()` ne les vérifie pas. Un volume formaté sans ces flags (ex. : image de développement ancienne) serait monté sans erreur.

**Correction requise :**
```rust
pub fn verify(&self) -> ExofsResult<()> {
    if self.magic != EXOFS_MAGIC {
        return Err(ExofsError::BadMagic);
    }
    if self.version_major != FORMAT_VERSION_MAJOR {
        return Err(ExofsError::InvalidArgument);
    }
    // ── CORRECTION FS-11 : vérifier les flags incompatibles ──────────
    let missing = incompat_flags::REQUIRED & !self.incompat_flags;
    if missing != 0 {
        return Err(ExofsError::IncompatibleFeatures);
    }
    // ─────────────────────────────────────────────────────────────────
    // Checksum ...
    let expected = self.compute_checksum();
    // ...
    Ok(())
}
```

---

## FS-12 — `object_id_from_blob()` Dupliquée (MOYENNE)

**Fichier :** `kernel/src/fs/exofs/syscall/object_create.rs`

La fonction `object_id_from_blob()` est définie deux fois dans le même fichier :

```rust
// Définition 1 — non-pub, lignes ~200 :
fn object_id_from_blob(blob_id: &BlobId) -> ObjectId {
    let mut obj_bytes = [0u8; 32];
    let bid_bytes = blob_id.as_bytes();
    let mut i = 0usize;
    while i < 32 {
        obj_bytes[i] = bid_bytes[i] ^ 0x5A;  // XOR 0x5A
        i = i.wrapping_add(1);
    }
    ObjectId(obj_bytes)
}

// Définition 2 — standalone à la fin du fichier, ligne ~220 :
fn object_id_from_blob(blob_id: &BlobId) -> ObjectId {
    // ... même logique XOR 0x5A ...
}
```

La duplication ne cause pas d'erreur de compilation car l'une shadow l'autre selon le scope, mais crée un risque de divergence si l'une est modifiée et pas l'autre.

**Correction requise :** Supprimer l'une des deux définitions. Idéalement, déplacer `object_id_from_blob()` dans `core/blob_id.rs` comme fonction utilitaire partagée.

---

## FS-13 — Commentaire Erroné "Multiple de BLOCK_SIZE : 512B" (FAIBLE)

**Fichier :** `kernel/src/fs/exofs/storage/superblock.rs` ligne 35

```rust
/// Taille de la structure superblock sur disque (multiple de BLOCK_SIZE : 512B)
pub const SUPERBLOCK_DISK_SIZE: usize = 512;
```

512 n'est PAS un multiple de `BLOCK_SIZE = 4096`. C'est un sous-multiple. Le commentaire dit le contraire de la vérité.

**Correction requise :**
```rust
/// Taille de la structure superblock sur disque : 512 octets (1/8 de BLOCK_SIZE).
/// La structure est écrite en 512 octets ; les octets restants jusqu'à BLOCK_SIZE
/// ne font pas partie du superblock.
pub const SUPERBLOCK_DISK_SIZE: usize = 512;
```

---

## FS-14 — Buffer de Lecture Superblock 4096 Octets Pour 512 Octets Lus (FAIBLE)

**Fichier :** `kernel/src/fs/exofs/storage/superblock_backup.rs` ligne 216

```rust
fn read_superblock_mirror(...) -> MirrorReadResult {
    let sb_size = size_of::<ExoSuperblockDisk>(); // = 512
    let mut buf = [0u8; 4096];                   // ← 4096 mais lit sb_size = 512
    let n = match read_fn(offset, &mut buf[..sb_size]) {
```

Le buffer de 4096 octets sur la stack est alloué mais seuls 512 octets sont utilisés. Cela gaspille inutilement 3584 octets de stack dans un contexte kernel no_std où la stack est précieuse.

**Correction requise :**
```rust
let mut buf = [0u8; 512]; // = SUPERBLOCK_DISK_SIZE
let n = match read_fn(offset, &mut buf) {
```

---

## FS-15 — `boot_recovery_sequence()` Ignore `disk_size_bytes` (FAIBLE)

**Fichier :** `kernel/src/fs/exofs/recovery/boot_recovery.rs` ligne 369

```rust
pub fn boot_recovery_sequence(_disk_size_bytes: u64) -> ExofsResult<()> {
//                              ↑ préfixe _ = intentionnellement inutilisé
```

Le paramètre `disk_size_bytes` est préfixé `_` ce qui indique intentionnellement qu'il est ignoré. Il est pourtant nécessaire pour :
- Calculer l'offset du miroir tertiaire du superblock (`disk_size - 4 KiB`)
- Valider la capacité minimale (`MIN_DISK_SIZE = 16 MiB`)
- Initialiser le `SuperblockManager::mount(disk_size, ...)`

Ce point est résolu par FS-01 (correction du stub).

---

## Cartographie des Dépendances Entre Bugs FS

```
FS-01 (boot_recovery stub)
  └─ FS-15 (disk_size ignoré)
  └─ FS-09 (CURRENT_EPOCH pas synchronisé après recovery)
       └─ FS-02 (do_commit écrit à l'epoch 1 même si epoch=47)

FS-02 (pas de 3 barrières)
  └─ FS-03 (flush_dirty marque dirty)
  └─ FS-04 (collect_epoch_blobs lit les data user)

FS-05 (LBA mapping non persisté)
  └─ FS-10 (next_lba repart de 0)

FS-06 (is_immutable absent write)
  └─ FS-07 (GC ignore is_immutable)
```

---

## Ordre de Correction Recommandé

```
1. FS-05 + FS-10  → Persister l'index LBA (blocage fondamental)
2. FS-04          → Stocker l'epoch dans les métadonnées blob
3. FS-02 + FS-03  → Relier do_commit() au vrai protocole 3 barrières
4. FS-09          → Synchroniser CURRENT_EPOCH au boot
5. FS-01 + FS-15  → Déstubbifier boot_recovery_sequence()
6. FS-06 + FS-07  → is_immutable() dans write et GC sweeper
7. FS-08          → Appeler check_quota() sur write/create
8. FS-11          → Vérifier incompat_flags au montage
9. FS-12          → Dédupliquer object_id_from_blob()
10. FS-13 + FS-14 → Corrections cosmétiques
```

---

## Test de Validation Post-Correction

```bash
# Test 1 : Persistance de base
echo "persistence_test" > /data/probe.txt
sync  # force epoch commit
reboot
cat /data/probe.txt   # doit afficher "persistence_test"

# Test 2 : Epoch correcte après reboot
exosh> exofs_stat  # affiche epoch courante
# [ExoFS] epoch=47 (lu depuis superblock)
# et non epoch=1 (valeur initiale hardcodée)

# Test 3 : Protection immutabilité
exosh> exofs_setimmutable /audit/log.bin
exosh> echo "tamper" > /audit/log.bin
# [ExoFS] EPERM : write refusé sur objet immutable
# [ExoLedger] WriteAttemptOnImmutable blob_id=... pid=42

# Test 4 : GC ne supprime pas les immutables
exosh> exofs_gc_run
exosh> cat /audit/log.bin  # doit toujours être lisible

# Test 5 : Quota appliqué
exosh> exofs_quota_set uid=1000 bytes=1M
exosh> dd if=/dev/zero of=/data/bigfile bs=1M count=2
# [ExoFS] ENOSPC : quota dépassé (2M > 1M)

# Test 6 : Commit 3 barrières
# Vérifier que chaque commit génère 3 NVMe flush dans les stats
exosh> exofs_stats | grep barriers
# barriers_data=N barriers_root=N barriers_record=N (N > 0 et égaux)
```

---

*claude iota — AUDIT_PROFOND_EXOFS_CLAUDE_IOTA.md — 2026-05-21*
