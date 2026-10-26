# Analyse Approfondie — Module `kernel/src/arch/x86_64`

**Chemin :** `Exo-OS/kernel/src/arch/x86_64`  
**Fichiers analysés :**
1. `mod.rs`
2. `gdt.rs`
3. `idt.rs`
4. `tss.rs`
5. `memory_iface.rs`
6. `paging.rs`
7. `sched_iface.rs`
8. `syscall.rs`
9. `boot_display.rs`
10. `framebuffer_early.rs`
11. `terminal.rs`
12. `vga_early.rs`

---

## 1. Vue d'Ensemble du Module

Le dossier `kernel/src/arch/x86_64/` concentre l'implémentation bas niveau pour l'architecture x86_64. Il réalise la virtualisation matérielle du CPU, l'isolation mémoire, les tables de descripteurs et l'interface avec les anneaux de privilège (Ring 0 / Ring 3).

---

## 2. Analyse Détaillée Fichier par Fichier

### 2.1. `mod.rs` — Primitives Architecturales Fondamentales

- **Rôle :** Point d'entrée x86_64. Exporte les instructions assembleur privilégiées et les barrières.
- **Contrôles de Registres & Barrières (Lignes 131–230) :**
  - `memory_barrier()` (`mfence`), `load_fence()` (`lfence`), `store_fence()` (`sfence`). Spécifiés avec `preserves_flags` et `nostack`.
  - `read_cr3()`, `write_cr3(cr3_val)` : manipulation de la racine de pagination. `write_cr3` est correctement qualifié `unsafe` car un pointeur invalide provoquerait un crash immédiat (#PF ou triple fault).
  - `read_cr2()` : lecture de l'adresse de faute sans effet de bord.
  - `read_cr4()`, `write_cr4(cr4_val)` : contrôle SMEP, SMAP, PCIDE, PGE.
- **Gestion des Interruptions & RFLAGS (Lignes 234–304) :**
  - `disable_interrupts()` (`cli`), `enable_interrupts()` (`sti`).
  - `irq_save()` et `irq_restore(flags)` : sauvegarde atomique de RFLAGS via `pushfq; pop` et restauration via `push; popfq`.
- **Ports I/O (Lignes 311–385) :**
  - `outb`, `inb`, `outl`, `inl` : manipulation directe des ports 8/32 bits (`al`/`eax`, `dx`). Tous qualifiés `unsafe`.
  - `io_delay()` : écriture sur port 0x80 (délai physique ~1 µs pour bus ISA/legacy).
- **Idle Loop & Scheduler (Lignes 96–120) :**
  - `run_scheduler_idle()` : active explicitement les interruptions (`sti`) avant `hlt` pour que les interruptions matérielles (APIC Timer, IPI, clavier) réveillent le CPU.
- **Verdict :** **SAFE**. Déclarations assembleur exactes et gestion adéquate des attributs `nomem` et `nostack`.

---

### 2.2. `gdt.rs` — Global Descriptor Table (GDT)

- **Rôle :** Segments 64-bit et compatibilité MSR STAR pour les instructions `SYSCALL`/`SYSRET`.
- **Layout des Sélecteurs (Lignes 38–67) :**
  ```rust
  0x00 : NULL
  0x08 : KERNEL_CS  (Ring 0 Code)
  0x10 : KERNEL_DS  (Ring 0 Data)
  0x18 : USER_CS32  (Ring 3 Compat Code)
  0x20 : USER_DS    (Ring 3 Data - SYSRET SS)
  0x28 : USER_CS64  (Ring 3 64-bit Code - SYSRET CS)
  0x30 : TSS (descripteur système 16 octets sur 0x30 et 0x38)
  ```
- **Assertions Statiques de Sécurité (Lignes 46–58) :**
  - Vérification statique que `GDT_KERNEL_DS == GDT_KERNEL_CS + 8` (exigé par SYSCALL).
  - Vérification statique que `GDT_USER_DS == GDT_USER_CS32 + 8` et `GDT_USER_CS64 == GDT_USER_CS32 + 16` (exigé par SYSRET).
- **Chargement du TSS & Rechargement des Registres de Segments (Lignes 240–285) :**
  - GDTR chargé via `lgdt`.
  - Rechargement de CS via far-return `push {kcs}; lea tmp, [rip+1f]; push tmp; retfq`.
  - Rechargement de DS, ES, SS avec `GDT_KERNEL_DS`.
  - Réinitialisation de FS et GS à 0 (configurés ultérieurement via MSRs).
  - Chargement du Task Register via `ltr`.
- **Verdict :** **SAFE**. La structure est vérifiée statiquement et dynamiquement par `validate_star_layout()`.

---

### 2.3. `idt.rs` — Interrupt Descriptor Table (IDT)

- **Rôle :** Table de 256 descripteurs d'interruption (4 KiB) partagée entre cœurs.
- **Politique IST & Prévention des Corruptions (#25) :**
  - **IST1 (`IST_EXOPHOENIX_IPI`) :** Dédié aux signaux critiques de résurrection ExoPhoenix (0xF0, 0xF1, 0xF2).
  - **IST3 (`IST_NMI`) :** NMI matériel.
  - **IST4 (`IST_DOUBLE_FAULT`) :** Double Fault (#DF) — pile dédiée isolée pour éviter toute triple fault.
  - **IST5 (`IST_MACHINE_CHECK`) :** Machine Check (#MC).
  - **Protection Page Fault (#PF) (Lignes 386–410) :** `#PF` utilise explicitement la pile noyau du thread (`rsp0`), et **NON** une IST partagée. Une IST par-CPU écrasait les stack frames lors des fautes de page imbriquées ou en concurrence post-fork (cause racine du bug historique #25).
- **Intégrité Cryptographique IDT (Lignes 216–232 & 640–643) :**
  - `idt_sealed_fingerprint` calcule un hash FNV-1a des 4 KiB de l'IDT après son initialisation.
  - `idt_integrity_ok()` permet au handler NMI de vérifier sans allocation ni verrou qu'aucun descripteur IDT n'a été corrompu en mémoire.
- **Verdict :** **SAFE**. Respect strict des exigences de ré-entrance et de non-corruption de pile.

---

### 2.4. `tss.rs` — Task State Segment (TSS)

- **Rôle :** Définition du TSS x86_64 AMD64 ABI (104 octets) par CPU et gestion des piles IST.
- **Alignement & Piles IST (Lignes 143–212) :**
  - Pool early réservé pour piles avec guard page logique (`alloc_guarded_stack`) sans dépendance heap.
  - `validate_ist_alignment()` vérifie que chaque pile IST configurée est alignée sur 16 octets (`ist_addr % 16 == 0`), comme requis par l'ABI AMD64 pour les instructions SSE/AVX.
- **Mise à jour de RSP0 (Ligne 240) :**
  - `update_rsp0(cpu_id, rsp0)` met à jour le pointeur de pile de retour Ring 0 lors des changements de contexte.
- **Verdict :** **SAFE**.

---

### 2.5. `memory_iface.rs` — Pont Architecture ↔ Gestion Mémoire

- **Rôle :** Point de liaison entre `arch/` et `memory/` (Couche 0).
- **TLB Shootdown (Lignes 158–200) :**
  - `send_tlb_ipi_to_mask(base_cpu, cpu_mask)` envoie l'IPI fixe `IPI_TLB_SHOOTDOWN_VECTOR`.
  - Exclut le CPU courant pour respecter la règle TLB-01 : `flush_local()` PUIS IPI synchrone.
  - Enregistrement par pointeur de fonction `register_tlb_ipi_sender()` pour briser la dépendance cyclique entre Couche 0 et Arch.
- **Allocateurs de Défaut (`UserFaultAllocator` & `KernelFaultAllocator`) :**
  - Implémente `FaultAllocator` et `FrameAllocatorForWalk` pour la résolution des `#PF` noyau et utilisateur.
- **Verdict :** **SAFE**.

---

### 2.6. `paging.rs` — Hiérarchie de Pagination x86_64

- **Rôle :** Manipulation des tables à 4 niveaux (PML4, PDPT, PD, PT).
- **Protection des Pages :**
  - Prise en charge des drapeaux matériels : `PTE_PRESENT`, `PTE_WRITABLE`, `PTE_USER`, `PTE_NO_EXEC` (bit 63 NX/XD), `PTE_COW` (bit 9 logiciel), `PTE_SHM_PINNED` (bit 10).
  - Gestion des Huge Pages (2 MiB dans PD, 1 GiB dans PDPT).
  - `unmap_4k_page()` et `translate_virt()` détectent les conflits avec les huge pages (`HugePageConflict`).
- **Verdict :** **SAFE**.

---

### 2.7. `sched_iface.rs` — Pont Architecture ↔ Ordonnanceur

- **Rôle :** Exports C ABI (`arch_send_reschedule_ipi` et `arch_set_cpu_pstate`).
- **Analyse & Correction Appliquée :**
  - *Constat :* La documentation de `arch_set_cpu_pstate` spécifiait que l'écriture sur le MSR `IA32_PERF_CTL` (0x199) étant locale au cœur, l'appel devait être ignoré si `cpu != current_cpu`. Or, le code ne vérifiait pas cette égalité et écrivait systématiquement sur le cœur exécutant, modifiant la fréquence du mauvais processeur en cas d'appel distant.
  - *Correction appliquée :* Ajout de la validation `let local_id = percpu::current_cpu_id(); if cpu != local_id { return; }`.
- **Verdict :** **CORRIGÉ & SAFE**.

---

### 2.8. `syscall.rs` — Entrée & Sortie Système 64-bit

- **Rôle :** Entrée `SYSCALL` / sortie `SYSRETQ`.
- **Sécurité Critique & Atténuation Vulnérabilités :**
  - **Atténuation CVE-2012-0217 (Lignes 268–308) :** L'instruction `SYSRETQ` lève un `#GP` en anneau Ring 0 sur les processeurs Intel si RCX ou RSP contient une adresse non canonique. L'assembleur effectue le test canonique via `shl rax, 16; sar rax, 16; cmp rax, rcx`. En cas de non-canonicité, il bifurque vers un fallback `IRETQ` sécurisé qui force un défaut en Ring 3 sans compromettre le Ring 0.
  - **Mitigation Spectre v2 (Ligne 410) :** Appel de `spectre::apply_ibrs()` à l'entrée du handler Rust.
  - **KPTI (Kernel Page Table Isolation) :** Bascule automatique de CR3 entre la table noyau et la table utilisateur depuis `gs:[0x40]` et `gs:[0x48]`.
- **Verdict :** **SAFE**. Robustesse de niveau production face aux attaques spéculatives et d'élévation de privilèges.

---

### 2.9. `vga_early.rs`, `boot_display.rs`, `framebuffer_early.rs`, `terminal.rs`

- **Rôle :** Sorties de console et diagnostic graphique précoce.
- **Analyse & Correction dans `vga_early.rs` :**
  - *Constat :* Dans `vga_early.rs`, `put_cell` écrivait directement à `VGA_BASE + (row * VGA_COLS + col) * 2` sans vérification de bornes. Un appel avec `row >= VGA_ROWS` ou `col >= VGA_COLS` provoquait un dépassement mémoire hors du tampon vidéo 80×25.
  - *Correction appliquée :* Ajout du garde défensif `if col >= VGA_COLS || row >= VGA_ROWS { return; }`.
- **Verdict :** **CORRIGÉ & SAFE**.

---

## 3. Matrice de Sécurité et Corrections Appliquées

| Fichier | Risque Initial | Gravité | Action Réalisée | Statut Final |
| :--- | :--- | :--- | :--- | :--- |
| `sched_iface.rs` | Écriture MSR P-state sur mauvais CPU si `cpu != local_id` | Moyenne | Ajout du filtre `if cpu != local_id { return; }` | **CORRIGÉ** |
| `vga_early.rs` | Débordement mémoire potentiel hors du buffer VGA 80x25 | Faible | Ajout de vérification de bornes dans `put_cell` | **CORRIGÉ** |
| `syscall.rs` | Vulnérabilité historique CPU CVE-2012-0217 (non-canonical sysret) | Critique | Déjà mitigé par test de signe + fallback `iretq` | **SAFE** |
| `idt.rs` | Collision ré-entrante des piles `#PF` (Bug #25) | Élevée | Sécurisé via pile `rsp0` dédiée par thread | **SAFE** |
