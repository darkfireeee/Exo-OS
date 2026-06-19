# Drivers de stockage Exo-OS — virtio-blk · NVMe · AHCI/SATA

> Date : 2026-06-16. Trois drivers bloc **réels** (zéro stub), chacun présentant
> la **même surface** (`read_block` / `write_block` / `flush` / `block_size` /
> `total_blocks`) sur des blocs ExoFS de **4096 octets**.

## État

| Driver | Crate | Cible | État | Tests |
|--------|-------|-------|------|-------|
| VirtIO-blk | `drivers/storage/virtio_blk` (`exo-virtio-blk`) | virtio-blk PCI (QEMU) | **Réel** (délègue à la crate auditée `virtio-drivers` 0.7) | round-trips sparse |
| NVMe (SSD) | `drivers/storage/nvme` (`exo-nvme`) | NVM Express 1.4 | **Réel, neuf** | **29** (logique pure + contrôleur simulé) |
| AHCI/SATA (HDD) | `drivers/storage/ahci` (`exo-ahci`) | Serial ATA AHCI 1.3.1 | **Réel, neuf** | **18** (logique pure + HBA simulé) |

## Architecture commune (microkernel)

Les drivers tournent en **Ring 3** (serveurs userspace). Le kernel fournit les
primitives matérielles via un *HAL* injecté :

- **VirtIO** : `hal::ExoHalOps { dma_alloc, dma_dealloc, mmio_phys_to_virt }`
  (installé par `fs/exofs/storage/virtio_adapter.rs`).
- **NVMe** : trait `NvmeHal { dma_alloc, dma_dealloc, mmio_read32/64, mmio_write32/64 }`.
- **AHCI** : trait `AhciHal { dma_alloc, dma_dealloc, mmio_read32, mmio_write32 }`.

Chaque driver est **générique** sur son HAL → testable avec un mock (contrôleur
simulé en mémoire) **et** branché en production sur l'allocateur DMA + le mapping
MMIO du BAR fournis par le kernel.

## Sélection du driver par classe PCI (point d'intégration)

L'énumération PCI (`drivers/pci_cfg.rs`) identifie le contrôleur ; le device
server instancie le bon driver :

| Périphérique | Class | Subclass | Prog-IF | Driver |
|--------------|-------|----------|---------|--------|
| NVMe | `0x01` (Mass Storage) | `0x08` (NVM) | `0x02` (NVMe) | `exo-nvme` (BAR0) |
| AHCI | `0x01` | `0x06` (SATA) | `0x01` (AHCI) | `exo-ahci` (ABAR = BAR5) |
| VirtIO-blk | vendor `0x1AF4` | device `0x1001`/`0x1042` | — | `exo-virtio-blk` |

Séquence d'attachement (analogue à `virtio_adapter`) :
1. PCI : lire BAR, activer Bus Master + MMIO (command register).
2. Mapper le BAR via `mmio_phys_to_virt` → base MMIO du HAL.
3. `XxxDevice::new(hal)` : init contrôleur + identify.
4. Exposer via le trait `BlockDevice` du kernel
   (`fs/exofs/recovery/boot_recovery.rs`) — signatures identiques, adaptateur trivial.

## Mesures anti-bug / anti-CVE (par conception)

Communes :
- **I/O synchrone une-commande-à-la-fois** → pas de course sur les files/slots.
- **Toutes les attentes matérielles bornées** (compteur de spins `SPIN_LIMIT`)
  → impossible de hang sur un contrôleur muet.
- **`dma_alloc` fail-closed** : jamais d'adresse physique fictive.
- Encodage/bornage isolé dans des modules **purs et testés** (offsets de
  registre, indices d'anneau, PRP/PRDT, FIS) — la classe de bugs « mauvais champ »
  est couverte par les tests unitaires.

NVMe :
- Phase tag CQ + wraparound d'index testés (200 commandes traversant la file).
- PRP bornées à ≤ 2 pages ; transfert bloc = 1 page = 1 PRP (jamais de PRP list
  mal formée).
- Tailles de file plafonnées à `min(CAP.MQES+1, 64)`.

AHCI :
- PRDT : **une** entrée, taille bornée à ≤ 4 Mio (DBC 22 bits, 0-based) ; refus
  explicite si hors borne (pas de troncature silencieuse).
- Reset du moteur de commandes (CR/FR) borné ; attente BSY/DRQ bornée ; détection
  d'erreur Task File (IS.TFES).
- LBA48 (`READ/WRITE DMA EXT` 0x25/0x35) ; taille de secteur lue depuis IDENTIFY.

## Vérification

- Logique pure : offsets/bitfields, encodage de commande, anneaux/phase, PRP/PRDT,
  FIS — **testée unitairement** (host).
- Bout-en-bout : un **contrôleur simulé** en mémoire (init → identify →
  read/write/flush) prouve que la machine submit/poll, le phase tag et l'encodage
  fonctionnent **ensemble**.
- ⚠️ Vérification **sur matériel/QEMU** (`-device nvme`, `-device ich9-ahci`) à
  faire une fois le boot-to-shell réparé (#25). Les drivers sont **additifs** :
  ils ne modifient aucun chemin existant (virtio reste le défaut) → zéro
  régression sur le stockage actuel.

## Sources spec
- NVM Express Base Specification 1.4 (registres CAP/CC/CSTS/AQA, doorbells, PRP).
- Serial ATA AHCI Specification 1.3.1 (HBA/port, command list, FIS, PRDT).
- OSDev Wiki (NVMe, AHCI) — schémas d'init de référence.

---

# Table de partitions GPT/MBR — `exo-partition`

> Date : 2026-06-19. Comble le **gros trou** : avant, ExoOS n'avait **aucun**
> parseur de table de partitions — le LBA de début du volume était **codé en dur**
> (`KERNEL_PARTITION_LBA_START = 2048` côté bootloader, superblock supposé au LBA 0
> côté kernel). Équivalent de `redox-os/drivers/storage/partitionlib`.

## Crate `drivers/storage/partition` (`exo-partition`)

`no_std` + `alloc`, **aucune dépendance externe** (CRC-32, GUID, structures
on-disk en Rust pur). **Source UNIQUE** partagée par le bootloader (`exo-boot`)
**et** le kernel → impossible que les deux divergent.

| Module | Rôle | Tests |
|--------|------|-------|
| `guid.rs` | GUID mixed-endian on-disk + `parse_str` canonique + `Display` ; type-GUIDs ESP / ExoFS ROOT / ExoFS DATA | 3 |
| `crc32.rs` | CRC-32 IEEE (poly `0xEDB88320`) — validation header + table | 3 |
| `gpt.rs` | `GptHeader::parse` (signature `EFI PART` + **CRC header**), `validate_table_crc`, `GptPartitionEntry` | 4 |
| `mbr.rs` | `Mbr::parse` (4 entrées, signature `0xAA55`), détection **MBR protecteur GPT** (`0xEE`) | 3 |
| `lib.rs` | trait `BlockReader`, `scan()` (GPT primaire → **fallback backup** → MBR legacy), résolution ESP/ROOT/DATA par type-GUID, garde anti-OOM `MAX_GPT_ENTRIES=256` | 4 |

**17 tests** (host) — disque GPT synthétique avec CRC réels, fallback header de
backup, MBR legacy, rejet signature/CRC corrompus.

## Câblage kernel — `fs/exofs/storage/partition_scan.rs`

Le kernel ne suppose **plus** « disque entier = volume ». Au montage
(`exofs_init`, après `init_global_disk`, avant le boot recovery) :

1. `scan_root()` enveloppe le `BlockDevice` global dans un `BlockReader` et lance
   `exo_partition::scan`.
2. Si — et seulement si — un **GPT valide** contient une partition **ExoFS ROOT**
   (par type-GUID), le disque global est enveloppé dans un `PartitionOffsetDevice`
   qui **décale tout l'I/O ExoFS** vers le LBA de début de la partition et **borne**
   la capacité à la taille de la partition. Le superblock est alors lu au début de
   la partition, pas au LBA 0 du disque.
3. **Additif — zéro régression** : disque brut (images mkfs actuelles, superblock
   au LBA 0), MBR legacy, GPT sans partition ROOT, ou **toute** erreur de parsing
   (CRC/signature) → **aucun décalage**, comportement LBA 0 inchangé.

**5 tests** (host) : détection ROOT sur GPT, `None` sur disque brut / MBR legacy,
translation de LBA, write round-trip à travers l'offset.

## Câblage bootloader — `exo-boot/src/disk/gpt.rs` (UEFI)

Adaptateur `BlockReader` sur `EFI_BLOCK_IO_PROTOCOL` (lecture seule, non
exclusive → pas de conflit avec le driver FAT du firmware). `scan_boot_disk()`
énumère les disques physiques (Block I/O non-partition) et localise les
partitions ExoFS par type-GUID. Diagnostic **non fatal** loggé au boot (le kernel
re-scanne de toute façon). Le chemin UEFI charge le kernel via le protocole
fichier FAT de l'ESP — le firmware gère déjà le partitionnement pour CE besoin ;
ce module sert à tracer/valider la table et préparer le passage des LBA ExoFS.

## Vérification GPT
- `exo-partition` : **17 tests** host (✅).
- `partition_scan` (kernel) : **5 tests** host + suite storage **190 tests** (✅,
  zéro régression).
- Build **bare-metal kernel** `x86_64-unknown-none` (✅) et **exo-boot UEFI**
  `x86_64-unknown-uefi` (✅).
- ⚠️ Bout-en-bout QEMU avec disque réellement partitionné GPT : à faire une fois
  le boot-to-shell réparé (#25). En l'état, les images QEMU sont des volumes ExoFS
  bruts (pas de GPT) → le chemin LBA 0 reste actif (comportement inchangé).

## Sources spec GPT
- UEFI Specification 2.x §5.3 (GPT : header, partition entry array, CRC-32).
- `redox-os/drivers/storage/partitionlib` (référence d'architecture).
