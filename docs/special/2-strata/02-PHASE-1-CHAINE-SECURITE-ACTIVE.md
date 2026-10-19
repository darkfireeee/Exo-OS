# 02 — PHASE 1 : Chaîne de Sécurité Active (Ring 0)

## 1. État Réel dans le Dépôt
* Dans `kernel/src/lib.rs` (`kernel_init`), la séquence de sécurité est déjà câblée dans le bon ordre architectural :
  1. **Phase 2e** : `crate::security::exoseal::exoseal_boot_phase0()` verrouille CET/PKS/IOMMU avant la création des runqueues et threads.
  2. **Phase 5** : `crate::security::security_init(kaslr_entropy, KERNEL_LOAD_PHYS_ADDR)` initialise les capabilities, la cryptographie et la graine SipHash de la table futex (`init_futex_seed`), puis démarre le kthread `start_integrity_monitor()` qui vérifie périodiquement `.text`/`.rodata` et journalise dans `ExoLedger`.
  3. **Phase 5b** : `crate::exophoenix::stage0::stage0_init_all_steps(true)` initialise le domaine IOMMU de blocage[cite: 7].
* Les syscalls `SYS_EXO_CAP_CREATE` (320), `SYS_EXO_CAP_REVOKE` (322), `SYS_EXO_CAP_CHECK` (323) et `SYS_EXO_PLEDGE` (324) sont implémentés dans `kernel/src/syscall/table/ipc/security_ops.rs`[cite: 7].
* **Écarts restants** :
  1. Dans `kernel/Cargo.toml`, la feature `strict_exec_signatures` n'est pas dans `default`, ce qui laisse `do_execve()` en mode avertissement au lieu de refuser l'exécution (`ExecError::SignatureVerificationFailed`)[cite: 7].
  2. Dans `kernel/build.rs`, si `KERNEL_A_IMAGE_PATH` n'est pas défini lors du build, le script génère un contrat ExoPhoenix nul (`ZERO_HASH`), faisant échouer `verify_merkle()` au runtime[cite: 7].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Pipeline de Build Double-Passe ExoPhoenix (`kernel/build.rs` & `Makefile`)
1. Configurer le système de build pour compiler le noyau en deux passes conformes à `kernel/build.rs`[cite: 7] :
   * **Passe 1 (Rôle A)** : Compiler avec `EXOPHOENIX_BUILD_ROLE=A` pour produire l'image ELF propre de Kernel A (`write_artifacts` écrit alors des hash nuls sans warning dégradé)[cite: 7].
   * **Passe 2 (Rôle B / Production)** : Compiler avec `KERNEL_A_IMAGE_PATH=<chemin_elf_passe_1>` et `EXOPHOENIX_REQUIRE_HASHES=1`[cite: 7]. `kernel/build.rs` calcule alors `blake3::hash(&image)` (`kernel_a_image_hash.bin`) et `compute_kernel_a_merkle_root(&image)` sur `.text || .rodata` (`kernel_a_merkle_root.bin`), puis embarque `kernel_a_image.bin` pour que `seed_kernel_a_image_blob()` (Phase 7 de `kernel_init`) l'injecte dans ExoFS[cite: 7].

### B. Activation de `strict_exec_signatures` et `CAP-01`
1. Dans `kernel/Cargo.toml`, activer `strict_exec_signatures` dans les profils de release (ou gérer le flag `EXOSEAL_DEV_BYPASS` en le journalisant explicitement dans `ExoLedger`) afin que `check_chain_of_trust()` bloque tout ELF non signé[cite: 7, 10, 11].
2. Dans chaque serveur Ring 1 (`servers/*/src/main.rs`), appliquer la règle `CAP-01` documentée dans `libs/exo_types/src/cap.rs` : appeler `exo_types::verify_cap_token(&boot_token, CapabilityType::...)` (comparaison en temps constant via `subtle::ConstantTimeEq`) dès l'entrée de `_start` lorsque le token de démarrage est transmis[cite: 19].

### C. Persistance d'`ExoLedger` et Vérification de Chaîne (`1.19`–`1.21`)
1. Dès la fin de la Phase 7 (`FS`) dans `kernel/src/lib.rs` (juste après `exofs_init`), attacher le backend de persistance d'`ExoLedger` à un objet scellé ExoFS (`/var/log/exoledger.sealed`) et exécuter la vérification de la chaîne de hachage BLAKE3 depuis le bloc de genèse[cite: 7, 8, 10, 11, 13].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Contraintes SIMD / SSE2 en Ring 0 (`x86_64-unknown-none`)** : Comme documenté dans `kernel/Cargo.toml`, la cible bare-metal désactive SSE2 (`-sse2`)[cite: 7]. **Ne jamais ajouter les crates `chacha20`, `chacha20poly1305` ou `aes-gcm` dans `kernel/Cargo.toml`**, car `poly1305` et `polyval` déclenchent un crash LLVM (`Do not know how to split 128-bit ops`)[cite: 7]. Utiliser exclusivement `exo-fscrypt` (XChaCha20 en arithmétique `u32` pure + MAC BLAKE3) et `sha2` avec `features = ["force-soft"]` dans le noyau[cite: 7, 17].
* **Invariant `stage0_init_all_steps(true)`** : Dans `kernel/src/lib.rs` (Phase 5b), le paramètre `kernel_a_boot` passé à `stage0_init_all_steps(true)` doit **impérativement rester à `true`** lors du boot BSP[cite: 7]. Le passer à `false` exécuterait les étapes réservées à Kernel B qui écrasent la TSS, l'IDT et le timer APIC du BSP, provoquant un gel immédiat après l'étape `SECURITY`[cite: 7].
* **Monotonie de `sys_exo_pledge` (324)** : Dans `kernel/src/syscall/table/ipc/security_ops.rs`, `restrict_process(caller_pid, restrictions)` est irréversible et monotone, et refuse toujours `PID 0` et `PID 1` (`init_server`)[cite: 7]. Ne jamais appeler `SYS_EXO_PLEDGE` depuis `init_server`[cite: 7].