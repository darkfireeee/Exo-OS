# Rapport d'Analyse : Dynamic Linker & Outils Système (loader/ et tools/)

## 1. Vue d'Ensemble

Le chargeur dynamique et les outils d'infrastructure complètent la chaîne de confiance et d'exécution d'Exo-OS :
1. `loader/` : Chargeur dynamique utilisateur et runtime linker (`/lib/ld-exo.so`), responsable de l'exécution de tous les binaires ELF64 dynamiques (Ring 1 et Ring 3).
2. `tools/exofs_mkroot` : Générateur de volume racine ExoFS créant l'image disque `exofs-root.img` avec arbres de Merkle, superblocs et structures de chiffrement at-rest.
3. `tools/kernel_signer` : Outil de signature cryptographique Ed25519 pour valider l'intégrité du noyau bare-metal avant amorçage par `exo-boot`.

---

## 2. Analyse Ligne par Ligne et Invariants de Sûreté

### 2.1 `loader` (`/lib/ld-exo.so`)

- **Point d'Entrée Bas Niveau (`src/entry.rs` / `src/main.rs`) :**
  - Entrée en assembleur `_start` recevant le vecteur auxiliaire (*auxv*), la pile initiale, et les arguments `argc`, `argv`, `envp`.
  - Auto-relocation : Avant de pouvoir utiliser des variables globales ou des fonctions externes, le linker applique ses propres relocations R_X86_64_RELATIVE en mémoire (`apply_self_relocations`).

- **Validation Stricte des Segments ELF (`src/elf/`) :**
  - `validator.rs` vérifie la conformité de l'en-tête ELF64 : magic `0x7F 'E' 'L' 'F'`, architecture 64 bits (`ELFCLASS64`), endianness little-endian (`ELFDATA2LSB`), et machine EM_X86_64 (`0x3E`).
  - `segments.rs` mappe les segments `PT_LOAD` avec les permissions minimales requises (W^X respecté : aucune page n'est simultanément inscriptible et exécutable).
  - `relocations.rs` traite les relocations dynamiques `R_X86_64_GLOB_DAT`, `R_X86_64_JUMP_SLOT`, et `R_X86_64_RELATIVE`.

- **Sûreté et Confinement (`src/security/`) :**
  - `pie_aslr.rs` : Impose la randomisation d'adresse (PIE obligatoire pour tous les binaires Ring 1).
  - `verify_signature.rs` : Vérifie la chaîne de confiance Ed25519 du binaire avant tout saut vers son point d'entrée (`e_entry`).
  - `capability_check.rs` : Contrôle les permissions requises par le binaire contre la table des capabilities système.

---

### 2.2 `tools/exofs_mkroot`

- **Construction Déterministe d'Images Disque :**
  - Lit l'arborescence de fichiers source et produit une image partitionnée compatible GPT/MBR.
  - Utilise le crate partagé `exo-fscrypt` pour dériver la KEK par Argon2id et chiffrer les blobs de données en place avec XChaCha20 + MAC BLAKE3 keyé.
  - Construit les blocs d'index, la table des inodes, et calcule les condensés Merkle BLAKE3 pour garantir l'immutabilité des binaires système.

---

### 2.3 `tools/kernel_signer`

- **Garantie de Chaîne de Confiance :**
  - Charge l'image ELF du noyau (`target/x86_64-unknown-none/debug/exo-os-kernel`).
  - Hache les segments chargeables avec SHA-512 et signe le condensé avec la clé privée Ed25519 de l'autorité racine Exo-OS.
  - Insère la signature dans la section de certification dédiée `.exo_signature` ou dans un conteneur externe vérifié par `exo-boot` et `exo-verity`.

---

## 3. Synthèse de Sûreté

| Composant | Vulnérabilité Potentielle | Contre-Mesure Implémentée |
| :--- | :--- | :--- |
| **ld-exo.so** | Injection de bibliothèques malveillantes via LD_PRELOAD | Chemins de recherche stricts (`/lib`, `/usr/lib`), signature Ed25519 obligatoire. |
| **ld-exo.so** | Écriture dans le code exécutable lors des relocations | Pages marquées `RW` temporairement puis verrouillées en `RX` via `mprotect` (W^X). |
| **exofs_mkroot** | Nonces répétés ou dérivation faible | Dérivation déterministe `block_nonce` via BLAKE3 sur `blob_id \|\| disk_offset`. |
| **kernel_signer** | Utilisation de clés compromises | Signature Ed25519 résistante aux collisions, vérification fail-closed à l'amorçage. |

---

## 4. Statut de Conformité

- **Conformité aux Spécifications :** Conforme aux spécifications ELF64 System V ABI, POSIX.1-2017, et Exo-OS Secure Boot v1.0.
