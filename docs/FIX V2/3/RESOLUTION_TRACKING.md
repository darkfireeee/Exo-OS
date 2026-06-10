# FIX V2/3 — Suivi de résolution (mémoire de travail)

**Démarré :** 2026-06-10 · **Branche :** main · **Build/test :** via WSL uniquement
**Sources :** `exoos_ipc_incoherences.md`, `ExoOS_Security_Audit_Passe2.md`, `ExoOS_Security_Application_Audit.md`

## Règles de travail (à ne pas oublier)
- Compilation/tests : `wsl` → `make build`, `make test`, `cargo check -p exo-os-kernel`
- Outils anti-erreurs : `tools/` (semgrep, python, cargo-deny)
- TLA+ : `docs/Exo-OS-TLA+/` — si le code change le comportement modélisé, mettre à jour/compléter le module TLA+ correspondant
- Docs de référence : `docs/recast/` (architecture v7, CORR-01..54), puis `docs/Vision v0.2.0/`
- Lock order kernel : Memory → Scheduler → Security → IPC → FS
- Politique kernel IPC : `kernel/src/security/ipc_policy.rs` = source de vérité (NB: audit IPC dit 51 paires, audit sécurité dit 92 — À VÉRIFIER dans le code)

## Constantes clés vérifiées
- `SYS_EXO_IPC_SEND = 300`, `SYS_EXO_IPC_RECV = 301`, `SYS_EXO_IPC_RECV_NB = 302`, `SYS_IPC_LOOKUP = 306`
- `IPC_INLINE_PAYLOAD_SIZE = 192` (ABI), `ABI_IPC_ENVELOPE_SIZE = 200`
- Endpoints fixes : exo_shield=10, input=11, tty=12, fb=20

## Plan de correction — État

### Phase 1 — IPC critiques (ipc_router + exo_shield) — ✅ TERMINÉE (2026-06-10)
- [x] INC-2 (CRIT) router.rs : FIX-ROUTER-01 était présent MAIS router.rs/load_balancer.rs
      n'étaient PAS COMPILÉS (absents de lib.rs/main.rs). → FIX-ROUTER-02 : modules câblés
      dans lib.rs, `crate::syscall::syscall6` (inexistant) → `syscall_abi::syscall6`,
      bug src_pid passé en position FLAGS corrigé (flags=0), récursion infinie LoadBalanced
      corrigée, iter→iter_mut load_balancer (E0594), router_init() appelé au boot.
- [x] INC-1/GAP-03 (CRIT) FIX-EXOCORDON-02 : DAG aligné = miroir EXACT des 51 paires kernel
      (const assert == 51). 9 arêtes hors-politique retirées (le routeur, IpcBroker wildcard
      kernel, aurait blanchi des chemins refusés en direct), 16 paires manquantes ajoutées.
- [x] INC-3 (HAUTE) RÉGRESSION détectée : policy.rs corrigé à 10 mais main.rs restait à 12
      → default Deny sur TOUTES les requêtes exo_shield. Fix : EXO_SHIELD_POLICY_ID importé
      de ipc_gate::policy (pub, const assert == endpoint 10) ; exo_cap_check utilise le PID
      runtime via SYS_GETPID (le kernel exige caller==target).
- [x] INC-4 (HAUTE) déjà corrigé (FIX-IPC-04, MAX_INLINE_PAYLOAD = IPC_INLINE_PAYLOAD_SIZE).
- [x] INC-6 (MOY) FIX-EXOCORDON-03 : table dynamique PID→ServiceId lock-free (32 slots),
      peuplée à IPC_MSG_REGISTER par résolution de nom (même table que le kernel
      service_class_for_endpoint_name) ; endpoint fixe 20→Fb ajouté.
- [x] INC-5 (MOY) FIX-REGISTRY-SYNC : le registre kernel (SYS_IPC_LOOKUP) est la source de
      vérité — registration locale acceptée seulement si le nom est connu du kernel, et
      c'est l'endpoint kernel qui est stocké. (FNV-32→FNV-64 déjà fait par FIX-FNV64.)
- [x] INC-7 (FAIBLE) main.rs appelle audit_log_violation() sur verdict de refus (EVENT_REPORT
      0x10 non-bloquant vers ep 10) ; double comptage record_violation retiré.
- Compilation : `cargo check -p exo-ipc-router -p exo-shield` OK (0 warning).

### Phase 2 — Kernel P0 — ✅ VÉRIFIÉE (déjà corrigée dans le code, validée ligne par ligne)
- [x] A-01 : design retenu = cap-check centralisé dans validate_ipc_envelope_auth()
      (EACCES si token invalide), send_raw conservé après validation. OK avec A-02 fixé.
- [x] A-02 : FIX-IPC-AUTH — bypass len!=200 éliminé (hors-format → EACCES sauf endpoints
      éphémères bit 63 et trusted callers).
- [x] C-01 : FIX-EXEC-SIG — is_chain_verified()/check_chain_of_trust() après load_elf ;
      bloquant si feature strict_exec_signatures, warning sinon (dev).
- [x] B-01 : FIX-IBPB — MSR_IA32_PRED_CMD écrit au context switch si cross-process Ring3
      + CPU has_ibpb.
- [x] B-02 : FIX-B-02 — apply_ibrs() au syscall entry (idempotent si EIBRS).
- [x] GAP-02 : FIX-APP-02 — audit_syscall_entry (verdicts Deny/Kill appliqués) + exit
      dans dispatch.rs.
- [x] GAP-01 : FIX-APP-01 — zero_trust::verify_syscall() câblé dans dispatch (hors
      fast-path yield/getpid/clock_gettime).
- [x] GAP-08 (partiel) : log_event ExoLedger sur EXECVE/CLONE/FORK/VFORK dans dispatch.

### Phase 3 — Serveurs Ring 1 P0
- [x] D-02/GAP-04 memory_server : FIX-SHM-ATTACH + FIX-APP-04. RENFORCEMENT KERNEL :
      FIX-IPC-SENDER-AUTH (sys_exo_ipc_send force sender_pid=caller_pid pour non-trusted →
      empêche la forge sender_pid=1 qui contournait tous les contrôles owner_pid Ring1).
- [x] D-03/GAP-05 scheduler_server : FIX-SCHED-RT (Realtime/Deadline réservé PID 1/8/auto).
- [x] D-01 network_server : FIX-SOCK-RAW. CORRIGÉ : handle_open appelait le wrapper compat
      (pid=1 implicite) → contrôle court-circuité. PID réel passé désormais.
- [x] GAP-06 vfs_server : FIX-APP-06. COMPLÉTÉ : garde étendue de VFS_WRITE seul à
      mkdir/unlink/rmdir/rename/truncate.
- [x] GAP-10 device_server : FIX-APP-10 (DEVICE_CLAIM_ALLOWED_PIDS).

### Phase 4 — Câblage subsystèmes P1 — ✅ TERMINÉE
- [x] GAP-07 exokairos : FIX-P1-KAIROS register_ttl_for_cap câblé dans capability::create().
      exoveil revoke_domain à exit = sémantiquement FAUX (lockdown GLOBAL) → caps meurent
      avec pcb.cap_table au reap ; documenté dans exit.rs.
- [x] GAP-08 ExoLedger : FIX-APP-08 (dispatch EXECVE/CLONE/FORK, fork spawn, exit Custom).
- [x] GAP-09 loader : FIX-APP-09 check_exec_permission (detect_signature_note).
- [x] A-03 PCB : FIX-P2-A03 cap_table: Box<CapTable>.
- Kernel `cargo check -p exo-os-kernel` OK (warning ZERO_HASH attendu en dev).

### Phase 5 — Hygiène unsafe (P1) — ✅ TERMINÉE (2026-06-10)
- Outil ajouté : `tools/scan_unsafe_contracts.py` (mesure unsafe sans // SAFETY:).
- [x] E-01 module security/ : DÉJÀ à 0 (les FIX antérieurs avaient documenté
      exonmi/exokairos/exoargos/exoveil ; vérifié par le scanner).
- [x] E-02 fichiers ciblés ramenés à 0 :
      - scheduler/core/switch.rs : 11 → 0 (MSR/FPU/percpu/IrqGuard contrats)
      - drivers/dma.rs : 16 → 0 (map/unmap user_as, Box ownership, TLB shootdown)
      - arch/x86_64/time/sources/tsc.rs : 15 → 0 (CPUID/LFENCE/RDTSC + from_raw_parts)
      - arch/x86_64/exceptions.rs : 17 → 0 (TCB/frame/user_as derefs, IPI handlers)
      - memory/physical/frame/emergency_pool.rs : 1 → 0
      - arch/x86_64/cpu/tsc.rs : 1 → 0 (port PIT)
- NB : ~650 autres unsafe sans SAFETY subsistent hors périmètre audit (servers/exosh
      93, vendors exclus). Hors scope FIX V2/3 — non traités (pas signalés par l'audit).
- Kernel `cargo check` OK après ajouts (3m00, commentaires uniquement).

### Phase 6 — TLA+ & validation
- [ ] Mettre à jour modules TLA+ impactés (IPC policy, capability checks)
- [ ] make build + make test via WSL
- [ ] semgrep + cargo-deny via tools/
- [ ] run_tests.sh si dispo

## Découvertes en cours de route
(à compléter)

## Décisions prises
(à compléter)
