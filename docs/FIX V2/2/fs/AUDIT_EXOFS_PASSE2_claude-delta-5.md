# ExoOS — Audit Passe Profonde ExoFS — 2e Passe — Snapshot 2026-05-20
## Rapport de stabilisation — Itération 5

**Rédigé par** : Claude Delta  
**Date** : 2026-05-20  
**Périmètre** : Passe 2 ExoFS — init, writeback, quota, crypto, posix_bridge, mmap, GC disk  
**Précédents rapports** : Itérations 1–4 (voir `docs/FIX V2/`)

---

## Correction du rapport précédent (Itération 4)

### FS-P0-1 révisé — NVMe flush hook : enregistré, mais neutralisé par le mauvais BAR

L'itération 4 affirmait que `register_nvme_flush_fn()` n'était jamais appelé. **C'est inexact.**

`exofs_init()` appelle `register_storage_flush_barrier()` qui enregistre bien le hook :

```rust
// kernel/src/fs/exofs/mod.rs:185
fn register_storage_flush_barrier() {
    crate::fs::exofs::epoch::epoch_barriers::register_nvme_flush_fn(
        crate::fs::exofs::storage::virtio_adapter::flush_global_disk,
    );
}
```

Le hook **est enregistré**. Mais `flush_global_disk()` appelle `with_global_disk(|d| d.flush())`, qui envoie la commande de flush au device VirtIO initialisé avec `base_address = 0x1000_0000` (BAR incorrect). Le flush atteint de la RAM, pas le contrôleur disque réel. L'effet pratique est identique à un no-op — mais le chemin d'enregistrement lui-même est correct.

**FS-P0-1 est donc reformulé** : le hook est enregistré, mais il est inefficace tant que le BAR VirtIO pointe sur `0x1000_0000`. La correction du BAR (P0-1, itération 2) résout **simultanément** l'inefficacité du flush et l'écriture physique des blocs.

---

## Sommaire des gravités — Passe 2 ExoFS

| Gravité | Nombre | Domaine |
|---------|--------|---------|
| **P0 — Bloquant** | 2 | BAR silencieux pire qu'une erreur, encryption irrécupérable |
| **P1 — Majeur** | 4 | Quota fantôme, 2e chemin d'écriture sans gardes, W^X absent, GC orphelin du disque |
| **P2 — Mineur** | 3 | Double SHA-256, MAC sans séparation de clés, `do_shutdown_commit` insuffisant |

---

## P0 — Incohérences Bloquantes

### FS2-P0-1 · `init_global_disk_with_mmio()` enregistre un device avec BAR invalide sans retourner d'erreur — les écritures corrompent la RAM silencieusement

**Fichier concerné** : `kernel/src/fs/exofs/storage/virtio_adapter.rs:65–77`

**Constat** :

```rust
pub fn init_global_disk_with_mmio(base_address: usize, capacity_bytes: usize) {
    let _ = register_global_disk(Arc::new(VirtioBlockAdapter::new(
        base_address,   // ← 0x1000_0000 = adresse RAM, pas BAR PCI
        capacity_bytes,
    )));
    // ← Aucune vérification de la validité du device
    // ← Aucune erreur retournée
}
```

`VirtioBlockAdapter::new()` n'effectue **aucune validation** de `base_address`. Il ne tente pas de lire les registres `VIRTIO_MAGIC_VALUE` (`0x74726976`) ni `VIRTIO_DEVICE_ID` pour vérifier qu'un device VirtIO est bien présent à cette adresse. Le device est enregistré inconditionnellement.

Conséquence : `has_global_disk()` retourne `true`, `persist_blob_data_if_disk()` s'exécute sans erreur, et chaque écriture bloc opère sur `0x1000_0000` — une adresse dans la plage RAM physique avec `-m 256M`. Les blocs de données ExoFS **écrasent de la RAM kernel** à des décalages arbitraires (calculés à partir de `next_lba`), sans jamais déclencher d'erreur ni de page fault (la plage est dans la physmap).

Ce comportement est **pire qu'un retour `false`** : au lieu de détecter l'absence de disque et de basculer en mode mémoire volatile, ExoFS opère en mode "disque présent" en corrompant silencieusement la RAM kernel.

**Impact observable** : après quelques dizaines d'opérations d'écriture ExoFS, des structures kernel résidant aux adresses `0x1000_0000 + N×512` (BLOB_CACHE, scheduler runqueue, etc.) peuvent être écrasées → kernel panic non reproductible ou corruption silencieuse.

**Correction** : Vérifier `VIRTIO_MAGIC_VALUE` et `VIRTIO_DEVICE_ID` avant d'enregistrer le device, et retourner une erreur si la validation échoue :

```rust
pub fn init_global_disk_with_mmio(base_address: usize, capacity_bytes: usize)
    -> Result<(), VirtioInitError>
{
    let adapter = VirtioBlockAdapter::new(base_address, capacity_bytes);

    // Lire le magic VirtIO pour vérifier que le device existe
    let magic = unsafe { core::ptr::read_volatile(base_address as *const u32) };
    if magic != VIRTIO_MAGIC_VALUE {
        return Err(VirtioInitError::NoDeviceAtAddress { base_address });
    }

    register_global_disk(Arc::new(adapter))
        .map_err(|_| VirtioInitError::AlreadyRegistered)?;
    Ok(())
}
```

`exofs_init()` doit propager cette erreur et ne pas enregistrer le hook de flush en cas d'échec.

---

### FS2-P0-2 · `KeyStorage` purement en mémoire — tous les objets chiffrés deviennent illisibles après reboot

**Fichiers concernés** :
- `kernel/src/fs/exofs/crypto/key_storage.rs` — `static KEY_STORAGE: KeyStorage` (BTreeMap en RAM)
- `kernel/src/fs/exofs/crypto/master_key.rs` — `MasterKey` zéroïsée au drop, jamais persistée
- `kernel/src/fs/exofs/crypto/mod.rs:175` — `KEY_STORAGE.load_key_256(slot_id)?` (lecture depuis RAM)
- Aucun mécanisme de sauvegarde/restauration des clés depuis le disque

**Constat** :

`KEY_STORAGE` est une `SpinLock<BTreeMap<KeySlotId, KeyEntry>>` initialisée vide à chaque démarrage. Les clés de chiffrement d'objets (`KeyKind::Object`, `KeyKind::Volume`) sont générées en session et insérées dans cette table. Aucune fonction `persist_key_storage()` n'existe. La `MasterKey` zéroïse ses octets au drop — ce qui est correct pour la sécurité mémoire, mais implique qu'elle est irrécupérable sans la passphrase.

Scénario de perte permanente :

```
Session 1 :
  - exo write secret.txt "données confidentielles"
  - ExoFS génère ObjectKey → insérée dans KEY_STORAGE (RAM)
  - Blob chiffré écrit dans BLOB_CACHE / disque

Reboot

Session 2 :
  - exo read secret.txt
  - KEY_STORAGE est vide → load_key_256(slot_id) → Err(KeyNotFound)
  - "Impossible de déchiffrer l'objet"
  - Les données sont sur disque mais INACCESSIBLES pour toujours
```

Le flag `EXO_ENCRYPT` dans `incompat_flags` peut être activé au format du volume. Sans mécanisme de persistance des clés, activer ce flag rend **l'intégralité du volume irrécupérable** après le premier reboot.

**Impact** : Tout objet `ObjectKind::Secret` écrit en session 1 est définitivement perdu en session 2. Ce bug est orthogonal à FS-P0-2 (OBJECT_STORE en mémoire) : même si le catalogue LBA était persisté, les données resteraient illisibles sans les clés.

**Correction** : Chiffrer et sérialiser `KeyStorage` sur disque à chaque commit epoch, en utilisant la `MasterKey` comme KEK. La clé maître elle-même doit être dérivée d'une passphrase persistée hors-disque (TPM, UEFI variable, ou prompt utilisateur au boot) :

```rust
// Sur commit epoch
fn commit_epoch(...) {
    // ...
    let wrapped_storage = KEY_STORAGE.serialize_and_wrap(&master_key)?;
    persist_blob_data_if_disk(KEY_STORAGE_BLOB_ID, &wrapped_storage, true)?;
}

// Sur mount
fn exofs_init(...) {
    // ...
    let raw = load_blob_data_if_available(KEY_STORAGE_BLOB_ID)?;
    if let Some(data) = raw {
        KEY_STORAGE.load_and_unwrap(&data, &master_key)?;
    }
}
```

---

## P1 — Incohérences Majeures

### FS2-P1-1 · Quota : `check_quota()` et `quota_check_write()` définis, compilés, jamais appelés

**Fichiers concernés** :
- `kernel/src/fs/exofs/syscall/quota_query.rs:262` — `pub fn check_quota()`
- `kernel/src/fs/exofs/quota/mod.rs:352` — `pub fn quota_check_write()`
- `kernel/src/fs/exofs/syscall/object_write.rs` — aucun appel
- `kernel/src/fs/exofs/syscall/object_create.rs` — aucun appel
- `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs:734` — `vfs_write()` aucun appel

**Constat** :

Grep exhaustif sur tous les sites d'écriture ExoFS :

```
check_quota       → 1 occurrence : quota_query.rs (définition + test)
quota_check_write → 1 occurrence : quota/mod.rs   (définition + test)
```

Ni `object_write.rs`, ni `object_create.rs`, ni `vfs_compat.rs::vfs_write()` n'appellent ces fonctions. Le système de quotas est fonctionnellement complet (limites `soft`/`hard` par UID, compteurs atomiques, `SYS_EXOFS_QUOTA_QUERY`) mais **n'est jamais consulté avant une opération d'écriture**.

Un processus Ring3 peut écrire en boucle jusqu'à saturer le cache (`MAX_BLOB_CACHE_SIZE`) ou le disque sans aucun retour `EDQUOT`. Le critère QUOTA-01 est formellement violé.

**Correction** : Ajouter un appel de vérification en tête de chaque chemin d'écriture :

```rust
// object_write.rs — write_blob()
fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    // Vérification quota avant toute écriture
    let owner_uid = current_process_uid();
    quota_check_write(
        QuotaKey::ByUid(owner_uid),
        data.len() as u64,
        tsc_now(),
    )?;
    // ... suite inchangée
}
```

Même ajout dans `vfs_write()` et `object_create()`.

---

### FS2-P1-2 · `vfs_write()` : 2e chemin d'écriture sans vérification `is_immutable()` ni quota

**Fichier concerné** : `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs:734–762`

**Constat** :

Il existe deux chemins d'écriture indépendants dans ExoFS :

| Chemin | Syscall | Vérifie `is_immutable()` | Vérifie quota |
|--------|---------|-------------------------|---------------|
| `object_write.rs::write_blob()` | `SYS_EXOFS_OBJECT_WRITE` (507) | ❌ Non (FS-P0-2, iter.4) | ❌ Non |
| `vfs_compat.rs::vfs_write()` | `SYS_WRITE` (1) via posix_bridge | ❌ Non | ❌ Non |

`vfs_write()` accède directement à `BLOB_CACHE.write_at()` sans consulter les métadonnées de l'objet cible :

```rust
// vfs_compat.rs:759
BLOB_CACHE.write_at(blob_id, start_offset, &buf[..written])?;
// ← Aucune vérification is_immutable()
// ← Aucune vérification quota
// ← Aucune vérification capability ExoFsObjectWrite
```

Le chemin POSIX (`write(2)`) contourne entièrement les gardes de sécurité du chemin natif ExoFS. Un processus Ring3 utilisant l'interface POSIX classique peut modifier ExoLedger ou tout objet `META_FLAG_IMMUTABLE`.

**Impact** : La correction de FS-P0-2 (itération 4) sur `object_write.rs` sera insuffisante si `vfs_write()` reste non protégé. Les deux chemins doivent être corrigés simultanément.

**Correction** :

```rust
pub fn vfs_write(fd: u64, buf: &[u8], count: usize) -> ExofsResult<usize> {
    // ... validations existantes ...
    let entry = INODE_EMULATION.get_entry(desc.ino)
        .ok_or(ExofsError::ObjectNotFound)?;

    // Vérification immutabilité (MÊME garde que object_write.rs)
    if let Ok(meta) = blob_meta_cache_get(&entry.blob_id) {
        if meta.is_immutable() {
            exoledger_append(current_pid(), LedgerEvent::WriteOnImmutable {
                blob_id: entry.blob_id
            });
            return Err(ExofsError::AccessDenied(AccessDeniedReason::Immutable));
        }
    }

    // Vérification quota
    quota_check_write(QuotaKey::ByUid(current_uid()), count as u64, tsc_now())?;

    // ... écriture BLOB_CACHE inchangée ...
}
```

---

### FS2-P1-3 · `mmap()` : `PROT_WRITE | PROT_EXEC` autorisé — W^X non appliqué

**Fichier concerné** : `kernel/src/fs/exofs/posix_bridge/mmap.rs:206–235`

**Constat** :

La validation `validate_args()` accepte toute combinaison valide des flags de protection, sans vérifier si `PROT_WRITE` et `PROT_EXEC` sont activés simultanément :

```rust
fn validate_args(length: u64, prot: u32, flags: u32) -> ExofsResult<()> {
    let known_prot = map_prot::PROT_READ | map_prot::PROT_WRITE
                   | map_prot::PROT_EXEC | map_prot::PROT_NONE;
    if prot & !known_prot != 0 {
        return Err(ExofsError::InvalidArgument);
    }
    // ← AUCUNE vérification PROT_WRITE & PROT_EXEC
    Ok(())
}
```

Un processus Ring3 peut créer un mapping `PROT_WRITE | PROT_EXEC` et y écrire du shellcode exécutable. La politique W^X (Write XOR Execute) d'ExoCage, correctement appliquée dans le kernel pour les pages statiques, est contournée via l'interface POSIX.

**Impact** : Exploitation triviale depuis Ring3 — `mmap(NULL, 4096, PROT_WRITE|PROT_EXEC, MAP_ANONYMOUS|MAP_PRIVATE, -1, 0)` crée une page RWX. Tout débordement de tampon exploitable peut placer du shellcode dans cette région. La VISION v0.2.0 §4.3 liste W^X comme invariant de sécurité.

**Correction** :

```rust
fn validate_args(length: u64, prot: u32, flags: u32) -> ExofsResult<()> {
    // Politique W^X : PROT_WRITE et PROT_EXEC mutuellement exclusifs
    if prot & map_prot::PROT_WRITE != 0 && prot & map_prot::PROT_EXEC != 0 {
        return Err(ExofsError::PolicyViolation(PolicyViolation::WxMapping));
    }
    // ... reste inchangé ...
}
```

Note : `mmap_protect()` (pour `mprotect(2)`) doit appliquer la même vérification.

---

### FS2-P1-4 · GC kthread : orphelins retirés de `BLOB_CACHE` mais leurs LBAs jamais libérés dans `OBJECT_STORE`

**Fichiers concernés** :
- `kernel/src/fs/exofs/mod.rs:120–137` — `exofs_gc_kthread()`
- `kernel/src/fs/exofs/syscall/gc_trigger.rs` — `run_gc_two_phase()`
- `kernel/src/fs/exofs/syscall/object_store.rs` — aucune fonction `free_lba()` appelée par GC

**Constat** :

`run_gc_two_phase()` identifie les blobs orphelins (tombstonés, non atteignables) et les retire de `BLOB_CACHE`. Mais il n'appelle **jamais** `OBJECT_STORE.free_lba()` pour récupérer les blocs disque correspondants :

```rust
// gc_trigger.rs — phase collect
fn collect_phase(candidates: &[BlobId]) -> GcResult {
    for blob_id in candidates {
        BLOB_CACHE.invalidate(blob_id);           // ← retiré du cache
        OBJECT_TABLE.remove_all_fds_for(blob_id); // ← fds fermés
        // ← OBJECT_STORE.free_lba(blob_id) ABSENT
    }
}
```

Ce problème est distinct de FS-P1-4 (itération 4) qui signalait l'absence de free-list dans l'allocateur. Ici la free-list pourrait exister que le GC ne l'alimenterait pas de toute façon.

La combinaison des deux bugs produit une fuite d'espace disque inconditionnelle :
- Chaque blob créé consomme des LBAs (via `reserve_for_write`)
- Chaque blob supprimé / collecté ne les libère jamais
- Sur un volume de 512 MiB avec des blobs de 64 KiB en moyenne : après ~8 192 suppressions, le disque est plein

**Correction** : Ajouter l'appel dans la phase collect :

```rust
fn collect_phase(candidates: &[BlobId]) -> GcResult {
    for blob_id in candidates {
        // Libérer les LBAs AVANT d'invalider le cache
        OBJECT_STORE.free_lba(blob_id);           // ← à ajouter
        BLOB_CACHE.invalidate(blob_id);
        OBJECT_TABLE.remove_all_fds_for(blob_id);
    }
}
```

---

## P2 — Incohérences Mineures

### FS2-P2-1 · `master_key.rs` : implémentation SHA-256 dupliquée — viole la règle S-06

**Fichier concerné** : `kernel/src/fs/exofs/crypto/master_key.rs:301–380`

**Constat** :

Le fichier `master_key.rs` contient une implémentation complète de SHA-256 (80 lignes, constants K, initialisation IV, compression) au lieu d'utiliser le module `security::crypto::kdf` qui délègue à la crate `sha2` :

```rust
// master_key.rs:301 — SHA-256 inline
fn sha256(msg: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [0x428a2f98, 0x71374491, ...];  // 64 constantes
    // ... 60 lignes d'implémentation ...
}

// security/crypto/kdf.rs:22 — SHA-256 via crate auditée
use sha2::{Sha256, Sha512};
```

Deux implémentations de la même primitive cryptographique dans le même kernel — une testée via la crate `sha2`, l'autre maison. La règle S-06 interdit ce type de duplication pour les primitives crypto. Une divergence de comportement entre les deux (sur des cas limites de padding, de blocs vides, ou d'overflow de longueur) pourrait causer des incohérences de vérification silencieuses.

**Correction** : Supprimer la fonction `sha256()` locale et la remplacer par un appel au module `security::crypto` :

```rust
// Remplacer sha256() locale par :
fn sha256(msg: &[u8]) -> [u8; 32] {
    crate::security::crypto::hash::sha256(msg)
}
```

---

### FS2-P2-2 · `compute_wrap_mac()` utilise la KEK comme clé MAC — violation de la séparation des clés

**Fichier concerné** : `kernel/src/fs/exofs/crypto/master_key.rs:259–272`

**Constat** :

```rust
fn compute_wrap_mac(magic, key_id, salt, ciphertext, mac_key: &[u8; 32]) -> [u8; 32] {
    // mac_key = KEK = même clé que celle utilisée pour le XOR de chiffrement
    hmac_sha256(mac_key, &data)
}
```

La même clé dérivée (`KEK = HKDF(passphrase, salt)`) est utilisée à la fois pour :
1. Chiffrer : `ciphertext[i] = master_key[i] ^ kek[i]`
2. Authentifier : `mac = HMAC-SHA256(kek, magic || key_id || salt || ciphertext)`

Cette réutilisation viole le principe de séparation des clés (NIST SP 800-108). Un attaquant qui obtient le `ciphertext` et le `mac` peut monter une attaque par extension de longueur sur HMAC-SHA256 (bien que difficile) ou exploiter des propriétés de la construction XOR si la KEK est partiellement connue.

La correction documentée dans le code (`// à remplacer par AES-256-KW en production`) indique que cette limitation est connue mais non adressée.

**Correction** : Dériver deux sous-clés distinctes depuis la même passphrase avec des contextes différents :

```rust
let kek_enc = HKDF(passphrase, salt, info = b"exofs-wrap-enc-v1");  // 32B pour XOR
let kek_mac = HKDF(passphrase, salt, info = b"exofs-wrap-mac-v1");  // 32B pour HMAC
ciphertext[i] = master_key[i] ^ kek_enc[i];
mac = HMAC-SHA256(kek_mac, magic || key_id || salt || ciphertext);
```

---

### FS2-P2-3 · `do_shutdown_commit()` appelle `do_commit()` (in-memory) — shutdown ne persiste rien

**Fichier concerné** :
- `kernel/src/fs/exofs/mod.rs:198–217` — `exofs_shutdown()`
- `kernel/src/fs/exofs/syscall/epoch_commit.rs:374–376` — `do_shutdown_commit() → do_commit()`

**Constat** :

```rust
// mod.rs:213
match do_shutdown_commit(&commit_args) {
    Ok(_) | Err(ExofsError::CommitInProgress) => {}
    Err(e) => return Err(e),
}
EXOFS_INITIALIZED.store(false, Ordering::Release);
```

`do_shutdown_commit()` appelle `do_commit()` — qui est le même chemin en-mémoire documenté en FS-P1-1 (itération 4). Le shutdown ExoFS ne persiste aucune donnée sur disque, n'appelle pas le protocole 3-barrières, et ne met pas à jour le superblock.

**Cas pratique** : `exos shutdown` lance `exofs_shutdown()` → `do_shutdown_commit()` → opérations RAM → retourne `Ok(())`. Le système s'éteint avec la conviction que les données sont sauvées. Ce n'est pas le cas.

Ce bug est une conséquence directe de FS-P1-1 (le vrai protocole disque n'est jamais appelé). Sa correction est donc **automatiquement résolue** par la correction de FS-P1-1 : une fois que `do_commit()` appellera le protocole 3-barrières, `do_shutdown_commit()` le fera aussi.

Il est documenté séparément pour clarifier que la séquence de shutdown semble correcte architecturalement (elle appelle bien un commit forcé) mais est neutralisée par le bug amont.

---

## Carte complète des incohérences ExoFS — Toutes passes confondues

### Vue par gravité

```
P0 — BLOQUANT (5 total)
  FS-P0-1†  BAR VirtIO 0x1000_0000 → flush sur RAM (résolution: lire BAR depuis PCI)
  FS2-P0-1  VirtioBlockAdapter::new() sans validation → RAM corrompue silencieusement
  FS-P0-2   OBJECT_STORE en RAM → catalogue LBA perdu au reboot
  FS-P0-3   evict_to_fit_except() évince les dirty sans flush (race 5s avec writeback)
  FS2-P0-2  KeyStorage en RAM → objets chiffrés illisibles après reboot
  † FS-P0-1 original reformulé : flush hook enregistré mais inefficace par BAR incorrect

P1 — MAJEUR (8 total)
  FS-P1-1   syscall EPOCH_COMMIT n'appelle pas epoch/epoch_commit.rs (protocole fantôme)
  FS-P1-2   object_delete : TOCTOU open_count ↔ invalidate
  FS-P1-3   path_resolve : kind=0, size=0 → stat() non fonctionnel
  FS-P1-4   OBJECT_STORE allocateur linéaire sans free-list
  FS2-P1-1  check_quota() jamais appelé depuis write/create
  FS2-P1-2  vfs_write() : pas is_immutable(), pas quota (2e chemin d'écriture)
  FS2-P1-3  mmap() : PROT_WRITE | PROT_EXEC autorisé → W^X bypassé
  FS2-P1-4  GC : BLOB_CACHE.invalidate() sans OBJECT_STORE.free_lba() → fuite LBA

P2 — MINEUR (5 total)
  FS-P2-1   [ObjectFdEntry; 65532] = 5.24 MiB BSS statique
  FS-P2-2   EpochRecord.prev_slot toujours DiskOffset(0) → recovery walk brisée
  FS2-P2-1  SHA-256 dupliqué dans master_key.rs (viole S-06)
  FS2-P2-2  KEK = clé MAC = même clé dans compute_wrap_mac()
  FS2-P2-3  do_shutdown_commit() → do_commit() in-memory (corollaire de FS-P1-1)
```

### Vue par composant

| Composant | Bugs | Critique |
|-----------|------|---------|
| `virtio_adapter.rs` | FS-P0-1, FS2-P0-1 | BAR incorrect + pas de validation device |
| `object_store.rs` | FS-P0-2, FS-P1-4, FS2-P1-4 | LBA catalog perdu, no free-list, GC ne libère pas |
| `blob_cache.rs` | FS-P0-3 | Dirty eviction sans flush |
| `epoch_commit.rs` (syscall) | FS-P1-1, FS2-P2-3 | Protocole disque jamais appelé |
| `key_storage.rs` + `master_key.rs` | FS2-P0-2, FS2-P2-1, FS2-P2-2 | Clés non persistées, SHA-256 dupliqué, MAC sans séparation |
| `object_delete.rs` | FS-P1-2 | TOCTOU |
| `path_resolve.rs` | FS-P1-3 | stat() non fonctionnel |
| `quota_query.rs` / `quota/mod.rs` | FS2-P1-1 | Quota non appliqué |
| `vfs_compat.rs` | FS2-P1-2 | 2e chemin sans gardes |
| `mmap.rs` | FS2-P1-3 | W^X absent |
| `object_fd.rs` | FS-P2-1 | 5.24 MiB BSS |
| `epoch_commit.rs` (core) | FS-P2-2 | prev_slot toujours 0 |

### Ordre de correction — priorité absolue

```
Étape 1 — Prérequis (aucune durabilité possible sans ces corrections) :
  1a. Lire BAR0 depuis PCI config space (FS-P0-1 / FS2-P0-1)
  1b. Valider magic VirtIO dans VirtioBlockAdapter::new() avec retour d'erreur

Étape 2 — Durabilité session courante :
  2a. Relier syscall/epoch_commit.rs → epoch/epoch_commit.rs (FS-P1-1)
  2b. Flusher dirty avant eviction dans evict_to_fit_except() (FS-P0-3)

Étape 3 — Durabilité cross-session :
  3a. Persister OBJECT_STORE sur disque au commit + restaurer au mount (FS-P0-2)
  3b. Persister KeyStorage chiffré au commit + restaurer au mount (FS2-P0-2)
  3c. Passer prev_slot réel dans EpochRecord (FS-P2-2)

Étape 4 — Intégrité et sécurité :
  4a. Vérifier is_immutable() dans write_blob() (FS-P0-2 iter.4)
  4b. Vérifier is_immutable() + quota dans vfs_write() (FS2-P1-2)
  4c. Appliquer check_quota() dans object_create/write (FS2-P1-1)
  4d. Rejeter PROT_WRITE | PROT_EXEC dans validate_args() (FS2-P1-3)
  4e. Corriger TOCTOU dans object_delete (FS-P1-2)

Étape 5 — Efficacité stockage :
  5a. Ajouter free_lba() dans GC collect_phase (FS2-P1-4)
  5b. Ajouter free-list dans OBJECT_STORE allocateur (FS-P1-4)

Étape 6 — Enrichissement fonctionnel :
  6a. Enrichir path_resolve avec métadonnées réelles (FS-P1-3)

Étape 7 — Nettoyage technique :
  7a. Supprimer sha256() inline dans master_key.rs (FS2-P2-1)
  7b. Séparer KEK et clé MAC dans compute_wrap_mac() (FS2-P2-2)
  7c. Réduire FD_MAX à 4096 dans object_fd.rs (FS-P2-1)
```

---

*— Claude Delta, passe profonde ExoFS — 2e passe — snapshot kernel.zip 2026-05-20.*  
*Itération 5 — fait suite aux rapports des 2026-05-14 et 2026-05-20 (×4).*
