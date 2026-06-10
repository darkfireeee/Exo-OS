# ExoOS — Audit d'Application du Code de Sécurité
## Propagation du Kernel / ExoShield vers les Modules Périphériques

**Dépôt :** `github.com/darkfireeee/Exo-OS` — cloné le 05 juin 2026  
**Périmètre :** `kernel/src/security/`, `servers/exo_shield/` → tous les autres modules  
**Méthodologie :** analyse statique exhaustive (grep cross-référentiel, lecture de fichiers clés)

---

## 1. Architecture de Sécurité du Kernel — État Réel

### 1.1 Sous-système `kernel/src/security/` (13 modules)

L'orchestration via `security_init()` est correcte et ordonnée en 13 étapes :

```
ExoSeal phase0 → integrity_check → capability → zero_trust → crypto
→ isolation → exploit_mitigations → audit → access_control
→ exoledger → exokairos → exoargos → exonmi → ExoSeal complete → SECURITY_READY
```

| Module | Fonction | Qualité |
|---|---|---|
| `capability/` | Tokens 24 bytes, table 512 slots, verify O(1), révocation, délégation, namespace | ✅ Complet |
| `access_control/checker.rs` | Point d'entrée unifié `check_access()` v6 | ✅ Bien conçu |
| `ipc_policy.rs` | Whitelist DAG 92 paires, `check_direct_ipc()` | ✅ Présent |
| `exocage/` | CET Shadow Stack + IBT, `cp_handler` câblé dans IDT | ✅ Câblé |
| `exoseal/` | IOMMU PKS, domaines par PID | ✅ Câblé |
| `exoledger/` | Audit chaîné Blake3, zone P0 immuable | ⚠️ Sous-utilisé |
| `exoargos/` | PMC snapshot hook au context-switch | ✅ Hook installé |
| `exonmi/` | Watchdog NMI via LAPIC IRQ | ✅ Câblé |
| `zero_trust/` | Labels MLS, Bell-LaPadula + Biba, `verify_access()` | ❌ Code mort |
| `audit/syscall_audit.rs` | `audit_syscall_entry/exit()` | ❌ Jamais appelé |
| `exokairos/` | Temporal capabilities | ❌ Aucun appelant externe |
| `exoveil/` | PKS domain revocation | ❌ Aucun appelant externe |
| `exploit_mitigations/` | KASLR, canaries, CFG, CET, SafeStack | ✅ Init |

### 1.2 Points d'Entrée Effectivement Câblés (propagation confirmée)

```
arch/x86_64/smp/init.rs          ← spin-wait SECURITY_READY (CVE-EXO-001 fixé)
arch/x86_64/exceptions.rs:1016   ← cp_handler (ExoCage #CP)
arch/x86_64/exceptions.rs:1082   ← exonmi::tick() via LAPIC ExoNmiWatchdog
process/core/tcb.rs:474,504      ← enable_cet_for_thread() au spawn
process/lifecycle/exit.rs        ← drivers::driver_do_exit → IOMMU cleanup
scheduler/core/switch.rs         ← pmc_snapshot hook (ExoArgos)
scheduler/timer/tick.rs:153      ← process_iommu_faults() (timer tick)
syscall/table.rs:3757            ← check_direct_ipc() sur SYS_IPC_SEND
syscall/table.rs:4498+           ← ensure_domain_for_pid() IOMMU syscalls DMA
ipc/capability_bridge/check.rs   ← check_access() pour IPC (endpoint/channel/shm)
fs/exofs/syscall/object_write.rs ← exo_ledger_append() sur écriture fichier
```

---

## 2. Application dans les Serveurs Ring 1

### 2.1 Tableau de Couverture par Serveur

| Serveur | check_access | ExoCordon | CapToken | ExoLedger | ZeroTrust | Verdict |
|---|:---:|:---:|:---:|:---:|:---:|---|
| `exo_shield` | ✅ ipc_gate | ✅ (via policy) | ✅ `ServiceCapRequirement` | ✅ report.rs | — | ✅ Bien intégré |
| `ipc_router` | — | ✅ security_gate | — | — | — | ⚠️ Partiel |
| `crypto_server` | — | — | ✅ `ExoCapTokenWire` | — | — | ⚠️ Partiel |
| `exosh` | — | ✅ | — | — | — | ⚠️ Minimal |
| `init_server` | — | — | — (boot_info only) | — | — | ❌ Absent |
| `memory_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `vfs_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `scheduler_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `network_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `device_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `fb_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `input_server` | ❌ | ❌ | ❌ | ❌ | ❌ | 🔴 **P0** |
| `tty_server` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ Absent |
| `virtio_drivers` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ Absent |

### 2.2 Application dans les Drivers

**100% des drivers ont zéro intégration sécurité directe** :
`drivers/display/`, `drivers/input/ps2/`, `drivers/network/e1000/`, `drivers/network/virtio_net/`,
`drivers/storage/virtio_blk/`, `drivers/tty/`, `drivers/fs/ext4/`, `drivers/fs/fat32/`

> **Note :** Les drivers étant des processus Ring 1, ils sont supposés être protégés par la politique IPC du kernel (`ipc_policy.rs`). Ce serait acceptable si le kernel gérait l'authentification à l'entrée. Ce n'est que partiellement vrai (voir §3).

---

## 3. Gaps Critiques Identifiés

### GAP-01 🔴 P0 — `zero_trust::verify_access()` est du code mort

**Fichier :** `kernel/src/security/zero_trust/verify.rs`  
**Règle déclarée :** `RÈGLE ZT-VERIFY-01 : verify_access() est appelé pour CHAQUE accès à une ressource protégée`  
**Réalité :** `grep -rn "verify_access"` en dehors du module security → **zéro résultat**.

L'intégralité du modèle Bell-LaPadula + Biba (labels MLS, contextes de confiance, policy stricte) est déclaré, initialisé, mais jamais invoqué. Les modules `fs/`, `process/`, `syscall/handlers/` ignorent totalement ce canal de vérification. L'architecture de sécurité multi-niveaux n'est pas opérationnelle.

---

### GAP-02 🔴 P0 — `audit_syscall_entry/exit()` jamais appelés

**Fichiers :** `kernel/src/security/audit/syscall_audit.rs`  
**Réalité :** Aucun appel dans `kernel/src/syscall/dispatch.rs`, `syscall/table.rs`, ou `syscall/fast_path.rs`.

```rust
// Ce code existe mais n'est jamais invoqué :
pub fn audit_syscall_entry(syscall_nr: u32, pid: u32, tid: u32, uid: u16) -> AuditVerdict { ... }
pub fn audit_syscall_exit(tid: u32, result: i64) { ... }
```

Tous les syscalls passent sans audit. L'AuditVerdict (qui permet de bloquer un syscall suspecté) ne participe pas au dispatch. L'entrée syscall en `arch/x86_64/syscall.rs` n'appelle pas la couche audit.

---

### GAP-03 🔴 P0 — ExoCordon userspace : divergence critique avec kernel

**Fichier :** `servers/ipc_router/src/exocordon.rs`  
**kernel/src/security/ipc_policy.rs** : 92 paires de services autorisées  
**servers/ipc_router/src/exocordon.rs** : **5 arêtes seulement**

```rust
// ExoCordon userspace — seules 5 routes protégées :
AuthEdge::new(ServiceId::Init,    ServiceId::Memory,        4, 10_000),
AuthEdge::new(ServiceId::Init,    ServiceId::Vfs,           4, 10_000),
AuthEdge::new(ServiceId::Vfs,     ServiceId::Crypto,        2, 50_000),
AuthEdge::new(ServiceId::Network, ServiceId::Vfs,           2, 100_000),
AuthEdge::new(ServiceId::Device,  ServiceId::VirtioDrivers, 1, 1_000_000),
```

Services **absents** du DAG userspace alors qu'ils sont dans la policy kernel : `ExoShield`, `Scheduler`, `Input`, `Tty`, `Fb`, `Ps2`, `IpcBroker`. Toute communication impliquant ces services passe par le router sans validation ExoCordon. La règle **IPC-01** (DAG ExoCordon = source de vérité unique) est brisée.

Par ailleurs, le kernel ne passe **pas** tous les messages IPC par le `ipc_router`. Le chemin kernel IPC direct (`syscall/table.rs` → `SYS_IPC_SEND`) appelle `check_direct_ipc()` (basé sur `ipc_policy.rs`) mais **bypass l'ipc_router**. L'ExoCordon userspace n'est donc qu'un filtre supplémentaire facultatif, pas le premier rempart.

---

### GAP-04 🔴 P0 — `memory_server` : aucune vérification de capability

**Fichier :** `servers/memory_server/src/main.rs`

```rust
fn dispatch(request: &MemoryRequest) -> MemoryReply {
    match request.msg_type {
        MEMORY_MSG_ALLOC    => service.handle_alloc(request.sender_pid, &request.payload),
        MEMORY_MSG_FREE     => service.handle_free(request.sender_pid, &request.payload),
        MEMORY_MSG_PROTECT  => service.handle_protect(request.sender_pid, &request.payload),
        MEMORY_MSG_SHM_CREATE => ...
        // ← aucun check_access, aucun CapToken vérifié
    }
}
```

Un processus peut libérer ou modifier les protections de pages appartenant à un autre PID en forgeant simplement un `sender_pid`. Le seul garde-fou est la politique IPC kernel (InitServer → MemoryServer autorisé), mais une fois le message livré, le serveur ne valide pas l'identité du demandeur réel avec une capability.

---

### GAP-05 🔴 P0 — `scheduler_server` : escalade de priorité libre

**Fichier :** `servers/scheduler_server/src/main.rs`

```rust
fn handle_register(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
    // Le sender_pid est lu depuis la payload IPC — non authentifié par capability
    let tid = read_u32(payload, 0)?;
    let class = SchedulingClass::from_u32(raw_class)...;
    // SchedulingClass::RealTime ou FIFO accepté sans vérification
}
```

N'importe quel processus peut demander une classe `SCHED_REALTIME` ou une affinité CPU exclusive. L'admission temps-réel (`realtime_admit.rs`) vérifie les budgets mais pas les droits du demandeur. Aucun `CapToken` n'est requis pour `SCHED_MSG_SET_PRIORITY` ou `SCHED_MSG_REALTIME_ADMIT`.

---

### GAP-06 🔴 P0 — `vfs_server` : zéro intégration sécurité

Le VFS server (`servers/vfs_server/`) n'a aucun appel à `check_access`, aucun `CapToken`, aucun `exoledger`. Les opérations de fichier (`open`, `read`, `write`, `rename`, `stat`) passent directement à la couche de traduction POSIX. Le `check_access_flags()` dans le kernel ExoFS (`object_open.rs`) vérifie les flags POSIX (O_RDONLY/O_WRONLY/O_RDWR) mais pas le capability token du processus appelant.

---

### GAP-07 🟠 P1 — `exokairos` et `exoveil` : subsystèmes isolés

**ExoKairos** (capabilities temporelles avec TTL) est initialisé dans `security_init()` mais aucun serveur ni module kernel externe n'appelle `ttl_for_right()` ou ne crée de `TemporalCap`. Les tokens IPC n'expirent donc jamais.

**ExoVeil** (PKS domain revocation O(1)) est initialisé, `revoke_domain()` et `restore_domain()` sont exportés, mais aucun module externe n'appelle ces fonctions. Les domaines PKS ne sont jamais révoqués dynamiquement lors d'un handoff ou d'une terminaison de processus.

---

### GAP-08 🟠 P1 — `ExoLedger` : seuls 2 points de log actifs

Sur l'ensemble du kernel, ExoLedger est appelé seulement :
- `syscall/table.rs:3766` — sur SYS_IPC_SEND (refus)
- `fs/exofs/syscall/object_write.rs:125` — sur accès refusé en écriture

Les événements non audités incluent : process spawn/exit, capability creation/revocation, memory map/unmap, driver claim, SMP AP startup, IPC channel creation, ExoShield quarantine events.

---

### GAP-09 🟠 P1 — `loader/security/capability_check.rs` : stub non fonctionnel

```rust
// loader/src/security/capability_check.rs — TOTALITÉ DU FICHIER :
pub const CAP_EXEC: u64 = 1 << 0;
pub fn may_exec(mask: u64) -> bool {
    mask & CAP_EXEC != 0
}
```

Le loader ne vérifie pas la signature de module via `security::verify_module_signature()`, ne consulte pas `integrity_check::check_chain_of_trust()`. Un binaire non signé peut être exécuté si le bitmask `CAP_EXEC` est présent.

---

### GAP-10 🟡 P2 — `device_server` : claim_validator sans authentification

```rust
pub fn validate_claim(registry, phys_base, size, owner_pid, bdf_raw, flags) -> Result<(), i64> {
    // Vérifie BDF, taille, flags — mais pas de CapToken pour "qui a le droit de claim ce device"
    if snapshot.owner_pid != 0 { return Err(EBUSY); }  // seul garde-fou
}
```

Tout processus Ring 1 capable d'envoyer un message au `device_server` peut réclamer un périphérique PCI si `owner_pid == 0`. Aucune capability `CAP_DEVICE_CLAIM` ou `CAP_DMA` n'est vérifiée.

---

## 4. Ce Qui Fonctionne Correctement

### 4.1 Chemin IPC kernel (SYS_IPC_SEND)

```
SYS_IPC_SEND
 ├─ check_direct_ipc() ← ipc_policy.rs DAG 92 paires ✅
 ├─ validate_ipc_envelope_auth() ✅
 ├─ check_endpoint_access() → check_access() → capability::verify() ✅
 └─ exo_ledger_append() sur refus ✅
```

C'est le chemin le mieux gardé du système.

### 4.2 CET / ExoCage (hardware)

Le handler `#CP` est câblé dans l'IDT (`exceptions.rs:1016`). `enable_cet_for_thread()` est appelé au spawn (`tcb.rs:474,504`). La protection est hardware et ne peut pas être contournée par du code en espace utilisateur.

### 4.3 IOMMU (ExoSeal)

`ensure_domain_for_pid()` et `release_domain_for_pid()` sont appelés dans les syscalls DMA (`syscall/table.rs:4498,4548,4672,4690`) et dans `process/lifecycle/exit.rs` via `driver_do_exit()`. Le cleanup IOMMU à l'exit est robuste.

### 4.4 CVE-2012-0217 (SYSRETQ)

La vérification canonique de RCX avant `sysretq` est présente dans `arch/x86_64/syscall.rs:268-290`, avec fallback `iretq` sur adresse non-canonique. Correctement implémenté.

### 4.5 SECURITY_READY (CVE-EXO-001)

Spin-wait dans `arch/x86_64/smp/init.rs:173` et `ipc/capability_bridge/check.rs:23,36,45` — protège bien la fenêtre de race SMP au boot.

---

## 5. Synthèse et Priorités de Remédiation

### Priorité P0 — Bloquant pour v0.2.0

| ID | Module | Action |
|---|---|---|
| GAP-01 | Tous les modules d'accès (`fs/`, `process/`, `syscall/handlers/`) | Ajouter `security::verify_access()` aux chemins `open/read/write/exec/mmap` |
| GAP-02 | `syscall/dispatch.rs` ou `fast_path.rs` | Appeler `audit_syscall_entry()` avant dispatch, `audit_syscall_exit()` après |
| GAP-03 | `servers/ipc_router/src/exocordon.rs` | Aligner le DAG sur les 92 paires de `ipc_policy.rs` ; ajouter ExoShield, Scheduler, Tty, Input, Fb, Ps2 |
| GAP-04 | `servers/memory_server/src/mmap_service.rs` | Valider un `CapToken` (type `MEMORY_ALLOC` ou `SHM_CREATE`) avant toute opération cross-PID |
| GAP-05 | `servers/scheduler_server/src/main.rs` | Exiger `CapToken` de type `SCHED_RT` pour `REALTIME_ADMIT` et `SET_PRIORITY` vers classe RT |
| GAP-06 | `servers/vfs_server/src/` | Ajouter `check_access()` (ou un wrapper POSIX-capability) sur les handlers `open/read/write` |

### Priorité P1 — Important pour stabilité sécurité

| ID | Action |
|---|---|
| GAP-07 | Brancher `exokairos::ttl_for_right()` sur la création des CapTokens IPC et fs ; brancher `exoveil::revoke_domain()` dans `process_exit_cleanup()` |
| GAP-08 | Ajouter `exo_ledger_append()` dans : process spawn, process exit, capability create/revoke, driver claim, ExoShield quarantine |
| GAP-09 | Remplacer le stub `may_exec()` dans le loader par un appel à `security::verify_module_signature()` |

### Priorité P2 — Amélioration

| ID | Action |
|---|---|
| GAP-10 | Ajouter `CAP_DEVICE_CLAIM` vérifié dans `device_server/claim_validator.rs` |
| BONUS | Activer `audit_syscall_entry()` sur les syscalls à risque (`mmap`, `execve`, `clone`, `SYS_IPC_SEND`) avant l'activation globale |

---

## 6. Schéma de Propagation Actuel vs Cible

```
ÉTAT ACTUEL
═══════════
kernel/security/
   ├─ capability/  ──────────────────┬─→ ipc/capability_bridge/  ✅
   ├─ access_control/check_access() ─┤─→ (ipc seulement)         ⚠️
   ├─ exocage/     ─────────────────→┤─→ IDT + TCB spawn          ✅
   ├─ exoseal/IOMMU ────────────────→┤─→ syscall DMA + exit       ✅
   ├─ ipc_policy/  ────────────────→─┤─→ SYS_IPC_SEND             ✅
   ├─ zero_trust/  ─────────────────→┤─→ (MORT — 0 appelant)      ❌
   ├─ audit/syscall ───────────────→─┤─→ (MORT — 0 appelant)      ❌
   ├─ exokairos/   ─────────────────→┤─→ (MORT — 0 appelant)      ❌
   └─ exoveil/     ─────────────────→┘─→ (MORT — 0 appelant)      ❌

servers/
   ├─ exo_shield/  ✅ (ipc_gate, caps, forensics)
   ├─ ipc_router/  ⚠️ (ExoCordon 5 arêtes seulement)
   ├─ crypto_server/ ⚠️ (CapToken sur key ops uniquement)
   ├─ memory_server/ ❌ AUCUNE SÉCURITÉ
   ├─ vfs_server/  ❌ AUCUNE SÉCURITÉ
   ├─ scheduler_server/ ❌ AUCUNE SÉCURITÉ
   └─ network_server/  ❌ AUCUNE SÉCURITÉ

CIBLE v0.2.0
════════════
   ├─ zero_trust/verify_access() → fs/ + process/ + syscall/handlers/
   ├─ audit_syscall_entry/exit() → syscall/dispatch.rs
   ├─ exokairos TTL             → CapToken IPC + fs create
   ├─ exoveil revoke            → process exit cleanup
   ├─ memory_server             → CapToken MEMORY_ALLOC
   ├─ vfs_server                → CapToken FILE_READ/WRITE
   ├─ scheduler_server          → CapToken SCHED_RT
   └─ ipc_router ExoCordon      → 92 arêtes (sync avec ipc_policy.rs)
```

---

*Rapport généré par analyse statique directe du dépôt — juin 2026*
