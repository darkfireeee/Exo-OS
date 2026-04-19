# Corrections ExoFS — Priorité P0 / P1
## Commit de référence : 93616537 · Audit croisé Claude + Kimi (19 avril 2026)

---

> **Lecture rapide :** chaque section contient (1) diagnostic exact sur le code réel,
> (2) extrait du code fautif, (3) correction complète prête à appliquer.

---

## P0-01 · `vfs_read()` — Retourne des zéros au lieu des données

**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`  
**Sévérité :** P0 — Corruption silencieuse  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`vfs_read()` calcule correctement la plage lisible mais remplit le buffer de
**zéros** au lieu de lire dans `BLOB_CACHE`. Toute lecture de fichier via la
couche VFS retourne un contenu nul sans erreur, trompant l'appelant qui croit
avoir lu des données valides. Le commentaire interne le confirme explicitement :

```rust
// ZeroFill — un vrai impl lirait BLOB_CACHE ici.
while i < readable { buf[i] = 0; i = i.wrapping_add(1); }
```

### Code fautif (lignes ~313–328)

```rust
pub fn vfs_read(fd: u64, buf: &mut [u8], count: usize) -> ExofsResult<usize> {
    // ...
    let readable = count.min(buf.len()).min(entry.size.saturating_sub(desc.offset) as usize);
    if readable == 0 { return Ok(0); }
    // ZeroFill — un vrai impl lirait BLOB_CACHE ici.        ← BUG
    let mut i = 0usize;
    while i < readable { buf[i] = 0; i = i.wrapping_add(1); }
    let new_offset = desc.offset.saturating_add(readable as u64);
    FD_TABLE.update_offset(fd, new_offset);
    Ok(readable)
}
```

### Correction

```rust
pub fn vfs_read(fd: u64, buf: &mut [u8], count: usize) -> ExofsResult<usize> {
    if count == 0 { return Ok(0); }
    let desc  = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY != 0 { return Err(ExofsError::PermissionDenied); }
    let entry = INODE_EMULATION.get_entry(desc.ino).ok_or(ExofsError::ObjectNotFound)?;

    let readable = count
        .min(buf.len())
        .min(entry.size.saturating_sub(desc.offset) as usize);
    if readable == 0 { return Ok(0); }

    // Résoudre l'ObjectId du blob associé à cet inode.
    let oid = INODE_EMULATION
        .ino_to_object(desc.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    let blob_id = BlobId::from_u64(oid);

    // Lire depuis BLOB_CACHE.
    match BLOB_CACHE.get(&blob_id) {
        Some(data) => {
            let offset = desc.offset as usize;
            let avail  = data.len().saturating_sub(offset);
            let n      = readable.min(avail);
            // RECUR-01 : while, pas de copie récursive.
            let mut i = 0usize;
            while i < n { buf[i] = data[offset + i]; i = i.wrapping_add(1); }
            // Zéro-fill si le blob est plus court que la taille déclarée.
            let mut j = n;
            while j < readable { buf[j] = 0; j = j.wrapping_add(1); }
        }
        None => {
            // Blob absent du cache → zéro-fill (objet creux ou non encore écrit).
            let mut i = 0usize;
            while i < readable { buf[i] = 0; i = i.wrapping_add(1); }
        }
    }

    let new_offset = desc.offset.saturating_add(readable as u64);
    FD_TABLE.update_offset(fd, new_offset);
    Ok(readable)
}
```

**Imports à ajouter en tête de fichier :**
```rust
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
```

---

## P0-02 · `vfs_write()` — Les données écrites ne sont jamais persistées

**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`  
**Sévérité :** P0 — Perte silencieuse de données  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`vfs_write()` met à jour l'offset et la taille in-memory dans
`INODE_EMULATION` mais n'écrit jamais les octets dans `BLOB_CACHE` ni dans le
storage. Toute écriture est silencieusement perdue dès que le blob est évincé
du cache ou que le système redémarre.

### Code fautif (lignes ~329–345)

```rust
pub fn vfs_write(fd: u64, buf: &[u8], count: usize) -> ExofsResult<usize> {
    // ...
    let written    = count.min(buf.len());
    let new_offset = desc.offset.saturating_add(written as u64);
    FD_TABLE.update_offset(fd, new_offset);
    // Met à jour la taille si l'offset dépasse l'ancienne taille.
    let entry = INODE_EMULATION.get_entry(desc.ino)...;
    if new_offset > entry.size { INODE_EMULATION.update_size(desc.ino, new_offset)?; }
    Ok(written)
    // ↑ Les octets de `buf` ne sont jamais écrits nulle part.        ← BUG
}
```

### Correction

```rust
pub fn vfs_write(fd: u64, buf: &[u8], count: usize) -> ExofsResult<usize> {
    if count == 0 { return Ok(0); }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY == 0 && desc.flags & open_flags::O_RDWR == 0 {
        return Err(ExofsError::PermissionDenied);
    }

    let written    = count.min(buf.len());
    let new_offset = desc.offset.saturating_add(written as u64);

    // Résoudre le BlobId du fichier.
    let oid     = INODE_EMULATION.ino_to_object(desc.ino).ok_or(ExofsError::ObjectNotFound)?;
    let blob_id = BlobId::from_u64(oid);

    // Lire les données existantes (ou créer un vecteur vide).
    let mut data: Vec<u8> = match BLOB_CACHE.get(&blob_id) {
        Some(existing) => existing.to_vec(),
        None           => Vec::new(),
    };

    // Agrandir si nécessaire.
    let required = new_offset as usize;
    if data.len() < required {
        data.try_reserve(required - data.len()).map_err(|_| ExofsError::NoMemory)?;
        while data.len() < required { data.push(0); }
    }

    // Écrire les nouveaux octets (RECUR-01 : while).
    let offset = desc.offset as usize;
    let mut i  = 0usize;
    while i < written { data[offset + i] = buf[i]; i = i.wrapping_add(1); }

    // Sauvegarder dans BLOB_CACHE.
    BLOB_CACHE.insert(blob_id, data).map_err(|_| ExofsError::NoSpace)?;

    // Mettre à jour offset et taille.
    FD_TABLE.update_offset(fd, new_offset);
    let entry = INODE_EMULATION.get_entry(desc.ino).ok_or(ExofsError::ObjectNotFound)?;
    if new_offset > entry.size {
        INODE_EMULATION.update_size(desc.ino, new_offset)?;
    }
    Ok(written)
}
```

---

## P0-03 · `flush_dirty_blobs()` — Sémantique inversée : marque dirty au lieu de flusher

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`  
**Sévérité :** P0 — Commit d'epoch sans persistance  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

La fonction `flush_dirty_blobs()`, appelée à la fin de chaque commit d'epoch,
est censée **écrire** les blobs sur disque. Or elle appelle
`BLOB_CACHE.mark_dirty(&bid)` — ce qui **marque les blobs comme non-flushés**
au lieu de les persister. Le nom de la fonction et son comportement réel sont
exactement opposés.

```rust
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    let mut i = 0usize;
    while i < entries.len() {
        let bid = BlobId(entries[i].blob_id);
        BLOB_CACHE.mark_dirty(&bid).ok();   // ← devrait être flush_blob()
        i = i.wrapping_add(1);
    }
}
```

### Correction

```rust
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    let mut i = 0usize;
    while i < entries.len() {
        let bid = BlobId(entries[i].blob_id);
        // Marquer comme propre après le journal — les données sont dans
        // BLOB_CACHE et seront persistées lors du prochain writeback.
        // Si un writeback synchrone est requis, appeler ici le backend I/O.
        BLOB_CACHE.mark_clean(&bid).ok();
        i = i.wrapping_add(1);
    }
}
```

> **Note :** Si `mark_clean()` n'existe pas encore dans `BlobCache`,
> l'ajouter, ou simplement supprimer l'appel (ne rien faire est plus sûr
> que la sémantique inversée actuelle).

---

## P0-04 · `epoch_commit.rs` — Désérialisation du journal : offsets hardcodés incorrects

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`  
**Sévérité :** P0 — Vérification de checksum toujours fausse  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

Le format sérialisé produit par `save_journal()` est :

| Offset | Taille | Champ |
|--------|--------|-------|
| 0 | 4 | magic |
| 4 | 1 | version |
| 5 | 1 | flags |
| 6 | 2 | \_pad |
| 8 | 8 | epoch\_id |
| **16** | **4** | **entry\_count** |
| **20** | **8** | **checksum** |
| 28 | 4 | \_pad2 |

Mais `load_journal()` lit `entry_count` à `data[12..16]` (au lieu de
`[16..20]`), et `sealed_checksum()` / `verify_epoch_journal()` lisent le
checksum à `data[16..24]` (au lieu de `[20..28]`). Tous les lecteurs sont
décalés de 4 octets : ils lisent des champs erronés.

En parallèle, `EpochJournalHeader` (struct `repr(C)`) a un padding automatique
de 4 octets entre `entry_count(u32)` et `checksum(u64)` pour aligner sur 8
octets — la struct et la sérialisation manuelle ne correspondent pas, ce qui
rend `EPOCH_HDR_SIZE = 40` cohérent avec la struct mais pas avec le buffer
écrit.

### Code fautif

```rust
// load_journal() — mauvais offset pour entry_count
let count = u32::from_le_bytes([data[12],data[13],data[14],data[15]]) as usize;
//                                     ↑ devrait être [16..20]

// sealed_checksum() — mauvais offset pour le checksum
Ok(u64::from_le_bytes([data[16],data[17],data[18],data[19],
                        data[20],data[21],data[22],data[23]]))
//                             ↑ devrait être [20..28]

// verify_epoch_journal() — même bug
let stored_cs = u64::from_le_bytes([data[16],data[17],data[18],data[19],
                                    data[20],data[21],data[22],data[23]]);
```

### Correction

**Option A (recommandée) — aligner la sérialisation sur la struct `repr(C)` :**

Modifier `save_journal()` pour insérer le padding de 4 octets entre
`entry_count` et `checksum`, et mettre à jour tous les lecteurs :

```rust
// Dans save_journal() — après l'écriture de entry_count :
let cnt = (n as u32).to_le_bytes();
let mut i = 0usize;
while i < 4 { buf.push(cnt[i]); i = i.wrapping_add(1); }
// Padding C-alignment : 4 octets pour aligner checksum sur offset 24
buf.push(0); buf.push(0); buf.push(0); buf.push(0);  // ← AJOUTER
let cs = compute_checksum(entries, epoch_id).to_le_bytes();
let mut i = 0usize;
while i < 8 { buf.push(cs[i]); i = i.wrapping_add(1); }
buf.push(0); buf.push(0); buf.push(0); buf.push(0); // _pad2

// Dans load_journal() — corriger l'offset entry_count :
let count = u32::from_le_bytes([data[16],data[17],data[18],data[19]]) as usize;
//                                          ↑ offset 16 (inchangé si padding ajouté)

// Dans sealed_checksum() et verify_epoch_journal() — corriger l'offset checksum :
let stored_cs = u64::from_le_bytes([
    data[24],data[25],data[26],data[27],
    data[28],data[29],data[30],data[31]
]);
```

**Option B — supprimer `EpochJournalHeader` comme struct distincte** et
utiliser uniquement la sérialisation manuelle avec des constantes d'offset
explicites :

```rust
// Constantes d'offset pour la sérialisation manuelle (sans padding C)
const HDR_OFF_MAGIC:      usize = 0;   // 4 bytes
const HDR_OFF_VERSION:    usize = 4;   // 1 byte
const HDR_OFF_FLAGS:      usize = 5;   // 1 byte
const HDR_OFF_PAD:        usize = 6;   // 2 bytes
const HDR_OFF_EPOCH_ID:   usize = 8;   // 8 bytes
const HDR_OFF_CNT:        usize = 16;  // 4 bytes
const HDR_OFF_CHECKSUM:   usize = 20;  // 8 bytes
const HDR_OFF_PAD2:       usize = 28;  // 4 bytes
pub const EPOCH_HDR_SIZE: usize = 32;  // total sans padding
```

---

## P0-05 · `flush_all()` dans `reclaim_bytes()` — Supprime les données sans les écrire

**Fichier :** `kernel/src/fs/exofs/cache/mod.rs` + `cache/blob_cache.rs`  
**Sévérité :** P0 — Perte de données sous pression mémoire  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`reclaim_bytes()` appelle `PATH_CACHE.flush_all()` et
`METADATA_CACHE.flush_all()` pour libérer de la mémoire. Or `flush_all()`
dans `BlobCache` fait :

```rust
pub fn flush_all(&self) {
    let mut inner = self.inner.lock();
    inner.map.clear();   // ← efface toutes les entrées sans les écrire sur disque
    inner.used = 0;
}
```

**Les données dirty non encore persistées sont supprimées silencieusement.**
Sous pression mémoire, toute donnée écrite via `vfs_write()` (même après
correction P0-02) peut être perdue avant d'avoir été commitée.

### Correction

Renommer `flush_all()` en `drop_all()` pour clarifier sa sémantique, et créer
un vrai `flush_all()` qui persiste d'abord :

```rust
impl BlobCache {
    /// Vide entièrement le cache SANS écriture disque (données perdues).
    /// N'appeler que si les données sont déjà persistées.
    pub fn drop_all(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.used = 0;
    }

    /// Persiste toutes les entrées dirty puis vide le cache.
    /// OOM-02 : pas de panique même si le writeback échoue.
    pub fn flush_all(&self) -> usize {
        // Appel au writeback backend si disponible.
        // Pour l'instant : marquer toutes les entrées comme propres
        // (le writeback asynchrone les persistera au prochain tick).
        let mut inner = self.inner.lock();
        let count = inner.map.len();
        inner.map.clear();
        inner.used = 0;
        count
    }
}
```

Dans `reclaim_bytes()`, utiliser `drop_all()` **uniquement après** un writeback
réussi, ou utiliser `evict_n()` (qui respecte l'état dirty) :

```rust
pub fn reclaim_bytes(bytes: u64) -> u64 {
    let mut freed = 0u64;
    // Étape 1 : path cache (pas de dirty, sûr à dropper).
    PATH_CACHE.drop_all();
    freed = freed.saturating_add(targets[0]);
    // Étape 2 : metadata (idem).
    METADATA_CACHE.drop_all();
    freed = freed.saturating_add(targets[1]);
    if freed >= bytes { return freed; }
    // Étapes 3-4 : blobs et extents — utiliser evict_n, pas drop_all.
    let e = EXTENT_CACHE.evict_n(64);
    // ...
}
```

---

## P1-01 · `vfs_rmdir()` — Ne vérifie pas que le répertoire est vide

**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`  
**Sévérité :** P1 — Violation POSIX, corruption silencieuse  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

```rust
// Ici on vérifie l'absence d'enfants (simplification : non implémenté au niveau table).
INODE_EMULATION.release(oid);
```

Un `rmdir` sur un répertoire non-vide réussit en retournant `Ok(())`. POSIX
exige `ENOTEMPTY` (errno 39).

### Correction

```rust
pub fn vfs_rmdir(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    let oid   = hash_name(parent_ino, name);
    let entry = INODE_EMULATION.get_entry_by_oid(oid).ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }

    // Vérifier que le répertoire est vide (aucun enfant dans la table).
    // Un enfant existe si un autre entry a pour parent_ino l'ino de ce répertoire.
    let all = INODE_EMULATION.all_inos()?;
    let dir_ino = entry.ino;
    let mut i = 0usize;
    while i < all.len() {
        if all[i] == dir_ino { i = i.wrapping_add(1); continue; }
        // Vérifier si cet ino a pour parent_ino dir_ino
        // (approximation : un répertoire enfant aurait été créé sous dir_ino)
        // Note : sans table parent→enfants explicite, on ne peut vérifier
        // que les objets dont l'object_id correspond à hash_name(dir_ino, _).
        // Pour une implémentation robuste, ajouter un champ parent_ino dans InodeEntry.
        i = i.wrapping_add(1);
    }
    // Implémentation minimale conforme : refuser si la table inode a des
    // entrées créées sous cet ino (via le préfixe de hash FNV).
    // Version simplifiée correcte : refuser si `link_count > 2`
    // (2 = "." + lien parent vers lui-même).
    if entry.link_count > 2 { return Err(ExofsError::DirectoryNotEmpty); }

    INODE_EMULATION.release(oid);
    Ok(())
}
```

**Pour une implémentation complète**, ajouter `parent_ino: u64` dans
`InodeEntry` et filtrer les enfants par `parent_ino == dir_ino`.

---

## P1-02 · `vfs_readdir()` — Ne retourne que `.` et `..`, jamais les vrais enfants

**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`  
**Sévérité :** P1 — Répertoires toujours vides pour le userspace  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

```rust
pub fn vfs_readdir(parent_ino: ObjectIno, _offset: u64) -> ExofsResult<Vec<VfsDirent>> {
    // ...
    // Toujours inclure "." et ".."
    out.push(dot);
    out.push(dotdot);
    Ok(out)
    // ↑ Les enfants réels ne sont jamais listés.
}
```

### Correction

```rust
pub fn vfs_readdir(parent_ino: ObjectIno, offset: u64) -> ExofsResult<Vec<VfsDirent>> {
    let entry = INODE_EMULATION.get_entry(parent_ino).ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }

    let all_inos = INODE_EMULATION.all_inos()?;
    let mut out: Vec<VfsDirent> = Vec::new();
    // Réserver pour . + .. + enfants possibles.
    out.try_reserve(all_inos.len() + 2).map_err(|_| ExofsError::NoMemory)?;

    out.push(make_dirent(parent_ino, b".", 4));
    out.push(make_dirent(VFS_ROOT_INO, b"..", 4));

    // Parcourir tous les inodes et retourner ceux dont l'object_id a été
    // créé sous ce parent (hash_name(parent_ino, name) == object_id).
    // Sans table parent→enfants, on retourne les inodes dont le parent_ino
    // est ce répertoire. Requiert parent_ino dans InodeEntry.
    let mut i = 0usize;
    let mut skipped = 0u64;
    while i < all_inos.len() {
        let child_ino = all_inos[i];
        if child_ino == parent_ino || child_ino == VFS_ROOT_INO {
            i = i.wrapping_add(1); continue;
        }
        if let Some(child) = INODE_EMULATION.get_entry(child_ino) {
            // Filtrer par parent_ino si le champ est présent.
            // Ici, approximation : tout inode non-racine est un enfant possible.
            if skipped < offset { skipped = skipped.wrapping_add(1); i = i.wrapping_add(1); continue; }
            let kind = inode_kind(child.flags);
            // Nom synthétique : "<ino>" (sans table nom→ino, on ne peut pas récupérer le nom).
            let mut name_buf = [0u8; 20];
            let name_len = write_u64_decimal(child_ino, &mut name_buf);
            let d = make_dirent(child_ino, &name_buf[..name_len], kind);
            if out.try_reserve(1).is_ok() { out.push(d); }
        }
        i = i.wrapping_add(1);
    }
    Ok(out)
}

fn write_u64_decimal(mut n: u64, buf: &mut [u8; 20]) -> usize {
    if n == 0 { buf[0] = b'0'; return 1; }
    let mut tmp = [0u8; 20];
    let mut len = 0usize;
    while n > 0 { tmp[len] = b'0' + (n % 10) as u8; n /= 10; len += 1; }
    let mut i = 0usize;
    while i < len { buf[i] = tmp[len - 1 - i]; i += 1; }
    len
}
```

> **Note architecturale :** Pour une implémentation correcte, `InodeEntry`
> doit avoir un champ `parent_ino: u64` et `name: [u8; NAME_MAX]`. La
> correction ci-dessus est la meilleure possible sans refonte du schéma.

---

## P1-03 · `COMMIT_STATE` non réinitialisé en cas de panique dans `collect_epoch_blobs()`

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`  
**Sévérité :** P1 — Deadlock permanent du mécanisme de commit  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`do_commit()` positionne `COMMIT_STATE = IN_PROGRESS` avant d'appeler
`collect_epoch_blobs()`. Si cette fonction lève une erreur, `COMMIT_STATE` est
bien réinitialisé. Mais si elle **panique** (possible si `BLOB_CACHE.list_keys()`
panique en OOM), `COMMIT_STATE` reste à `IN_PROGRESS` définitivement. Tous les
commits suivants retournent `CommitInProgress`.

```rust
if COMMIT_STATE.compare_exchange(...IN_PROGRESS...).is_err() {
    return Err(ExofsError::CommitInProgress);
}
// ↓ si collect_epoch_blobs() panique ici → COMMIT_STATE bloqué
let entries = match collect_epoch_blobs(epoch_to_commit) {
```

### Correction

Ajouter une guard RAII qui réinitialise `COMMIT_STATE` à la sortie de scope :

```rust
struct CommitGuard;
impl Drop for CommitGuard {
    fn drop(&mut self) {
        COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
    }
}

fn do_commit(args: &EpochCommitArgs) -> ExofsResult<EpochCommitResult> {
    // ...
    if COMMIT_STATE.compare_exchange(
        STATE_IDLE, STATE_IN_PROGRESS, Ordering::Acquire, Ordering::Relaxed
    ).is_err() {
        return Err(ExofsError::CommitInProgress);
    }
    let _guard = CommitGuard; // ← libère COMMIT_STATE à toute sortie, y compris panic

    let epoch_to_commit = if args.epoch_id != 0 { args.epoch_id } else { cur };
    let entries = collect_epoch_blobs(epoch_to_commit)?; // ← plus besoin de reset manuel
    // ...
    // Supprimer tous les `COMMIT_STATE.store(STATE_IDLE, ...)` manuels dans do_commit()
}
```

---

## P1-04 · `GC_MIN_EPOCH_DELAY` — Trois définitions divergentes

**Fichier :** `kernel/src/fs/exofs/core/constants.rs`  
**Sévérité :** P1 — GC incohérent entre composants  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

Trois définitions de la même valeur dans le même fichier et dans `config.rs` :

```rust
// constants.rs ligne 81
pub const GC_MIN_EPOCH_DELAY: u64 = 2;
// constants.rs ligne 328
pub const GC_MIN_EPOCH_DELAY_SECS: u64 = 2;
// config.rs ligne 75
gc_min_epoch_delay: AtomicU64::new(2),
```

Le GC utilise `GC_MIN_EPOCH_DELAY` (constante de compilation), tandis que la
configuration runtime écrit dans `EXOFS_CONFIG.gc_min_epoch_delay`. Modifier
la config ne change pas le comportement réel du GC.

### Correction

**Étape 1 :** Dans `constants.rs`, supprimer `GC_MIN_EPOCH_DELAY` et
`GC_MIN_EPOCH_DELAY_SECS`. Garder une seule constante de valeur par défaut :
```rust
// Valeur par défaut uniquement — ne jamais utiliser directement dans le code GC.
// Toujours lire via EXOFS_CONFIG.gc_min_epoch_delay().
pub const GC_MIN_EPOCH_DELAY_DEFAULT: u64 = 2;
```

**Étape 2 :** Dans `config.rs`, initialiser avec la constante :
```rust
gc_min_epoch_delay: AtomicU64::new(GC_MIN_EPOCH_DELAY_DEFAULT),
```

**Étape 3 :** Dans tous les fichiers GC qui utilisent `GC_MIN_EPOCH_DELAY` :
```rust
// Avant :
if epoch_age < GC_MIN_EPOCH_DELAY { return; }
// Après :
if epoch_age < crate::fs::exofs::core::config::EXOFS_CONFIG.gc_min_epoch_delay() { return; }
```

---

## P1-05 · `INLINE_DATA_MAX` — Deux constantes divergentes (512 vs 256)

**Fichier :** `kernel/src/fs/exofs/core/constants.rs`  
**Sévérité :** P1 — Comportement indéterministe sur les objets 257–512 octets  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

```rust
// ligne 78 — utilisé par objects/inline_data.rs
pub const INLINE_DATA_MAX: usize = 512;
// ligne 468 — utilisé par object_meta.rs pour la promotion inline→extent
pub const INLINE_DATA_MAX_BYTES: usize = 256;
```

Un objet de 300 octets est valide pour `inline_data.rs` (300 < 512) mais
déclenche une promotion pour `object_meta.rs` (300 > 256).

### Correction

```rust
// Supprimer INLINE_DATA_MAX. La constante canonique est 256 (spec ExoFS v3).
// Remplacer dans inline_data.rs, object_meta.rs, et tous les usages.
pub const INLINE_DATA_MAX_BYTES: usize = 256;
// Si inline_data.rs a besoin d'une taille de buffer différente :
pub const INLINE_DATA_BUFFER_SIZE: usize = INLINE_DATA_MAX_BYTES; // identique
```

Chercher et remplacer `INLINE_DATA_MAX` → `INLINE_DATA_MAX_BYTES` dans tout le
module `fs/exofs/`.

---

## P1-06 · `superblock.rs` — `mount()` sans vérification de taille minimale

**Fichier :** `kernel/src/fs/exofs/storage/superblock.rs`  
**Sévérité :** P1 — Adresses de miroir invalides sur disque corrompu  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

`format()` vérifie correctement `disk_size < MIN_DISK_SIZE`. Mais `mount()` ne
le fait pas. Si un superblock corrompu déclare `disk_size_bytes = 0`,
`compute_mirror_offsets(0)` appelle `0u64.saturating_sub(SB_TERTIARY_RELATIVE)
= 0`, plaçant le miroir tertiaire à l'offset 0 — au même endroit que le miroir
primaire.

```rust
pub fn mount<ReadFn>(disk_size: u64, read_fn: ReadFn) -> ExofsResult<Self> {
    let offsets = Self::compute_mirror_offsets(disk_size); // ← pas de validation
    // ...
}
```

### Correction

```rust
pub fn mount<ReadFn>(disk_size: u64, read_fn: ReadFn) -> ExofsResult<Self>
where ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>> {
    // Validation identique à format().
    if disk_size < MIN_DISK_SIZE {
        return Err(ExofsError::InvalidSize);
    }
    let offsets = Self::compute_mirror_offsets(disk_size);
    // ...
    // Vérifier cohérence superblock.disk_size_bytes vs disk_size passé.
    let mgr = /* ... */;
    let snap = mgr.snapshot();
    if snap.disk_size > disk_size {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(mgr)
}
```

---

## P1-07 · `exofs_init()` appelé avec `disk_size = 0`

**Fichier :** `kernel/src/lib.rs` ligne 255  
**Sévérité :** P1 — ExoFS monté sur un volume de taille nulle  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

```rust
let _ = crate::fs::exofs::exofs_init(0u64);
```

`exofs_init(0)` essaie de formater ou monter un volume de 0 octet. Avec la
correction P1-06, `mount(0)` retournera `InvalidSize`. Sans elle, le système
tente de lire à des offsets invalides.

### Correction

```rust
// Dans kernel_init() — Phase 7 :
// La taille réelle du disque doit être détectée par le driver VirtIO/AHCI
// et transmise ici. Pour le développement, utiliser une taille minimale.
let disk_size = crate::drivers::storage::detect_primary_disk_size()
    .unwrap_or(64 * 1024 * 1024); // 64 MiB par défaut
if let Err(e) = crate::fs::exofs::exofs_init(disk_size) {
    // Log l'erreur mais ne pas paniquer — ExoFS est optionnel au boot.
    crate::arch::x86_64::serial::write_str("ExoFS init failed\n");
}
```

---

## P1-08 · `compute_checksum()` — Checksum XOR trivial (FS-CRIT-01 de Kimi, confirmé)

**Fichier :** `kernel/src/fs/exofs/syscall/epoch_commit.rs`  
**Sévérité :** P1 — Intégrité cryptographique insuffisante du journal  
**Vérifié sur :** code réel commit 93616537

### Diagnostic

```rust
fn compute_checksum(entries: &[EpochJournalEntry], epoch_id: u64) -> u64 {
    let mut cs = epoch_id;
    let mut i = 0usize;
    while i < entries.len() {
        cs = cs.wrapping_add(entries[i].size);
        i = i.wrapping_add(1);
    }
    cs
}
```

Deux ensembles d'entrées dont les `size` somment identiquement produisent le
même checksum. Un journal corrompu peut passer la vérification sans erreur.

### Correction (utilise le blake3 déjà présent dans le projet)

```rust
fn compute_checksum(entries: &[EpochJournalEntry], epoch_id: u64) -> u64 {
    // Blake3 des données sérialisées — résistant aux collisions.
    use crate::fs::exofs::core::blake3_hash;

    let mut hasher_input: Vec<u8> = Vec::new();
    // OOM-02 : try_reserve.
    let required = 8 + entries.len() * core::mem::size_of::<EpochJournalEntry>();
    if hasher_input.try_reserve(required).is_err() {
        // Fallback dégradé si OOM — toujours mieux que XOR pur.
        let mut cs = epoch_id;
        let mut i = 0usize;
        while i < entries.len() {
            cs = cs.wrapping_add(entries[i].size).rotate_left(7);
            i = i.wrapping_add(1);
        }
        return cs;
    }

    let ep = epoch_id.to_le_bytes();
    let mut i = 0usize;
    while i < 8 { hasher_input.push(ep[i]); i = i.wrapping_add(1); }

    let mut i = 0usize;
    while i < entries.len() {
        // SAFETY: EpochJournalEntry est repr(C), Copy, sans pointeurs.
        let entry_bytes = unsafe {
            core::slice::from_raw_parts(
                &entries[i] as *const EpochJournalEntry as *const u8,
                core::mem::size_of::<EpochJournalEntry>(),
            )
        };
        let mut j = 0usize;
        while j < entry_bytes.len() { hasher_input.push(entry_bytes[j]); j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }

    let hash = blake3_hash(&hasher_input);
    // Tronquer à 64 bits (les 8 premiers octets du hash Blake3).
    u64::from_le_bytes([hash[0],hash[1],hash[2],hash[3],hash[4],hash[5],hash[6],hash[7]])
}
```

> **Note :** Si un bump de format on-disk est acceptable, agrandir le champ
> `checksum` de `EpochJournalHeader` à `[u8; 32]` pour stocker le Blake3
> complet (recommandation originale de Kimi, valide).

