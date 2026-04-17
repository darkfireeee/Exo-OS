# 📋 DOC 6 — MODULE FS/ : CONCEPTION COMPLÈTE v2
> Exo-OS · Couche 3 · Architecture kernel v6
> Ext4plus amélioré · FAT32 · Ext4 classique · Compatibilité disque
> Règles strictes anti-divagation IA

---

## POSITION DANS L'ARCHITECTURE

```
┌──────────────────────────────────────────────────────────────────┐
│  fs/  ← COUCHE 3                                                 │
│                                                                  │
│  DÉPEND DE : memory/ + scheduler/ + security/                    │
│  CAPABILITIES : via security::access_control::check_access()     │
│  IPC via : fs/ipc_fs/ (shim obligatoire — jamais import direct)  │
│  NE PEUT PAS : être appelé par scheduler/ ou memory/             │
│                                                                  │
│  TROIS SYSTÈMES DE FICHIERS DISTINCTS :                          │
│    fs/ext4plus/  ← Système principal Exo-OS (disque interne)     │
│    fs/drivers/ext4/  ← Ext4 classique (lecture/écriture Linux)   │
│    fs/drivers/fat32/ ← FAT32 (clés USB, échange universel)       │
│                                                                  │
│  CORRECTIONS APPORTÉES (v2) :                                    │
│    + fs/drivers/ avec ext4 classique et fat32                    │
│    + fs/compatibility/ redéfini (syscalls, pas filesystems)      │
│    + Ext4plus : Data=Ordered, Delayed Alloc, Reflinks            │
│    + Incompat flags dans superblock.rs                           │
└──────────────────────────────────────────────────────────────────┘
```

---

## ARBORESCENCE COMPLÈTE fs/

```
kernel/src/fs/
├── mod.rs                          # API publique VFS — point d'entrée unique
│
├── core/                           # VFS — couche d'abstraction unifiée
│   ├── mod.rs
│   ├── vfs.rs                      # Interface VFS : ops table par filesystem
│   │                               # Chaque FS enregistre ses ops ici au mount
│   ├── inode.rs                    # Inode (RCU read path, lock-free)
│   ├── dentry.rs                   # Dentry cache (RCU)
│   ├── descriptor.rs               # File descriptors + fdtable par processus
│   ├── superblock.rs               # Superblock générique VFS (≠ ext4plus/superblock.rs)
│   │                               # Chaque FS monté = 1 entrée ici
│   └── types.rs                    # FileMode, Permissions, Stat, Dirent
│
├── io/                             # Couche I/O
│   ├── mod.rs
│   ├── uring.rs                    # io_uring backend natif
│   ├── zero_copy.rs                # sendfile/splice (DMA)
│   ├── aio.rs                      # POSIX AIO compat
│   ├── mmap.rs                     # Memory-mapped files
│   ├── direct_io.rs                # O_DIRECT (bypass cache)
│   └── completion.rs               # I/O completion queues
│
├── cache/                          # Caches partagés entre TOUS les filesystems
│   ├── mod.rs
│   ├── page_cache.rs               # Page cache LRU
│   │                               # ⚠️ Delayed Alloc : flag DIRTY géré ICI
│   │                               # Writeback thread vide le cache toutes les 5s
│   ├── dentry_cache.rs             # Dentry cache (hashmap + LRU)
│   ├── inode_cache.rs              # Inode cache (metadata)
│   ├── buffer.rs                   # Buffer cache (bloc layer)
│   ├── prefetch.rs                 # Prefetch readahead adaptatif (IA-guided)
│   ├── writeback.rs                # ← NOUVEAU : thread writeback + delayed alloc
│   │                               # Vide page_cache DIRTY vers disque
│   │                               # Moment d'allocation réelle des blocs (delayed alloc)
│   └── eviction.rs                 # Shrinker callback → memory/utils/shrinker.rs
│
├── integrity/                      # Intégrité — utilisé par ext4plus UNIQUEMENT
│   ├── mod.rs
│   ├── checksum.rs                 # Blake3 checksums
│   │                               # ⚠️ INTERDIT dans ext4/ et fat32/ (formats étrangers)
│   ├── journal.rs                  # Write-Ahead Log (WAL)
│   │                               # Mode Data=Ordered : WAL sur MÉTADONNÉES SEULES
│   │                               # Données → écrites directement à destination finale
│   ├── recovery.rs                 # Crash recovery (replay journal)
│   ├── scrubbing.rs                # Background vérif données
│   ├── healing.rs                  # Auto-healing (Reed-Solomon)
│   └── validator.rs                # Hooks validation intégrité
│
├── ext4plus/                       # FS principal Exo-OS — disque interne uniquement
│   │                               # ⚠️ NE PAS confondre avec fs/drivers/ext4/
│   │                               # ext4plus ≠ ext4 : format différent sur disque
│   ├── mod.rs
│   ├── superblock.rs               # Superblock ext4plus sur disque
│   │                               # ⚠️ CONTIENT LES INCOMPAT FLAGS (voir règle FS-EXT4P-01)
│   │                               # s_feature_incompat doit contenir EXO_BLAKE3 | EXO_DELAYED
│   ├── group_desc.rs               # Block group descriptors
│   │
│   ├── inode/
│   │   ├── mod.rs
│   │   ├── ops.rs                  # read/write/truncate
│   │   │                           # ✅ r15 garanti préservé par switch_asm.s
│   │   ├── extent.rs               # Extent tree
│   │   │                           # ⚠️ REFLINKS implémentés ICI (voir règle FS-EXT4P-04)
│   │   │                           # Reflink = nouvel inode → mêmes blocs physiques
│   │   │                           # CoW déclenché uniquement à la modification
│   │   ├── xattr.rs
│   │   └── acl.rs
│   │
│   ├── directory/
│   │   ├── mod.rs
│   │   ├── htree.rs                # HTree O(log n)
│   │   ├── linear.rs
│   │   └── ops.rs
│   │
│   └── allocation/
│       ├── mod.rs
│       ├── balloc.rs               # Block allocator
│       │                           # ⚠️ N'alloue PAS pendant write() (delayed alloc)
│       │                           # Alloue uniquement depuis writeback thread
│       ├── mballoc.rs              # Multi-block allocator
│       │                           # Cherche un bloc CONTIGU pour tout le dirty batch
│       └── prealloc.rs             # Préallocation spéculative
│
├── drivers/                        # ← NOUVEAU dossier — FS tiers (lecture/écriture)
│   │                               # ⚠️ ISOLATION STRICTE : chaque driver est indépendant
│   │                               # Aucun driver ne partage de code avec ext4plus
│   │                               # Aucun driver n'utilise Blake3 ou les journaux ext4plus
│   │
│   ├── mod.rs                      # Registre des drivers FS (enregistrement au boot)
│   │
│   ├── ext4/                       # Ext4 CLASSIQUE — lecture/écriture disques Linux
│   │   │                           # ⚠️ C'est ext4 STANDARD, pas ext4plus
│   │   │                           # But : monter un disque Linux sans modifier ses données
│   │   ├── mod.rs
│   │   ├── superblock.rs           # Lecture superblock ext4 standard (s_magic = 0xEF53)
│   │   │                           # Vérification : si s_feature_incompat contient un flag
│   │   │                           # INCONNU → refus de montage (protection données)
│   │   ├── inode.rs                # Inodes ext4 (format disque Linux identique)
│   │   ├── extent.rs               # Extent tree ext4 standard
│   │   ├── dir.rs                  # Répertoires (htree + linear)
│   │   ├── journal.rs              # JBD2 journal ext4 (lecture seule du journal)
│   │   │                           # ⚠️ JAMAIS ré-implémenter JBD2 — monter en read-write
│   │   │                           # uniquement si journal propre (needs_recovery = false)
│   │   ├── xattr.rs
│   │   └── compat.rs               # Vérification flags compatibilité avant mount
│   │                               # COMPAT, INCOMPAT, RO_COMPAT — strict
│   │
│   └── fat32/                      # FAT32 — clés USB, échange universel
│       │                           # ⚠️ Pas de journaling, pas de permissions UNIX
│       │                           # But : lire/écrire des clés USB lisibles par Windows/Linux
│       ├── mod.rs
│       ├── bpb.rs                  # BIOS Parameter Block (secteur de boot FAT32)
│       │                           # Parsing : bytes_per_sector, sectors_per_cluster, etc.
│       ├── fat_table.rs            # Table FAT (chaîne de clusters)
│       │                           # FAT32 : entrées 28 bits (4 octets, bits 31-28 réservés)
│       ├── dir_entry.rs            # Entrées répertoires (8.3 + LFN long file names)
│       │                           # LFN : entrées Unicode UTF-16 en ordre inversé
│       ├── cluster.rs              # Lecture/écriture clusters
│       ├── alloc.rs                # Allocation clusters (first-fit depuis last_alloc_hint)
│       └── compat.rs               # Vérification : FAT32 seulement (pas FAT12/FAT16)
│                                   # Refus si BPB invalide ou signature incorrecte
│
├── pseudo/                         # Filesystems virtuels (RAM, /proc, /sys, /dev)
│   ├── mod.rs
│   ├── procfs.rs                   # /proc — informations processus
│   ├── sysfs.rs                    # /sys — devices, drivers
│   ├── devfs.rs                    # /dev — périphériques
│   └── tmpfs.rs                    # tmpfs (RAM filesystem)
│
├── ipc_fs/                         # Shim FS ↔ IPC (seul chemin autorisé)
│   ├── mod.rs
│   ├── pipefs.rs                   # Pipes POSIX
│   └── socketfs.rs                 # Sockets unix domain
│
├── block/                          # Couche bloc (partagée par tous les FS)
│   ├── mod.rs
│   ├── device.rs                   # Block device abstraction
│   ├── scheduler.rs                # I/O scheduler (deadline/mq-deadline)
│   ├── queue.rs                    # Request queue
│   └── bio.rs                      # Block I/O structure
│
└── compatibility/                  # ⚠️ REDÉFINI v2 — syscalls seulement, PAS de FS
    ├── mod.rs
    ├── posix.rs                    # Compatibilité POSIX 2024 (syscalls open/read/write...)
    └── linux_compat.rs             # Compat numéros syscalls Linux (ioctls, flags)
    #
    # ⚠️ IMPORTANT : fs/compatibility/ NE CONTIENT PAS de drivers filesystem
    # Les drivers filesystem sont dans fs/drivers/ext4/ et fs/drivers/fat32/
    # fs/compatibility/ = couche syscall uniquement
```

---

## SÉPARATION DES TROIS SYSTÈMES DE FICHIERS

```
┌─────────────────────────────────────────────────────────────────────┐
│  QUEL FS POUR QUEL CAS D'USAGE ?                                    │
├──────────────────┬──────────────────────────────────────────────────┤
│  fs/ext4plus/    │ Disque interne Exo-OS                            │
│                  │ Blake3 checksums                                  │
│                  │ Data=Ordered journal                              │
│                  │ Delayed Allocation                                │
│                  │ Reflinks                                          │
│                  │ Incompat flags → Linux REFUSE de monter (sûr)    │
│                  │ Performance maximale + intégrité                  │
├──────────────────┼──────────────────────────────────────────────────┤
│  fs/drivers/     │ Disque externe branché depuis Linux               │
│  ext4/           │ Format identique à ext4 standard Linux            │
│                  │ Lecture + écriture (si journal propre)            │
│                  │ AUCUN Blake3, AUCUN delayed alloc ext4plus        │
│                  │ Linux peut remonter ce disque sans problème        │
├──────────────────┼──────────────────────────────────────────────────┤
│  fs/drivers/     │ Clés USB, échange avec Windows/Linux/Mac          │
│  fat32/          │ Pas de permissions UNIX (tout le monde lit tout)  │
│                  │ Pas de journaling (risque perte données si arrêt) │
│                  │ Universellement compatible                         │
└──────────────────┴──────────────────────────────────────────────────┘
```

---

## RÈGLES CRITIQUES — EXT4PLUS AMÉLIORATIONS

### 📌 FS-EXT4P-01 — Incompat Flags (PRIORITÉ ABSOLUE)

```rust
// kernel/src/fs/ext4plus/superblock.rs
//
// RÈGLE FS-EXT4P-01 : Tout format propriétaire DOIT avoir un incompat flag.
// Sans ce flag, un Linux externe peut monter le disque et CORROMPRE les données.
//
// FLAGS OBLIGATOIRES pour Exo-OS ext4plus :

/// Blake3 checksums sur les données (incompatible ext4 standard)
pub const EXT4_FEATURE_INCOMPAT_EXO_BLAKE3: u32   = 0x8000;

/// Delayed allocation avec writeback thread (structure alloc différente)
pub const EXT4_FEATURE_INCOMPAT_EXO_DELAYED: u32  = 0x10000;

/// Reflinks (inodes partageant des blocs physiques)
pub const EXT4_FEATURE_INCOMPAT_EXO_REFLINK: u32  = 0x20000;

/// Combinaison obligatoire pour tout disque ext4plus formaté par Exo-OS
pub const EXO_REQUIRED_INCOMPAT: u32 =
    EXT4_FEATURE_INCOMPAT_EXO_BLAKE3 |
    EXT4_FEATURE_INCOMPAT_EXO_DELAYED |
    EXT4_FEATURE_INCOMPAT_EXO_REFLINK;

#[repr(C)]
pub struct Ext4PlusSuperblock {
    // ... champs standard ext4 ...
    pub s_magic:             u16,   // 0xEF53 (identique ext4 pour détection initiale)
    pub s_feature_compat:    u32,
    pub s_feature_incompat:  u32,   // DOIT contenir EXO_REQUIRED_INCOMPAT
    pub s_feature_ro_compat: u32,
    // ... champs ext4plus spécifiques ...
    pub s_exo_version:       u32,   // version format exo (commence à 1)
    pub s_exo_checksum:      [u8; 32], // Blake3 du superblock lui-même
}

impl Ext4PlusSuperblock {
    /// Vérification au mount — INTERDIT de monter sans les flags obligatoires
    pub fn verify_exo_flags(&self) -> Result<(), FsError> {
        let missing = EXO_REQUIRED_INCOMPAT & !self.s_feature_incompat;
        if missing != 0 {
            return Err(FsError::InvalidSuperblock {
                reason: "Incompat flags Exo-OS manquants — disque corrompu ou ancien format",
                missing_flags: missing,
            });
        }
        Ok(())
    }
}

// COMPORTEMENT GARANTI :
// Exo-OS formate un disque → inscrit EXO_REQUIRED_INCOMPAT dans le superblock
// Linux tente de monter ce disque → lit s_feature_incompat → voit 0x8000 inconnu
// Linux dit : "unknown incompatible feature" → REFUSE le montage → données sûres
```

---

### 📌 FS-EXT4P-02 — Mode Data=Ordered dans journal.rs

```rust
// kernel/src/fs/integrity/journal.rs
//
// RÈGLE FS-EXT4P-02 : Le WAL couvre les MÉTADONNÉES UNIQUEMENT (mode Data=Ordered).
// Les données (contenu fichier) sont écrites DIRECTEMENT à leur emplacement final.
//
// SÉQUENCE OBLIGATOIRE pour toute écriture ext4plus :
//
//   ÉTAPE 1 : Écrire les DONNÉES à l'emplacement final sur disque (bio submit)
//   ÉTAPE 2 : Attendre ACK disque que les données sont physiquement écrites
//   ÉTAPE 3 : Écrire les MÉTADONNÉES dans le journal WAL (taille, mtime, blocs alloués)
//   ÉTAPE 4 : Commiter le journal
//
// POURQUOI cet ordre est critique :
//   Si l'OS crash après étape 1 mais avant étape 3 → données écrites, méta pas encore
//   → Au reboot, recovery rejoue le journal → méta cohérente avec données réelles
//   → JAMAIS de pointeur de méta vers une donnée non écrite
//
// INTERDIT : écrire les métadonnées dans le journal AVANT que les données
//            soient physiquement sur le disque.

pub enum JournalMode {
    /// Mode Data=Journal : données ET métadonnées dans le WAL (double écriture)
    /// ⚠️ NON UTILISÉ dans ext4plus — trop de write amplification
    DataJournal,

    /// Mode Data=Ordered : données directes, WAL sur métadonnées seules ← UTILISÉ
    DataOrdered,

    /// Mode Data=Writeback : aucune garantie ordre données/méta
    /// ⚠️ INTERDIT dans ext4plus — risque de corruption
    DataWriteback,
}

/// Transaction WAL — contient UNIQUEMENT des métadonnées
pub struct JournalTransaction {
    /// Identifiant unique de transaction (monotone croissant)
    pub tid: TransactionId,

    /// Blocs de métadonnées modifiés (inodes, group descriptors, superblock)
    /// ⚠️ JAMAIS de blocs de données dans cette liste
    pub meta_blocks: Vec<JournalBlock>,

    /// Barrière d'écriture : données confirmées sur disque AVANT commit
    /// set à true uniquement après ACK DMA des données
    pub data_barrier_passed: bool,
}

impl JournalTransaction {
    /// Commit — vérifie la barrière avant tout
    pub fn commit(&self, journal: &mut Journal) -> Result<(), JournalError> {
        // RÈGLE ABSOLUE : ne jamais commiter si les données ne sont pas sur disque
        if !self.data_barrier_passed {
            return Err(JournalError::DataBarrierNotPassed);
        }
        journal.write_commit_block(self.tid)?;
        Ok(())
    }
}
```

---

### 📌 FS-EXT4P-03 — Delayed Allocation

```rust
// kernel/src/fs/cache/page_cache.rs  +  fs/cache/writeback.rs
//
// RÈGLE FS-EXT4P-03 : Lors d'un write() applicatif, NE PAS allouer de blocs disque.
// Garder les données en RAM avec flag DIRTY. Allouer au moment du writeback.
//
// SÉQUENCE DELAYED ALLOC :
//
//   write() syscall reçu
//     → page_cache.rs : données copiées en RAM, page marquée DIRTY
//     → RETOUR immédiat à l'application (sans accès disque)
//     → (l'application ne sait pas que rien n'est encore sur disque)
//
//   Writeback thread (toutes les ~5s ou si mémoire faible) :
//     → Collecte toutes les pages DIRTY d'un même fichier
//     → allocation/mballoc.rs : cherche un GRAND bloc contigu pour toutes ces pages
//     → Écrit toutes les données en une seule opération DMA (zéro fragmentation)
//     → Écriture des métadonnées dans le journal (Data=Ordered)
//
// AVANTAGE CRITIQUE :
//   Fichiers temporaires (créés + supprimés en < 5s) → jamais sur disque physique
//   Fichiers écrits progressivement → écrits en UN seul bloc contigu (pas fragmentés)

pub struct PageCacheEntry {
    pub data:      [u8; PAGE_SIZE],
    pub flags:     PageFlags,
    pub file_id:   InodeId,
    pub offset:    u64,
}

bitflags! {
    pub struct PageFlags: u32 {
        /// Page modifiée, pas encore sur disque — bloc PAS encore alloué
        const DIRTY          = 1 << 0;
        /// Writeback en cours (DMA soumis)
        const WRITEBACK      = 1 << 1;
        /// Page verrouillée (I/O en cours)
        const LOCKED         = 1 << 2;
        /// Page référencée récemment (LRU)
        const REFERENCED     = 1 << 3;
    }
}

// kernel/src/fs/cache/writeback.rs
// Thread dédié — lancé au boot par fs::core::vfs::init()

pub fn writeback_thread_loop() -> ! {
    loop {
        // Attendre déclencheur (5s timeout OU signal mémoire faible)
        writeback_wakeup.wait_timeout(Duration::from_secs(5));

        // Collecter toutes les pages DIRTY groupées par inode
        let dirty_groups = page_cache::collect_dirty_pages();

        for (inode_id, dirty_pages) in dirty_groups {
            // VÉRIFICATION CAPABILITY au moment du writeback
            // Le token peut avoir été révoqué depuis le write() initial
            let table = process::cap_table_for_inode(inode_id);
            if security::access_control::check_access(
                &table, inode_token, ObjectKind::File, Rights::WRITE, "fs::writeback"
            ).is_err() {
                // Token révoqué entre write() et writeback → annuler, libérer pages
                page_cache::discard_dirty_pages(inode_id);
                audit::log_writeback_denied(inode_id);
                continue;
            }

            // Allouer UN bloc contigu pour toutes les pages de cet inode
            let block_range = ext4plus::allocation::mballoc::alloc_contiguous(
                dirty_pages.len()
            )?;

            // Écriture DMA directe (Data=Ordered étape 1)
            dma::submit_write(block_range, &dirty_pages)?;
            dma::wait_completion()?;  // barrière — données sur disque

            // Journal des métadonnées (Data=Ordered étape 2)
            let txn = journal::begin_transaction();
            txn.record_inode_update(inode_id, block_range);
            txn.set_data_barrier_passed(); // données confirmées
            txn.commit()?;

            page_cache::clear_dirty_flags(inode_id);
        }
    }
}
```

---

### 📌 FS-EXT4P-04 — Reflinks dans extent.rs

```rust
// kernel/src/fs/ext4plus/inode/extent.rs
//
// RÈGLE FS-EXT4P-04 : Les reflinks créent un second inode pointant vers les
// MÊMES blocs physiques. Le CoW est déclenché UNIQUEMENT à la modification.
//
// COMPORTEMENT :
//   cp --reflink fichier_10Go copie_10Go
//     → INSTANTANÉ : création d'un nouvel inode, même extent tree
//     → Aucune donnée copiée sur disque
//     → Les deux fichiers partagent les mêmes blocs physiques
//
//   echo "modification" >> copie_10Go   (écriture dans la copie)
//     → Seul le bloc modifié est copié (CoW ciblé)
//     → Les blocs non modifiés restent partagés
//
// COMPTEUR DE RÉFÉRENCES OBLIGATOIRE :
//   Chaque bloc physique partagé a un refcount dans memory/cow/tracker.rs
//   refcount > 1 → CoW sur écriture
//   refcount = 1 → écriture directe (plus de partage)

/// Extent — plage de blocs physiques contigus
#[repr(C)]
pub struct Extent {
    /// Premier bloc logique dans le fichier
    pub logical_block: u32,
    /// Nombre de blocs dans cet extent (max 32768 pour ext4)
    pub len: u16,
    /// Premier bloc physique (48 bits : hi + lo)
    pub physical_hi: u16,
    pub physical_lo: u32,
    /// Compteur de références — > 1 si blocs partagés par reflink
    /// ⚠️ DOIT être atomique — plusieurs processus peuvent accéder simultanément
    pub refcount: AtomicU32,
}

impl Extent {
    /// Déclencher CoW si le bloc est partagé — appelé AVANT toute écriture
    pub fn cow_if_shared(
        &self,
        inode: &mut InodeData,
        logical_offset: u32,
    ) -> Result<PhysBlock, FsError> {
        if self.refcount.load(Ordering::Acquire) == 1 {
            // Bloc non partagé → écriture directe, pas de CoW
            return Ok(PhysBlock::from_extent(self, logical_offset));
        }

        // Bloc partagé → allouer un nouveau bloc physique
        let new_block = ext4plus::allocation::balloc::alloc_single()?;

        // Copier les données de l'ancien bloc vers le nouveau
        memory::dma::ops::memcpy::dma_copy(
            PhysBlock::from_extent(self, logical_offset),
            new_block,
            BLOCK_SIZE,
        )?;

        // Décrémenter refcount de l'ancien bloc
        self.refcount.fetch_sub(1, Ordering::Release);

        // Mettre à jour l'extent de cet inode vers le nouveau bloc
        inode.replace_extent_block(logical_offset, new_block);

        Ok(new_block)
    }
}
```

---

## RÈGLES CRITIQUES — EXT4 CLASSIQUE (fs/drivers/ext4/)

### 📌 FS-EXT4-01 — Vérification des flags avant montage

```rust
// kernel/src/fs/drivers/ext4/compat.rs
//
// RÈGLE FS-EXT4-01 : Vérifier les flags INCOMPAT avant tout accès aux données.
// Un flag inconnu = format inconnu = risque de corruption = REFUS DE MONTAGE.
//
// FLAGS INCOMPAT CONNUS ET SUPPORTÉS PAR LE DRIVER EXT4 D'EXO-OS :
pub const EXT4_KNOWN_INCOMPAT_FLAGS: u32 =
    EXT4_FEATURE_INCOMPAT_FILETYPE     | // 0x0002 — type dans dentry
    EXT4_FEATURE_INCOMPAT_RECOVER      | // 0x0004 — journal à rejouer
    EXT4_FEATURE_INCOMPAT_META_BG      | // 0x0010
    EXT4_FEATURE_INCOMPAT_EXTENTS      | // 0x0040 — extent tree (obligatoire)
    EXT4_FEATURE_INCOMPAT_64BIT        | // 0x0080
    EXT4_FEATURE_INCOMPAT_MMP          | // 0x0100
    EXT4_FEATURE_INCOMPAT_FLEX_BG      | // 0x0200
    EXT4_FEATURE_INCOMPAT_EA_INODE     | // 0x0400
    EXT4_FEATURE_INCOMPAT_DIRDATA      | // 0x1000
    EXT4_FEATURE_INCOMPAT_LARGEDIR     | // 0x4000
    EXT4_FEATURE_INCOMPAT_INLINE_DATA;  // 0x8000 — ⚠️ attention : même valeur que EXO_BLAKE3

// ⚠️ COLLISION : EXT4_FEATURE_INCOMPAT_INLINE_DATA = 0x8000 = EXO_BLAKE3
// Solution : le driver ext4 classique lit d'abord s_exo_version
// Si s_exo_version != 0 → c'est un disque ext4plus → refuser avec message clair

pub fn verify_before_mount(sb: &Ext4Superblock) -> Result<MountMode, FsError> {
    // Détecter un disque ext4plus présenté par erreur au driver ext4 classique
    if sb.s_exo_version != 0 {
        return Err(FsError::WrongDriver {
            reason: "Ce disque est ext4plus (Exo-OS). Utiliser le driver ext4plus.",
            hint: "Monter via fs::ext4plus, pas fs::drivers::ext4",
        });
    }

    // Vérifier les flags incompat
    let unknown = sb.s_feature_incompat & !EXT4_KNOWN_INCOMPAT_FLAGS;
    if unknown != 0 {
        return Err(FsError::UnknownIncompatFeature {
            flags: unknown,
            reason: "Flags incompat inconnus — risque de corruption si monté",
        });
    }

    // Journal : monter en read-write uniquement si journal propre
    if sb.needs_recovery() {
        // Journal pas rejoué → proposer read-only uniquement
        return Ok(MountMode::ReadOnly { reason: "Journal nécessite recovery — monter ro" });
    }

    Ok(MountMode::ReadWrite)
}
```

---

## RÈGLES CRITIQUES — FAT32 (fs/drivers/fat32/)

### 📌 FS-FAT32-01 — Validation BPB avant montage

```rust
// kernel/src/fs/drivers/fat32/compat.rs
//
// RÈGLE FS-FAT32-01 : Valider le BPB et rejeter FAT12/FAT16.
// Une clé USB FAT12 montée comme FAT32 → corruption garantie.

pub fn verify_fat32(bpb: &BiosParameterBlock) -> Result<(), FsError> {
    // Signature de boot sector
    if bpb.boot_sector_signature != 0xAA55 {
        return Err(FsError::InvalidSignature { expected: 0xAA55, got: bpb.boot_sector_signature });
    }

    // Calcul du type FAT par le SEUL critère correct (nombre de clusters)
    let root_dir_sectors = ((bpb.root_entry_count * 32) + bpb.bytes_per_sector - 1)
        / bpb.bytes_per_sector;
    let data_sectors = bpb.total_sectors_32
        - (bpb.reserved_sector_count as u32
            + (bpb.num_fats as u32 * bpb.fat_size_32)
            + root_dir_sectors as u32);
    let cluster_count = data_sectors / bpb.sectors_per_cluster as u32;

    // Microsoft spec : FAT32 = cluster_count >= 65525
    if cluster_count < 65525 {
        return Err(FsError::WrongFatType {
            reason: "Ce volume est FAT12 ou FAT16, pas FAT32",
            cluster_count,
            hint: "Reformater en FAT32 pour utilisation avec Exo-OS",
        });
    }

    // Vérifier OEM name (informatif, pas bloquant)
    // Vérifier bytes_per_sector = 512 | 1024 | 2048 | 4096
    if ![512u16, 1024, 2048, 4096].contains(&bpb.bytes_per_sector) {
        return Err(FsError::InvalidSectorSize { size: bpb.bytes_per_sector });
    }

    Ok(())
}
```

### 📌 FS-FAT32-02 — Absence de permissions UNIX

```rust
// kernel/src/fs/drivers/fat32/mod.rs
//
// RÈGLE FS-FAT32-02 : FAT32 n'a pas de permissions UNIX.
// Comportement défini explicitement pour éviter les failles de sécurité.

impl FatFs {
    /// FAT32 : tous les fichiers sont lisibles/écrivables par tous
    /// ⚠️ Ne jamais exécuter un binaire depuis FAT32 sans vérification explicite
    pub fn get_permissions(_entry: &DirEntry) -> FileMode {
        // rwxrwxrwx pour tout le monde — FAT32 n'a pas de notion d'owner
        // L'OS DOIT refuser d'exécuter un binaire ELF depuis FAT32
        // (monté avec MS_NOEXEC obligatoire — voir règle FS-FAT32-03)
        FileMode::from_bits(0o777).unwrap()
    }
}
```

---

## TABLEAU COMPLET DES RÈGLES FS/ v2

```
┌─────────────────────────────────────────────────────────────────────┐
│ RÈGLES FS/ COMMUNES (tous les filesystems)                          │
├──────────────┬──────────────────────────────────────────────────────┤
│ FS-01        │ Relâcher lock inode AVANT sleep (release-before-sleep)│
│ FS-02        │ io_uring : EINTR propre (IORING_OP_ASYNC_CANCEL)     │
│ FS-03        │ IPC via fs/ipc_fs/ UNIQUEMENT (jamais import direct) │
│ FS-04        │ Capabilities : check_access() de security/           │
│               │ access_control/ — v6, appel direct                  │
│ FS-05        │ Thundering herd : completion callbacks sélectifs     │
│ FS-06        │ ElfLoader trait enregistré pour process/exec         │
│ FS-07        │ Slab shrinker → memory/utils/shrinker.rs            │
│ FS-08        │ Blake3 checksums : ext4plus UNIQUEMENT               │
│               │ INTERDIT dans ext4 classique et fat32               │
│ FS-09        │ WAL : ext4plus UNIQUEMENT (mode Data=Ordered)        │
│               │ INTERDIT d'appliquer le WAL ext4plus à ext4 ou fat32│
├──────────────┴──────────────────────────────────────────────────────┤
│ RÈGLES EXT4PLUS SPÉCIFIQUES                                         │
├──────────────┬──────────────────────────────────────────────────────┤
│ FS-EXT4P-01  │ Incompat flags OBLIGATOIRES dans superblock.rs       │
│               │ EXO_BLAKE3 | EXO_DELAYED | EXO_REFLINK au format   │
│               │ → Linux refuse de monter → données sûres            │
│ FS-EXT4P-02  │ Data=Ordered : WAL sur métadonnées SEULES            │
│               │ Données → disque final AVANT commit journal          │
│               │ Barrière obligatoire (data_barrier_passed = true)   │
│ FS-EXT4P-03  │ Delayed Alloc : pas d'allocation pendant write()     │
│               │ Allocation UNIQUEMENT depuis writeback thread        │
│               │ Vérification capability au moment du writeback       │
│               │ (pas seulement à l'open)                            │
│ FS-EXT4P-04  │ Reflinks : CoW ciblé, refcount atomique par bloc     │
│               │ cow_if_shared() appelé AVANT toute écriture         │
│               │ refcount géré dans memory/cow/tracker.rs            │
├──────────────┴──────────────────────────────────────────────────────┤
│ RÈGLES EXT4 CLASSIQUE                                               │
├──────────────┬──────────────────────────────────────────────────────┤
│ FS-EXT4-01   │ Vérifier flags INCOMPAT avant montage                │
│               │ Flag inconnu → REFUS (jamais monter pour essayer)   │
│ FS-EXT4-02   │ Détecter ext4plus (s_exo_version != 0) → refuser    │
│               │ avec message clair orientant vers bon driver         │
│ FS-EXT4-03   │ Journal not-clean → proposer read-only UNIQUEMENT    │
│               │ Jamais rejouer JBD2 Linux depuis Exo-OS             │
│ FS-EXT4-04   │ AUCUN Blake3, AUCUN delayed alloc ext4plus           │
│               │ Le format sur disque doit rester 100% Linux-compat  │
├──────────────┴──────────────────────────────────────────────────────┤
│ RÈGLES FAT32                                                        │
├──────────────┬──────────────────────────────────────────────────────┤
│ FS-FAT32-01  │ Valider BPB + calculer cluster_count avant montage   │
│               │ Refuser FAT12 et FAT16 (calcul Microsoft spec exact) │
│ FS-FAT32-02  │ Permissions : 0o777 pour tout (FAT32 sans owner)     │
│               │ Documenter explicitement dans get_permissions()      │
│ FS-FAT32-03  │ Monter FAT32 avec MS_NOEXEC obligatoire              │
│               │ Jamais exécuter un binaire ELF depuis FAT32          │
│ FS-FAT32-04  │ LFN : ordre inversé des entrées, UTF-16 → UTF-8      │
│               │ Vérifier checksum LFN vs entrée 8.3                 │
│ FS-FAT32-05  │ Écriture FAT : toujours écrire FAT1 ET FAT2 (miroir) │
│               │ (bpb.num_fats = 2 sur tout FAT32 valide)            │
├──────────────┴──────────────────────────────────────────────────────┤
│ INTERDITS ABSOLUS                                                   │
├─────────────────────────────────────────────────────────────────────┤
│ ✗  Scheduler/ ou memory/ appelle fs/ (sens interdit)               │
│ ✗  Lock inode tenu pendant sleep                                    │
│ ✗  ipc/ importe fs/ directement (passer par ipc_fs/)              │
│ ✗  Blake3 ou journal ext4plus appliqués à ext4 ou fat32            │
│ ✗  Monter un disque avec des flags INCOMPAT inconnus               │
│ ✗  Exécuter un binaire depuis une partition FAT32                   │
│ ✗  Écrire des métadonnées dans le journal AVANT les données disque  │
│ ✗  Allouer des blocs pendant write() (delayed alloc = writeback)    │
│ ✗  CoW sans décrémenter refcount de l'ancien bloc                  │
│ ✗  Driver ext4 classique appliqué à un disque ext4plus             │
│ ✗  fs/compatibility/ contient des drivers filesystem               │
│    (fs/compatibility/ = syscalls seulement)                         │
└─────────────────────────────────────────────────────────────────────┘
```

---

## RÈGLES ANTI-DIVAGATION IA — INSTRUCTIONS STRICTES

Ces règles existent pour éviter que l'IA mélange les trois systèmes de fichiers,
applique des fonctionnalités au mauvais endroit, ou invente des chemins inexistants.

```
┌─────────────────────────────────────────────────────────────────────┐
│ INSTRUCTIONS STRICTES POUR L'IA — FS/                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  QUESTION 1 : Dans quel fichier implémenter X ?                     │
│                                                                     │
│  Blake3 checksum         → fs/integrity/checksum.rs                 │
│  WAL / journal           → fs/integrity/journal.rs                  │
│  Delayed alloc           → fs/cache/writeback.rs + allocation/      │
│                             mballoc.rs                              │
│  Reflinks                → fs/ext4plus/inode/extent.rs              │
│  Incompat flags          → fs/ext4plus/superblock.rs                │
│  Lecture disque Linux    → fs/drivers/ext4/ (jamais ext4plus/)      │
│  Lecture clé USB         → fs/drivers/fat32/ (jamais ext4plus/)     │
│  Compat syscall POSIX    → fs/compatibility/posix.rs                │
│  Compat syscall Linux    → fs/compatibility/linux_compat.rs         │
│  Permissions fichiers    → fs/core/types.rs (FileMode)              │
│  Cache partagé           → fs/cache/ (pour TOUS les FS)             │
│                                                                     │
│  QUESTION 2 : Quels modules peuvent s'appeler mutuellement ?        │
│                                                                     │
│  fs/ext4plus/ → fs/cache/         ✓ (page cache partagé)           │
│  fs/ext4plus/ → fs/integrity/     ✓ (journal, checksum)            │
│  fs/ext4plus/ → fs/block/         ✓ (I/O bloc)                     │
│  fs/drivers/  → fs/cache/         ✓ (même page cache)              │
│  fs/drivers/  → fs/integrity/     ✗ INTERDIT (journal = ext4plus)  │
│  fs/drivers/  → fs/ext4plus/      ✗ INTERDIT (drivers isolés)      │
│  fs/ext4plus/ → fs/drivers/       ✗ INTERDIT (isolation)           │
│  fs/drivers/ext4/ → fs/drivers/fat32/ ✗ INTERDIT                   │
│                                                                     │
│  QUESTION 3 : Quelle est la différence ext4plus vs ext4 ?           │
│                                                                     │
│  ext4plus = format PROPRIÉTAIRE Exo-OS, incompatible Linux          │
│  ext4 = format standard Linux, lisible par Linux                    │
│  Ne JAMAIS confondre les deux dans le code                          │
│  Ne JAMAIS appliquer les améliorations ext4plus au driver ext4      │
│                                                                     │
│  QUESTION 4 : Delayed Allocation — qui alloue les blocs ?           │
│                                                                     │
│  write() syscall    → JAMAIS d'allocation → données en RAM DIRTY    │
│  writeback thread   → TOUJOURS l'allocation → puis DMA → journal    │
│  Si l'IA écrit du code qui alloue dans write() → INCORRECT          │
│                                                                     │
│  QUESTION 5 : Reflinks — quand copier les données ?                 │
│                                                                     │
│  cp --reflink → JAMAIS de copie → nouvel inode, mêmes blocs        │
│  Écriture dans la copie → CoW sur le bloc modifié SEULEMENT         │
│  Si l'IA écrit du code qui copie au moment du cp → INCORRECT        │
│                                                                     │
│  QUESTION 6 : FAT32 — peut-on exécuter des binaires ?              │
│                                                                     │
│  JAMAIS. MS_NOEXEC obligatoire. Le binaire doit d'abord être       │
│  copié sur le disque ext4plus avant d'être exécuté.                │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

---

## SÉQUENCE DE BOOT fs/ — MISE À JOUR v2

```
INITIALISATION FS/ (dans la séquence globale boot — steps 22-25)

22. fs::block::device::init()           # Block devices avant VFS
23. fs::core::vfs::init()               # VFS — enregistre les drivers FS
    ├── fs::drivers::ext4::register()   # ← NOUVEAU : driver ext4 classique
    ├── fs::drivers::fat32::register()  # ← NOUVEAU : driver FAT32
    └── fs::ext4plus::register()        # ← Driver principal
24. fs::cache::writeback::start_thread() # ← NOUVEAU : thread delayed alloc
25. fs::ext4plus::mount_root()          # Monter / sur ext4plus
    └── Vérifier incompat flags avant mount (FS-EXT4P-01)
```

---

## CORRESPONDANCE AVEC LES DOCS PRÉCÉDENTS

| Règle ancienne | Règle v2 | Changement |
|---|---|---|
| FS-04 "via security/capability/" | FS-04 "via security/access_control/" | v6 (plus de bridge) |
| FS-08 "Blake3 sur toutes écritures" | FS-08 "Blake3 sur ext4plus UNIQUEMENT" | Périmètre précisé |
| `fs/compatibility/posix.rs` | Inchangé — syscalls | Confirmé |
| `fs/compatibility/linux_compat.rs` | Inchangé — syscalls | Confirmé |
| Pas de fat32 | `fs/drivers/fat32/` | ← AJOUTÉ |
| Pas de ext4 classique | `fs/drivers/ext4/` | ← AJOUTÉ |
| Pas de writeback.rs | `fs/cache/writeback.rs` | ← AJOUTÉ (delayed alloc) |
| Pas de mode journal | `journal.rs` mode Data=Ordered | ← PRÉCISÉ |

---

*DOC 6 — Module FS/ v2 — Exo-OS Architecture v6*
*Ext4plus amélioré · FAT32 · Ext4 classique · Règles strictes anti-divagation IA*
