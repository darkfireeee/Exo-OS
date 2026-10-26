# Rapport d'Analyse : Chargeur d'Amorçage (exo-boot)

## 1. Vue d'Ensemble

Le sous-système `exo-boot` constitue le chargeur d'amorçage natif d'Exo-OS. Il unifie deux flux de démarrage distincts :
1. **Chemin UEFI Moderne (`x86_64-unknown-uefi`) :** Exécuté directement en mode 64 bits par le firmware UEFI, il gère la détection GOP (Graphics Output Protocol), l'interrogation de la table mémoire système, la vérification cryptographique Ed25519 de l'image noyau ELF (`exo-verity`), la randomisation KASLR, et le passage sans retour via `ExitBootServices`.
2. **Chemin BIOS Legacy (`x86_64-unknown-none`) :** Amorcé depuis le MBR (`stage1.asm` puis `stage2.asm`), il bascule en mode long 64 bits, récupère la carte mémoire E820, initialise le VGA en mode texte, valide les signatures, et transfère le contrôle au noyau.

---

## 2. Analyse Ligne par Ligne et Invariants de Boot

### 2.1 Séquence d'Amorçage UEFI (`efi_main`)

- **Étape 1 : Initialisation de l'Environnement (`uefi-services`) :**
  - Alloue l'allocateur de mémoire UEFI pour les types dynamiques et configure le logger de console `boot_println!`.
- **Étape 2 : Chargement de Configuration (`exo-boot.cfg`) :**
  - Lit le fichier de configuration depuis la partition ESP (`EFI/BOOT/exo-boot.cfg`).
  - Extrait les options : chemin du noyau (`/EFI/EXO/KERNEL.ELF`), drapeaux KASLR (`kaslr_enabled`), et politique de sécurité (`secure_boot_required`).
- **Étape 3 : Framebuffer GOP & Diagnostic GPT :**
  - Initialise le GOP pour obtenir la résolution native, le stride et l'adresse physique du framebuffer.
  - Déclenche `scan_boot_disk` via le parseur partagé `exo-partition` sur `EFI_BLOCK_IO` pour localiser dynamiquement les GUIDs des partitions racine (`ExoFS-ROOT`) et de données (`ExoFS-DATA`).
  - Enregistre l'entrée de démarrage dans la NVRAM UEFI (`BootXXXX` et `BootOrder`).
- **Étape 4 : Carte Mémoire UEFI :**
  - Collecte les descripteurs de mémoire physique du firmware (`collect_uefi_memory_map`).
- **Étape 5 : Sécurité Fail-Closed Ed25519 (RÈGLE BOOT-02) :**
  - Charge l'image ELF complète en mémoire.
  - Exécute `kernel_loader::verify::enforce_or_panic` en s'appuyant sur le crate `exo-verity` :
    - Si l'image est altérée (*Tampered*) $\rightarrow$ refus absolu d'exécution et panic immédiat.
    - Si l'image est non signée $\rightarrow$ refus si `secure_boot_required` ou UEFI Secure Boot enforcing, avertissement sinon (environnement de développement).
- **Étape 6 : Entropie Cryptographique (RÈGLE BOOT-05) :**
  - Récupère 64 octets d'entropie matérielle via `EFI_RNG_PROTOCOL`.
- **Étape 7 : Parsing ELF & KASLR :**
  - Analyse les en-têtes ELF64, valide les segments PT_LOAD, applique les relocations PIE si KASLR est actif, et alloue la mémoire de destination.
- **Étape 8 & 9 : Construction de `BootInfo` et Tables de Pages :**
  - Alloue le pool de tables de pages **avant** `ExitBootServices`.
  - Construit la structure canonique `BootInfo` (mémoire, framebuffer, pointeur RSDP ACPI, entropie, offsets ELF).
- **Étape 10 : `ExitBootServices` — Point de Non-Retour (RÈGLE BOOT-06) :**
  - Coupe définitivement les services de boot du firmware UEFI.
  - Construit les tables de pages de niveau 4 (PML4) du noyau à partir du pool pré-alloué sans aucun service UEFI.
- **Étape 11 & 12 : Barrières et Saut Kernel :**
  - Flush complet du TLB (`mov cr3, rax`) et barrière mémoire `mfence`.
  - Invoque `handoff_to_kernel`, passant l'adresse de `BootInfo` dans les registres et sautant sur `_start_uefi` / `_start64`.

---

### 2.2 Séquence d'Amorçage BIOS (`exoboot_main_bios`)

- Réception des registres de `stage2.asm` avec le tampon E820.
- Affichage direct via pilote VGA texte 80x25 (`VgaWriter`).
- Lecture directe de l'image kernel depuis la mémoire shadow à 2 MiB (`STAGE2_DISK_SHADOW_BASE = 0x0020_0000`).
- Validation Ed25519 avec politique de refus si `secure_boot_required = true`.
- Collecte d'entropie via le timer matériel PIT/TSC (`collect_entropy_bios`).
- Construction des tables de pages dans `BIOS_PAGE_TABLE_POOL` et handoff vers le noyau.

---

## 3. Synthèse de Sûreté et Robustesse

| Aspect | Risque | Mécanisme de Défense / Invariant |
| :--- | :--- | :--- |
| **Authenticité Noyau** | Exécution d'un binaire malveillant ou altéré | Vérification Ed25519 fail-closed via `exo-verity` intégrée systématiquement. |
| **KASLR** | Prédictibilité de l'adresse de chargement | 64 octets d'entropie matérielle via `EFI_RNG_PROTOCOL`. |
| **ExitBootServices** | Crash dû à une allocation après sortie de l'UEFI | Pool de tables de pages pré-alloué avant `ExitBootServices`. |
| **Transitions 32/64** | Corruption d'états lors du switch | Stack et GDTR dédiées, `mfence`, rechargement de CR3. |

---

## 4. Statut de Conformité

- **Conformité aux Spécifications :** Conforme aux règles BOOT-01 à BOOT-07 (DOC10).
- **Sécurité :** Absence de porte dérobée, signatures cryptographiques obligatoires en mode enforcing.
