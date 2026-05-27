# SPEC-SECURITY-COMPLETE-V0.2 — Sécurité ExoOS Complète
## ExoArgos · Pledge · MLS · CFG/SafeStack · ipc_policy · yara-x roadmap

**Auteur :** claude-alpha  
**Date :** 2026-05-15  
**Statut :** SPEC OFFICIELLE v0.2.0 — Complète le SPEC-EXO-SECURITY-ACTIVATION.md

---

## 1. Vue d'Ensemble — Architecture Sécurité Réelle

Le kernel ExoOS implémente **15 sous-systèmes de sécurité**, non 7 comme documenté précédemment. Les 8 supplémentaires sont réels dans le code — ils n'étaient pas dans les specs.

```
COUCHE 1 — BOOT INTEGRITY
  ExoSeal         Chaîne de hash boot + IOMMU statique NIC

COUCHE 2 — ISOLATION HARDWARE  
  ExoCage         CET Shadow Stack + IBT + handler #CP
  ExoVeil         PKS domains — isolation mémoire O(1) par WRPKRU
  CFG             Control Flow Guard — bitmap cibles appels indirects
  SafeStack       Stack de retour séparée (fallback logiciel CET)
  KASLR           Kernel Address Space Layout Randomization

COUCHE 3 — VÉRIFICATION CONTINUE
  Zero Trust      Étiquettes MLS Bell-LaPadula + Biba sur chaque IPC
  ipc_policy      ServiceClass policy — flux IPC inter-serveurs
  capability      CapToken sur chaque ressource

COUCHE 4 — DROITS & ISOLATION PROCESSUS
  ExoKairos       Budgets temporels inline — capabilities avec TTL
  Pledge          Réduction de surface d'attaque (style OpenBSD)
  isolation       Domaines, namespaces, sandbox, pledge

COUCHE 5 — DÉTECTION RUNTIME
  ExoArgos        PMC monitoring — détection side-channel (Intel PMU)
  ExoNMI          NMI watchdog — intégrité kernel permanente
  stack_protector Canaries de pile par thread

COUCHE 6 — AUDIT IMMUABLE
  ExoLedger       Journal chaîné BLAKE3, zone P0 immuable
  audit           Ring-buffer ISR-safe, catégories, syscall audit

COUCHE 7 — ANALYSE RÉPONSE (Ring1)
  exo_shield      Threat engine, behavioral, ML, IDS, forensics, hooks
```

---

## 2. ExoArgos — Détection Side-Channel par PMU

### 2.1 Principe

ExoArgos lit 5 compteurs PMU Intel à chaque context switch :

| Compteur | MSR | Ce qu'il mesure |
|---------|-----|----------------|
| `IA32_FIXED_CTR0` | 0x309 | Instructions retired |
| `IA32_FIXED_CTR1` | 0x30A | Core cycles unhalted |
| `IA32_PMC0` | 0xC1 | L3 cache misses (MEM_LOAD_RETIRED.L3_MISS) |
| `IA32_PMC1` | 0xC2 | Branch mispredictions (BR_MISP_RETIRED.ALL) |
| TSC | RDTSCP | Timestamp |

**PmcSnapshot** : structure de 64 bytes alignée sur le layout SSR (zone per-CPU d'ExoPhoenix).

### 2.2 Algorithme de Détection

```
1. Premiers N context switches d'un processus → établir baseline (moyenne PMC)
2. Chaque snapshot suivant → compute_discordance(baseline, snapshot)
3. discordance = |oracle - pmc| (différence absolue, fixed-point × 10000)
4. DECEPTION_THRESHOLD = 3500 (= 0.35 en fixed-point)
5. check_anomaly() → true si discordance > DECEPTION_THRESHOLD sur 2+ compteurs
```

### 2.3 Ce que ça Détecte

| Attaque | Signal PMC |
|---------|-----------|
| **Spectre v1** (branch misprediction) | pic `br_mispred` |
| **Meltdown** (transient execution) | pic `l3_miss` + `inst_retired` anormal |
| **Flush+Reload** (cache timing) | pic `l3_miss` périodique |
| **Prime+Probe** (LLC eviction) | `l3_miss` élevé de manière soutenue |
| **Timing attack sur crypto** | `clk_unhalted` / `inst_retired` ratio anormal |
| **Code qui simule un comportement** (deception) | discordance globale élevée |

### 2.4 Intégration Bridge → exo_shield (À Implémenter)

```rust
// Dans kernel/src/security/exoargos.rs — AJOUTER après check_anomaly()

/// Envoie une anomalie PMC détectée vers exo_shield via IPC.
/// Appelé depuis pmc_snapshot() si check_anomaly() retourne true.
pub fn report_anomaly_to_shield(pid: u32, snap: &PmcSnapshot, discordance: u32) {
    // Construire le payload IPC
    let mut payload = [0u8; 64];
    payload[0..4].copy_from_slice(&pid.to_le_bytes());
    payload[4..12].copy_from_slice(&snap.inst_retired.to_le_bytes());
    payload[12..20].copy_from_slice(&snap.clk_unhalted.to_le_bytes());
    payload[20..28].copy_from_slice(&snap.l3_miss.to_le_bytes());
    payload[28..36].copy_from_slice(&snap.br_mispred.to_le_bytes());
    payload[36..44].copy_from_slice(&snap.tsc.to_le_bytes());
    payload[44..48].copy_from_slice(&discordance.to_le_bytes());

    // IPC non-bloquant vers exo_shield (drop si plein — PMC est statistique)
    let _ = crate::ipc::try_send(
        EXO_SHIELD_ENDPOINT,
        PMC_ANOMALY_REPORT,  // msg_type = 6
        &payload,
    );
}
```

### 2.5 Règles ExoArgos

- **ARGOS-01** : PMU initialisé uniquement si `cpuid` confirme le support (vMSR en QEMU = OK)
- **ARGOS-02** : `pmc_snapshot()` est ISR-safe — pas d'allocation, atomics uniquement
- **ARGOS-03** : La baseline est établie sur les 16 premiers context switches du processus
- **ARGOS-04** : `check_anomaly()` ne déclenche jamais de kill directement — il signale à exo_shield
- **ARGOS-05** : Les snapshots PMC survivent à une bascule ExoPhoenix (stockés dans la zone SSR per-CPU)

---

## 3. ExoVeil — Isolation Mémoire PKS

### 3.1 Principe

PKS (Protection Keys for Supervisor, Intel Ice Lake+) étend le mécanisme PKey (PKRU pour userspace) au mode superviseur. ExoVeil utilise PKS pour créer des **domaines de mémoire kernel** :

```
Domain 0 (PKS_DOMAIN_DEFAULT)  → Mémoire kernel standard
Domain 1 (PKS_DOMAIN_CRYPTO)   → Pages du crypto_server (clés, etc.)
Domain 2 (PKS_DOMAIN_LEDGER)   → Pages ExoLedger (lecture seule)
Domain 3 (PKS_DOMAIN_SHIELD)   → Pages exo_shield
...
Domain N (PKS_DOMAIN_REVOKED)  → Domaine révoqué = aucun accès
```

### 3.2 Révocation O(1)

```rust
pub fn revoke_domain(domain: PksDomain) {
    // Une seule écriture WRPKRS → domaine entier inaccessible
    // Pas de TLB shootdown requis — c'est un registre par-CPU
    let pkrs = current_pkrs();
    let new_pkrs = pkrs | (PksPermission::NoAccess.bits() << (domain as u32 * 2));
    unsafe { write_pkrs(new_pkrs); }
    revoke_count.fetch_add(1, Ordering::Relaxed);
}
```

**Usage ExoPhoenix :** Avant la bascule A→B, `exoveil_revoke_all_on_handoff()` révoque tous les domaines sauf `DEFAULT`. Après la bascule, `pks_restore_for_normal_ops()` restaure les domaines légitimes. Aucune donnée sensible n'est lisible pendant le handoff.

### 3.3 Disponibilité

PKS est disponible uniquement sur Intel Ice Lake+ et certains AMD (Zen 4+). Sur QEMU avec `-cpu host`, PKS est disponible si le host le supporte. `pks_available()` détecte le support au boot.

**Fallback si PKS absent :** Les domaines sont simulés par des tables de permissions logicielles (plus lent, même sécurité fonctionnelle).

---

## 4. Pledge — Isolation des Processus POSIX

### 4.1 Principe

Pledge (inspiré d'OpenBSD) permet à un processus de **renoncer irrévocablement** à des classes de syscalls. Une fois pledgé, le processus ne peut que réduire ses droits — jamais les augmenter.

**RÈGLE PLEDGE-01 :** Un processus ne peut QUE retirer des pledges.  
**RÈGLE PLEDGE-02 :** Violation → SIGKILL immédiat.  
**RÈGLE PLEDGE-03 :** `init_server` ne peut pas appeler pledge().

### 4.2 Pledges Disponibles

```rust
pub mod pledge_flags {
    pub const STDIO:   u64 = 1 << 0;  // read, write, recvfrom, sendto
    pub const RPATH:   u64 = 1 << 1;  // open(read), stat
    pub const WPATH:   u64 = 1 << 2;  // open(write)
    pub const CPATH:   u64 = 1 << 3;  // creat, unlink, rename
    pub const TMPPATH: u64 = 1 << 4;  // accès /tmp
    pub const INET:    u64 = 1 << 5;  // sockets TCP/UDP
    pub const UNIX:    u64 = 1 << 6;  // sockets Unix Domain
    pub const DNS:     u64 = 1 << 7;  // résolution DNS
    pub const GETPW:   u64 = 1 << 8;  // /etc/passwd, /etc/group
    pub const PROC:    u64 = 1 << 9;  // fork(), waitpid()
    pub const EXEC:    u64 = 1 << 10; // execve()
    pub const ID:      u64 = 1 << 11; // setuid/setgid/getuid/getgid
    pub const ROUTE:   u64 = 1 << 12; // table de routage
    pub const SHM:     u64 = 1 << 13; // mémoire partagée
    pub const SIGNAL:  u64 = 1 << 14; // sigaction, sigprocmask
    pub const TTY:     u64 = 1 << 15; // ioctl terminal
    // ...
}
```

### 4.3 Intégration dans `exo compat install`

Chaque app POSIX installée doit recevoir un `PledgeSet` calculé depuis ses besoins :

```
calendar    → STDIO | RPATH | WPATH | TMPPATH | SIGNAL | TTY
vim         → STDIO | RPATH | WPATH | CPATH | TMPPATH | SIGNAL | TTY
curl        → STDIO | RPATH | INET | DNS | SIGNAL
htop        → STDIO | RPATH | PROC | SIGNAL | TTY
python3     → STDIO | RPATH | WPATH | CPATH | TMPPATH | INET | DNS | PROC | EXEC | SIGNAL
vlc (audio) → STDIO | RPATH | INET | DNS | SHM | SIGNAL | TTY
```

### 4.4 Interaction avec CapTokens

Le Pledge et les CapTokens sont complémentaires :
- **CapTokens** : contrôle basé sur la ressource ("peux-tu accéder à *cet* objet ExoFS ?")
- **Pledge** : contrôle basé sur la classe d'action ("peux-tu exécuter *ce type* de syscall ?")

Un processus doit satisfaire les DEUX pour réaliser une opération.

---

## 5. Zero Trust MLS — Bell-LaPadula + Biba

### 5.1 Modèle

Le module `zero_trust/labels.rs` implémente deux modèles de sécurité classiques :

**Bell-LaPadula (confidentialité) :**
- `no read up` : un sujet ne peut pas lire un objet de niveau supérieur
- `no write down` : un sujet ne peut pas écrire dans un objet de niveau inférieur
- Protège contre la fuite de secrets vers des niveaux moins confidentiels

**Biba (intégrité) :**
- `no write up` : un sujet ne peut pas modifier un objet de niveau supérieur
- `no read down` : un sujet ne peut pas lire un objet de niveau inférieur
- Protège l'intégrité des données de haut niveau contre des données de bas niveau

### 5.2 Niveaux de Confiance ExoOS

```
Niveau 3 : SYSTEM      (kernel, init_server, ipc_broker)
Niveau 2 : SERVICE     (crypto_server, vfs_server, network_server, exo_shield)
Niveau 1 : PRIVILEGED  (apps avec caps élevées)
Niveau 0 : USER        (apps standard, exo compat)
```

### 5.3 Labels sur les IPC

Chaque message IPC porte un `SecurityLabel` :
```rust
pub struct SecurityLabel {
    pub sensitivity: u8,   // Niveau Bell-LaPadula (0..=3)
    pub integrity:   u8,   // Niveau Biba (0..=3)
    pub compartment: u32,  // Compartiment de sécurité (bitmask)
}
```

`verify_access(src_label, dst_label, action)` vérifie les deux modèles simultanément.

---

## 6. CFG + SafeStack — Protection Flux de Contrôle

### 6.1 CFG (Control Flow Guard)

```rust
// Enregistrement des cibles légitimes pour les appels indirects
cfg_register_target(function_ptr);    // une fonction
cfg_register_range(start, end);       // une plage

// Validation avant chaque appel indirect (call reg, call [mem])
cfg_validate_indirect_call(target)?;  // → Err si cible non enregistrée

// Verrouillage en fin de boot (plus d'ajout possible)
cfg_lock();
```

**Impact :** Toute tentative de ROP (Return-Oriented Programming) qui détourne un appel indirect vers une cible non enregistrée → `CfgError::InvalidTarget` → kill du processus.

### 6.2 SafeStack

```
Stack normale (pile de données) :
  [locals, buffers, objets complexes]
  
Stack sécurisée (pile de retour) :
  [adresses de retour uniquement]
  
Séparation physique en mémoire → débordement dans la stack normale
ne peut pas écraser les adresses de retour
```

SafeStack est actif si CET est absent. Si CET est présent, le Shadow Stack hardware prend le relais.

---

## 7. ipc_policy — Politique de Flux Inter-Serveurs

### 7.1 ServiceClass

```rust
pub enum ServiceClass {
    Unknown,
    InitServer,      // PID 1
    IpcBroker,       // PID 2
    MemoryServer,
    VfsServer,
    CryptoServer,
    NetworkServer,
    DeviceServer,
    ExoShield,
    UserApp,
    CompatApp,       // App via musl-exo
}
```

### 7.2 Politique de Flux

```
InitServer  → tous les services   : Allowed (c'est l'init)
IpcBroker   → tous les services   : Allowed (c'est le broker)
UserApp     → VfsServer           : Allowed (accès fichiers)
UserApp     → NetworkServer       : RequiresCap (cap net:r)
UserApp     → CryptoServer        : RequiresCap (cap crypto)
UserApp     → ExoShield           : Denied (sauf EVENT_REPORT)
UserApp     → UserApp             : Denied (pas d'IPC direct P2P)
CompatApp   → VfsServer           : Allowed (via musl-exo)
CompatApp   → NetworkServer       : RequiresCap (pledge INET)
ExoShield   → tous les services   : Allowed (serveur de sécurité)
```

### 7.3 Intégration avec le Fast Path IPC

`check_direct_ipc(src, dst)` est appelé **dans le fast path IPC du kernel** avant de router le message. Si le résultat est `Denied`, le message est drop + ExoLedger entry.

---

## 8. Séquence Complète security_init — 13 Étapes

```
Phase 0   exoseal_boot_phase0()      CET default-deny + PKS default-deny + watchdog
Phase 1   integrity_init()           Hash .text/.rodata + chaîne de confiance modules
Phase 2   capability::init()         Tables CapToken, révocation, délégation
Phase 3   zero_trust (lazy)          Labels MLS Bell-LaPadula + Biba
Phase 4   crypto_init()              CSPRNG RDRAND + ChaCha20 + BLAKE3
Phase 5   isolation (static)         Domaines, namespaces, pledge, sandbox
Phase 6   mitigations_init()         KASLR figé + canary global + CFG + CET + SafeStack
Phase 7   audit_init()               Ring-buffer 65536 entrées, règles par défaut
Phase 8   access_control::init()     Mappings ObjectKind v6 (step 18 boot)
Phase 9   exo_ledger_init()          Journal chaîné BLAKE3 + zone P0 immuable
Phase 10  exokairos init_kernel_secret()  CSPRNG → secret pour capabilities temporelles
Phase 11  exoargos_init()            PMC MSR programmés + hook context_switch installé
Phase 12  exonmi_init()              NMI watchdog armé (LAPIC, 200ms)
Phase 12b exocage per-thread BSP     CET Shadow Stack activé pour le thread courant
Phase 13  exoseal_boot_complete()    PKS ops normales + SECURITY_READY.store(Release)
```

**Probe de debug QEMU :** port 0xE9, un caractère par phase (`j` à `y`). Visible dans la console série QEMU.

---

## 9. yara-x — Plan d'Intégration

### 9.1 Situation Actuelle

Notre engine YARA-like (839 lignes) est no_std mais limité :
- Patterns max 8 bytes (→ 64 bytes avec CORR-75-E)
- Pas de regex
- Pas de wildcards hex (`?? ?? 90`)
- Pas de modules (PE, ELF)
- Max 128 règles

### 9.2 yara-x — Capacités

`yara-x` (VirusTotal, Rust natif) offre :
- Patterns de taille arbitraire
- Regex complète
- Hex strings avec wildcards : `{ 4D 5A ?? ?? 50 45 }`
- Modules PE, ELF, math, hash
- Conditions booléennes complexes : `all of them`, `2 of ($a, $b, $c)`
- Règles avec métadonnées et tags

### 9.3 Plan d'Intégration (v0.3.0)

**Prérequis :** Activer `extern crate alloc` dans `exo_shield` (nécessite exo-alloc opérationnel — Phase 0 de v0.2.0).

```toml
# exo_shield/Cargo.toml — v0.3.0
[dependencies]
spin.workspace = true
exo-syscall-abi = { path = "../syscall_abi" }
exo-alloc = { path = "../exo-alloc" }  # Active alloc
# yara-x : évaluer si no_std+alloc est supporté en v0.3.0
```

**Stratégie :** Conserver l'engine actuel (amélioré à 64 bytes) comme **fast path** pour les règles simples. Ajouter yara-x comme **deep scan** pour les analyses complexes (scan à la demande, pas en temps réel).

```
Niveau 1 : Engine interne 64-bytes → < 1µs par scan (temps réel)
Niveau 2 : yara-x → < 10ms par scan (analyse approfondie)
Niveau 3 : ML inference → comportemental (continu)
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-SECURITY-COMPLETE-V0.2.md*
