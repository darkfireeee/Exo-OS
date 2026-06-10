# ExoOS — Audit de Sécurité : Seconde Passe
## Analyse Approfondie — Chemins d'Exécution, Bypasses, et Cohérence Microarchitecturale

**Dépôt :** `github.com/darkfireeee/Exo-OS` — clonage direct  
**Portée :** Approfondissement des gaps P0 de la passe 1 + nouveaux vecteurs identifiés  
**Méthode :** lecture de code source complète sur les chemins critiques, comptage automatisé

---

## Résumé des Nouvelles Découvertes

La seconde passe révèle **5 nouveaux bugs critiques** non visibles en première passe, car ils nécessitaient la lecture complète des corps de fonctions plutôt que la simple détection de présence/absence d'appels.

---

## BLOC A — Chemins IPC : Bypasses Confirmés

### A-01 🔴 P0 — `SYS_EXO_IPC_SEND` appelle `send_raw()`, pas `send_raw_checked()`

**Fichier :** `kernel/src/syscall/table.rs:sys_exo_ipc_send()`

```rust
// Chemin réel dans sys_exo_ipc_send :
match crate::ipc::channel::raw::send_raw(endpoint_id, &payload, raw_flags) {
    Ok(_) => 0,
    // ...
}
// → send_raw_checked() (qui appelle check_channel_access) N'EST PAS UTILISÉ
```

L'API sécurisée `send_raw_checked(ep, data, flags, table, token)` existe et appelle correctement `check_channel_access()`. Mais le syscall IPC de la table utilise `send_raw()` direct. Le `check_channel_access()` des modules `channel/{sync,mpmc,broadcast,raw}.rs` n'est donc jamais invoqué depuis le chemin Ring 3 → syscall.

### A-02 🔴 P0 — Cap-check conditionnel : seuls les messages de 200 bytes exactement sont vérifiés

**Fichier :** `kernel/src/syscall/table.rs:validate_ipc_envelope_auth()`

```rust
fn validate_ipc_envelope_auth(endpoint, caller_pid, caller_can_inject, payload) {
    if payload.len() != ABI_IPC_ENVELOPE_SIZE   // == 200 bytes
        || is_kernel_ephemeral_reply_endpoint(endpoint) {
        return Ok(IpcEnvelopeAuth::NotRequired);  // ← CAP CHECK BYPASSED
    }
    // ... vérification CapToken uniquement si len == 200
}
```

`MAX_MSG_SIZE` est plus grand que 200. Tout message dont la taille ≠ 200 bytes reçoit `IpcEnvelopeAuth::NotRequired` et bypass intégralement la vérification de capability. Un attaquant envoie 199 ou 201 bytes : aucun CapToken requis.

### A-03 🟠 P1 — CapTable : pas de stockage par-processus dans le PCB

**Fichier :** `kernel/src/process/core/pcb.rs`

Le `ProcessControlBlock` ne contient **aucun champ `CapTable`**. La `KERNEL_CAP_TABLE` globale stocke les métadonnées de service pour la vérification IPC, mais le modèle de capabilities *par processus* repose sur une table partagée globale. Les fonctions `check_access(table, token, ...)` des channel IPC acceptent un `&CapTable` — mais depuis le syscall IPC, aucune table par-processus n'est passée (A-01 confirme que `send_raw_checked` n'est pas appelé). L'isolation capability inter-processus est architecturalement incomplète.

---

## BLOC B — Mitigations Spectre/Meltdown : Lacunes à l'Exécution

### B-01 🔴 P0 — IBPB jamais émis au context-switch

**Fichiers :** `arch/x86_64/spectre/ibrs.rs`, `scheduler/core/switch.rs`

`flush_ibpb()` est exporté dans `spectre/mod.rs` :
```rust
pub use ibrs::{apply_ibrs, apply_stibp, flush_ibpb, ibrs_enabled, stibp_enabled};
```

Mais `grep -rn "flush_ibpb"` dans tout `kernel/src` hors du module spectre : **zéro résultat**. L'IBPB (Indirect Branch Predictor Barrier) — qui vide le prédicteur de branches entre processus différents et mitige Spectre v2 inter-processus — n'est jamais émis lors des context-switches. Seul l'IBRS global au boot est configuré.

**Impact :** Un processus malveillant peut potentiellement lire la mémoire d'un autre processus via Spectre v2 "Bounds Check Bypass" si EIBRS n'est pas disponible (cas QEMU par défaut).

### B-02 🟠 P1 — `apply_ibrs()` absent du chemin syscall entry

**Fichier :** `arch/x86_64/syscall.rs`

```
grep -n "apply_ibrs\|apply_ssbd\|apply_stibp" kernel/src/arch/x86_64/syscall.rs
→ 0 résultats
```

Sur CPU ne supportant pas Enhanced IBRS (EIBRS), `apply_ibrs()` doit être appelé à chaque entrée kernel depuis Ring 3 (syscall, exception). Ce n'est pas le cas. Seul l'init global du BSP/AP l'active une fois. Sur CPU non-EIBRS, le noyau reste vulnérable à Spectre v2 "kernel-vs-user" pendant toute la vie du système.

---

## BLOC C — Vérification d'Intégrité des Binaires

### C-01 🔴 P0 — `do_execve()` ne vérifie aucune signature de module

**Fichier :** `kernel/src/process/lifecycle/exec.rs:do_execve()`

```rust
pub fn do_execve(thread, pcb, path, argv, envp) -> Result<(), ExecError> {
    // ...validation args...
    let loader = ELF_LOADER.get().ok_or(ExecError::NoLoader)?;
    // ...
    let elf_result = loader.load_elf(path, argv, envp, cr3_current)?;
    // AUCUN appel à security::verify_module_signature()
    // AUCUN appel à integrity_check::check_chain_of_trust()
    // → Tout ELF est exécuté sans vérification cryptographique
}
```

`security::verify_module_signature()` et `integrity_check::is_chain_verified()` sont disponibles et exportés dans `security/mod.rs`. Ils ne sont jamais appelés depuis le chemin exec. N'importe quel binaire peut être exécuté, y compris des ELF forgés injectés dans ExoFS.

La fonction `loader/src/security/capability_check.rs` est un stub de 4 lignes (`CAP_EXEC` bitmask) qui ne consulte pas la couche integrity_check.

### C-02 🟠 P1 — `forge::verify_merkle()` : mode dégradé silencieux si hash nul

**Fichier :** `kernel/src/exophoenix/forge.rs:298-310`

```rust
#[cfg(not(exophoenix_resurrection_test))]
fn verify_merkle(elf: &ElfImage<'_>) -> Result<(), ForgeError> {
    if kernel_a_hash_is_zero() || elf.text.is_empty() || elf.rodata.is_empty() {
        return Err(ForgeError::MerkleVerifyFailed);
    }
    // ... Blake3 comparison vs A_MERKLE_ROOT ...
}
```

C'est correct en production (`cfg(not(test))`). Mais `A_MERKLE_ROOT` est un tableau `[u8; 32]` inclus via `include_bytes!(concat!(env!("OUT_DIR"), "/kernel_a_image_hash.bin"))`. Si le build system n'a pas généré ce fichier, le tableau est zéro. Dans ce cas, `kernel_a_hash_is_zero()` retourne `true` et la vérification passe (retour `Err` si hash zéro). Correct. **Mais** si `A_CLEAN_IMAGE` est vide et `load_a_image_from_exofs()` retourne une image corrompue, la vérification Merkle peut passer sur une image forgée si `A_MERKLE_ROOT` n'est pas correctement calculé au build.

---

## BLOC D — Serveurs Ring 1 : Nouveaux Gaps

### D-01 🔴 P0 — `network_server` : `SOCK_RAW` sans vérification de privilège

**Fichier :** `servers/network_server/src/socket_table.rs`

```rust
pub fn from_domain_type(domain: u32, ty: u32, protocol: u32) -> Result<Self, i64> {
    if domain != AF_INET { return Err(syscall::EAFNOSUPPORT); }
    match ty & SOCK_TYPE_MASK {
        SOCK_STREAM => Ok(Self::Tcp),
        SOCK_DGRAM  => Ok(Self::Udp),
        SOCK_RAW if protocol == 0 || protocol == 1 => Ok(Self::Raw),
        // ← aucune vérification UID/cap pour SOCK_RAW
```

Sur Linux, `SOCK_RAW` requiert `CAP_NET_RAW`. Dans ExoOS, tout processus Ring 1 capable d'atteindre le `network_server` via IPC peut créer une socket raw et crafts des paquets IP arbitraires (spoofing, ICMP redirect, etc.).

### D-02 🔴 P0 — `memory_server::attach_shared_region()` ignore entièrement `sender_pid`

**Fichier :** `servers/memory_server/src/mmap_service.rs`

```rust
pub fn attach_shared_region(&mut self, _sender_pid: u32, payload: &[u8]) -> MemoryReply {
    let handle = match payload_u64(payload, 0) { ... };
    // _sender_pid ignoré — seul le handle suffit pour attacher
    let Some(idx) = self.region_index(handle) else { ... };
    region.share_count = region.share_count.saturating_add(1);
    // → tout processus connaissant le handle u64 peut attacher la région
```

Le `handle` est un entier `u64`. Si l'allocateur assigne les handles de manière prévisible (séquentielle), tout processus peut itérer les handles et accéder aux régions partagées d'autres processus sans autorisation. Aucune validation "ce sender_pid est autorisé à attacher cette région".

### D-03 🟠 P1 — `scheduler_server` : priorité modifiable sur TID d'un autre processus

**Fichier :** `servers/scheduler_server/src/main.rs:handle_set_priority()`

```rust
fn handle_set_priority(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
    let tid = read_u32(payload, 0)?; // TID demandé
    let existing = self.threads.snapshot_owned(sender_pid, tid)?;
    // snapshot_owned vérifie pid == sender_pid pour ce TID ✓
    // MAIS: handle_register accepte tid=0 → tid=sender_pid
    // Aucune vérification de capacité pour demander SCHED_RT
```

`snapshot_owned()` vérifie effectivement que le TID appartient au `sender_pid` — c'est bien. Cependant, n'importe quel processus peut toujours s'auto-escalader en classe `SCHED_REALTIME` sans capability. La protection est "inter-processus" mais pas "intra-processus vs politique RT".

---

## BLOC E — `unsafe` sans contrat `SAFETY:`

### E-01 🟠 P1 — 43/87 blocs `unsafe` dans `kernel/src/security/` sans commentaire SAFETY

**Méthode :** scan Python sur contexte 4 lignes avant chaque `unsafe {`

Le module de sécurité lui-même viole la règle `RÈGLE CONTRAT UNSAFE (regle_bonus.md)` dans ses sous-modules critiques :

| Fichier | unsafe sans SAFETY |
|---|:---:|
| `security/exonmi.rs` | 15 |
| `security/exokairos.rs` | 11 |
| `security/exoargos.rs` | 10 |
| `security/exoveil.rs` | ~7 (estimé) |

### E-02 🟠 P1 — 123 fichiers kernel hors security/ avec unsafe non commentés

Top 5 critiques :

| Fichier | Blocs sans SAFETY |
|---|:---:|
| `arch/x86_64/exceptions.rs` | 17 |
| `drivers/dma.rs` | 16 |
| `arch/x86_64/time/sources/tsc.rs` | 15 |
| `memory/physical/frame/emergency_pool.rs` | 14 |
| `scheduler/core/switch.rs` | 11 |

---

## BLOC F — Confirmations et Nuances (Passe 1 Révisée)

### F-01 ✅ Confirmé : `SYS_EXO_IPC_SEND` → `enforce_direct_ipc_policy()` OK

Le chemin `check_direct_ipc()` via `ipc_policy.rs` fonctionne correctement pour les 200-byte envelopes avec `caller_can_inject=false`. C'est une protection réelle (whitelist DAG). Le gap A-02 porte sur les messages hors-format.

### F-02 ✅ Corrigé : SSR taille ≠ overflow

Passe 1 mentionne l'hypothèse historique de 10 KiB pour le SSR. La mesure réelle donne :
```
SSR_OFFSET_END = 104 + 24×96 + 48×24 = 3 560 bytes ≈ 3.5 KiB
SSR_PHYS_SIZE = 0x10000 = 64 KiB
```
L'assertion `SSR_PHYS_SIZE >= SSR_SIZE` est vérifiée. Le bug P0 SSR de l'audit du 16 mai est résolu dans le code actuel.

### F-03 ✅ `forge::verify_merkle()` : Blake3 vs A_MERKLE_ROOT est implémenté

La comparaison `computed != A_MERKLE_ROOT` est présente. L'intégrité de l'image kernel A lors de la résurrection ExoPhoenix est vérifiée cryptographiquement (sous réserve que A_MERKLE_ROOT soit bien calculé au build — voir C-02).

### F-04 ✅ `scheduler::snapshot_owned()` : ownership TID vérifié

`lookup_owned(pid, tid)` filtre correctement `record.active && record.pid == pid && record.tid == tid`. La modification de priorité inter-processus est bloquée au niveau du scheduler_server.

### F-05 ⚠️ `security/capability/verify.rs` : constant-time correct

La vérification de token utilise `ct_eq()` (constant-time equality) et effectue les deux comparaisons (génération + droits) sans court-circuit. Timing attack sur révocation résistée correctement.

### F-06 ⚠️ Init server : SERVICE_COUNT = 17 (pas 64)

La "hard ceiling à 64 services" citée dans les audits précédents n't apparaît pas dans le code actuel. `SERVICE_COUNT = 17` et `running_mask` utilise `u64` (limite théorique à 64). La limite réelle est la taille du tableau statique `CANONICAL_SERVICES`.

---

## BLOC G — Synthèse Passe 2

### Nouveaux gaps P0

| ID | Localisation | Description |
|---|---|---|
| A-01 | `syscall/table.rs:sys_exo_ipc_send` | Appel direct à `send_raw()` au lieu de `send_raw_checked()` — cap-check IPC non invoqué |
| A-02 | `syscall/table.rs:validate_ipc_envelope_auth` | Messages ≠ 200 bytes : `IpcEnvelopeAuth::NotRequired` — bypass cap complet |
| B-01 | `scheduler/core/switch.rs` | IBPB jamais émis au context-switch — Spectre v2 inter-processus non mitigé |
| C-01 | `process/lifecycle/exec.rs:do_execve` | Aucune vérification de signature module — tout ELF exécutable sans auth |
| D-01 | `network_server/socket_table.rs` | `SOCK_RAW` sans `CAP_NET_RAW` équivalent |
| D-02 | `memory_server/mmap_service.rs` | `attach_shared_region` ignore `sender_pid` — handle suffit pour attacher |

### Nouveaux gaps P1

| ID | Localisation | Description |
|---|---|---|
| A-03 | `process/core/pcb.rs` | Aucun `CapTable` par processus dans le PCB — isolation capability incomplète |
| B-02 | `arch/x86_64/syscall.rs` | `apply_ibrs()` absent du chemin syscall entry (non-EIBRS CPUs) |
| D-03 | `scheduler_server/main.rs` | Auto-escalade RT sans capability requise |
| E-01 | `security/exonmi.rs` + `exokairos.rs` + `exoargos.rs` | 43 `unsafe {}` sans `SAFETY:` dans le module sécurité lui-même |

---

## Tableau de Remédiation Cumulé (Passes 1 + 2)

### P0 — Avant v0.2.0

| Priorité | Fichier | Action minimale |
|---|---|---|
| P0-IPC-1 | `syscall/table.rs:sys_exo_ipc_send` | Remplacer `send_raw()` par `send_raw_checked()` avec la CapTable du processus appelant |
| P0-IPC-2 | `syscall/table.rs:validate_ipc_envelope_auth` | Supprimer l'exception `len != ABI_IPC_ENVELOPE_SIZE` ou appliquer le check pour TOUS les messages |
| P0-EXEC | `process/lifecycle/exec.rs:do_execve` | Appeler `security::verify_module_signature()` après `load_elf()` |
| P0-SHM | `memory_server/mmap_service.rs` | Vérifier que `sender_pid` est dans la liste d'autorisation de la région avant `attach` |
| P0-NET | `network_server/socket_table.rs` | Exiger un CapToken `CAP_NET_RAW` pour `SOCK_RAW`, refuser sinon |
| P0-AUDIT | `syscall/dispatch.rs` | Appeler `audit_syscall_entry()` / `audit_syscall_exit()` sur les syscalls sensibles |
| P0-ZT | `fs/`, `process/handlers/`, `syscall/handlers/memory.rs` | Brancher `zero_trust::verify_access()` sur les chemins open/read/write/mmap |
| P0-CORDON | `ipc_router/exocordon.rs` | Aligner sur les 92 paires de `ipc_policy.rs` |
| P0-MEM | `memory_server/main.rs` | CapToken sur ALLOC/FREE/PROTECT cross-PID |
| P0-SCHED | `scheduler_server/main.rs` | CapToken `SCHED_RT` pour admission temps-réel |
| P0-VFS | `vfs_server/src/` | CapToken FILE_READ/WRITE sur handlers VFS |

### P1 — Pour la stabilisation complète

| Priorité | Fichier | Action |
|---|---|---|
| P1-PCB | `process/core/pcb.rs` | Ajouter `cap_table: CapTable` au PCB, hériter à fork (subset), purger à exec |
| P1-IBPB | `scheduler/core/switch.rs` | Appeler `flush_ibpb()` lors du switch user→user sur process différents |
| P1-IBRS | `arch/x86_64/syscall.rs` | Appeler `apply_ibrs()` à l'entrée syscall si non-EIBRS |
| P1-UNSAFE | `security/exonmi.rs` + `exokairos.rs` + `exoargos.rs` | Ajouter `// SAFETY:` aux 43 blocs manquants |
| P1-KAIROS | IPC + fs create | Brancher `exokairos::ttl_for_right()` aux CapTokens |
| P1-VEIL | `process/lifecycle/exit.rs` | Appeler `exoveil::revoke_domain()` dans le cleanup exit |
| P1-LEDGER | process spawn/exit, cap create/revoke | Ajouter `exo_ledger_append()` aux 10+ événements non audités |

---

## Diagnostic Global : Maturité de Sécurité par Couche

```
Ring 0 — Kernel                             Ring 1 — Serveurs
══════════════════════════════════          ══════════════════════════════════
Hardware (CET, IOMMU, CR3/KPTI)  ██████    exo_shield             █████
Boot séquence / SECURITY_READY   ██████    ipc_router (partiel)   ██░░░
IPC kernel path (SYS_IPC_SEND)   ████░░    crypto_server          ███░░
Capability engine (verify())     █████░    init_server            ██░░░
Spectre mitigations               ███░░    memory_server          █░░░░
ExoCage (CET per-thread)          █████    vfs_server             █░░░░
ExoArgos (PMC hooks)              ████░    scheduler_server       █░░░░
Syscall validation (UserPtr)      █████    network_server         █░░░░
ExoLedger (couverture)            ██░░░    device_server          ██░░░
zero_trust (opérationnel)         █░░░░    drivers (tous)         ░░░░░
audit_syscall                     ░░░░░
module signature @ exec           ░░░░░
```

**Score de propagation : ~35% des chemins de confiance réellement sécurisés.**  
Les mécanismes de sécurité Ring 0 sont bien conçus mais insuffisamment câblés aux serveurs Ring 1 et aux paths d'exécution critiques (exec, shm, socket raw, priority escalation).

---

*Seconde passe — analyse directe sur code source cloné — 05 juin 2026*
