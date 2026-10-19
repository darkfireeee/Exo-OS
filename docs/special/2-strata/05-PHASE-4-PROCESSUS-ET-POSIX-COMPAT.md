# 05 — PHASE 4 : Processus, Cycle de Vie & Compatibilité POSIX (`musl-exo`)

## 1. État Réel dans le Dépôt
* Le cycle de vie des processus dans `kernel/src/process/lifecycle/` et `kernel/src/syscall/table/process.rs` supporte déjà :
  * `SYS_FORK` (57) et `SYS_VFORK` (58) interceptés in-place dans `dispatch.rs` avec clonage CoW (`KERNEL_AS_CLONER` enregistré en Phase 3c de `kernel_init`)[cite: 7].
  * `SYS_EXECVE` (59) avec résolution dynamique via `/lib/ld-exo.so` (`kernel/src/userspace_boot.rs`)[cite: 7].
  * `SYS_CLONE` (56) supportant `CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SETTLS` (`kernel/src/syscall/table/process.rs`)[cite: 7].
  * `SYS_WAIT4` (61) et `SYS_WAITID` (247) (`kernel/src/syscall/handlers/process.rs`)[cite: 7].
  * `SYS_FUTEX` (202) câblé sur `kernel/src/memory/utils/futex_table.rs` avec graine SipHash initialisée au boot[cite: 7].
* **Écart bloquant identifié sur les descripteurs de sockets POSIX** :
  * Lorsqu'un programme POSIX (compilé avec `musl-exo`) crée un socket via `SYS_SOCKET` (41), `net_bridge.rs` retourne un descripteur taggé avec `NET_HANDLE_TAG = 0x4000_0000` (ex: `0x4001_0000`)[cite: 7, 18].
  * Dans `kernel/src/syscall/table/fs/descriptors.rs`, `sys_close` vérifie bien `if let Some(socket_fd) = net_bridge::socket_handle_from_raw(fd)`[cite: 7].
  * **Mais dans `kernel/src/syscall/table/fs/io.rs`, `sys_read`, `sys_write`, `sys_readv` et `sys_writev` n'ont pas ce branchement** : ils appellent directement `validate_fd(fd)` qui rejette tout `fd > 65535` avec `EINVAL` (`-22`)[cite: 7] ! Toute application POSIX standard utilisant `read()`/`write()` sur un socket TCP échoue donc systématiquement[cite: 7].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Routage des Sockets dans `sys_read`, `sys_write`, `sys_readv`, `sys_writev` (`kernel/src/syscall/table/fs/io.rs`)
1. Dans `kernel/src/syscall/table/fs/io.rs`, ajouter en tête de `sys_read` (avant `validate_fd(fd)`)[cite: 7] :
   ```rust
   if let Some(socket_fd) = crate::syscall::net_bridge::socket_handle_from_raw(fd) {
       let len = count as usize;
       if len > IO_BUF_MAX { return E2BIG; }
       return crate::syscall::net_bridge::bridge_result(
           crate::syscall::net_bridge::net_recvfrom(socket_fd, buf_ptr, len, 0, 0, 0)
       );
   }


Ajouter le branchement symétrique en tête de sys_write vers net_bridge::net_sendto(socket_fd, buf_ptr, len, 0, 0, 0)[cite: 7].

Ajouter le support des sockets dans sys_poll (kernel/src/syscall/table/fs/poll_event.rs) pour que poll() / select() sur un fd portant le masque 0x4000_0000 interroge l'état du socket auprès de network_server au lieu de retourner POLLNVAL[cite: 7, 19].

B. Finalisation des 127 Syscalls musl-exo (4.5–4.9)
Vérifier dans servers/vfs_server/src/translation_layer/musl.rs et syscalls.rs que les alias musl_exo_alias (SYS_OPEN -> 519, SYS_GETDENTS64 -> 520) et les appels de bootstrap (SYS_ARCH_PRCTL = 158, SYS_SET_TID_ADDRESS = 218, SYS_UNAME = 63, SYS_WAITID = 247, SYS_CLOCK_NANOSLEEP = 230) passent les tests d'intégration fork_exec_wait, socket_tcp_connect et getdents64.

Dans kernel/src/syscall/handlers/misc.rs, compléter la branche ARCH_GET_FS (0x1003) de sys_arch_prctl (qui retourne actuellement ENOSYS) en lisant le MSR 0xC000_0100 (rdmsr) et en l'écrivant à addr via write_user_typed::<u64>(addr, fs_base)[cite: 7].

3. Vigilance et Invariants pour ne pas Perturber le Noyau
Interdiction de déréférencer directement un pointeur Ring 3 en Ring 0 (SMAP / KPTI) : Dans kernel/src/syscall/handlers/misc.rs, sys_uname écrit actuellement directement sur buf_ptr as *mut u8 avec write_bytes et copy_nonoverlapping[cite: 7]. Lorsque SMAP est actif (ExoCage), tout accès direct déclenche un Page Fault kernel. Toujours passer par copy_from_user / copy_to_user (kernel/src/syscall/validation.rs) qui résout la page dans l'UserAddressSpace du processus courant, déclenche le demand-paging/CoW si nécessaire, et copie via l'alias phys_to_virt de la physmap kernel[cite: 7].

Règle SIGNAL-01 & SA_RESTORER : Tout processus sans libc qui installe un handler de signal via SYS_RT_SIGACTION (13) doit fournir SA_RESTORER (0x0400_0000) et pointer sa_restorer sur exo_syscall_abi::sigreturn_trampoline() (__exo_sigreturn_trampoline qui exécute mov rax, 15; syscall) afin d'éviter un saut à l'adresse 0x0 au retour du handler[cite: 7, 18].