# Analyse Approfondie — Module `kernel/src/arch/x86_64/boot/`

## 1. Vue d'Ensemble du Module

Le sous-module `kernel/src/arch/x86_64/boot/` constitue le point de convergence entre le chargeur d'amorçage (Multiboot2 hérité ou exo-boot UEFI GPT moderne) et le noyau Exo-OS 64 bits :
- **Séquenceur d'Amorçage Précoce** (`early_init.rs`) : orchestre les 14 étapes séquentielles obligatoires pour hisser le processeur de démarrage (BSP) depuis son état brut post-bootloader jusqu'à un environnement sécurisé multiprocesseur prêt pour `kernel_main()`.
- **Pont Mémoire Boot ↔ Noyau** (`memory_map.rs`) : traduit la cartographie physique de la RAM (tables E820 de Multiboot2 ou descripteurs EFI d'exo-boot) vers les 4 phases du gestionnaire de mémoire (`emergency_pool`, `bitmap`, `buddy DMA32`, `slab/slub`, `numa`).
- **Parseur Multiboot2** (`multiboot2.rs`) : extrait les balises MB2 (ligne de commande, modules initramfs, mémoire de base, carte E820, pointeur RSDP, et framebuffer graphique linéaire).
- **Trampoline SMP Multicœur** (`trampoline_asm.rs`) : code hybride 16/32/64 bits copié à l'adresse physique `0x6000` pour recevoir le signal SIPI (*Startup Inter-Processor Interrupt*), basculer chaque cœur applicatif (AP) en mode long 64 bits sous la table PML4 du BSP, et entrer dans `ap_entry()`.
- **Support Protocole UEFI** (`uefi.rs`) : structures de descripteurs mémoire EFI (`EfiMemoryDescriptor`) avec itérateur tenant compte des tailles variables de descripteurs retournées par `ExitBootServices()`.
- **Point d'Entrée & Dépréciation** (`mod.rs`) : expose l'API unifiée de démarrage et documente la stratégie de transition vers l'architecture UEFI-only (vision Strata).

---

## 2. Analyse Fichier par Fichier : Sûreté, Cohérence et Vulnérabilités

### 2.1. `mod.rs`
- **Rôle :** Organisation modulaire de l'amorçage.
- **Interconnexions :** Conditionne l'inclusion de `multiboot2` via la fonctionnalité `multiboot2_compat` (actif par défaut pour la compatibilité QEMU/dev, désactivable en production).
- **Jugement de sûreté :** `SAFE`.

---

### 2.2. `early_init.rs`
- **Rôle :** Fonction maîtresse `arch_boot_init()` appelée dès la première instruction Rust en Ring 0.
- **Interconnexions :**
  - Appelle successivement tous les sous-systèmes d'architecture : `cpu::features`, `gdt`, `idt`, `irq::pic`, `smp::percpu`, `cpu::tsc`, `cpu::fpu`, `virt::detect`, `acpi::parser`, `apic`, `syscall`, `memory_map`, `spectre`, `security`, `trampoline_asm`.
- **Audit de sûreté & vulnérabilités :**
  - **Sondes de diagnostic matérielles (`probe!`) :** Écrit des caractères ascii séquentiels (`b'1'` à `b'9'`, `b'a'` à `b'i'`) sur le port d'E/S de débogage `0xE9`. Si un blocage ou un crash matériel intervient au boot, la dernière sonde identifie instantanément l'étape défaillante.
  - **Priorité absolue de l'EmergencyPool (Règle MEM-02 DOC2) :** Avant d'initialiser tout allocateur physique (bitmap ou buddy), `crate::memory::physical::frame::emergency_pool::init()` est systématiquement appelé. Cette réserve inviolable de frames garantit qu'une situation de famine mémoire au démarrage ne pourra jamais corrompre les structures fondamentales du noyau.
  - **Séquence KPTI et Mitigations Spectre :** `spectre::apply_mitigations_bsp()` est exécuté à l'étape 12b, *après* que la pagination kernel (`KERNEL_AS`) et l'allocateur de frames soient opérationnels. KPTI nécessitant l'allocation dynamique de pages d'ombres (*shadow page tables*) pour isoler l'espace utilisateur, tenter de l'activer plus tôt provoquait un écroulement par faute de page.
  - **Initialisation KASLR sécurisée :** L'entropie injectée dans `security::security_init()` combine le TSC, les paramètres du bootloader, et un tirage aléatoire matériel cryptographique `rdrand_fill()`, garantissant la robustesse de la randomisation des adresses noyau.
- **Jugement :** `EXCELLENT / SAFE`. Respect rigoureux des précédences temporelles.

---

### 2.3. `multiboot2.rs`
- **Rôle :** Décodage des tags Multiboot2 passés en mémoire.
- **Interconnexions :** Connecté à `early_init.rs` et `memory_map.rs`.
- **Audit de sûreté & vulnérabilités :**
  - **Contrôle de taille des balises :** Toutes les lectures de tags (`TAG_MODULE`, `TAG_MMAP`, `TAG_FRAMEBUFFER`) sont protégées par des gardes `if tag_size >= SEUIL`, prévenant toute lecture hors de la zone mémoire allouée par le chargeur d'amorçage.
  - **Calcul protégé du stride framebuffer :** L'opération `fb_pitch.checked_div(bytes_per_pixel).unwrap_or(0)` neutralise toute division par zéro si un firmware ou émulateur retournait un champ `bpp` corrompu.
  - **Lectures non alignées (`read_unaligned`) :** La spécification Multiboot2 aligne les tags sur 8 octets mais permet des champs internes arbitraires. L'utilisation de `read_unaligned` empêche tout UB sur processeur x86_64 strict.
- **Jugement :** `SAFE`.

---

### 2.4. `trampoline_asm.rs`
- **Rôle :** Trampoline d'éveil des cœurs secondaires APs.
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/smp/init.rs` qui déclenche les interruptions IPI INIT/SIPI.
  - Exécute `ap_entry(cpu_id, lapic_id, kernel_stack_top)` sur chaque AP une fois le mode long 64 bits établi.
- **Audit de sûreté & vulnérabilités :**
  - **Copie d'image atomique à l'adresse 0x6000 :** L'adresse physique `0x6000` réside dans la mémoire basse conventionnelle (*Low Memory* sous 640 KiB), garantie libre et identiquement mappée. La taille du bloc ASM est délimitée par les symboles du linker `trampoline_start` et `trampoline_end`.
  - **Alignement strict de pile 64 bits :** Avant de sauter dans le code Rust compilé (`call ap_entry`), l'instruction `and rsp, -16` force l'alignement sur 16 octets exigé par la convention d'appel System V AMD64 ABI, éliminant les fautes d'alignement `#GP` lors des premières instructions vectorielles SSE du runtime.
  - **Handshake et PML4 partagé :** L'adresse physique de la table PML4 du BSP lue dans `CR3` est écrite directement à l'offset `0x6020` du trampoline avant le premier SIPI, assurant que les APs rejoignent immédiatement le même espace d'adressage virtuel que le BSP.
- **Jugement :** `CRITIQUE & SAFE`. Implémentation conforme aux spécifications Intel MultiProcessor.

---

### 2.5. `uefi.rs`
- **Rôle :** Parsing de la carte mémoire fournie par l'interface UEFI.
- **Interconnexions :** Connecté à `memory_map.rs`.
- **Audit de sûreté & vulnérabilités :**
  - L'itérateur `UefiMemMapIterator` avance par bonds de `map.desc_size` (indiqué dynamiquement par le firmware UEFI) plutôt que par `sizeof(EfiMemoryDescriptor)`. Cette précaution architecturale est vitale car de nombreux firmwares UEFI ajoutent des champs propriétaires ou d'alignement à la fin des descripteurs standards.
  - `is_usable()` filtre précisément les régions de mémoire conventionnelle et de services de boot libérés, excluant la mémoire d'exécution réservée (`EFI_RUNTIME_SERVICES_*`) et les plages MMIO ACPI.
- **Jugement :** `SAFE`.

---

### 2.6. `memory_map.rs`
- **Rôle :** Passerelle entre les cartes de mémoire physique brutes et les allocateurs du noyau.
- **Interconnexions :**
  - Démarre la bitmap de bootstrap (`BOOTSTRAP_BITMAP`), le buddy allocator pour la zone basse DMA32 (`< 4 GiB`), et enregistre le `KernelSlabProvider` auprès du système SLAB/SLUB.
- **Audit de sûreté & vulnérabilités :**
  - La bitmap statique `BUDDY_DMA32_BITMAP` allouée en `.bss` est dimensionnée à 16 384 mots `u64` (128 KiB), ce qui couvre exactement les 1 048 576 pages de 4 KiB de l'espace physique 4 GiB.
  - `KernelSlabProvider` implémente un double mécanisme de repli : tente d'allouer via le buddy allocator dès qu'il est actif, et bascule de manière transparente sur la bitmap de bootstrap si le sous-système de mémoire physique est encore en cours d'initialisation (`AllocError::NotInitialized`), évitant les blocages circulaires lors de la création des premiers caches d'objets.
- **Jugement :** `SAFE`.

---

## 3. Synthèse des Correctifs & Améliorations Apportés

1. **Suppression du cast redondant `bsp_lapic as u32`** dans `early_init.rs`.
2. **Effondrement des gardes imbriquées** `TAG_MODULE if tag_size >= 16` et `TAG_FRAMEBUFFER if tag_size >= 30` dans `multiboot2.rs`.
3. **Sécurisation de la division du stride framebuffer** avec `.checked_div(bytes_per_pixel).unwrap_or(0)` dans `multiboot2.rs`.
