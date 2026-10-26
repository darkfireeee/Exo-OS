# Analyse Approfondie — Module `kernel/src/arch/x86_64/cpu/`

## 1. Vue d'Ensemble du Module

Le sous-module `kernel/src/arch/x86_64/cpu/` encapsule l'intégralité des interactions matérielles de bas niveau avec le microprocesseur x86_64 :
- **CPUID & Détection matérielle** (`features.rs`) : identification du constructeur (Intel, AMD), détection dynamique des jeux d'instructions étendus (SSE, AVX, AVX-512), des fonctions de sécurité (SMEP, SMAP, UMIP, PKU/PKS, CET) et des vulnérabilités micro-architecturales (Meltdown, Spectre v1-v4, MDS).
- **MSR (Model-Specific Registers)** (`msr.rs`) : constantes d'adresses et opérations de lecture/écriture privilégiées (`RDMSR`/`WRMSR`), configuration du PAT (*Page Attribute Table*) et gestion des registres de base `FS`/`GS`.
- **FPU & Extensions Vectorielles** (`fpu.rs`) : manipulation des contextes FPU/SSE/AVX (`FXSAVE`/`FXRSTOR`, `XSAVE`/`XRSTOR`, `XSAVEOPT`, `XSAVEC`), configuration de `CR0`/`CR4`/`XCR0` et exports ABI C pour découpler la couche d'ordonnancement de l'architecture matérielle.
- **Topologie Système** (`topology.rs`) : découverte de la hiérarchie physique des processeurs (Package / Socket → Core → SMT/Hyper-Thread), intégration de la matrice de distance NUMA issue des tables ACPI SRAT/SLIT, et identification du CPU courant via le segment per-CPU `GS`.
- **Horloge Matérielle & TSC** (`tsc.rs`) : lecture sérialisée du Time Stamp Counter (`RDTSC`, `RDTSCP`, barrières `LFENCE`), calibration multi-sources (CPUID feuille 0x15, PIT canal 2, PM Timer ACPI, HPET) et conversion nanosecondes / cycles.

---

## 2. Analyse Fichier par Fichier : Sûreté, Cohérence et Vulnérabilités

### 2.1. `mod.rs`
- **Rôle :** Point d'entrée du sous-module `cpu`. Déclare et réexporte `features`, `fpu`, `msr`, `topology`, `tsc`.
- **Interconnexions :**
  - Isolateur strict de responsabilités : formalise la règle architecturale selon laquelle ce module ne doit contenir que des primitives d'instructions matérielles et de détection, tandis que la machine à états et les stratégies d'invalidation (ex. lazy FPU context switching) résident dans `kernel/src/scheduler/fpu/`.
- **Jugement de sûreté :** `SAFE`. Aucun code exécutable ou état mutable ; uniquement de l'organisation modulaire.

---

### 2.2. `msr.rs`
- **Rôle :** Accès brut aux registres MSR via les instructions privilégiées `RDMSR` et `WRMSR`.
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/syscall.rs` pour l'enregistrement des MSR `STAR`, `LSTAR`, `SFMASK`.
  - Connecté à `kernel/src/arch/x86_64/smp/percpu.rs` pour la manipulation de `GS_BASE` et `KERNEL_GS_BASE`.
  - Connecté à `kernel/src/arch/x86_64/spectre/` pour l'activation des barrières prédictives via `SPEC_CTRL`, `PRED_CMD` et `FLUSH_CMD`.
- **Audit de sûreté & vulnérabilités :**
  - `read_msr` et `write_msr` sont correctement qualifiés `pub unsafe fn` car l'émission d'un `RDMSR`/`WRMSR` vers un numéro MSR non implémenté sur le processeur hôte lève immédiatement une exception faute de protection générale (`#GP(0)`).
  - L'utilisation de `options(nostack, nomem)` dans les blocs ASM `read_msr` et `write_msr` est parfaitement sémantique car les registres MSR n'impactent pas directement la mémoire RAM adressable par le compilateur.
  - Dans `configure_pat()`, les entrées de la Page Attribute Table sont programmées de manière robuste : l'entrée PAT 7 est configurée à `0x01` (`Write-Combining`) pour accélérer les écritures linéaires dans le framebuffer vidéo, tout en maintenant la compatibilité sur les entrées 0 à 3 (`WB`, `WT`, `UC-`, `UC`).
- **Jugement :** `SAFE sous contrat d'appel`. Les invariants matériels sont respectés et documentés.

---

### 2.3. `fpu.rs`
- **Rôle :** Gestion matérielle de la FPU, des registres SSE et des extensions AVX.
- **Interconnexions :**
  - Connecté à `kernel/src/scheduler/fpu/save_restore.rs` via des fonctions externes C (`arch_xsave64`, `arch_xrstor64`, `arch_fxsave64`, `arch_fxrstor64`, `arch_has_xsave`, `arch_has_avx`). Ce découplage empêche les dépendances circulaires entre couches architecturales.
  - Connecté à `kernel/src/arch/x86_64/cpu/features.rs` pour la lecture des capacités `XSAVE` et `AVX`.
- **Audit de sûreté & vulnérabilités :**
  - `FpuRawArea` impose un alignement de 64 octets (`#[repr(C, align(64))]`) avec une taille statique dimensionnée à `XSAVE_AREA_MAX` (2696 octets), ce qui garantit qu'aucun dépassement de mémoire tampon n'intervient lors d'une sauvegarde AVX-512 ou PKRU.
  - Dans `init_fpu_for_cpu()`, la séquence d'initialisation matérielle suit scrupuleusement le manuel Intel SDM Vol. 3A :
    1. `CR0.EM = 0` (pas d'émulation logicielle) et `CR0.MP = 1` + `CR0.NE = 1` (gestion native des exceptions x87).
    2. Activation de `CR4.OSFXSR` (bit 9) et `CR4.OSXMMEXCPT` (bit 10) avant toute exécution d'instruction SSE.
    3. Activation de `CR4.OSXSAVE` (bit 18) conditionnée à `features.has_xsave()`.
    4. Écriture sécurisée de `XCR0` avec un masque validé dynamiquement (ne tente jamais d'activer `AVX` ou `AVX512` si le CPU ne l'indique pas dans CPUID, ce qui prévient un `#GP`).
    5. Exécution de `FINIT` et configuration du registre `MXCSR` à la valeur standard `0x1F80` (masquage de toutes les exceptions SSE non fatales afin d'éviter les interruptions intempestives lors du calcul en virgule flottante).
  - *Observation Clippy :* Les fonctions `arch_has_xsave` et `arch_has_avx` utilisent `map_or(false, ...)` qui sera simplifié en `is_some_and(...)`.
- **Jugement :** `EXCELLENT / SAFE`. Précision chirurgicale dans la gestion des états FPU.

---

### 2.4. `features.rs`
- **Rôle :** Découverte et publication singleton des drapeaux de fonctionnalités CPU.
- **Interconnexions :**
  - Utilisé par quasiment tous les sous-systèmes du noyau : mémoire (NX, 1GB pages, PCID), ordonnanceur (FPU/AVX), sécurité/spectre (IBRS, IBPB, MD_CLEAR, RDCL_NO), APIC/x2APIC.
- **Audit de sûreté & vulnérabilités :**
  - **Gestion de `EBX` sous LLVM PIC :** L'instruction `CPUID` modifie `EBX`. Or, dans les exécutables avec relocalisation ou compilation PIC, LLVM réserve le registre `RBX`. L'utilisation du pattern ASM `xchg {tmp:r}, rbx` avant et après `cpuid` préserve l'intégrité de l'environnement de compilation sans risque de corruption de pile ou de registre.
  - **Singleton `CPU_FEATURES` :** Utilise `AtomicBool` avec `compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)` pour garantir que seul le BSP (Bootstrap Processor) initialise la structure avant le démarrage des processeurs applicatifs (APs). L'accès via `get()` valide en `debug_assert!` que l'initialisation a bien été effectuée, et retourne une référence partagée immuable (`&'static CpuFeatures`).
  - *Observation Clippy :* Dans `CpuFeatures::detect()`, `(ebx >> 0) as u8` contient des décalages redondants par zéro qui doivent être nettoyés.
- **Jugement :** `SAFE`. Thread-safe et résistant aux accès concurrents après boot.

---

### 2.5. `topology.rs`
- **Rôle :** Cartographie topologique du système multiprocesseur (packages, cœurs, SMT, NUMA).
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/smp/` pour l'enregistrement dynamique des coeurs AP au boot via `register_ap()`.
  - Connecté à `kernel/src/arch/x86_64/acpi/` pour la mise à jour des identifiants et distances NUMA via les tables SRAT/SLIT.
  - Fournit `current_cpu_id()` utilisé par les mécanismes de verrouillage et de structures de données per-CPU.
- **Audit de sûreté & vulnérabilités :**
  - `detect_topo_shifts()` interroge en priorité la feuille CPUID `0x1F` (topologie V2 étendue multi-die/multi-tile) avec fallback sur la feuille classique `0x0B`. Les décalages de bits (`smt_shift`, `core_shift`, `package_shift`) permettent de partitionner fidèlement l'APIC ID.
  - `register_ap()` utilise une incrémentation atomique `CPU_COUNT_TOTAL.fetch_add(1, Ordering::AcqRel)` pour attribuer un index unique dans la table `CPU_DESCRIPTORS`. Si le nombre de cœurs dépasse `MAX_CPUS` (256), le processeur en surplus est immédiatement stoppé via `crate::arch::x86_64::halt_cpu()` afin de prévenir tout débordement de tampon mémoire.
  - Dans `current_cpu_id()`, le code délègue explicitement à la variable de segment `GS` per-CPU plutôt que d'émettre une instruction `RDTSCP`, car sous certains hyperviseurs ou environnements d'émulation pure (ex. QEMU TCG), `RDTSCP` peut lever une exception `#UD` ou retourner un identifiant auxiliaire non synchronisé.
- **Jugement :** `SAFE`. Conception robuste face à la sur-allocation de processeurs.

---

### 2.6. `tsc.rs`
- **Rôle :** Horloge haute précision via le Time Stamp Counter x86.
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/time/` pour alimenter le temps noyau monotone `ktime`.
  - Connecté aux pilotes de temporisation : HPET, PIT 8254 et PM Timer ACPI.
- **Audit de sûreté & vulnérabilités :**
  - **Sérialisation stricte :** `read_tsc_begin()` force une barrière `LFENCE` avant `RDTSC` pour empêcher la spéculation ou la réorganisation des instructions mémoire antérieures. Symétriquement, `read_tsc_end()` exécute `RDTSCP` suivi d'un `LFENCE`, garantissant un encadrement temporel parfait des sections critiques sans dérive spéculative.
  - **Résilience aux blocages d'émulateurs (Anti-Hang) :** Lors de la calibration via PIT, HPET ou PM Timer, l'attente active n'utilise pas uniquement des compteurs de cycles TSC (qui avancent 50 à 100 fois trop lentement sous QEMU en mode TCG sans KVM), mais s'appuie sur des bornes d'itérations adaptées (`MAX_SYNC_ITERS`, `MAX_MEASURE_ITERS`), évitant tout blocage infini au démarrage.
  - **Précision arithmétique :** Les conversions de cycles en nanosecondes utilisent l'arithmétique entière 128 bits (`u128`) pour prévenir les débordements de capacité lors de calculs sur des durées étendues.
- **Jugement :** `SAFE`. Prévient les pièges classiques de calibration sous virtualisation.

---

## 3. Synthèse des Correctifs & Améliorations Apportés

1. **Remplacement des constructions `map_or(false, ...)`** par `.is_some_and(...)` dans `fpu.rs` (`arch_has_xsave`, `arch_has_avx`), `topology.rs` et `tsc.rs`.
2. **Nettoyage des décalages de bits nuls `>> 0`** dans `features.rs` (`(ebx >> 0) as u8` → `ebx as u8`).
3. **Élimination de la variable intermédiaire redondante** avant return dans `tsc.rs::calibrate_tsc_with_pit()`.
