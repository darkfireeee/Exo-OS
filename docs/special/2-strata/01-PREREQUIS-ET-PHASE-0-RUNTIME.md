# 01 — BLOCS PRÉREQUIS (-1 & 0) & PHASE 0 : Fondations Runtime

## 1. État Réel dans le Dépôt
* **BLOC -1 (Bugs Kernel Bloquants)** :
  * `B-01` (VirtIO/AHCI BAR PCI) est résolu dans `drivers/storage/ahci/src/main.rs` qui lit `pci_info.bars[5]` (ABAR) via `SYS_PCI_FIND_DEVICE` (547) et dans `drivers/storage/virtio_blk/src/main.rs` qui lit `pci_info.bar0_phys`[cite: 7, 10, 17].
  * `B-05` (`cgroup::init()` avant `runqueue_init()`) est résolu en Phase 2f de `kernel/src/lib.rs` (`crate::process::resource::cgroup::init()` appelé avant `crate::scheduler::init()`)[cite: 7, 8, 10].
  * `B-09` (Authentification IPC `len == 128`) est résolu dans `kernel/src/syscall/table/ipc/channel.rs` via `validate_ipc_envelope_auth()`, qui exige une enveloppe canonique de `ABI_IPC_ENVELOPE_SIZE` (200 octets) contenant un `CapToken` de 20 octets vérifié par `check_token_owner()`[cite: 7, 10, 18, 19].
  * **Écart restant sur `B-03`/`B-04`** : Le trampoline ASM Multiboot2 `_start` dans `kernel/src/main.rs` ne construit qu'une seule table `_boot_pd` de 512 huge pages de 2 Mio, limitant le mapping initial `PML4[0]` et `PML4[256]` (`0xFFFF_8000_0000_0000`) à **1 Gio**[cite: 7, 10]. En revanche, `exo-boot/src/memory/paging.rs` mappe déjà **4 Gio** (4 tables PD dans `pdpt_identity` et `pdpt_higher`)[cite: 20].
* **PHASE 0 (Fondations Runtime)** :
  * `0.1` (SSR 256-core) : `libs/exo_types/src/ssr_layout.rs` définit déjà `SSR_MAX_CORES_LAYOUT = 256` et `SSR_SIZE = 0x1_0000` (64 Kio) à `SSR_BASE_PHYS = 0x0100_0000`[cite: 10, 11, 19].
  * `0.3`–`0.5` (`exo-alloc`) : **Écart critique** — `libs/exo_allocator/build.rs` compile du code C (`vendor/mimalloc/src/static.c`) via la crate `cc`[cite: 19], alors que `SPEC-EXO-CRATES-STRATA.md` exige un allocateur `libs/exo-alloc` 100 % Rust `#![no_std]` basé sur `SYS_MMAP` (9), `SYS_MUNMAP` (11) et `SYS_MREMAP` (25), sans aucun symbole `libc` (`malloc`/`free`)[cite: 10, 11, 15].
  * `0.6`–`0.9` (`generic-rt`) : `libs/generic-rt/src/lib.rs` implémente `GenericTcb<Os>` avec lecture `fs:[offset]` sur `x86_64`[cite: 19], mais il manque l'export explicite de `__tls_get_addr` pour les binaires ELF dynamiques chargés par `/lib/ld-exo.so`[cite: 7, 10, 11].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Extension du Trampoline Multiboot2 à 4 Gio (`kernel/src/main.rs`)
1. Dans la section `.bss` de `kernel/src/main.rs`, remplacer `_boot_pd` unique par 4 tables alignées sur 4096 octets (`_boot_pd0`, `_boot_pd1`, `_boot_pd2`, `_boot_pd3`).
2. Dans `_pd_loop` (`.code32`), remplir les 2048 entrées (4 × 512 huge pages de 2 Mio) et lier `_boot_pdpt[0..3]` à `_boot_pd0..3` afin que la physmap `PML4[256]` (`0xFFFF_8000_0000_0000`) couvre **0 à 4 Gio** dès la première instruction 64 bits (`_start64`), garantissant un boot sans panic avec `-m 2G` (`B-03`, `B-04`, `O-04`)[cite: 7, 8, 10].

### B. Implémentation de `libs/exo-alloc/src/lib.rs` (Zéro C, Zéro `libc`)
1. Dans `libs/exo-alloc/src/lib.rs`, écrire l'allocateur `#![no_std]` pur s'appuyant sur `exo-syscall-abi`[cite: 15, 18] :
   * **`alloc(layout)`** : Calculer `size = (layout.size().max(layout.align()) + 4095) & !4095` et appeler `syscall6(SYS_MMAP, 0, size as u64, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, u64::MAX, 0)`[cite: 7, 15, 18]. Retourner `null_mut()` si la valeur de retour est `< 0`[cite: 7, 15].
   * **`dealloc(ptr, layout)`** : Calculer la taille alignée et appeler `syscall2(SYS_MUNMAP, ptr as u64, size as u64)`[cite: 7, 15, 18].
   * **`realloc(ptr, layout, new_size)`** : Appeler `syscall5(SYS_MREMAP, ptr as u64, old_aligned as u64, new_aligned as u64, 1 /* MREMAP_MAYMOVE */, 0)` qui bénéficie déjà du chemin zéro-copie `do_mremap_zero_copy` dans `kernel/src/syscall/table/fs/system_misc.rs`[cite: 7, 15].
2. Retirer `exo_allocator` (C/mimalloc) des dépendances des binaires Ring 1 / Ring 3 au profit de `exo-alloc`[cite: 15, 19].

### C. Ajout de `__tls_get_addr` dans `libs/generic-rt/src/lib.rs`
1. Exporter `#[no_mangle] pub unsafe extern "C" fn __tls_get_addr(ti: *const [usize; 2]) -> *mut u8` lisant le pointeur DTV via `GenericTcb::arch_read(0)` (`fs:[0]`) configuré par `SYS_ARCH_PRCTL` (`ARCH_SET_FS = 0x1002`) ou lors de `SYS_CLONE` (`CLONE_SETTLS = 0x0008_0000`)[cite: 7, 10, 11, 19].

### D. Outillage d'Audit (BLOC 0)
1. Créer `tools/audit_constants.py` vérifiant la synchronisation stricte entre `kernel/src/syscall/numbers.rs`, `servers/syscall_abi/src/lib.rs` et `libs/exo_types/src/ssr_layout.rs` (en complément du test existant `servers/syscall_abi/tests/syscall_contract_stress.rs`)[cite: 7, 10, 18, 19].
2. Créer `deny.toml` interdisant formellement les crates rejetées dans `INDEX-STRATA.md` : `linux-pam`, `shadow-rs`, `libsodium`, `libfuse`, `rtnetlink`, `zbus`, `relibc`, `async-std`, `tokio`, `pkgcraft`[cite: 9, 10, 11].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Collision mémoire basse avec la SSR (16 Mio)** : La pile `_exo_boot_stack` dans `kernel/src/main.rs` est fixée à 512 Kio car une pile de 4 Mio entrait en collision avec la région physique SSR d'ExoPhoenix située à `0x0100_0000` (16 Mio). **Ne jamais agrandir `.boot_stack` ni les tables `_boot_pd*` au point de franchir l'adresse physique `0x0100_0000`**[cite: 7, 19].
* **Séparation des registres `GS` (Kernel) et `FS` (User TLS)** : En Ring 0 (après `SWAPGS`), `gs:[0x20]` contient le pointeur vers le `ThreadControlBlock` (`TCB`) du noyau (`kernel/src/syscall/fast_path.rs`), tandis que le TLS utilisateur Ring 3 utilise `fs:[0]` configuré via le MSR `IA32_FS_BASE` (`0xC000_0100` dans `sys_arch_prctl`)[cite: 7, 19]. `generic-rt` en Ring 3 doit **toujours** lire `fs:[...]` et ne jamais modifier `gs` sur `x86_64`[cite: 7, 19].