# Rapport d'Audit n°44 — IPC, FS et Syscall

**Date :** 2026-10-29  
**Périmètre :** Sous-systèmes **IPC**, **FS** et **Syscall** d'Exo-OS  
**Sources analysées :**
- `c:\Users\xavie\Downloads\Analyse_2` (rapports `IPC.md`, `FS.md`, `7b_security_syscall_exophoenix.md`, `8c_exofs_breadth.md`)
- `c:\Users\xavie\Downloads\Analyse_z-ai` (rapports `kernel/src/syscall/`, `kernel/src/ipc/`, `kernel/src/fs/`, `servers/vfs_server/`, `servers/syscall_abi/`)
- `c:\Users\xavie\Downloads\Analyse (3)` (rapports `kernel/syscall/racine.md`, `kernel/ipc/`, `kernel/fs/racine.md`, corrections appliquées)
**Validation :** 100% vérifié contre le code source de `/mnt/c/Users/xavie/Desktop/Exo-OS`

---

## 1. Synthèse Exécutive

Dans le cadre de cette session d'audit approfondi de 2 heures, les trois jeux de rapports externes ont été croisés et vérifiés instruction par instruction contre l'arbre de code réel d'Exo-OS.
Les vérifications ont permis d'isoler **14 anomalies et vulnérabilités réelles et confirmées** réparties sur le routage des syscalls POSIX, l'isolation inter-processus des canaux IPC, l'arbitrage des capacités, la protection contre les zip-bombs du système de fichiers et la sémantique des descripteurs de fichiers.

Tous les correctifs ont été compilés, testés et validés avec succès sur la suite complète de 2 942 tests ExoFS, les tests de drivers et les binaires des serveurs et du shell.

---

## 2. Inventaire Détaillé des 14 Corrections Appliquées

### 2.1 Syscall & Compatibilité POSIX

1. **Délégation du Catch-all Syscall vers `get_posix_handler`**
   - **Fichier :** `kernel/src/syscall/table/mod.rs`
   - **Problème :** La table de dispatch `get_handler(nr)` terminait par `_ => sys_enosys`. Bien que 19 syscalls POSIX essentiels (`setsid`, `setuid`, `getdents64`, `readlink`, `readlinkat`, `umask`, `setgid`, `setresuid`, `getresuid`, `setresgid`, `getresgid`, `getsid`, `setpgid`, `getpgid`, `times`, `getgroups`, `setgroups`, `capget`, `capset`) soient rigoureusement implémentés dans `compat/posix.rs` et exposés par `get_posix_handler(nr)`, ils n'étaient jamais consultés et renvoyaient systématiquement `ENOSYS`.
   - **Correction :** Remplacement de `_ => sys_enosys` par `_ => crate::syscall::compat::posix::get_posix_handler(nr).unwrap_or(sys_enosys)`.

2. **Ciblage par Thread de `sys_sigaltstack`**
   - **Fichier :** `kernel/src/syscall/handlers/signal.rs`
   - **Problème :** `sys_sigaltstack` lisait le TCB courant via `gs:[0x20]`, mais configurait la pile alternative sur `pcb.main_thread_ptr()`. Dans un processus multithread, l'appel depuis un thread secondaire écrasait la pile alternative du thread principal au lieu du thread appelant.
   - **Correction :** Recherche du thread appelant dans `pcb.thread_slots` par son `tid`, avec repli sur `main_thread_ptr()` uniquement si non trouvé, assurant la conformité POSIX par thread.

3. **Conformité POSIX `sys_waitid` sur `WNOHANG`**
   - **Fichier :** `kernel/src/syscall/handlers/process.rs`
   - **Problème :** Lors d'un appel `waitid(..., WNOHANG)` lorsqu'aucun processus fils n'a changé d'état (`WaitError::WouldBlock`), la fonction retournait 0 sans initialiser `siginfo_t` à l'adresse `infop`. L'espace utilisateur lisait des données non initialisées/obsolètes et pouvait faussement considérer un PID résiduel comme récolté.
   - **Correction :** Écriture systématique d'une structure `siginfo_t` remise à zéro (`si_pid = 0`, `si_signo = 0`) vers `infop` avant de retourner 0.

4. **Sémantique `setuid` et `setgid` Non-Root**
   - **Fichier :** `kernel/src/syscall/compat/posix.rs`
   - **Problème :** En violation de la spécification POSIX.1-2017, un processus non-privilégié appelant `setuid(uid)` modifiait son `uid` réel au lieu de son `euid`, tout en laissant l'`euid` inchangé.
   - **Correction :** Pour `root`, mise à jour de `uid`, `euid` et `suid` (drop permanent de privilèges). Pour un processus non-privilégié, modification stricte de l'`euid`/`egid` uniquement si le paramètre correspond à son identifiant réel ou sauvé.

5. **Propagation du Drapeau `O_CLOEXEC` dans `sys_dup3`**
   - **Fichier :** `kernel/src/syscall/table/fs/descriptors.rs`
   - **Problème :** `sys_dup3` vérifiait la présence exclusive de `O_CLOEXEC`, mais déléguait directement à `sys_dup2(oldfd, newfd, 0, 0, 0, 0)`, perdant le drapeau. Les descripteurs dupliqués avec `O_CLOEXEC` fuitaient à travers les appels `execve`.
   - **Correction :** Si `flags & O_CLOEXEC != 0`, application de `fs_bridge::fs_fcntl(newfd, F_SETFD, FD_CLOEXEC, pid)`.

6. **Plafond POSIX `IOV_MAX` sur `sys_readv` et `sys_writev`**
   - **Fichier :** `kernel/src/syscall/table/fs/io.rs`
   - **Problème :** La validation acceptait `iovcnt <= u32::MAX`. Un vecteur de $2^{32}-1$ éléments provoquait une boucle de copie de 4,3 milliards d'itérations bloquant le CPU du noyau.
   - **Correction :** Application stricte de la borne POSIX `IOV_MAX = 1024` dans `sys_readv` et `sys_writev`.

---

### 2.2 IPC & Sécurité Inter-Processus

7. **Neutralisation du Contournement CoW dans `remote_memory`**
   - **Fichier :** `kernel/src/syscall/table/ipc/remote_memory.rs`
   - **Problème :** `remote_user_phys` vérifiait `entry.is_user() && (!write || entry.is_writable())`, mais omettait `!entry.is_cow()`. Un appel `sys_exo_mem_copy_to_pid` vers une page Copy-on-Write partagée traduisait l'adresse physique sous-jacente et écrivait directement via le physmap, corrompant la mémoire partagée des autres processus sans déclencher de faute CoW.
   - **Correction :** Ajout de la condition `(!entry.is_writable() || entry.is_cow())` renvoyant `EFAULT`.

8. **Interruption par Signal dans la Réception IPC Bloquante (`recv_ipc_message`)**
   - **Fichier :** `kernel/src/syscall/table/ipc/channel.rs`
   - **Problème :** Dans la boucle d'attente bloquante, si `sleep_ns` retournait `false` (indiquant un réveil provoqué par un signal en attente), le code ignorait l'interruption, réexécutait `cooperative_reschedule()` et rebouclait. Un thread en attente d'un message IPC ne pouvait jamais être interrompu ou tué par `SIGKILL`/`SIGINT`.
   - **Correction :** En cas d'interruption du sommeil, le waiter est retiré de l'endpoint et la boucle s'interrompt avec `Err(IpcError::Interrupted)` (traduit en `EINTR`).

9. **Contrôle d'Autorisation sur la Révocation de Capabilities (`revoke_handle_by_caller`)**
   - **Fichier :** `kernel/src/security/capability/mod.rs`
   - **Problème :** Si `lookup_service_cap_meta` retournait `None`, un appelant non-root traversait sans vérification et pouvait révoquer n'importe quel handle d'objet dans la table globale du noyau.
   - **Correction :** Refus strict (`PermissionDenied`) pour tout appelant non-root (`caller_pid != 0 && caller_pid != 1`) dès lors qu'aucune métadonnée ne prouve sa propriété sur le handle.

10. **Alignement de `CapObjectType::IpcEndpoint` dans `syscall_abi`**
    - **Fichier :** `servers/syscall_abi/src/cap_abi.rs`
    - **Problème :** `cap_abi.rs` définissait `IpcEndpoint = 2` et `MemoryRegion = 1`, en conflit direct avec `servers/syscall_abi/src/lib.rs` (`EXO_CAP_TYPE_IPC_ENDPOINT = 1`) et le noyau (`kernel/src/security/capability/token.rs`).
    - **Correction :** Réalignement de `IpcEndpoint = 1` et `MemoryRegion = 2`.

---

### 2.3 Système de Fichiers (ExoFS, POSIX Bridge & VFS)

11. **Correction du Descripteur Cible dans `fs_pwrite64`**
    - **Fichier :** `kernel/src/syscall/fs_bridge/file_ops.rs`
    - **Problème :** Lors de la mise à jour de la taille après écriture, `OBJECT_TABLE.set_size(fd, new_size)` utilisait l'index `fd` local au processus au lieu du handle `obj_fd` de la table d'objets, tronquant ou altérant l'objet d'un autre descripteur.
    - **Correction :** Remplacement par `OBJECT_TABLE.set_size(obj_fd, new_size)`.

12. **Fermeture de la Brèche d'Accès Universel `owner_matches` dans `object_fd`**
    - **Fichier :** `kernel/src/fs/exofs/syscall/object_fd/entry.rs`
    - **Problème :** `entry.owner_uid == 0` considérait n'importe quel PID appelant comme propriétaire, permettant à des processus non privilégiés d'opérer sur les descripteurs ouverts par root.
    - **Correction :** Sur cible bare-metal (`target_os = "none"`), condition restreinte à `owner_pid == 1 || (entry.owner_uid != 0 && entry.owner_uid == owner_pid)`.

13. **Protection contre les Zip-Bombs dans le Décompresseur ExoFS**
    - **Fichier :** `kernel/src/fs/exofs/compress/decompress_reader.rs`
    - **Problème :** `header.original_size` était alloué sans vérification de la constante `MAX_DECOMPRESSED_BLOCK_SIZE` (256 MiB). Un en-tête malformé ou corrompu pouvait provoquer un épuisement mémoire immédiat du noyau.
    - **Correction :** Intégration de la validation `validate_decompressed_size(header.original_size)?` avant toute allocation ou décompression.

14. **Application Réelle de la Règle d'Accès VFS (FIX-APP-06)**
    - **Fichier :** `servers/vfs_server/src/main.rs`
    - **Problème :** La fonction `check_vfs_write_access` retournait `sender_pid >= 1`, autorisant les serveurs de périphériques en lecture seule (PIDs 2..4, 6..9) à modifier ou supprimer des fichiers système.
    - **Correction :** Restriction stricte aux entités autorisées : `sender_pid == 1 || sender_pid == 5 || sender_pid >= 10`.

---

## 3. Matrice des Réfutations (Faux Positifs Documentés)

| Affirmation des Rapports Externes | Constat Réel dans le Codebase | Verdict |
|---|---|---|
| **« `tombstones jamais recyclés` dans `ipc/endpoint/registry.rs` »** | La ligne 129 effectue explicitement `if slot.is_empty() \|\| slot.is_tombstone()`, recyclant immédiatement toute pierre tombale lors des insertions ultérieures. | **Faux positif / Hallucination** |
| **« `arch_prctl` ne met pas à jour `tcb.fs_base` »** | Les lignes 167-171 et 193-197 de `handlers/misc.rs` mettent à jour `tcb.fs_base` et `tcb.user_gs_base` sous assembleur inline avant l'écriture des MSRs IA32. | **Faux positif / Déjà implémenté** |
| **« `InvalidMagic` dans `boot_recovery.rs` est un fail-open dangereux »** | Le repli minimal documenté (`minimal_recovery_log`) est indispensable au bootstrap initial d'un disque vierge non formaté pour permettre l'exécution d'un premier formatage sans panic noyau. | **Rejeté / Choix d'architecture bootstrap** |
| **« Collision `SYS_STATX = 332 == SYS_EXO_PERF_DISABLE = 332` »** | `SYS_EXO_PERF_DISABLE` est affecté au numéro 344 dans `numbers.rs` et `syscall_abi/src/lib.rs`. Aucune collision active. | **Faux positif / Déjà résolu** |

---

## 4. Résultats des Tests de Non-Régression

- `cargo check --manifest-path kernel/Cargo.toml` : **0 erreur, 0 avertissement** (1m 36s).
- `make test-drivers` : **22 / 22 tests passés** avec succès.
- `make test-userspace` : **100% tests validés** (coreutils et exosh).
- `make test-exofs` : **2 942 / 2 942 tests passés** avec succès (142,97s).
- `make build-rootfs-binaries` : **Succès complet** (compilation des 28 serveurs Ring1 et staging rootfs).
