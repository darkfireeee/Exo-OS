# AUDIT-SECURITY-ARCHITECTURE — Audit Complet de la Sécurité ExoOS
## exo_shield + kernel/security — Problèmes, Placements, Potentiel Inexploité

**Auteur :** claude-alpha  
**Date :** 2026-05-15  
**Sources auditées :** `exo_shield/` (serveur Ring1) + `security/` (kernel Ring0)  
**Statut :** DOCUMENT FONDATEUR — À lire avant tout travail sur la sécurité

---

## 1. Résumé Exécutif

Après lecture complète du code réel des deux archives, le constat est sans appel :

**ExoOS possède une des architectures de sécurité les plus avancées qui existent en Rust bare-metal.** Le problème n'est pas la qualité du code — il est excellent — c'est que **5 modules entiers du serveur `exo_shield` sont orphelins** (compilés mais jamais exposés ni appelés), et que **plusieurs composants de sécurité du kernel ne sont pas reflétés dans les specs existantes**.

Résumé des problèmes critiques :

| Problème | Gravité | Impact |
|----------|---------|--------|
| 5 modules `exo_shield` non exportés dans `lib.rs` | **CRITIQUE** | hooks, sandbox, network, ml, forensics = dead code |
| `main.rs` n'appelle pas les inits des 5 modules orphelins | **CRITIQUE** | Zéro hook actif au runtime |
| YARA engine limité à patterns de 8 bytes max | **ÉLEVÉE** | Détection de malware sévèrement bridée |
| `ExoArgos` (PMC monitoring) absent des specs | **ÉLEVÉE** | Composant unique ignoré dans la documentation |
| `Pledge` style OpenBSD non documenté | **MOYENNE** | Isolation des processus non utilisée |
| `Bell-LaPadula + Biba` MLS non documenté | **MOYENNE** | Modèle zero-trust sous-exploité |
| `ipc_policy` ServiceClass non documenté | **MOYENNE** | Politique IPC inter-serveurs non spécifiée |
| `security_init` a 13 étapes, spec en décrivait 10 | **FAIBLE** | Désalignement doc/code |

---

## 2. Problème Critique #1 — Les 5 Modules Orphelins de `exo_shield`

### 2.1 Le Bug de Placement

Le fichier `lib.rs` d'`exo_shield` n'exporte que 4 modules :

```rust
// exo_shield/src/lib.rs — ÉTAT ACTUEL (INCORRECT)
#![no_std]

pub mod behavioral;   // ✅ exporté
pub mod engine;       // ✅ exporté
pub mod ipc_gate;     // ✅ exporté
pub mod signatures;   // ✅ exporté
```

**Mais le répertoire `src/` contient 9 modules :**

```
src/
├── behavioral/    ✅ dans lib.rs
├── engine/        ✅ dans lib.rs
├── ipc_gate/      ✅ dans lib.rs
├── signatures/    ✅ dans lib.rs
├── hooks/         ❌ ABSENT de lib.rs — MORT
├── sandbox/       ❌ ABSENT de lib.rs — MORT
├── network/       ❌ ABSENT de lib.rs — MORT
├── ml/            ❌ ABSENT de lib.rs — MORT
└── forensics/     ❌ ABSENT de lib.rs — MORT
```

**Conséquence :** Ces 5 modules compilent séparément (rustc les voit), mais ils ne sont jamais liés au binaire `exo-shield`. Le serveur tourne sans hooks, sans sandbox, sans IDS réseau, sans ML, sans forensics.

### 2.2 Ce que Contiennent Ces Modules (Pertes Réelles)

**`hooks/` — Interception des événements**
```
hooks/exec_hooks.rs     → Interception exec(), détection de chaînes malveillantes
hooks/net_hooks.rs      → Monitoring connexions, détection port-scan, exfiltration DNS
hooks/memory_hooks.rs   → Surveillance allocations, détection overflow/UAF
hooks/syscall_hooks.rs  → Fréquence syscalls, séquences dangereuses
```
Sans `hooks/` : l'engine n'a aucune source d'événements live. Il fonctionne uniquement en mode "scan on demand" (quelqu'un lui envoie un SCAN_REQUEST IPC). Il ne détecte **rien** de manière proactive.

**`sandbox/` — Isolation des processus contenus**
```
sandbox/container.rs     → ContainerManager : cycle de vie des containments
sandbox/fs_restriction.rs → Restriction d'accès filesystem par politique
sandbox/net_isolation.rs  → Isolation réseau : filtrage IP/port/protocol
sandbox/syscall_filter.rs → Bitmap de syscalls autorisés par processus
```
Sans `sandbox/` : quand l'engine exécute `QUARANTINE_CMD / contain`, il marque le processus "contained" dans sa table interne — mais il n'applique **aucune restriction réelle** (pas de filtre syscall, pas d'isolation réseau, pas de restriction FS).

**`network/` — Sécurité réseau active**
```
network/firewall.rs         → Règles de firewall stateful
network/traffic_analysis.rs → Analyse de flux, détection de burst
network/dns_guard.rs        → Détection d'exfiltration DNS, DNS tunneling
network/ids.rs              → IDS avec signatures (jusqu'à 64 bytes de pattern)
```
Sans `network/` : le `network_server` smoltcp passe des paquets sans aucun filtrage par `exo_shield`.

**`ml/` — Détection comportementale par réseau de neurones**
```
ml/features.rs   → Extraction de 32 features depuis les données de processus
ml/model.rs      → Réseau de neurones 32→16, fixed-point Q16.16 (no f64)
ml/inference.rs  → Inférence par batch, classification Benign/Suspicious/Malicious
ml/update.rs     → Mise à jour des poids du modèle
```
Sans `ml/` : la détection comportementale dans `behavioral/` fonctionne uniquement par heuristiques simples. Le réseau de neurones est compilé mais jamais appelé.

**`forensics/` — Analyse post-incident**
```
forensics/memory_dump.rs → Stockage de dumps mémoire avec checksum CRC32
forensics/timeline.rs    → Reconstruction de timeline, corrélation d'événements
forensics/report.rs      → Génération de rapports forensiques sérialisables
```
Sans `forensics/` : quand un threat est détecté et résolu, il n'y a aucune trace exploitable pour l'analyse post-incident.

### 2.3 Fix Requis — lib.rs Corrigé

```rust
// exo_shield/src/lib.rs — CORRECT
#![no_std]

pub mod behavioral;
pub mod engine;
pub mod forensics;   // ← à ajouter
pub mod hooks;       // ← à ajouter
pub mod ipc_gate;
pub mod ml;          // ← à ajouter
pub mod network;     // ← à ajouter
pub mod sandbox;     // ← à ajouter
pub mod signatures;
```

### 2.4 Fix Requis — main.rs : Initialisation des Modules

```rust
// exo_shield/src/main.rs — section _start(), CORRECTION
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // AVANT (incomplet) :
    // ipc_gate::policy_init();
    // ipc_gate::audit_init();
    // engine::engine_init();
    // signatures::signatures_init();
    // behavioral::behavioral_init();

    // APRÈS (complet) :
    ipc_gate::policy_init();
    ipc_gate::audit_init();
    engine::engine_init();
    signatures::signatures_init();
    behavioral::behavioral_init();
    
    // Modules orphelins — maintenant initialisés
    hooks::exec_hooks::exec_hooks_init();
    hooks::net_hooks::net_hooks_init();
    hooks::memory_hooks::mem_hooks_init();
    hooks::syscall_hooks::syscall_hooks_init();
    
    sandbox::container::container_manager_init();  // si fonction existe
    
    network::ids::ids_init();                      // si fonction existe
    
    ml::model::model_init();                       // si fonction existe
    
    forensics::memory_dump::memory_dump_init();
    forensics::timeline::timeline_init();
    forensics::report::report_init();

    // ... reste du code inchangé
}
```

### 2.5 Fix Requis — Brancher les Hooks au Dispatch

Les hooks doivent être appelés depuis les handlers de messages existants :

```rust
// Dans handle_event_report() — APRÈS submit_event()
fn handle_event_report(req: &ShieldRequest) -> ShieldReply {
    // ... code existant ...
    let result = engine::submit_event(&event, tick);
    
    // NOUVEAU : router l'event vers le hook approprié
    match event.event_type {
        engine::EventType::Exec => {
            hooks::pre_exec_validate(event.pid, &req.payload[..]);
        }
        engine::EventType::Network => {
            hooks::pre_connect_check(event.pid, event.arg0, event.arg1 as u16);
        }
        engine::EventType::Memory => {
            hooks::pre_alloc_check(event.pid, event.arg0 as usize, event.arg1 as u32);
        }
        engine::EventType::Syscall => {
            hooks::pre_syscall_check(event.pid, event.opcode, event.arg0, event.arg1);
        }
        _ => {}
    }
    
    // ... suite inchangée
}

// Dans handle_quarantine_cmd() cmd=0 (contain) — APRÈS mark_process_contained()
fn handle_quarantine_cmd(req: &ShieldRequest) -> ShieldReply {
    // ... code existant cmd=0 ...
    let ok = engine::mark_process_contained(target_pid, tick);
    
    // NOUVEAU : appliquer les restrictions réelles
    if ok {
        // Appliquer le profil sandbox par défaut
        let profile = sandbox::ContainerProfile::default_quarantine();
        sandbox::container::apply_profile(target_pid, &profile);
        
        // Démarrer le dump forensique
        forensics::timeline::record_timeline_event(
            target_pid, 
            forensics::TimelineEventType::ContainmentStart,
            tick
        );
    }
    // ...
}
```

---

## 3. Problème Critique #2 — Le YARA Engine est Bridé par le Layout

### 3.1 Limitation Actuelle

Dans `signatures/yara.rs`, la structure `Condition` contient :

```rust
pub struct Condition {
    pub value: [u8; 8],   // ← MAXIMUM 8 BYTES DE PATTERN
    pub length: u8,        // ← donc max length = 8
    // ...
}
```

**Implication directe :** La signature d'un malware réel fait rarement moins de 8 bytes. Les signatures YARA professionnelles font couramment 16 à 256 bytes. Avec cette limitation, l'engine ne peut détecter que :
- Les magic bytes d'un format (4 bytes ELF = `\x7fELF`)
- De très petites chaînes (8 chars max)

Il **ne peut pas** détecter :
- Des shellcodes (séquences de 20+ bytes)
- Des indicateurs comportementaux complexes
- Des patterns avec wildcards au milieu
- Des expressions régulières

### 3.2 La Question yara-x

`yara-x` est la réécriture officielle en Rust de YARA par VirusTotal. Ses avantages :
- Patterns de taille arbitraire
- Regex, hex strings avec wildcards (`?? ?? 90`)
- Modules (PE, ELF, math, hash)
- Conditions booléennes complexes
- Nativement Rust

**Problème de compatibilité :** `yara-x` requiert `std` ou au minimum `alloc` avec un allocateur global. `exo_shield` est actuellement `#![no_std]` sans `alloc`.

**Solution :** `exo_shield` est un serveur Ring1 — il a accès à `exo-alloc` (snmalloc). Il peut activer `extern crate alloc` avec l'allocateur ExoOS.

```toml
# exo_shield/Cargo.toml — AJOUT
[dependencies]
spin.workspace = true
exo-syscall-abi = { path = "../syscall_abi" }
exo-alloc = { path = "../exo-alloc" }    # ← permet d'activer alloc
yara-x = { version = "0.x", default-features = false, features = ["no-std-compat"] }
# OU si yara-x n'a pas de feature no-std :
# → utiliser notre engine amélioré (voir SPEC-YARA-ENHANCED.md)
```

**Si yara-x ne supporte pas no_std/alloc uniquement :** Améliorer l'engine interne pour passer de patterns 8 bytes à patterns 64 bytes (coût minimal — changer `[u8; 8]` → `[u8; 64]`).

```rust
// signatures/yara.rs — FIX IMMÉDIAT sans dépendance externe
pub const MAX_PATTERN_LEN: usize = 64;  // ← était implicitement 8

pub struct Condition {
    pub cond_type: ConditionType,
    pub field: FieldType,
    pub offset: u16,
    pub length: u8,
    pub value: [u8; MAX_PATTERN_LEN],  // ← 8 → 64 bytes
    pub threshold: u64,
    pub logic_op: LogicOp,
    pub enabled: bool,
    _reserved: [u8; 3],
}
```

Avec 64 bytes de pattern, la plupart des signatures YARA réelles sont couvrable. yara-x reste l'objectif long terme (v0.3.0) quand `alloc` sera disponible dans `exo_shield`.

---

## 4. Composants Kernel Absents des Specs

### 4.1 ExoArgos — Détection de Side-Channel par PMC

`security/exoargos.rs` implémente un monitoring des Performance Monitoring Counters (PMC) Intel :

```
Compteurs surveillés :
  - IA32_FIXED_CTR0  : Instructions retired
  - IA32_FIXED_CTR1  : Core cycles unhalted  
  - IA32_PMC0        : MEM_LOAD_RETIRED.L3_MISS (cache misses L3)
  - IA32_PMC1        : BR_MISP_RETIRED (branch mispredictions)
  - TSC              : timestamp counter

DECEPTION_THRESHOLD = 3500 (fixedpoint = 0.35)

check_anomaly() → true si comportement PMC dévie > seuil par rapport à baseline
```

**Ce que ça détecte :**
- Attaques Spectre/Meltdown (pic de L3 misses + branch mispredictions)
- Timing attacks via cache (pattern de L3 misses anormal)
- Side-channel sur crypto (burst inhabituel de cycles/instructions)
- Deception attacks (exécution de code qui fait semblant d'être autre chose)

**Hook critique déjà câblé dans `mod.rs` :**
```rust
crate::scheduler::core::switch::install_context_switch_out_hook(
    exoargos_context_switch_snapshot,
);
```
À chaque context switch, ExoArgos capture un snapshot PMC. La **baseline** est établie sur les premiers N context switches du processus, puis chaque snapshot est comparé.

**Ce qui manque :** Le résultat de `check_anomaly()` n'est jamais envoyé à `exo_shield`. Le bridge kernel→exo_shield pour les anomalies PMC n'est pas câblé.

### 4.2 ExoVeil — Isolation Mémoire par PKS (Protection Keys for Supervisor)

`security/exoveil.rs` utilise PKS (Intel CET/PKS, disponible depuis Ice Lake) pour créer des domaines de mémoire kernel :
- Révocation O(1) de domaines entiers via `WRPKRU`
- Isolation entre sous-systèmes kernel (ex: crypto isolé du reste)
- Handoff sécurisé lors d'ExoPhoenix (révocation puis restauration)

### 4.3 Pledge — Réduction de Surface d'Attaque Style OpenBSD

`security/isolation/pledge.rs` implémente le mécanisme Pledge d'OpenBSD :
```
RÈGLE PLEDGE-01 : Un processus ne peut QUE retirer des pledges, jamais en ajouter
RÈGLE PLEDGE-02 : Violation → SIGKILL immédiat
RÈGLE PLEDGE-03 : init_server ne peut pas appeler pledge()
```

Pledges disponibles : `stdio`, `rpath`, `wpath`, `cpath`, `tmppath`, `inet`, `unix`, `dns`, `getpw`, `proc`, `exec`, `id`, `route`, `shm`, `signal`, `tty`...

**Ce n'est pas dans les specs**. Toute app POSIX installée via `exo compat install` devrait recevoir un `PledgeSet` adapté à ses besoins. `calendar` → `stdio | rpath | wpath | tmppath`. `curl` → `stdio | inet | dns | rpath`.

### 4.4 MLS Bell-LaPadula + Biba dans Zero Trust

`security/zero_trust/labels.rs` implémente des **étiquettes de sécurité MLS** (Multi-Level Security) :
- **Bell-LaPadula** : "no read up, no write down" (confidentialité)
- **Biba** : "no write up, no read down" (intégrité)

C'est le modèle de sécurité militaire classique. Il s'ajoute aux CapTokens pour donner un modèle dual : capability (ce qu'on peut faire) + label MLS (niveau de confiance de l'information).

### 4.5 CFG (Control Flow Guard) + SafeStack

`security/exploit_mitigations/cfg.rs` implémente un bitmap des cibles légitimes pour les appels indirects (registre/pointeur). Toute tentative d'appel indirect vers une cible non enregistrée → violation CFG → kill.

`security/exploit_mitigations/safe_stack.rs` implémente un SafeStack logiciel (fallback si CET absent) : la pile de retour est séparée de la pile de données.

### 4.6 ipc_policy — Contrôle de Flux IPC par ServiceClass

`security/ipc_policy.rs` gère une politique de flux IPC basée sur des classes de service :

```rust
pub enum ServiceClass {
    Unknown,
    InitServer,
    IpcBroker,
    MemoryServer,
    VfsServer,
    CryptoServer,
    NetworkServer,
    DeviceServer,
    ExoShield,
    UserApp,
    // ...
}
```

Chaque paire (src\_class, dst\_class) a une politique : Allowed / Denied / RequiresCap. Ce n'est pas documenté et pas intégré dans les specs des serveurs Ring1.

### 4.7 security_init — 13 Étapes (Pas 10)

La séquence réelle dans `mod.rs` :
```
0.  ExoSeal phase0  (CET + PKS default-deny + watchdog boot)
1.  integrity_check
2.  capability
3.  zero_trust      (lazy init)
4.  crypto
5.  isolation       (domaines, sandbox, pledge)
6.  exploit_mitigations (KASLR, canary, CFG, CET, SafeStack)
7.  audit
8.  access_control
9.  ExoLedger
10. ExoKairos       (kernel secret derivation depuis CSPRNG)
11. ExoArgos        (PMC init + context_switch hook)
12. ExoNmi          (watchdog armé)
12b. ExoCage per-thread (BSP)
13. ExoSeal complete (PKS ops normales + SECURITY_READY + watchdog final)
```

---

## 5. Tableau de Placement Correct des Composants

La question posée : **"Est-ce que exo_shield est dans le bon endroit ? Et security/ aussi ?"**

| Composant | Placement actuel | Placement correct | Justification |
|-----------|-----------------|-------------------|--------------|
| `security/capability/` | Ring0 kernel | ✅ Ring0 kernel | Gestion des tokens → doit être dans le TCB |
| `security/zero_trust/` | Ring0 kernel | ✅ Ring0 kernel | Vérification sur chaque IPC → fast path kernel |
| `security/crypto/` | Ring0 kernel | ✅ Ring0 kernel | RNG + crypto noyau (pas de clés user) |
| `security/audit/` | Ring0 kernel | ✅ Ring0 kernel | Logger ISR-safe → doit être en Ring0 |
| `security/exocage/` | Ring0 kernel | ✅ Ring0 kernel | CET MSR → Ring0 only |
| `security/exoveil/` | Ring0 kernel | ✅ Ring0 kernel | PKS WRPKRU → Ring0 only |
| `security/exoledger/` | Ring0 kernel | ✅ Ring0 kernel | P0 zone immuable → Ring0 |
| `security/exokairos/` | Ring0 kernel | ✅ Ring0 kernel | Budget temporel inline au scheduler |
| `security/exoargos/` | Ring0 kernel | ✅ Ring0 kernel | MSR PMU → Ring0 only |
| `security/exonmi/` | Ring0 kernel | ✅ Ring0 kernel | NMI handler → Ring0 |
| `security/exoseal/` | Ring0 kernel | ✅ Ring0 kernel | IOMMU statique → Ring0 |
| `security/isolation/` | Ring0 kernel | ✅ Ring0 kernel | Pledge + sandbox policy → appliqué au kernel |
| `security/exploit_mitigations/` | Ring0 kernel | ✅ Ring0 kernel | KASLR, CET, CFG → Ring0 |
| `security/integrity_check/` | Ring0 kernel | ✅ Ring0 kernel | Vérifie .text/.rodata → Ring0 |
| `security/ipc_policy/` | Ring0 kernel | ✅ Ring0 kernel | Check au point de dispatch IPC |
| `exo_shield/engine/` | Ring1 server | ✅ Ring1 server | Threat scoring → peut être Ring1 |
| `exo_shield/behavioral/` | Ring1 server | ✅ Ring1 server | Analyse comportementale → Ring1 |
| `exo_shield/signatures/` | Ring1 server | ✅ Ring1 server | Pattern matching → Ring1 |
| `exo_shield/ipc_gate/` | Ring1 server | ✅ Ring1 server | Policy + audit IPC → Ring1 |
| `exo_shield/hooks/` | Ring1 server | ✅ Ring1 server mais non connecté | Doit recevoir les events du kernel |
| `exo_shield/sandbox/` | Ring1 server | ✅ Ring1 server mais non connecté | Reçoit les ordres de containment du kernel |
| `exo_shield/network/` | Ring1 server | ✅ Ring1 server mais non connecté | S'intègre avec network_server |
| `exo_shield/ml/` | Ring1 server | ✅ Ring1 server mais non connecté | Inférence légère en Ring1 |
| `exo_shield/forensics/` | Ring1 server | ✅ Ring1 server mais non connecté | Dump/timeline → Ring1 |

**Conclusion placement :** Tout est correctement placé. Le problème n'est pas où les composants sont, mais qu'ils ne sont pas **connectés** entre eux.

---

## 6. Architecture Cible — Flux Complet

```
RING 0 — KERNEL (détection basse couche)
│
│  ExoArgos::pmc_snapshot()  [à chaque context switch]
│       ↓ anomalie détectée
│  ipc_policy::check_direct_ipc()  [à chaque IPC]
│       ↓ violation
│  exoledger::append()  [tout event de sécurité]
│       ↓
│  IPC → exo_shield [event_report(EVENT_PMC_ANOMALY)]
│
├──────────────────────────────────────────────────────
│
RING 1 — exo_shield (analyse et réponse)
│
│  IPC reçu → ipc_gate::evaluate_policy()
│                    ↓
│            handle_event_report()
│                    ↓
│      hooks::pre_syscall_check()     ← hooks/ branché
│      hooks::pre_exec_validate()     ← hooks/ branché
│      hooks::pre_connect_check()     ← hooks/ branché
│                    ↓
│      engine::submit_event()
│                    ↓
│      behavioral::analyze()
│      ml::inference::classify()      ← ml/ branché
│      signatures::yara::evaluate_all()
│                    ↓
│      score ≥ seuil critique ?
│           ↓ OUI
│      sandbox::container::apply_quarantine()   ← sandbox/ branché
│      network::firewall::block_pid()           ← network/ branché
│      forensics::timeline::record_event()      ← forensics/ branché
│      forensics::report::generate_report()     ← forensics/ branché
│           ↓
│      IPC → monitor_server [alert + rapport]
│
└──────────────────────────────────────────────────────
```

---

## 7. Synthèse des Corrections Prioritaires

### P0 — Débloquer les 5 modules orphelins (1 heure de travail)

1. Ajouter dans `lib.rs` : `pub mod hooks; pub mod sandbox; pub mod network; pub mod ml; pub mod forensics;`
2. Ajouter les inits dans `_start()`
3. Brancher les hooks dans `handle_event_report()` et `handle_quarantine_cmd()`

### P1 — Étendre les patterns YARA (30 minutes)

Changer `[u8; 8]` → `[u8; 64]` dans `Condition.value` + recalculer les offsets du parseur binaire.

### P2 — Documenter et Intégrer ExoArgos → exo_shield bridge

Créer le chemin kernel → IPC → exo_shield pour les anomalies PMC détectées par ExoArgos.

### P3 — Intégrer Pledge dans exo compat install

Tout process installé via `exo compat install` reçoit un `PledgeSet` calculé depuis ses dépendances.

### P4 (Post v0.2.0) — yara-x avec alloc

Activer `extern crate alloc` dans `exo_shield` + intégrer yara-x pour patterns arbitraires, regex, modules ELF/PE.

---

*claude-alpha — ExoOS v0.2.0 — AUDIT-SECURITY-ARCHITECTURE.md*
