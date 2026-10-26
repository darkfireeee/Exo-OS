# Rapport d'Analyse : Storage Crates (fscrypt, partition, virtio_blk)

## 1. Vue d'Ensemble

Les trois crates de stockage situés dans `drivers/storage/` assurent l'infrastructure physique et cryptographique du système de fichiers ExoFS :
1. `drivers/storage/fscrypt` : Cryptographie de chiffrement au repos (*at-rest*), source unique de vérité partagée entre le noyau (`no_std`) et l'outil de formatage de disques racine `exofs-mkroot` (`std`).
2. `drivers/storage/partition` : Parseur universel de tables de partitions GPT (GUID Partition Table) et MBR (Master Boot Record), partagé entre le chargeur d'amorçage `exo-boot` et le noyau Exo-OS.
3. `drivers/storage/virtio_blk` : Pilote matériel VirtIO Block Device pour contrôleurs d'E/S virtualisés (Legacy PCI Transport et backend sparse de test en mémoire).

---

## 2. Analyse Ligne par Ligne et Invariants de Conception

### 2.1 `drivers/storage/fscrypt`

- **Résolution des Conflits d'Architecture (`x86_64-unknown-none`) :**
  - **Problème matériel résolu :** La cible de compilation bare-metal tourne initialement sans extensions SIMD AVX/AVX512 et avec `SSE2` désactivé par sécurité d'intégrité de pile dans le noyau (`-sse2`). Le crate générique `chacha20` ou `poly1305` de RustCrypto déclenche une erreur interne LLVM (`LLVM ERROR: Do not know how to split (128-bit ops sans SSE2)`).
  - **Solution architecturale :** Implémentation certifiée conforme à la RFC 8439 en arithmétique `u32` pure (`quarter_round`, `double_rounds`, `hchacha20`, `chacha20_block`).
  - **Involution :** `xchacha20_xor` utilise un flux de bits XOR pur (compteur initial à 1), garantissant une symétrie exacte `encrypt == decrypt` et zéro allocation de mémoire.

- **Schéma AEAD (Authentification et Intégrité) :**
  - Combine le chiffrement de flux XChaCha20 avec un MAC BLAKE3 keyé sous le contexte strict `"ExoOS-Kernel-XChaCha20-BLAKE3-MAC-v1"`.
  - Tag tronqué à 16 octets (`TAG_LEN = 16`).
  - `aead_open` effectue une vérification du tag en temps constant (constant-time XOR accumulation `diff |= expected[i] ^ tag[i]`) avant toute tentative de déchiffrement en place, éliminant les attaques par oracle d'erreur ou altération de données.

- **Dérivation de Clé KEK et Wrap de Superblock :**
  - Utilise l'algorithme Argon2id (`derive_kek`) avec paramètres calibrés pour la résistance mémoire : mémoire = 64 MiB (`65536` blocs), temps = 3 itérations, parallélisme = 4 threads, taille de clé = 32 octets.
  - Structure `WRAPPED_VK_LEN = 110` octets :
    - Magic `EXVK` (4 octets).
    - Version `1` (1 octet).
    - Source `0` (passphrase, 1 octet).
    - Salt (32 octets).
    - Nonce (24 octets).
    - Clé chiffrée (32 octets).
    - Tag d'authentification (16 octets).
  - Dérivation déterministe par bloc : `blob_key` dérive la clé par blob via `blake3::derive_key("exofs-atrest-key-v1", volume_key || blob_id)`, et `block_nonce` hache le couple `blob_id || disk_offset`. Aucun nonce n'est réutilisé sous la même clé.

---

### 2.2 `drivers/storage/partition`

- **Suppression des LBA en Dur :**
  - Élimine le besoin de hardcoder les numéros de secteur LBA dans le chargeur d'amorçage ou le noyau. Le scanner analyse la véritable structure GPT du disque physique.
  - Identifiants de partitions canoniques :
    - `GUID_ESP` : Partition EFI System (`C12A7328-F81F-11D2-BA4B-00A0C93EC93B`).
    - `GUID_EXOOS_ROOT` : Partition racine immuable ExoFS (`45584F53-524F-4F54-0000-000000000001`).
    - `GUID_EXOOS_DATA` : Partition persistante de données ExoFS (`45584F53-4441-5441-0000-000000000002`).

- **Tolérance aux Pannes et Redondance GPT :**
  - **Inspection MBR Protecteur :** LBA 0 vérifié pour la signature `0xAA55` et le type `0xEE` (GPT protective MBR).
  - **Header Primaire et Backup :** Tente la lecture du header GPT primaire à LBA 1. En cas d'erreur de lecture physique ou de corruption de checksum CRC32, bascule automatiquement sur le header de secours (*backup GPT header*) situé sur le dernier LBA du disque (`num_blocks - 1`).
  - **Validation d'Intégrité CRC32 :** Implémentation optimisée sans table lourde de `crc32`, vérifiant le CRC du header et le CRC de la table d'entrées.
  - **Garde-fou Anti-Déni de Service :** `MAX_GPT_ENTRIES = 256` empêche toute allocation infinie de mémoire si un header de partition corrompu annonçait des millions d'entrées.

- **Conception Modulaire (`no_std` sans allocation) :**
  - Les modules `crc32`, `guid`, `gpt` et `mbr` ne requièrent aucun allocateur (`#[no_std]` pur).
  - Le trait `BlockReader` et la fonction `scan()` utilisent l'allocateur (`alloc::vec::Vec`) lorsque la feature `alloc` est active, garantissant une intégration sans friction dans le noyau comme dans `exo-boot`.

---

### 2.3 `drivers/storage/virtio_blk`

- **Abstraction et Interface Matérielle :**
  - Implémente le trait `exo_storage_common_hal::BlockDeviceOps` pour exposer une interface standard de périphérique bloc (`read_block`, `write_block`, `flush`, `total_blocks`, `block_size`).
  - Transport Legacy PCI : Gère les registres I/O ports standard de la spécification VirtIO 0.9.5 (`VirtIOBlk<ExoHal, LegacyPciTransport>`).
  - Traduction de secteurs : Convertit les blocs logiques ExoFS de 4096 octets (`EXOFS_BLOCK_SIZE`) en secteurs matériels de 512 octets (`SECTOR_SIZE`, soit un ratio de 8 secteurs par bloc ExoFS).

- **Sûreté Mémoire et Vérification des Bornes :**
  - Vérification de la taille du tampon : Tout appel avec `buf.len() != 4096` est immédiatement rejeté avec `ExoVirtioBlkError::InvalidBuffer`.
  - Protection contre les dépassements d'adresses : Vérifie `block_id >= self.total_blocks()` et prévient les débordements arithmétiques `block_id.checked_mul(SECTORS_PER_EXOFS_BLOCK)`.
  - Détection en lecture seule : Refuse l'attachement d'un disque configuré en mode lecture seule si des écritures sont requises.

- **Backend Sparse de Simulation :**
  - Fournit un backend de simulation `SparseBackend` basé sur une `BTreeMap<u64, Box<[u8]>>` protégée par un `spin::Mutex`.
  - Permet l'exécution de suites de tests unitaires et de stress sans nécessiter une machine virtuelle QEMU active.

---

## 3. Synthèse de Robustesse

| Composant | Risque Traité | Solution Déployée |
| :--- | :--- | :--- |
| **fscrypt** | Crash instruction SSE2/AVX sur `x86_64-unknown-none` | XChaCha20 en arithmétique u32 pure + MAC BLAKE3 keyé. |
| **fscrypt** | Divergence de clé entre formatage et montage | Source unique de vérité partagée entre `kernel` et `exofs-mkroot`. |
| **partition** | Corruption de table de partitions | Validation double CRC32 + fallback transparent sur le backup GPT. |
| **virtio_blk** | Dépassement de capacité ou buffer invalide | `checked_mul` et assertions de longueur à 4096 octets systématiques. |

---

## 4. Statut de Conformité

- **Conformité aux Spécifications :** Conforme aux standards RFC 8439, UEFI GPT Spécification 2.10, et VirtIO 0.9.5.
- **Sécurité Mémoire :** Zéro pointeur brut non protégé, absence d'UB.
