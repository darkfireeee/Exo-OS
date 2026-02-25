
# **Documentation cohérente par fichier**

## **1. `kernel/src/fs/mod.rs` — Point d’entrée VFS**
Ce fichier expose **l’API publique du VFS**. C’est le point d’entrée unique utilisé par le noyau pour toutes les opérations de fichiers : ouverture, lecture, écriture, montage, etc.  
Il centralise l’accès aux sous-modules (`core/`, `io/`, `cache/`, etc.) et sert de façade.

---

# **2. `kernel/src/fs/core/` — Couche d’abstraction VFS**

### **`core/mod.rs`**
Déclare les sous-modules du VFS et organise la couche d’abstraction.

### **`core/vfs.rs`**
Définit **l’interface VFS** : une table d’opérations (ops table) que chaque filesystem doit enregistrer lors du montage.  
C’est ici que les drivers ext4plus, ext4 et fat32 exposent leurs callbacks (read, write, lookup, mkdir…).

### **`core/inode.rs`**
Implémente les **inodes VFS**, avec un chemin de lecture **RCU lock-free**.  
Les inodes abstraits pointent vers les inodes réels des FS.

### **`core/dentry.rs`**
Gère le **dentry cache** (chemins → inodes), également basé sur RCU.  
Permet des résolutions de chemins rapides.

### **`core/descriptor.rs`**
Implémente les **file descriptors** et la **fdtable par processus**.  
Chaque `open()` crée un descriptor abstrait lié à un inode VFS.

### **`core/superblock.rs`**
Définit le **superblock générique VFS**, distinct des superblocks propres à chaque FS.  
Chaque filesystem monté correspond à une instance ici.

### **`core/types.rs`**
Contient les types communs : `FileMode`, `Permissions`, `Stat`, `Dirent`, etc.

---

# **3. `kernel/src/fs/io/` — Couche I/O**

### **`io/mod.rs`**
Organisation de la couche I/O.

### **`io/uring.rs`**
Backend natif **io_uring** pour I/O asynchrone haute performance.

### **`io/zero_copy.rs`**
Implémente `sendfile` et `splice` via **DMA zero-copy**.

### **`io/aio.rs`**
Compatibilité **POSIX AIO**.

### **`io/mmap.rs`**
Gestion des fichiers **memory-mapped**.

### **`io/direct_io.rs`**
Support du flag **O_DIRECT**, qui contourne le page cache.

### **`io/completion.rs`**
Gestion des **queues de complétion I/O**.

---

# **4. `kernel/src/fs/cache/` — Caches partagés**

### **`cache/mod.rs`**
Organisation des caches.

### **`cache/page_cache.rs`**
Implémente le **page cache LRU**.  
Gère le flag **DIRTY** et le mécanisme de **delayed allocation**.  
Le **writeback thread** vide le cache toutes les 5 secondes.

### **`cache/dentry_cache.rs`**
Cache des dentries (hashmap + LRU).

### **`cache/inode_cache.rs`**
Cache des métadonnées d’inodes.

### **`cache/buffer.rs`**
Buffer cache pour la couche bloc.

### **`cache/prefetch.rs`**
Readahead adaptatif basé sur heuristiques IA.

### **`cache/writeback.rs`**
Nouveau module :  
- thread writeback  
- vidage des pages DIRTY vers disque  
- **allocation réelle des blocs** (delayed alloc)

### **`cache/eviction.rs`**
Implémente le shrinker utilisé par `memory/utils/shrinker.rs`.

---

# **5. `kernel/src/fs/integrity/` — Intégrité (ext4plus uniquement)**

### **`integrity/mod.rs`**
Organisation du module.

### **`integrity/checksum.rs`**
Checksum **Blake3**.  
⚠️ **Interdit** dans ext4 et fat32.

### **`integrity/journal.rs`**
Journal **WAL** :  
- Mode Data=Ordered  
- Journal uniquement pour les **métadonnées**  
- Les données sont écrites directement à leur emplacement final

### **`integrity/recovery.rs`**
Relecture du journal après crash.

### **`integrity/scrubbing.rs`**
Vérification périodique des données.

### **`integrity/healing.rs`**
Auto-réparation via Reed-Solomon.

### **`integrity/validator.rs`**
Hooks de validation d’intégrité.

---

# **6. `kernel/src/fs/ext4plus/` — Filesystem principal Exo‑OS**

### **`ext4plus/mod.rs`**
Organisation du FS.

### **`ext4plus/superblock.rs`**
Superblock ext4plus sur disque.  
Contient les **incompat flags** obligatoires :  
`EXO_BLAKE3 | EXO_DELAYED`.

### **`ext4plus/group_desc.rs`**
Descripteurs de groupes de blocs.

---

## **6.1. `ext4plus/inode/`**

### **`inode/ops.rs`**
Implémente read/write/truncate.  
Garantit que **r15 est préservé** (switch_asm.s).

### **`inode/extent.rs`**
Gestion de l’extent tree.  
Implémente les **reflinks** :  
- un nouvel inode partage les mêmes blocs  
- CoW uniquement lors de la modification

### **`inode/xattr.rs`**
Attributs étendus.

### **`inode/acl.rs`**
ACLs.

---

## **6.2. `ext4plus/directory/`**

### **`directory/htree.rs`**
Indexation HTree en O(log n).

### **`directory/linear.rs`**
Fallback linéaire.

### **`directory/ops.rs`**
Opérations sur répertoires.

---

## **6.3. `ext4plus/allocation/`**

### **`allocation/balloc.rs`**
Allocateur de blocs.  
⚠️ **N’alloue jamais pendant write()** (delayed alloc).  
Allocation uniquement via le writeback thread.

### **`allocation/mballoc.rs`**
Allocateur multi-blocs cherchant un **gros bloc contigu** pour tout le batch dirty.

### **`allocation/prealloc.rs`**
Préallocation spéculative.

---

# **7. `kernel/src/fs/drivers/` — Drivers tiers isolés**

### **`drivers/mod.rs`**
Registre des drivers FS chargés au boot.  
Isolation stricte :  
- aucun partage de code avec ext4plus  
- pas de Blake3  
- pas de journaling ext4plus

---

## **7.1. `drivers/ext4/` — Ext4 standard Linux**

### **`ext4/mod.rs`**
Organisation du driver.

### **`ext4/superblock.rs`**
Lecture du superblock ext4 (magic = 0xEF53).  
Refus si un flag INCOMPAT inconnu est présent.

### **`ext4/inode.rs`**
Inodes ext4 (format Linux standard).

### **`ext4/extent.rs`**
Extent tree ext4.

### **`ext4/dir.rs`**
Répertoires (htree + linéaire).

### **`ext4/journal.rs`**
Lecture seule du journal JBD2.  
Montage RW uniquement si `needs_recovery = false`.

### **`ext4/xattr.rs`**
Attributs étendus.

### **`ext4/compat.rs`**
Vérification stricte des flags COMPAT / INCOMPAT / RO_COMPAT.

---

## **7.2. `drivers/fat32/` — FAT32 universel**

### **`fat32/mod.rs`**
Organisation du driver.

### **`fat32/bpb.rs`**
Lecture du **BIOS Parameter Block**.

### **`fat32/fat_table.rs`**
Gestion de la FAT (chaîne de clusters 28 bits).

### **`fat32/dir_entry.rs`**
Entrées de répertoire FAT32 :  
- format 8.3  
- LFN UTF‑16 en ordre inversé

### **`fat32/cluster.rs`**
Lecture/écriture de clusters.

### **`fat32/alloc.rs`**
Allocation first‑fit avec `last_alloc_hint`.

### **`fat32/compat.rs`**
Validation FAT32 uniquement.  
Refus si BPB invalide ou signature incorrecte.

---

# **8. `kernel/src/fs/pseudo/` — Filesystems virtuels**

### **`pseudo/procfs.rs`**
Expose les informations processus.

### **`pseudo/sysfs.rs`**
Expose les devices et drivers.

### **`pseudo/devfs.rs`**
Gestion des périphériques dans `/dev`.

### **`pseudo/tmpfs.rs`**
Filesystem en RAM.

---

# **9. `kernel/src/fs/ipc_fs/` — FS pour IPC**

### **`ipc_fs/pipefs.rs`**
Implémente les pipes POSIX.

### **`ipc_fs/socketfs.rs`**
Implémente les sockets Unix domain.

---

# **10. `kernel/src/fs/block/` — Couche bloc**

### **`block/device.rs`**
Abstraction des block devices.

### **`block/scheduler.rs`**
Ordonnanceur I/O (deadline / mq-deadline).

### **`block/queue.rs`**
Queues de requêtes.

### **`block/bio.rs`**
Structure BIO.

---

# **11. `kernel/src/fs/compatibility/` — Couche syscalls uniquement**

### **`compatibility/posix.rs`**
Compatibilité POSIX 2024 : open/read/write…

### **`compatibility/linux_compat.rs`**
Compatibilité numéros syscalls Linux (ioctls, flags).  
⚠️ Aucun driver filesystem ici.

-.
