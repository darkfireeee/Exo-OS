# Rapport d'Analyse : Kernel Root (main.rs, lib.rs, userspace_boot.rs)

## 1. Vue d'Ensemble

Le répertoire racine de `kernel/src/` orchestre le cycle de vie complet du noyau Exo-OS, depuis la prise en charge initiale par le chargeur d'amorçage (BIOS GRUB Multiboot2 ou UEFI via `exo-boot`) jusqu'à l'exécution de la boucle idle et le lancement de PID 1 (`/sbin/exo-init-server`).

### Fichiers Analysés :
1. `kernel/src/main.rs` : Point d'entrée binaire (`_start`, `_start_uefi`, `_start64`, `kernel_main`), trampoline assembleur 32-bit vers 64-bit, pile de boot BSP (512 KiB), tables de pages de boot initiales (identitée 0..1 GiB + MMIO APIC 3..4 GiB), et boucle de veille finale.
2. `kernel/src/lib.rs` : Déclaration de l'architecture en couches (Couches 0 à 3, transverse arch, drivers GI-03, ExoPhoenix), re-exports publics, gestionnaire de panic sans allocation (`kernel_panic` via port I/O 0xE9), gestionnaire d'erreur d'allocation (`alloc_error_handler`), et fonction maîtresse d'initialisation des sous-systèmes (`kernel_init`).
3. `kernel/src/userspace_boot.rs` : Handoff vers l'espace utilisateur Ring 1 depuis le disque racine ExoFS, chargement d'init (`/sbin/exo-init-server`), initialisation du PCB/TCB de PID 1, et transition de l'état Phoenix vers `PhoenixState::Normal`.

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 `main.rs`

- **En-tête Multiboot2 (lignes 36-54) :**
  - Section dédiée `.multiboot2`, alignée sur 8 octets, placée en tête du binaire dans `linker.ld` (< 32 KiB).
  - Magic `0xE85250D6`, architecture i386 (0), checksum valide `0x17ADAEFA`. Tag framebuffer demandant 1024x768x32.
  - Invariant : Permet à GRUB de basculer la machine en mode protégé 32-bit avec framebuffer VBE/GOP initialisé.

- **Tables de Pages de Boot Provisoires (lignes 70-92) :**
  - Allouées dans `.bss` alignées à 4096 octets : `_boot_pml4`, `_boot_pdpt`, `_boot_pd`, `_boot_pd_high`.
  - Invariant de sûreté : Zero-initialisées par le BSS. Le trampoline configure `_boot_pd` avec 512 huge pages de 2 MiB mappant l'adresse physique `0..1 GiB` en identité `0..1 GiB` (flags `P=1`, `R/W=1`, `PS=1` $\rightarrow$ `0x83`).
  - MMIO APIC : `_boot_pd_high` mappe les pages 2 MiB pour l'IOAPIC (`0xFEC00000`, index 502) et le LAPIC (`0xFEE00000`, index 503) avec cache disabling (`PCD=1`, `PWT=1` $\rightarrow$ `0x9B`), indispensable pour l'accès immédiat aux registres APIC avant la mise en place du VMM complet.
  - Physmap : `_boot_pml4[256]` pointe vers `_boot_pdpt` pour mapper `0xFFFF_8000_0000_0000` vers `0..1 GiB` physique, assurant que les accès noyau direct-map fonctionnent immédiatement.

- **GDT 64-bit Minimale de Boot (lignes 100-113) :**
  - 3 descripteurs de 8 octets :
    - 0x00 : Null Descriptor.
    - 0x08 : Code 64-bit DPL=0, L=1, Type=0xA.
    - 0x10 : Données 64-bit DPL=0, D=1, Type=0x2.
  - Invariant : GDTR (`_boot_gdtr`) pointe vers cette table avec limite 23 (24 octets).

- **Trampoline 32-bit vers 64-bit `_start` (lignes 127-243) :**
  - Entrée en mode protégé 32-bit.
  - `cli` et `cld` immédiats pour garantir l'absence d'interruption et le sens d'incrément des chaînes.
  - Sauvegarde de `EAX` (magic) et `EBX` (pointeur multiboot info physique) dans `_mb2_saved_magic` et `_mb2_saved_info`.
  - Configuration de `ESP` sur `_exo_boot_stack_top` aligné sur 16 octets.
  - Remplissage de la boucle de page directory (512 entrées de 2 MiB).
  - Chargement de `CR3` avec `_boot_pml4`.
  - Activation de `CR4.PAE` (bit 5).
  - Activation de `EFER.LME` (bit 8 de l'MSR 0xC0000080).
  - Activation de `CR0.PG` (bit 31), faisant basculer le processeur en mode compatibilité 64-bit.
  - Far return (`retf`) chargeant `CS=0x08` et `EIP=_start64` : transition atomique et matérielle vers le mode long 64-bit.

- **Entrée UEFI `_start_uefi` (lignes 251-260) :**
  - Entrée directe en mode 64-bit long mode depuis `exo-boot`.
  - Sauvegarde `EAX` et `RBX` dans `_mb2_saved_magic` / `_mb2_saved_info` puis saut direct vers `_start64`.

- **Entrée 64-bit `_start64` (lignes 263-295) :**
  - Diagnostic précoce : écriture de l'octet `'X'` (0x58) sur le port de débogage QEMU `0xE9`.
  - Rechargement de `DS`, `ES`, `SS` avec `0x10`, mise à zéro de `FS` et `GS`.
  - Configuration de `RSP` sur `_exo_boot_stack_top`.
  - Préparation des registres d'arguments selon l'ABI System V AMD64 :
    - `RDI` = `mb2_magic` (u32)
    - `RSI` = `mb2_info` (u64)
    - `RDX` = `rsdp_phys` (u64, 0 si non fourni)
  - Appel de `kernel_main`. Si retour inattendu, boucle `cli; hlt`.

- **Pile de Boot BSP `_exo_boot_stack_bottom` (lignes 297-313) :**
  - Taille : 512 KiB (524288 octets).
  - Placée dans la section dédiée `.boot_stack, "aw", @nobits`.
  - Sécurité : Évite tout débordement de pile lors de l'initialisation profonde d'ExoFS, VirtIO et Phoenix SSR (l'ancienne pile de 64 KiB débordait et corrompait les structures adjacentes).

- **Fonction `kernel_main` (lignes 325-438) :**
  - Point d'entrée Rust en Ring 0 (`unsafe extern "C"`).
  - Marqueur debug `'K'` (0x4B) sur le port `0xE9`.
  - Écran de boot précoce (`boot_display::boot_screen()`).
  - **Phase 1 :** Appel de `kernel::arch_boot_init(mb2_magic, mb2_info, rsdp_phys)`.
    - Initialise CPU, GDT définitive avec TSS, IDT, handlers d'exceptions/interruptions, per-CPU, TSC, APIC, ACPI, SMP et Spectre mitigations.
    - Marqueur `'A'` (0x41).
  - Prise en charge du framebuffer GOP/VBE (`attach_framebuffer`).
  - **Phases 2 à 7 :** Appel de `kernel::kernel_init(boot_info.cpu_count)`.
    - Marqueur `'I'` (0x49).
  - **Phase 8 :** Lancement de l'espace utilisateur (`boot_userspace()`).
  - Marqueur d'accomplissement : `\nOK\n` sur le port `0xE9`.
  - Exécution de la boucle idle du scheduler (`run_scheduler_idle()`), maintenant les interruptions actives (`STI; HLT`) pour permettre la préemption par le timer et le réveil des kthreads.

---

### 2.2 `lib.rs`

- **Ordonnancement Strict des Couches dans `kernel_init` :**
  1. **Phase 2a - Mémoire d'urgence :** `emergency_pool::init()` est invoqué en **premier absolu** (RÈGLE EMERGENCY-01) pour garantir qu'aucune allocation ne puisse échouer par manque de mémoire noyau critique.
  2. **Phase 2b - Allocateur Heap & CoW :** `hybrid::init()` (allocateur SLUB + Buddy), `cow::init()`, et remappage fixmap du LAPIC.
  3. **Phase 2c - Sous-système Temporel :** `time::time_init()` orchestre HPET, calibration TSC, initialisation PLL, ktime seqlock et clock timers.
  4. **Phase 2d - Drivers GI-03 :** `drivers::init()` initialise les files IOMMU et le bus de notification.
  5. **Phase 2e - ExoSeal Pré-Scheduler :** `exoseal_boot_phase0()` verrouille les registres matériels (CET, PKS, IOMMU) avant la création de threads.
  6. **Phase 2f - cgroup racine :** `cgroup::init()` prépare le groupe racine pour les quotas avant que les runqueues du scheduler ne démarrent.
  7. **Phase 3 - Scheduler :** `scheduler::init()` alloue et initialise les runqueues par CPU, lie le thread idle de boot du BSP et des APs.
  8. **Phase 3c - Cloner d'Espace d'Adressage :** Enregistrement de `KERNEL_AS_CLONER` pour permettre `fork()` avant `process::init()`.
  9. **Phase 4 - Processus :** `process::init()` avec masque d'interruptions (`IrqStateGuard`) pour éviter qu'un tick timer n'interrompe l'allocation de la table des PID (32768 slots) et le kthread reaper.
  10. **Phase 5 - Sécurité :** Initialisation du sous-système de sécurité avec entropie KASLR dérivée du TSC et du générateur RDRAND hardware (`security_init`). Initialisation de la graine SipHash de la table futex anti-DoS hash collision. Lancement du moniteur d'intégrité en arrière-plan.
  11. **Phase 5b - ExoPhoenix Stage 0 :** `stage0_init_all_steps(true)` configure le domaine IOMMU de blocage et isole les périphériques DMA.
  12. **Phase 6 - IPC :** Initialisation des anneaux SPSC lock-free, réservation d'un pool de mémoire partagée physique dédié (1 MiB, order 8), installation des hooks de blocage scheduler et des hooks VMM pour le mappage zero-copy.
  13. **Phase 7 - FS :** Initialisation d'ExoFS sur le périphérique bloc global, amorçage de l'image kernel dans la forge Phoenix, enregistrement du chargeur ELF (`EXO_ELF_LOADER`) et du fournisseur de demand-paging, puis activation des passerelles `fs_bridge` et `net_bridge`.

- **Gestion des Panics (`kernel_panic`) et OOM (`alloc_error_handler`) :**
  - Conçus selon les directives strictes de résilience : **aucune allocation dynamique** (`no-alloc`), aucun appel aux sous-systèmes verrouillés.
  - Sortie directe via l'assembleur inline sur le port I/O 0xE9 de débogage QEMU.
  - Formatage d'adresses et de nombres entiers sans `format!` ni heap buffer.
  - Désactivation immédiate des interruptions (`cli`) suivie d'une boucle infinie `hlt` pour figer l'état du processeur en vue d'une analyse post-mortem ou d'un déclenchement Phoenix NMI.

---

### 2.3 `userspace_boot.rs`

- **Bootstrap Ring 1 Découplé :**
  - Conformément à la philosophie micro-noyau d'Exo-OS, aucun binaire utilisateur n'est compilé dans l'image ELF du noyau.
  - `load_elf_for_boot("/sbin/exo-init-server", ...)` résout le chemin directement sur le volume ExoFS monté en racine.
  - `create_init_process_from_elf(elf)` crée le processus PID 1, lui attribue son espace d'adressage isolé, installe le trampoline dynamic-linker `/lib/ld-exo.so` et insère le thread initial dans la runqueue.
  - Transition Phoenix : Dès la création réussie de PID 1, `crate::exophoenix::set_state(PhoenixState::Normal)` valide le succès du démarrage.
  - Télémétrie : Signale chaque étape sur le port 0xE9 (`'u'`, `'L'`, `'C'`, `'D'`).

---

## 3. Synthèse de Sûreté et Robustesse

| Composant | Risque Identifié | Mécanisme de Protection / Invariant |
| :--- | :--- | :--- |
| **Boot Trampoline** | Débordement ou collision de mémoire sous 1 GiB | Tables de boot dédiées isolées dans `.bss`, mapping identité 0..1 GiB en huge pages 2 MiB, pile de boot de 512 KiB dans section `.boot_stack`. |
| **MMIO APIC** | Crash d'accès non paginé lors de l'init APIC | Mappage 2 MiB dédié non mis en cache (`PWT+PCD`, uncached) dans `_boot_pd_high` aux adresses `0xFEC00000` et `0xFEE00000`. |
| **Boot IRQ Race** | Timer APIC déclenchant un switch de thread durant `process::init()` | `IrqStateGuard` masquant les interruptions matérielles (`cli`) avec restauration garantie via `Drop`. |
| **Panic Récursif** | OOM ou panic tentant d'allouer de la mémoire dans le handler | Sortie `kernel_panic` et `alloc_error_handler` 100% no-alloc sur le port matériel I/O `0xE9`. |
| **ExoPhoenix Stage0** | DMA malveillant ou périphérique actif écrasant la mémoire | Domaine IOMMU de blocage configuré en Phase 5b avant toute initialisation IPC ou FS. |

---

## 4. Statut de Conformité

- **Architecture :** Conforme aux spécifications DOC1, DOC2 et DOC3.
- **Sécurité Mémoire :** Zéro allocation dans les chemins critiques de boot et d'erreur.
- **Compilation & Vérification :** Code propre, types vérifiés, invariants documentés.
