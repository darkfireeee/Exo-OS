# ExoOS v0.2.0 — Rapport d'Audit de Stabilisation Kernel
**Auditeur :** claude-gamma  
**Codebase :** `kernel.zip` — branche post-v0.1.0  
**Périmètre :** kernel/, servers/, drivers/, exo-boot/, userspace/  
**Fichiers analysés :** 1 034 fichiers `.rs` · 45 `Cargo.toml` · ~12 MB de code  
**Date :** 2026-05-15  

---

> **Contexte.** La v0.1.0 est validée. La v0.2.0 est le jalon de *stabilisation complète* avant l'introduction de Wayland, de l'installeur et de la couche visuelle. Ce rapport documente toutes les incohérences identifiées — classées par sévérité — qui doivent être résolues avant de déclarer le kernel stable.

---

## Récapitulatif exécutif

| Sévérité | Nombre | Domaines principaux |
|----------|--------|---------------------|
| **P0 — Bloquant** | 4 | KASLR, VirtIO disk, TTY pipeline, ELF loader |
| **P1 — Élevé** | 5 | RFLAGS fork, ExoPhoenix tests, fd table, CI ExoFS, scheduler |
| **P2 — Modéré** | 6 | Drivers vides, validate_range, rustc-ICE, build doc, AArch64, stack const |

---

## P0 — Bloquants : ne pas avancer vers v0.2.0 avant résolution

---

### P0-01 · KASLR cosmétique — offset calculé, jamais appliqué

**Fichiers :** `kernel/src/security/exploit_mitigations/kaslr.rs` · `kernel/src/lib.rs` · `kernel/src/memory/core/layout.rs`

**Constat.** `security_init()` appelle `kaslr_init(raw_entropy, phys_base)` qui stocke l'offset dans un `AtomicU64`. La valeur est ensuite exposée via `kaslr_offset()`. Cependant, une recherche exhaustive dans tout `kernel/src/` révèle que `kaslr_offset()` n'est référencé que dans deux fichiers de déclaration (`exploit_mitigations/mod.rs`, `security/mod.rs`) — jamais dans `phys_to_virt()`, jamais dans `virt_to_phys()`, jamais dans le linker script, jamais dans le trampoline de boot.

Le kernel est donc invariablement chargé à `KERNEL_LOAD_PHYS_ADDR = 0x0010_0000` (1 MiB fixe). KASLR est documenté, calculé, et non-fonctionnel.

**Impact.** Toutes les adresses kernel sont prédictibles. Le modèle de sécurité affiché dans la documentation est trompeur. Les fonctionnalités qui s'appuient implicitement sur la randomisation (ExoArgos, ExoKairos, ExoSeal) opèrent sur une base d'adresses fixes.

**Correction attendue.**
- Soit appliquer l'offset KASLR dans la conversion `phys_to_virt` / `virt_to_phys` et dans le trampoline de boot via `exo-boot`.
- Soit documenter explicitement « KASLR non implémenté en v0.2.0 » et supprimer l'initialisation du module pour éviter la confusion.
- Ajouter un test qui vérifie que `kaslr_offset() != 0` après `kaslr_init()` avec une entropie non nulle.

---

### P0-02 · Adresse MMIO VirtIO blk hardcodée à `0x1000_0000`

**Fichier :** `kernel/src/fs/exofs/storage/virtio_adapter.rs` lignes 53-61

```rust
*disk = Some(Arc::new(VirtioBlockAdapter::new(
    0x1000_0000,          // ← constante magique QEMU-spécifique
    1024 * 1024 * 512,    // ← 512 MiB fixe
)));
```

**Constat.** L'adresse `0x10000000` correspond au premier périphérique VirtIO-MMIO dans la configuration par défaut de QEMU (`-M virt` ou `-M q35` avec `-device virtio-blk-device`). Cette adresse n'est ni lue depuis l'arbre de périphériques ACPI, ni depuis les BARs PCI, ni depuis la table FDT. Si le disk est un périphérique PCI VirtIO (cas réel ou QEMU avec `-device virtio-blk-pci`), l'adresse est entièrement différente.

Le crate `exo-virtio-blk` dispose d'un `ExoVirtioBlkDevice::new(base_address, capacity)` qui attend une adresse fournie dynamiquement — l'infrastructure est correcte — mais l'appelant fournit une constante.

**Impact.** ExoFS ne peut accéder au disque que dans une configuration QEMU très précise. Sur tout autre matériel ou configuration, `exofs_init()` renvoie succès (la structure est créée) mais chaque I/O lit/écrit dans le vide ou provoque un #PF sur MMIO non mappé.

**Correction attendue.**
- Lire l'adresse MMIO depuis la table ACPI (déjà disponible dans `kernel/src/arch/x86_64/acpi/`) ou depuis l'énumération PCI BAR lors de `init_global_disk()`.
- À défaut, passer l'adresse via le `HandoffData` de `exo-boot` au kernel.
- Supprimer les deux constantes magiques (`0x1000_0000` et `1024*1024*512`).

---

### P0-03 · TTY server architecturalement déconnecté du chemin stdio

**Fichiers :** `kernel/src/syscall/fs_bridge.rs` · `kernel/src/arch/x86_64/terminal.rs` · `servers/tty_server/src/main.rs`

**Constat.** Deux chemins coexistent sans qu'ils soient reliés :

**Chemin A — réel (utilisé par exosh) :**
```
read(fd=0) → sys_read → fs_bridge::fs_read(fd=0)
           → terminal::read_byte_for_process()   ← poll direct PS/2 kernel
           → retourne 1 octet à exosh
```

**Chemin B — architectural (non câblé) :**
```
IRQ PS/2 → input_server → tty_server::handle_input(byte)
         → LineDiscipline::input_byte()
         → tty_server::handle_read_line()
         → exosh via IPC (TTY_MSG_READ_LINE)
```

`tty_server` est un binaire qui démarre, s'enregistre sur l'endpoint IPC `12`, traite ses messages, mais n'est jamais consulté lors d'un `read(fd=0)` standard. En particulier, personne n'envoie `TTY_MSG_INPUT_BYTE` au `tty_server` depuis le gestionnaire d'interruption clavier ou `input_server`. `LineDiscipline` (Ctrl+C, Ctrl+D, historique, canonique/raw) ne s'applique donc jamais au terminal interactif.

Par ailleurs, l'endpoint `12` est une constante arbitraire non partagée avec le reste du système (aucun autre binaire n'a connaissance de cette valeur).

**Impact.** Le shell fonctionne (via le chemin A), mais sans discipline de ligne kernel correcte. Ctrl+C ne génère pas `SIGINT` via le mécanisme `tty_server`. Le `tty_server` consomme de la mémoire et du temps CPU sans effet observable.

**Correction attendue.** Pour v0.2.0 (Wayland pré-requis), il est critique de choisir une architecture et de l'implémenter entièrement :
- **Option recommandée :** câbler `fs_bridge::fs_read(fd=0)` vers le `tty_server` via un appel IPC synchrone au lieu du poll PS/2. `input_server` envoie `TTY_MSG_INPUT_BYTE` au `tty_server` à chaque IRQ clavier.
- **Option alternative :** déprécier `tty_server` pour v0.2.0 et documenter que la discipline de ligne est dans `terminal.rs` jusqu'à l'introduction de Wayland.

---

### P0-04 · ELF loader : modèle hybride eager+demand incohérent

**Fichier :** `kernel/src/fs/elf_loader_impl.rs` lignes 265-285

**Constat.** Pour chaque segment `PT_LOAD`, le loader effectue séquentiellement :

1. `map_elf_segment()` — alloue des frames physiques ZEROED, copie les données ELF, et mappe les pages dans le `PageTableBuilder` (chargement **eager** avec frames physiques réels).
2. `install_elf_vma()` — installe une `VmaDescriptor` avec `VmaBacking::File` sur les mêmes plages d'adresses virtuelles.

Ces deux actions sont incompatibles : les pages sont déjà présentes dans les tables de pages avec `FLAG_PRESENT` (via le builder). Si un page fault survient ultérieurement sur ces pages (ex. après un fork CoW qui retire l'écriture), le handler `demand_paging::handle_demand_paging()` sera invoqué sur une VMA `File`-backed et tentera une réallocation. Si l'ELF est exécuté dans un processus forké, les pages CoW marquées en lecture seule déclenchent `handle_cow_fault()` correctement — mais les pages `.text` identifiées `VmaBacking::File` pourraient déclencher `handle_demand_paging()` au lieu de `handle_cow_fault()` selon le chemin emprunté dans `fault/handler.rs`.

Le modèle correct est soit :
- **Demand paging pur** : `install_elf_vma()` seul, aucun `map_elf_segment()` — le premier accès déclenche un fault.
- **Eager pur** : `map_elf_segment()` seul, `VmaBacking::Direct` ou `Anonymous` dans la VMA.

Le modèle actuel fait les deux, créant un état incohérent entre les tables de pages et le VMA tree.

**Impact.** Potentiel double-map silencieux après fork, corruption possible de l'espace d'adressage de processus enfants. Les pages `.text` des binaires Ring1 pourraient être ré-zérées par le demand paging handler lors d'un fault sur une page déjà chargée.

**Correction attendue.** Unifier sur le demand paging pur pour les segments file-backed : supprimer `map_elf_segment()` et ne conserver que `install_elf_vma()` avec `VmaBacking::File`. Les données seront chargées au premier accès via `FileFaultProvider` (déjà implémenté et enregistré). La pile reste en eager via `map_stack_pages()` + `VmaBacking::Anonymous` (chemin déjà correct).

---

## P1 — Élevés : à corriger avant gel du kernel

---

### P1-01 · RFLAGS fork : bits IOPL (12-13) hérités sans masquage

**Fichier :** `kernel/src/process/lifecycle/fork.rs` lignes ~295-310

```rust
const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0024_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_4100; // TF=0, NT=0, RF=0, VM=0
```

**Constat.** Les bits 12 et 13 de RFLAGS codent l'**IOPL** (I/O Privilege Level). Ils ne sont ni couverts par `RFLAGS_SAFE_MASK`, ni effacés par `RFLAGS_FORCE_CLR`. Un processus parent avec IOPL≠0 transmet donc ses privilèges d'accès aux ports I/O à son fils, contournant le modèle capability d'ExoOS. Sur x86-64 userspace sans `iopl()` syscall, IOPL est normalement 0 — mais un bug ou un test ad-hoc pourrait établir IOPL=3 dans un processus Ring3, et ce privilège se propagerait via fork à toute la descendance.

**Correction attendue.**
```rust
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_7100; // + bits 12-13 (IOPL=0)
```

---

### P1-02 · ExoPhoenix : 0 test pour ~120 KB de code critique

**Répertoire :** `kernel/src/exophoenix/`

**Constat.** Les 9 modules du sous-système dual-kernel totalisent environ 120 000 octets de code Rust (`stage0.rs` : 41 KB, `forge.rs` : 25 KB, `handoff.rs` : 18 KB, `sentinel.rs` : 11 KB). Un audit `grep -rn "#[test]" kernel/src/exophoenix/` retourne **zéro résultat**.

ExoPhoenix est le composant de résilience le plus complexe d'ExoOS : transitions d'état (`BootStage0 → Normal → Threat → IsolationSoft → IsolationHard → Restore`), handoff entre kernel A et kernel B, vérifications d'intégrité. Son absence totale de couverture de test est incompatible avec un jalon de stabilisation.

**Correction attendue.** Priorité minimale pour v0.2.0 :
- Tests unitaires des transitions d'état dans `mod.rs` et `sentinel.rs`.
- Test de `stage0::*` avec un mock `HandoffData`.
- Test de `forge::seed_kernel_a_image_blob()` sur un environnement host.

---

### P1-03 · PCB fd table : champ `handle` ignoré par le chemin I/O

**Fichiers :** `kernel/src/process/core/pcb.rs` · `kernel/src/process/lifecycle/create.rs` · `kernel/src/syscall/fs_bridge.rs`

**Constat.** À la création du processus init, la table de fichiers est initialisée :

```rust
const BOOT_TTY_HANDLE: u64 = 1;
pcb.files.lock().install_std_fds(BOOT_TTY_HANDLE, BOOT_TTY_HANDLE, BOOT_TTY_HANDLE);
```

Mais `fs_bridge::fs_read()` et `fs_bridge::fs_write()` reçoivent le **fd brut** (u32) depuis le syscall handler et effectuent une comparaison directe `if fd == 0` / `if fd == 1 || fd == 2` sans jamais consulter `pcb.files.get(fd)` pour résoudre le `handle`. Le champ `FileDescriptor::handle` est donc rempli à la création et jamais lu dans le chemin I/O réel.

Cette architecture signifie que `dup2()`, `open()`, et les remplacements de fd standard ne peuvent pas fonctionner correctement : `fd=0` sera toujours redirigé vers le terminal PS/2, même si le processus a `dup2(some_file, 0)`.

**Correction attendue.** `fs_read(fd, ...)` doit d'abord résoudre `handle = pcb_current().files.get(fd).handle`, puis dispatcher sur le handle (terminal, objet ExoFS, pipe, socket) — et non sur le numéro de fd brut. Cela nécessite une refonte du dispatcher dans `fs_bridge.rs`.

---

### P1-04 · ExoFS tier_4/tier_6 : couverture de code présente, exécution CI absente

**Fichier :** `kernel/src/fs/exofs/tests/TESTS_STATUS_REPORT.md`

**Constat.** Le rapport lui-même, commité dans le dépôt, contient :

> *« Ce fichier ne remplace pas une execution CI. La validation v0.2.0 doit rester bloquée sur l'execution effective [...] Les tests de persistance doivent verifier remount ou reconstruction de l'etat. »*

Les tiers 4 (backend VFS réel) et 6 (VirtIO + VFS réel) n'ont aucun log d'exécution attaché. ExoFS est le système de fichiers racine d'ExoOS — sa validation ne peut pas reposer uniquement sur la présence du code de test.

**Correction attendue.** Exécuter sous WSL/QEMU et attacher les logs :
```
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
  fs::exofs::tests::integration::tier_4_pipeline
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
  fs::exofs::tests::integration::tier_6_virtio_vfs
```
Bloquer le merge v0.2.0 jusqu'à obtention de logs verts.

---

### P1-05 · Scheduler : couverture de test insuffisante pour un composant de stabilité

**Répertoire :** `kernel/src/scheduler/`

**Constat.** Le scheduler représente ~92 KB de code Rust (`core/task.rs` : 33 KB, `core/runqueue.rs` : 30 KB, `core/switch.rs` : 29 KB) couvert par **8 tests**. Les chemins suivants sont sans aucun test :

- `core/pick_next.rs` — algorithme de sélection du prochain thread
- `smp/load_balance.rs` — rééquilibrage inter-CPU
- `smp/migration.rs` — migration de threads
- `policies/deadline.rs` — politique EDF
- `fpu/lazy.rs` / `fpu/save_restore.rs` — sauvegarde/restauration FPU lazy

Un scheduler non-testé est incompatible avec le gel de stabilisation précédant Wayland (contexte graphique, multithreading intensif).

**Correction attendue.** Couvrir au minimum `pick_next`, `enqueue/dequeue` sous charge, et la sélection CPU dans `load_balance`.

---

## P2 — Modérés : à traiter avant la release officielle de v0.2.0

---

### P2-01 · 54 fichiers `.rs` entièrement vides dans les drivers

**Répertoire :** `drivers/`

**Constat.** Un scan `find . -name "*.rs" -size 0` révèle **54 fichiers vides** répartis comme suit :

| Sous-dossier | Vides / Total |
|---|---|
| `drivers/audio/hda/` | 4/4 |
| `drivers/audio/virtio_sound/` | 1/1 |
| `drivers/clock/` | 5/5 |
| `drivers/display/virtio_gpu/` | 3/3 |
| `drivers/framework/` | **12/12** — entièrement vide |
| `drivers/input/evdev/` | 2/2 |
| `drivers/input/usb_hid/` | 4/4 |
| `drivers/manager/` | 6/6 |
| `drivers/network/e1000/` | 5/5 |
| `drivers/storage/ahci/` | 6/6 |
| `drivers/storage/nvme/` | 6/6 |

`drivers/framework/` est particulièrement critique : ce crate est censé abstraire les bus PCI, USB, platform, DMA, IRQ, capabilities. Il est entièrement vide, ce qui signifie que chaque driver qui en dépend importe une crate sans implémentation.

Ces fichiers **compilent sans erreur** (un fichier vide est un module Rust valide) mais ne fournissent aucune fonctionnalité. Ils masquent les manques réels lors d'un audit rapide.

**Correction attendue.** Pour v0.2.0 : soit implémenter les modules critiques (`framework/`, `clock/`, le driver d'entrée PS/2 ou `virtio_gpu/`), soit supprimer les stubs vides et documenter les modules absents comme « not-yet-implemented ». À ne pas laisser dans cet état intermédiaire.

---

### P2-02 · `validate_user_segment_range` : borne kernel couplée à l'absence de KASLR

**Fichier :** `kernel/src/fs/elf_loader_impl.rs` — `fn validate_user_segment_range()`

```rust
let kernel_start = KERNEL_LOAD_PHYS_ADDR; // = 0x100000
let kernel_end   = kernel_low_identity_end();
if kernel_end > kernel_start && start < kernel_end && end > kernel_start {
    return Err(ElfLoadError::InvalidElf);
}
```

**Constat.** La protection anti-overlap entre segments ELF utilisateur et le kernel utilise l'adresse physique `KERNEL_LOAD_PHYS_ADDR = 0x100000` comme borne basse. Cette valeur est correcte uniquement parce que KASLR n'est pas fonctionnel (P0-01). Si KASLR était appliqué, le kernel serait relocalisé à une adresse virtuelle différente, mais cette garde utiliserait toujours `0x100000`.

**Correction attendue.** Utiliser la borne virtuelle du kernel (`KERNEL_START` ou un symbole fourni par le linker script) plutôt que l'adresse physique fixe. Cette correction est intimement liée à la résolution de P0-01.

---

### P2-03 · Fichier `rustc-ice` commité dans le dépôt

**Fichier :** `servers/phase5-tests/rustc-ice-2026-04-18T23_43_24-667.txt`

**Constat.** Un Internal Compiler Error du compilateur Rust est commité dans `servers/phase5-tests/`. Ce fichier indique qu'une compilation du crate `phase5-tests` a provoqué un crash de `rustc` (problème de décodage dans l'émetteur d'erreurs `annotate_snippets`). L'ICE a été déclenché par le lint `dead_code` — ce qui suggère du code mort non nettoyé dans le crate.

**Impact.** Signale une instabilité non résolue dans un crate de tests. Le fichier ICE ne devrait pas figurer dans le dépôt.

**Correction attendue.**
1. Ajouter `servers/phase5-tests/rustc-ice-*.txt` dans `.gitignore`.
2. Identifier et nettoyer le code mort dans `phase5-tests` qui déclenche l'ICE.
3. Vérifier que `cargo test -p phase5-tests` passe sans ICE sur le toolchain cible.

---

### P2-04 · Procédure de build complète non documentée

**Constat.** Pour construire une ISO ExoOS bootable avec les binaires embarqués, il faut :
1. Compiler tous les serveurs Ring1 avec le target `x86_64-exo-os.json`.
2. Placer les binaires dans `EXO_BOOT_PAYLOAD_DIR`.
3. Compiler le kernel avec `RUSTFLAGS="--cfg exo_boot_payloads"` et la variable d'environnement `EXO_BOOT_PAYLOAD_DIR` définie.
4. Compiler `exo-boot`.
5. Produire l'image ISO.

Il n'existe **aucun Makefile, justfile, `build.sh` ou README racine** dans le dépôt. `EXO_BOOT_PAYLOAD_DIR` est uniquement mentionné dans `build.rs` via `cargo:rerun-if-env-changed`. La cfg flag `exo_boot_payloads` n'est pas dans `[features]` de `Cargo.toml` (elle est une rustc-cfg, pas une Cargo feature).

**Impact.** Un nouveau contributeur (ou une instance CI) ne peut pas produire une ISO sans documentation externe. Pour un jalon de stabilisation destiné à précéder Wayland, cela bloque toute vérification end-to-end indépendante.

**Correction attendue.** Créer un `Makefile` ou `justfile` racine documentant la chaîne complète : compilation des servers → payload dir → kernel → boot → ISO. Documenter `EXO_BOOT_PAYLOAD_DIR` et `--cfg exo_boot_payloads` dans un `BUILDING.md`.

---

### P2-05 · AArch64 stub : fonctions compilées sans garde de cible

**Fichier :** `kernel/src/arch/aarch64/mod.rs`

**Constat.** Le `compile_error!` est correctement protégé par `#[cfg(target_arch = "aarch64")]` et n'affecte pas le build x86_64. Cependant, les fonctions définies dans le même fichier (`read_tsc()`, `wfi()`, `sev()`, `dmb_ish()`, `isb()`, etc.) ne disposent **pas** du même attribut `#[cfg]`. Elles compilent pour x86_64, produisant du code mort non signalé par le lint `dead_code` (car le module est déclaré `pub mod`).

**Correction attendue.** Appliquer `#[cfg(target_arch = "aarch64")]` à chaque fonction du module, ou encapsuler le corps entier du fichier dans un bloc conditionnel.

---

### P2-06 · Couplage implicite entre `STACK_PAGES` dans l'ELF loader et `exec.rs`

**Fichiers :** `kernel/src/fs/elf_loader_impl.rs` ligne 298 · `kernel/src/process/lifecycle/exec.rs` ligne 288

**Constat.** La taille de la pile utilisateur est définie en deux endroits indépendants sans référence mutuelle :

```rust
// elf_loader_impl.rs
const STACK_PAGES: usize = 8;

// exec.rs
const DEFAULT_STACK_PAGES: u64 = 8;
```

Les deux valeurs sont identiques aujourd'hui. Si l'une est modifiée sans l'autre, `exec.rs` calculera un `stack_base` incorrect dans `thread.addresses`, créant un désalignement entre la pile physiquement mappée et l'adresse stockée dans le TCB.

**Correction attendue.** Définir une constante partagée unique — par exemple dans `memory/core/layout.rs` — et y référencer les deux sites :
```rust
// layout.rs
pub const USER_STACK_DEFAULT_PAGES: usize = 8;
```

---

## Annexe — Métriques de couverture de tests

| Module | Tests `#[test]` | Taille code (lignes est.) |
|---|---|---|
| `kernel/src/fs/` | 2 914 | ~très élevé |
| `kernel/src/security/` | 56 | ~élevé |
| `kernel/src/syscall/` | 40 | ~élevé |
| `kernel/src/ipc/` | 14 | ~élevé |
| `kernel/src/memory/` | 12 | ~très élevé |
| `kernel/src/process/` | 8 | ~élevé |
| `kernel/src/scheduler/` | **8** | ~élevé (P1-05) |
| `kernel/src/exophoenix/` | **0** | ~120 KB (P1-02) |

---

## Synthèse des priorités v0.2.0

```
URGENT (démarrage/sécurité)
├── P0-01  KASLR fonctionnel ou déprécié explicitement
├── P0-02  VirtIO blk : adresse MMIO depuis ACPI/PCI
├── P0-03  TTY server : choisir et câbler une architecture complète
└── P0-04  ELF loader : unifier sur demand paging pur

IMPORTANT (stabilisation kernel)
├── P1-01  Fork RFLAGS : masquer bits IOPL 12-13
├── P1-02  ExoPhoenix : tests unitaires minimaux
├── P1-03  PCB fd table : résolution handle dans fs_bridge
├── P1-04  ExoFS : exécuter et valider tier_4 / tier_6 CI
└── P1-05  Scheduler : couvrir pick_next, load_balance, deadline

QUALITÉ (release officielle)
├── P2-01  Drivers vides : implémenter ou supprimer les stubs
├── P2-02  validate_user_segment_range : borne kernel virtuelle
├── P2-03  Supprimer rustc-ice de git, résoudre le code mort
├── P2-04  Documenter la chaîne de build complète (Makefile/justfile)
├── P2-05  AArch64 : cfg-gater les fonctions stub
└── P2-06  STACK_PAGES : constante partagée layout.rs
```

---

*Rapport produit par* **claude-gamma** *sur la base d'une analyse statique exhaustive du code source. Aucune exécution QEMU n'a été effectuée dans ce contexte. Les bugs P0-03 et P0-04 ont été vérifiés par traçage du flux d'exécution dans le code. La résolution de P0-01 est un prérequis architectural pour plusieurs items P2.*
