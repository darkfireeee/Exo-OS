# ExoOS v0.2.0 — Audit Sécurité (P1)
## Sous-systèmes de Sécurité : Activation Incomplète ou Incorrecte

**Auteur :** claude-beta  
**Date :** 2026-05-20  
**Sévérité :** P1 — Bloquant pour les critères Pilier 2 de VISION-V0.2.0  
**Checklist :** BLOC 2 (S-12, S-16), BLOC 11 (ES-01 à ES-04)

---

## SEC-01 — ExoCage non activé en Phase 2 (CORR-82 / S-03)

**Fichiers :**  
- `kernel/src/lib.rs` (séquence `kernel_init`)  
- `kernel/src/security/mod.rs` (fonction `security_init`)  
**Checklist :** S-03, S-07, S-08, S-09, S-10, S-11

### Situation actuelle

La checklist CORR-82 exige :

```
Phase 2 : ExoCage (CR4, MSR) — sans heap requis
Phase 3 : ExoNMI watchdog (LAPIC disponible)
Phase 5 : security_init() pour le reste
Phase 6 : ExoSeal verify_chain()
Phase 7 : ExoShield IOMMU (avant Ring1)
```

Mais dans `kernel/src/lib.rs`, la séquence réelle est :

```
Phase 2a : EmergencyPool
Phase 2b : Heap (SLUB + large)
Phase 2c : Time subsystem
Phase 2d : drivers::init() → iommu_init()  ← IOMMU ici (Phase 7 requis ?)
Phase 3  : scheduler::init() → runqueue_init_percpu()
Phase 4  : process::init() → cgroup::init()
Phase 5  : security_init() → [integrity → cap → crypto → mitigations →
                               audit → access_control → exoledger →
                               exokairos → exoargos → exonmi → ExoCage ←]
```

**ExoCage (CET Shadow Stack)** est initialisé comme avant-dernier élément
de `security_init()`, donc en **Phase 5**, pas en Phase 2 comme requis.
Entre la fin du boot et la Phase 5, des threads peuvent s'exécuter sans
protection CET.

### Note sur SMEP/SMAP

SMEP et SMAP sont activés correctement tôt via `memory::protection::init()`
dans `arch_boot_init()`. C'est conforme. **Le problème est spécifiquement
ExoCage (CET)**.

### Correction requise

```rust
// kernel/src/lib.rs — dans kernel_init(), après Phase 2b (heap prêt)

// ── Phase 2-CET : ExoCage (CET MSR) — avant tout thread utilisateur ──────
// SAFETY: heap disponible pour l'allocation de la shadow stack.
// CORR-82 : ExoCage doit précéder tout autre init qui peut créer des threads.
if crate::arch::x86_64::cpu::features::CPU_FEATURES.has_cet_ss() {
    crate::security::exocage::exocage_global_enable();
}
// kdb(b'C');
crate::arch::x86_64::boot_display::stage_ok("EXCAGE");
```

---

## SEC-02 — ExoKairos : pas de reset de fenêtre temporelle (ERR-07 / S-16)

**Fichier :** `kernel/src/security/exokairos.rs`  
**Checklist :** S-16, S-17, S-18

### Situation actuelle

Le module ExoKairos implémente un **budget monotone décroissant** :

```rust
// kernel/src/security/exokairos.rs

pub fn verify(&self, calls: u64, bytes: u64) -> Result<(), ExoKairosError> {
    // ...
    // 5. Vérifier le budget restant
    if calls_left == 0 { return Err(ExoKairosError::BudgetCallsExhausted); }
    // ...
}
```

Il n'existe **aucune fenêtre glissante, aucun mécanisme de reset**.
Une capability temporelle dont le budget est épuisé ne se recharge jamais.

### Ce que la checklist requiert

```
S-16 : ExoKairos : budget avec reset fenêtre (ERR-07)
S-17 : ExoKairos : throttle à 100% budget fenêtre
S-18 : ExoKairos : kill à 200% budget fenêtre
```

Le modèle attendu est une **fenêtre glissante** :
- Dans une fenêtre de `KAIROS_WINDOW_NS` nanosecondes, chaque process a
  un budget d'appels/volume.
- À la fin de la fenêtre, le budget se recharge (reset).
- À 100% du budget dans la fenêtre : throttle (ralentissement).
- À 200% (accumulation inter-fenêtres) : kill.

### Correction requise

```rust
// Dans TemporalCap ou dans le vérificateur global ExoKairos

pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000; // 1 seconde

// Ajout à la structure TemporalCap :
struct TemporalCap {
    // existant :
    calls_left: AtomicU64,
    bytes_left: AtomicU64,
    deadline: u64,
    // NOUVEAU (ERR-07) :
    window_start_ns: AtomicU64,   // début de la fenêtre courante
    window_budget_calls: u64,      // budget initial par fenêtre
    window_budget_bytes: u64,
    window_used_calls: AtomicU64,  // consommation dans la fenêtre
    window_used_bytes: AtomicU64,
}

impl TemporalCap {
    fn check_and_advance_window(&self, now_ns: u64) {
        let ws = self.window_start_ns.load(Ordering::Acquire);
        if now_ns.saturating_sub(ws) >= KAIROS_WINDOW_NS {
            // Reset fenêtre
            self.window_start_ns.store(now_ns, Ordering::Release);
            self.window_used_calls.store(0, Ordering::Release);
            self.window_used_bytes.store(0, Ordering::Release);
        }
    }

    fn throttle_or_kill(&self, used: u64, budget: u64) -> Result<(), ExoKairosError> {
        if used >= budget * 2 {
            return Err(ExoKairosError::KillThresholdExceeded); // S-18 : 200%
        }
        if used >= budget {
            // S-17 : throttle 100% → yield ou sleep court
            crate::scheduler::yield_current();
        }
        Ok(())
    }
}
```

---

## SEC-03 — Zero Trust : absence de fast path Ring1↔Ring1 (ERR-09 / S-12)

**Fichier :** `kernel/src/security/zero_trust/` (tous les fichiers)  
**Checklist :** S-12

### Situation actuelle

Le module `zero_trust/verify.rs` fournit `verify_access()` qui effectue
une vérification complète (label MLS, Bell-LaPadula, Biba) pour **chaque**
appel IPC. Il n'existe aucune distinction entre :
- IPC Ring1 ↔ Ring1 (servers de confiance entre eux)
- IPC Ring3 → Ring1 (appel utilisateur vers serveur)

### Ce que la checklist requiert

```
S-12 : Zero Trust : fast path bitmask Ring1↔Ring1 (ERR-09)
S-13 : Zero Trust : slow path complet Ring3→Ring1
```

Le fast path attendu : si les deux PID appartiennent à des services
Ring1 connus (bitmask de services de confiance), court-circuiter la
vérification MLS complète et retourner `Ok` directement.

### Impact

Sans ce fast path, chaque IPC entre serveurs Ring1 (ex: vfs_server →
crypto_server → ipc_router) traverse la chaîne complète de vérification
Zero Trust. À haute fréquence (>100k IPC/s entre serveurs), cela représente
une surcharge significative non acceptable pour les objectifs de performance.

### Correction requise

```rust
// kernel/src/security/zero_trust/verify.rs

/// Bitmask des PIDs Ring1 de confiance (mis à jour au démarrage par init_server)
static RING1_TRUSTED_MASK: AtomicU64 = AtomicU64::new(0);

pub fn register_ring1_pid(pid: u32) {
    // pid <= 63 pour le bitmask simple (sinon table étendue)
    if pid < 64 {
        RING1_TRUSTED_MASK.fetch_or(1u64 << pid, Ordering::Release);
    }
}

pub fn verify_ipc_access(sender: PrincipalId, receiver: PrincipalId, ...) 
    -> Result<(), AccessError> 
{
    // FAST PATH ERR-09 : Ring1↔Ring1
    let mask = RING1_TRUSTED_MASK.load(Ordering::Acquire);
    if sender.pid() < 64 && receiver.pid() < 64 {
        let sender_trusted = (mask >> sender.pid()) & 1 == 1;
        let recv_trusted   = (mask >> receiver.pid()) & 1 == 1;
        if sender_trusted && recv_trusted {
            return Ok(()); // court-circuit, 1-2 cycles
        }
    }

    // SLOW PATH : vérification MLS complète pour Ring3→Ring1
    verify_access_full(sender, receiver, ...)
}
```

---

## SEC-04 — exo_shield/lib.rs : 5 modules présents sur disque mais non déclarés (CORR-75 / ES-01)

**Fichier :** `servers/exo_shield/src/lib.rs`  
**Checklist :** ES-01, ES-02, ES-03, ES-04

### Situation actuelle

```rust
// servers/exo_shield/src/lib.rs — état actuel

#![no_std]

pub mod behavioral;
pub mod engine;
pub mod ipc_gate;
pub mod signatures;
```

Les répertoires suivants **existent sur disque** avec du code mais ne sont
pas déclarés dans `lib.rs` :

```
servers/exo_shield/src/hooks/       ← syscall_hooks, exec_hooks, memory_hooks, net_hooks
servers/exo_shield/src/sandbox/     ← container, syscall_filter, net_isolation, fs_restriction
servers/exo_shield/src/network/     ← firewall, ids, dns_guard, traffic_analysis
servers/exo_shield/src/ml/          ← (non listé mais répertoire présent)
servers/exo_shield/src/forensics/   ← (non listé mais répertoire présent)
```

De plus, `main.rs` n'importe que `behavioral`, `engine`, `ipc_gate`,
`signatures` depuis la lib — les 5 modules fantômes sont complètement
inaccessibles au binaire.

### Impact

- `exo_shield` ne peut pas réaliser de containment réel (sandbox inexistante)
- Les hooks syscall/exec/memory ne sont pas branchés
- Le firewall réseau et l'IDS ne sont pas actifs
- YARA scanning limité aux patterns de `signatures` seulement

### Correction requise

```rust
// servers/exo_shield/src/lib.rs — version corrigée

#![no_std]

pub mod behavioral;
pub mod engine;
pub mod forensics;   // ← AJOUT ES-01
pub mod hooks;       // ← AJOUT ES-01
pub mod ipc_gate;
pub mod ml;          // ← AJOUT ES-01
pub mod network;     // ← AJOUT ES-01
pub mod sandbox;     // ← AJOUT ES-01
pub mod signatures;
```

```rust
// servers/exo_shield/src/main.rs — ajouter dans _start() (ES-02)

unsafe fn _start() -> ! {
    hooks::init();        // ← ES-02
    sandbox::init();      // ← ES-02
    network::init();      // ← ES-02
    ml::init();           // ← ES-02
    forensics::init();    // ← ES-02
    // ... boucle IPC existante ...
}
```

```rust
// Dans handle_event_report() (ES-03)
fn handle_event_report(req: &EventReport) -> ShieldResponse {
    let score = engine::core::score_event(req);
    hooks::on_event(req);          // ← ES-03 : brancher les hooks
    network::analyze_event(req);   // ← ES-03
    // ...
}

// Dans handle_quarantine_cmd() (ES-04)
fn handle_quarantine_cmd(req: &QuarantineCmd) -> ShieldResponse {
    sandbox::contain(req.pid)?;    // ← ES-04 : containment réel, pas no-op
    // ...
}
```

---

## Récapitulatif P1 — Sécurité

| ID | Fichier | Problème | Checklist |
|---|---|---|---|
| SEC-01 | `kernel/src/lib.rs` + `security/mod.rs` | ExoCage en Phase 5 au lieu de Phase 2 | S-03 à S-11 |
| SEC-02 | `kernel/src/security/exokairos.rs` | Pas de reset fenêtre temporelle | S-16, S-17, S-18 |
| SEC-03 | `kernel/src/security/zero_trust/verify.rs` | Pas de fast path Ring1↔Ring1 | S-12 |
| SEC-04 | `servers/exo_shield/src/lib.rs` | 5 modules présents non déclarés | ES-01 à ES-04 |

---

*claude-beta — ExoOS v0.2.0 Audit — AUDIT-SECURITE.md*
