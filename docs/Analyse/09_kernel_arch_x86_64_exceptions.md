# Analyse Approfondie — Module `kernel/src/arch/x86_64/exceptions/`

## 1. Vue d'Ensemble du Module

Le sous-module `kernel/src/arch/x86_64/exceptions/` constitue le système de réception, de confinement et de traitement de toutes les interruptions et fautes processeur sur architecture x86_64 :
- **Structure de Registres & Ponts** (`frame.rs`) : définit `ExceptionFrame` (correspondance exacte avec la trame de pile poussée par le matériel et les stubs ASM), implémente la détection d'origine de privilège (`from_userspace()` vs `from_kernel()`), orchestre le retour vers l'espace utilisateur avec livraison des signaux POSIX (`exception_return_to_user`), et établit les ponts ABI C avec l'ordonnanceur (`scheduler_tick`, `sched_fpu_handle_nm`, `sched_ipi_reschedule`).
- **Stubs d'Entrée Bas Niveau** (`stubs.rs`) : stubs écrits en assembleur pur (`global_asm!`) pour les 256 vecteurs IDT, gérant de façon totalement transparente le basculement `swapgs` conditionnel à `CS.RPL`, la commutation de tables de pages KPTI (`mov cr3, gs:[0x40]`), la sauvegarde/restauration des 15 registres généraux, l'alignement de la pile SysV sur 16 octets (`and rsp, -16`), et l'instruction finale `iretq`.
- **Fautes Matérielles CPU** (`cpu_faults.rs`) : 20 gestionnaires dédiés aux exceptions architecturales x86_64 (#DE, #DB, #NMI, #BP, #OF, #BR, #UD, #NM, #DF, #TS, #NP, #SS, #GP, #MF, #AC, #MC, #XM, #VE, #CP).
- **Gestionnaire de Faute de Page (#PF)** (`page_fault.rs`) : point d'entrée critique de la mémoire virtuelle, décomposition des bits d'erreur de `CR2`/`error_code`, résolution du Copy-on-Write (CoW), pagination à la demande (*demand paging*), et synchronisation d'ombre KPTI.
- **Interruptions & IPIs** (`irq.rs`) : acquittement et routage des interruptions matérielles externes (APIC Timer, IRQ 0..15) et des interruptions inter-cœurs SMP (Reschedule, Wakeup, TLB Shootdown, Hotplug, Panic, ExoPhoenix).

---

## 2. Analyse Fichier par Fichier : Sûreté, Cohérence et Vulnérabilités

### 2.1. `mod.rs`
- **Rôle :** Point d'entrée et exportation ségréguée des sous-modules d'exception.
- **Jugement de sûreté :** `SAFE`.

---

### 2.2. `frame.rs`
- **Rôle :** Représentation mémoire de la trame d'interruption et fonctions d'orchestration.
- **Interconnexions :**
  - Connecté à `kernel/src/scheduler/core/switch.rs` via `schedule_current_if_needed()`.
  - Connecté à `kernel/src/process/signal/delivery.rs` pour l'injection des signaux lors du franchissement de la frontière Ring 0 → Ring 3.
- **Audit de sûreté & vulnérabilités :**
  - **Représentation mémoire `#[repr(C)]` :** La disposition des 15 registres 64 bits (`r15`..`rax`), du code d'erreur (`error_code`), et des 5 registres matériels (`rip`, `cs`, `rflags`, `rsp`, `ss`) est strictement congruente à l'ordre des instructions `push` du stub ASM. Tout décalage de taille ou d'ordre provoquerait la corruption des registres restaurés par `pop`.
  - **Retour après faute récupérée (`exception_return_to_user_after_recovered_fault`) :** Après une réparation réussie de PTE (ex. duplication CoW sur une page de pile), cette variante désactive la préemption immédiate (`allow_resched = false`). Cela garantit que le thread fautif rejoue immédiatement son instruction (ex. un `call` interrompu au milieu de l'écriture de son adresse de retour) sans qu'un autre thread puisse s'intercaler et observer une pile partiellement mise à jour.
  - **Diagnostics sécurisés (`diag_user_exception_dump`) :** N'utilise que des lectures volatiles gardées vérifiant préalablement que l'adresse réside dans l'espace utilisateur canonique. Toute lecture non protégée dans un dump d'exception provoquerait un double-fault en cascade, détruisant l'information d'audit.
- **Jugement :** `SAFE & ROBUSTE`.

---

### 2.3. `stubs.rs`
- **Rôle :** Stubs d'assemblage configurés dans la table IDT.
- **Interconnexions :**
  - Interagit directement avec le registre `GS` per-CPU et les registres de contrôle `CR3`.
- **Audit de sûreté & vulnérabilités :**
  - **Gestion étanche de `SWAPGS` :** Le stub inspecte systématiquement le sélecteur `CS` empilé (`test qword ptr [rsp + 16], 3`). Si le niveau de privilège précédent était Ring 0 (interruption au sein du noyau), l'instruction `swapgs` est sautée. Si l'interruption provient de Ring 3, `swapgs` est exécuté pour charger le `GS` kernel, suivi de la commutation vers le `CR3` noyau complet (`gs:[0x40]`). Symétriquement, avant `iretq`, si `CS` indique un retour vers Ring 3, le `CR3` utilisateur d'ombre (`gs:[0x48]`) est rechargé et `swapgs` restaure le contexte utilisateur. Cette rigueur élimine tout risque de désynchronisation de `GS`.
  - **Alignement SysV ABI :** L'instruction `and rsp, -16` garantit que la fonction Rust appelée s'exécute avec une pile alignée sur 16 octets, tout en préservant le pointeur brut vers `ExceptionFrame` dans `R12` pour la restauration fidèle.
- **Jugement :** `CRITIQUE & SÉCURISÉ`.

---

### 2.4. `cpu_faults.rs`
- **Rôle :** Traitement des fautes matérielles.
- **Audit de sûreté & vulnérabilités :**
  - Toutes les fonctions sont qualifiées `pub unsafe extern "C" fn`, ce qui résout le problème de contrat C ABI et l'avertissement clippy `not_unsafe_ptr_arg_deref`.
  - **Confinement Ring 3 vs Panique Ring 0 :** Si une faute (#DE, #UD, #GP, #SS, #OF) se produit depuis l'espace utilisateur, le noyau ne panique jamais : il injecte le signal POSIX adéquat (`SIGFPE`, `SIGILL`, `SIGSEGV`, `SIGBUS`) et réoriente le flux vers le gestionnaire de signal du processus ou termine le thread fautif. Si la faute se produit en Ring 0 (code noyau défaillant), le système bascule immédiatement en `kernel_panic_exception` avec dump complet des registres.
  - **Gestion NMI sur IST2 :** L'interruption Non-Masquable (#NMI) s'exécute sur une pile dédiée `IST2` définie dans la TSS. Cela permet de diagnostiquer et traiter des corruptions matérielles même si la pile noyau principale `RSP0` est totalement corrompue.
- **Jugement :** `SAFE`.

---

### 2.5. `page_fault.rs`
- **Rôle :** Résolution des fautes de pagination.
- **Audit de sûreté & vulnérabilités :**
  - `do_page_fault` est correctement qualifié `pub unsafe extern "C" fn`.
  - Lit l'adresse virtuelle fautive directement depuis `CR2` et inspecte les bits d'erreur de `frame.error_code` (présence, écriture, mode utilisateur, bit réservé corrompu, exécution d'instruction NX, protection key).
  - Synchronise les tables d'ombre KPTI via `sync_kpti_user_fault_mapping()` dès qu'une nouvelle page utilisateur est allouée ou promue, interdisant toute divergence entre la vue noyau et la vue utilisateur de la table de pages.
- **Jugement :** `SAFE & EXCELLENT`.

---

### 2.6. `irq.rs`
- **Rôle :** Traitement des interruptions d'horloge, des lignes périphériques et des IPIs.
- **Audit de sûreté & vulnérabilités :**
  - Toutes les entrées IPI (`do_ipi_wakeup`, `do_ipi_reschedule`, `do_ipi_tlb_shootdown`, `do_ipi_cpu_hotplug`, `do_ipi_panic`, `do_exophoenix_*`) et handlers d'interruptions génériques (`do_irq_generic`, `define_generic_irq_handler!`) sont marqués `pub unsafe extern "C" fn` et documentés avec `# Safety`.
  - Dans `do_irq_timer()`, l'acquittement EOI de l'APIC est émis immédiatement au début du handler, minimisant la gigue d'interruption et prévenant le blocage du contrôleur APIC pendant le traitement du tick d'ordonnancement.
  - Pour les interruptions parasites (`do_irq_spurious`, vecteur `0xFF`), aucun EOI n'est envoyé, conformément au standard d'architecture Intel APIC (SDM Vol. 3A §10.9).
- **Jugement :** `SAFE`.

---

## 3. Synthèse des Correctifs & Améliorations Apportés

1. **Qualification `pub unsafe extern "C" fn"`** sur `do_page_fault`, `do_irq_generic`, et l'intégralité des gestionnaires d'IPI dans `irq.rs` et `cpu_faults.rs`.
2. **Ajout de sections explicites `# Safety`** sur toutes les fonctions d'interruption exposées au pont C ABI.
3. **Suppression des sauts de ligne intempestifs** après `#[inline]` dans `frame.rs`.
