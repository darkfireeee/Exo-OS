# 08 — PHASE 7 : Installeur `exo` & Gestionnaire de Paquets (`exo-pkg`)

## 1. État Réel dans le Dépôt
* Le format d'archive natif `.exoar` et les syscalls d'import/export d'objets ExoFS (`SYS_EXOFS_EXPORT_OBJECT = 516` et `SYS_EXOFS_IMPORT_OBJECT = 517`) sont déjà définis dans `kernel/src/syscall/numbers.rs` et `servers/syscall_abi/src/lib.rs`[cite: 5, 7, 18].
* La crate `libs/exo-pkg` est déjà déclarée dans le workspace `libs/Cargo.toml` et liée à `exo-syscall-abi` (`libs/Cargo.lock`)[cite: 19].
* **Écarts restants** :
  1. Le binaire utilisateur `exo` (sous-commandes `exo install`, `exo compat install`, `exo compat run`, `exo remove`, `exo list`, `exo doctor`) n'est pas encore câblé dans `userspace/` ni dans le routeur de commandes d'`exosh`[cite: 5, 6, 9, 10, 11].
  2. Les deux jalons applicatifs obligatoires de la Phase 7 (`calendar` en mode texte et `curl` en HTTPS) ne sont pas encore empaquetés et validés de bout en bout.

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Implémentation Complète de `libs/exo-pkg` (`7.1`, `7.6`, `7.7`, `7.8`)
1. Dans `libs/exo-pkg/src/`, implémenter le cycle de vie complet d'un paquet `.exoar` :
   * **Lecture et Vérification de Signature (`7.8`)** : Avant toute extraction, calculer le digest SHA-512 / BLAKE3 de l'archive et vérifier sa signature Ed25519 via `crypto_server` (ou `exo-verity` contre `/etc/exoos/trusted_signing.pub`)[cite: 8, 10, 11, 15, 17]. Refuser immédiatement l'installation si la signature est absente ou invalide[cite: 10, 11].
   * **Affichage et Confirmation du Manifeste (`7.7`)** : Parser le manifeste du paquet (droits ExoFS requis, endpoints IPC cibles, accès réseau) et l'afficher sur le TTY au format Capability natif (`@token`, `rwxlksd`) en demandant confirmation à l'utilisateur[cite: 10, 11, 16].
   * **Injection dans ExoFS (`7.1`)** : Extraire les objets dans `/apps/<pkg>/` via `SYS_EXOFS_IMPORT_OBJECT` (517) ou `SYS_EXOFS_OBJECT_CREATE` (504), créer les jetons de capacité correspondants via `SYS_EXO_CAP_CREATE` (320), et commiter l'epoch via `SYS_EXOFS_EPOCH_COMMIT` (518)[cite: 7, 10, 11, 16, 18].
   * **Désinstallation Propre (`exo remove <pkg>`, `7.6`)** : Révoquer tous les handles de capabilities du paquet via `SYS_EXO_CAP_REVOKE` (322), supprimer les objets de `/apps/<pkg>/` via `SYS_EXOFS_OBJECT_DELETE` (505), et commiter une nouvelle epoch.

### B. Mode `exo compat` et Jalons `calendar` & `curl` (`7.2`–`7.5`)
1. Implémenter `exo compat install <pkg>` :
   * Générer automatiquement le profil de sandbox `exo_shield` correspondant au manifeste du paquet et l'enregistrer dans `/etc/exoshield/`[cite: 8, 10, 11].
2. Implémenter `exo compat run <pkg>` :
   * Appliquer le masque `SYS_EXO_PLEDGE` (324) dérivé du manifeste, puis exécuter le binaire ELF lié à `musl-exo` / `/lib/ld-exo.so` via `SYS_EXECVE` (59)[cite: 7, 10, 11].
3. Valider les 2 jalons de la checklist :
   * **Jalon 1 (`7.2`, `7.3`)** : `exo compat install calendar` puis `exo compat run calendar` $\to$ affichage du calendrier texte dans `exosh` sans dépendance réseau[cite: 9, 10, 11].
   * **Jalon 2 (`7.4`, `7.5`)** : `exo compat install curl` puis `curl https://example.com` $\to$ résolution DNS + handshake TLS 1.3 + réponse HTTP `200 OK`[cite: 9, 10, 11].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Interdiction d'exécuter un binaire non signé si `strict_exec_signatures` est actif** : Dans `kernel/src/lib.rs` et `kernel/Cargo.toml`, `do_execve()` vérifie `check_chain_of_trust()` sur l'objet ExoFS[cite: 7]. Lorsqu'`exo-pkg` installe un binaire exécutable dans `/apps/<pkg>/bin/`, il doit impérativement créer l'objet avec `ObjectKind::Code` (droit `EXOFS_RIGHT_EXEC = 1 << 7`) et conserver ses métadonnées de signature via `SYS_EXOFS_OBJECT_SET_META` (507), sinon `do_execve()` retournera `ENOEXEC` (`-8`) ou `SignatureVerificationFailed`[cite: 7, 16, 18].
* **Bannissement des syntaxes `apt` / `pkgcraft`** : Conformément à `INDEX-STRATA.md`, seule la syntaxe `exo install <pkg>` (natif) et `exo compat install <pkg>` (POSIX sandboxé) doit exister dans le système[cite: 9].