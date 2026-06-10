# Patch ExoOS — commit fcacb38d
## 22 corrections issues des rapports d'audit

**Base :** commit `fcacb38d6a48f4d6790cc31949626dd0035ed054`
**Sources :** 6 rapports d'audit (ZIP exoos_incoherences.zip) + analyse kernel session

---

## Fichiers modifiés

| # | Fichier | Problème corrigé | Sévérité |
|---|---------|-----------------|----------|
| 1 | `servers/ipc_router/src/router.rs` | Syscall 302 (RECV_NB) → `SYS_IPC_SEND` (300) | 🔴 CRITIQUE |
| 2 | `servers/ipc_router/src/security_gate.rs` | `MAX_INLINE_PAYLOAD` 48 → 192 + `audit_log_violation` IPC réel | 🔴 CRITIQUE |
| 3 | `servers/ipc_router/src/exocordon.rs` | DAG 5 → 35 arêtes, ServiceId Input/Tty/Fb/Exosh/Ps2 | 🔴 CRITIQUE |
| 4 | `servers/exo_shield/src/ipc_gate/policy.rs` | `EXO_SHIELD_PID` 12→10 (était TTY !) | 🔴 CRITIQUE |
| 5 | `kernel/src/lib.rs` | `stage0_init_all_steps()` appelé + `#[cfg(x86_64)]` | 🔴 CRITIQUE |
| 6 | `kernel/src/security/exokairos.rs` | `saturating_mul(100)` → `checked_mul` + SAFETY | 🟠 HAUTE |
| 7 | `kernel/src/ipc/core/constants.rs` | `IPC_MAX_PROCESSES` aligné sur `MAX_PROCESSES` | 🟡 MOYEN |
| 8 | `kernel/src/scheduler/core/switch.rs` | IBPB au context-switch (Spectre v2) | 🔴 SÉCURITÉ |
| 9 | `kernel/src/security/exocage.rs` | `assert!()` OOB → `debug_assert!` + retour silencieux | 🟠 HAUTE |
| 10 | `kernel/src/arch/x86_64/smp/init.rs` | SECURITY_READY avant `publish_current_boot_idle` | 🟠 RACE |
| 11 | `kernel/src/process/lifecycle/exec.rs` | `verify_module_signature()` dans `do_execve` | 🔴 SÉCURITÉ |
| 12 | `kernel/src/syscall/table.rs` | `validate_ipc_envelope_auth` bypass + `send_raw_checked` | 🔴 SÉCURITÉ |
| 13 | `servers/memory_server/src/mmap_service.rs` | `attach_shared_region` vérifie `sender_pid` | 🔴 SÉCURITÉ |
| 14 | `servers/network_server/src/socket_table.rs` | `SOCK_RAW` → `EPERM` sans privilege | 🔴 SÉCURITÉ |
| 15 | `servers/network_server/src/main.rs` | `handle_open` passe `sender_pid` | 🟠 HAUTE |
| 16 | `servers/scheduler_server/src/main.rs` | Garde RT_ALLOWED_PIDS pour `SCHED_REALTIME` | 🔴 SÉCURITÉ |
| 17 | `servers/input_server/src/main.rs` | Multi-abonnés (4 slots) au lieu de 1 | 🟠 HAUTE |
| 18 | `servers/vfs_server/src/main.rs` | `MountTable` `UnsafeCell` → `SpinMutex` | 🟠 HAUTE |
| 19 | `servers/ipc_router/src/main.rs` | Registry FNV-32 → FNV-64a (collision hash) | 🟠 HAUTE |
| 20 | `servers/ipc_router/src/security_gate.rs` | `audit_log_violation` envoie IPC EVENT_REPORT | 🟠 HAUTE |
| 21 | `kernel/src/security/exokairos.rs` | Commentaires SAFETY sur blocs `unsafe` | 🟡 QUALITÉ |
| 22 | `kernel/src/exophoenix/forge.rs` | `verify_merkle` refuse hash nul explicitement | 🔴 SÉCURITÉ |

---

## Instructions de commit

```bash
# Copier les fichiers dans le dépôt local
cp -r patch_fcacb38d/kernel/     Exo-OS/
cp -r patch_fcacb38d/servers/    Exo-OS/

# Vérification avant commit
cd Exo-OS
python3 tools/verify_patch_fcacb38d.py --repo-root .

# Commit
git add -A
git commit -m "fix: 22 patches audit — IPC/sécurité/scheduler/mémoire

- FIX-ROUTER-01: syscall 302 → SYS_IPC_SEND dans router.rs
- FIX-IPC-04: MAX_INLINE_PAYLOAD 48 → 192 (IPC_INLINE_PAYLOAD_SIZE)
- FIX-EXOCORDON-01: DAG 5 → 35 arêtes + ServiceId Input/Tty/Fb/Exosh/Ps2
- FIX-SHIELD-PID: EXO_SHIELD_PID 12 → 10 (était TTY!)
- FIX-STAGE0: stage0_init_all_steps() appelé dans kernel_init
- FIX-KAIROS-01: saturating_mul → checked_mul (overflow)
- FIX-IPC-PROCS: IPC_MAX_PROCESSES aligné sur MAX_PROCESSES
- FIX-IBPB: IBPB au context-switch cross-processus (Spectre v2)
- FIX-EXOCAGE-ASSERT: assert! OOB → debug_assert! + retour silencieux
- FIX-SMP-RACE: SECURITY_READY avant publish_current_boot_idle
- FIX-EXEC-SIG: verify_module_signature dans do_execve
- FIX-IPC-AUTH: validate_ipc_envelope_auth bypass éliminé
- FIX-SHM-ATTACH: attach_shared_region vérifie sender_pid
- FIX-SOCK-RAW: SOCK_RAW → EPERM sans privilege
- FIX-SCHED-RT: garde RT_ALLOWED_PIDS pour SCHED_REALTIME
- FIX-INPUT-MULTI: multi-abonnés 4 slots dans input_server
- FIX-VFS-MOUNT: MountTable UnsafeCell → SpinMutex
- FIX-FNV64: Registry FNV-32 → FNV-64a (collision)
- FIX-AUDIT-IPC: audit_log_violation envoie IPC vers exo_shield
- FIX-MERKLE-DEGRADE: verify_merkle refuse hash nul explicitement"
```
