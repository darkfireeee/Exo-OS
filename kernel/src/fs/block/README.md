# Block Device Layer - Module Documentation

Module de périphériques bloc avancé pour Exo-OS avec fonctionnalités de niveau production.

## Architecture

Le module `block/` fournit une abstraction complète des périphériques de stockage bloc avec:

### 1. **device.rs** - Abstraction de périphérique bloc
- Trait `BlockDevice` universel pour tous les types de stockage
- Support async avec `Future` (AsyncRead/AsyncWrite)
- Implémentation `RamDisk` pour tests et tmpfs
- Registre global `BlockDeviceRegistry` pour gestion centralisée

**Performance**:
- read(): < 1000 cycles (cache hit)
- write(): < 1500 cycles (write-back)
- Zero-copy via slices pour DMA

### 2. **partition.rs** - Gestion des partitions
- Détection automatique MBR et GPT
- Parsing complet des tables de partitions
- `PartitionedDevice` pour encapsuler une partition comme device
- Support des types de systèmes de fichiers (FAT32, ext4, NTFS, etc.)

**Fonctionnalités**:
- Lecture MBR (4 partitions primaires)
- Lecture GPT (jusqu'à 128 partitions)
- Flags de partition (bootable, read-only, hidden)

### 3. **scheduler.rs** - Ordonnanceur I/O
- **Deadline Scheduler**: Priorise les deadlines, évite la famine
- **CFQ (Completely Fair Queuing)**: Distribution équitable de bande passante
- **Noop Scheduler**: FIFO simple, overhead minimal pour SSD/NVMe

**Architecture**:
- Queues séparées pour lectures/écritures (Deadline)
- Tri par priorité et LBA (CFQ)
- Support des priorités (0=urgent, 3=low)

### 4. **nvme.rs** - Optimisations NVMe
- Auto-tuning de la profondeur de queue (2-65536)
- Sélection intelligente de queue I/O
- Priorisation des commandes (Urgent, High, Medium, Low)
- Gestion parallèle avec `ParallelIoManager`

**Métriques**:
- Commandes en vol (in-flight tracking)
- Statistiques détaillées par type d'opération
- Auto-tuning basé sur latence moyenne

### 5. **stats.rs** - Statistiques I/O
- Compteurs atomiques sans locks
- Tracking de throughput (bytes/sec)
- IOPS (I/O per second)
- Latency histograms (8 buckets: <100us à >=100ms)
- Détection séquentiel/aléatoire
- Taux d'erreur

**Métriques disponibles**:
- Bytes read/written
- Ops read/write/flush/discard
- Latences moyennes/max
- Throughput instantané
- Pourcentage d'accès séquentiels
- Erreurs par 1000 opérations

### 6. **raid.rs** - RAID Logiciel
- **RAID 0**: Striping (performance max, aucune redondance)
- **RAID 1**: Mirroring (redondance complète)
- **RAID 5**: Striping + parité distribuée (1 disque de tolérance)
- **RAID 6**: Striping + double parité (2 disques de tolérance)
- **RAID 10**: Stripes mirrorés (performance + redondance)

**Fonctionnalités**:
- Calcul automatique de stripe
- Gestion de pannes de disques
- Mode dégradé (degraded mode)
- Reconstruction (à implémenter)

### 7. **mod.rs** - Interface publique
- Re-exports de tous les types publics
- Registre global `BLOCK_DEVICE_REGISTRY`
- Fonctions helper pour créer des devices de test

## Utilisation

### Exemple basique: RAM Disk

```rust
use exo_kernel::fs::block::*;

// Créer un RAM disk de 64MB
let device = device::RamDisk::new("ramdisk0".into(), 64 * 1024 * 1024, 512);

// Enregistrer globalement
BLOCK_DEVICE_REGISTRY.register(device.clone());

// Lire/écrire
let mut buf = [0u8; 512];
device.write().write(0, &[42u8; 512])?;
device.read().read(0, &mut buf)?;
```

### Exemple: Détection de partitions

```rust
let device = get_device("sda").unwrap();
let partition_table = partition::PartitionTable::detect(&*device.read())?;

for part in partition_table.all() {
    println!("Partition {}: {} blocks at LBA {}",
             part.number, part.size_blocks, part.start_lba);
}
```

### Exemple: Scheduler avec statistiques

```rust
// Device avec scheduler Deadline
let scheduled = scheduler::ScheduledDevice::new(
    device.clone(),
    scheduler::SchedulerType::Deadline,
);

// Statistiques
let stats = stats::IoStats::new();

// Soumettre requête
let request = scheduler::IoRequest::new(
    scheduler::IoOperation::Read,
    lba,
    count,
).with_priority(0);

scheduled.read().submit(request)?;

// Traiter
scheduled.write().process_next()?;

// Voir stats
let snapshot = stats.snapshot();
println!("IOPS: {}, Throughput: {} MB/s",
         snapshot.read_iops,
         snapshot.read_throughput_bps / 1_000_000);
```

### Exemple: RAID 5

```rust
// Créer 3 disques
let devices: Vec<Arc<RwLock<dyn BlockDevice>>> = vec![
    RamDisk::new("disk0".into(), 256 * 1024 * 1024, 512),
    RamDisk::new("disk1".into(), 256 * 1024 * 1024, 512),
    RamDisk::new("disk2".into(), 256 * 1024 * 1024, 512),
];

// Configuration RAID 5
let config = raid::RaidConfig::new(
    raid::RaidLevel::Raid5,
    64 * 1024, // 64KB chunks
    "raid5_array".into(),
);

// Créer array
let raid_array = raid::RaidArray::new(config, devices)?;

// Utiliser comme device normal
let mut buf = [0u8; 512];
raid_array.write().write(0, &buf)?;
```

## Architecture Technique

### Zero-Copy Philosophy
Toutes les opérations I/O utilisent des slices (`&[u8]`, `&mut [u8]`) pour permettre:
- DMA direct sans copie intermédiaire
- Support de buffers custom (aligned, pinned, etc.)
- Optimisation par le compilateur

### Lock-Free Statistics
Les statistiques utilisent `AtomicU64`/`AtomicU32` pour:
- Pas de contention sur les locks
- Performance maximale en multi-thread
- Overhead minimal (<10 cycles par opération)

### Traits et Composition
Le système utilise le pattern trait pour:
- Abstraction uniforme de tous les devices
- Composition via wrappers (ScheduledDevice, NvmeDevice, etc.)
- Testing facile avec mocks

### Safety
- Pas de `unsafe` sauf nécessaire pour atomics
- Tous les accès device protégés par `RwLock`
- Validation stricte des offsets et tailles

## Tests

Voir `examples.rs` pour des exemples complets d'utilisation.

## Statistiques de Code

- **Total**: 2491 lignes de code
- **device.rs**: 316 lignes
- **partition.rs**: 422 lignes
- **scheduler.rs**: 392 lignes
- **nvme.rs**: 332 lignes
- **stats.rs**: 475 lignes
- **raid.rs**: 415 lignes
- **mod.rs**: 139 lignes

## Performances Cibles

| Opération | Latence | Throughput |
|-----------|---------|------------|
| RAM Disk Read | < 1000 cycles | > 10 GB/s |
| RAM Disk Write | < 1500 cycles | > 8 GB/s |
| Scheduler Overhead | < 200 cycles | - |
| Stats Recording | < 10 cycles | - |
| RAID 0 Read | ~1000 cycles | Nx throughput |
| RAID 1 Read | ~1000 cycles | Nx throughput |
| RAID 5 Read | ~1000 cycles | (N-1)x throughput |

## Future Enhancements

1. **DMA Support**: Direct Memory Access pour devices matériels
2. **TRIM/Discard**: Support complet pour SSD wear leveling
3. **RAID Rebuild**: Reconstruction automatique après panne
4. **NCQ/TCQ**: Native Command Queuing pour SATA/SCSI
5. **io_uring Integration**: Support du ring buffer moderne Linux
6. **Compression**: Compression transparente au niveau bloc
7. **Encryption**: Chiffrement au niveau bloc (dm-crypt style)

## Dépendances

- `alloc`: Allocations heap (Vec, Arc, String)
- `spin`: RwLock sans_std
- `core::sync::atomic`: Compteurs atomiques

## Compatibilité

- **no_std**: Compatible bare-metal
- **Architecture**: x86_64, aarch64
- **Devices supportés**: RAM, virtio-blk, NVMe (via abstraction)
