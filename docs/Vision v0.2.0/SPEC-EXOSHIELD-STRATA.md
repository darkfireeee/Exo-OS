# SPEC-EXOSHIELD-STRATA — Serveur EDR Complet Ring1
## ExoShield Phase 3 — ExoOS v0.2.0 Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXO-SECURITY-ACTIVATION.md

---

## 1. Positionnement

ExoShield n'est pas un composant kernel. C'est un **serveur Ring1 complet** — le dernier démarré (Vague 5 de `init_server`), précisément parce qu'il doit voir tous les autres serveurs déjà actifs pour les surveiller.

Il est distinct des composants de sécurité kernel (ExoSeal, ExoCage, ZeroTrust, CapToken, ExoKairos, ExoLedger, ExoShield-IOMMU, ExoNMI) qui s'activent pendant le boot en Ring0. Le serveur ExoShield est la couche **observabilité et réponse** qui opère en Ring1 après que le kernel est sécurisé.

**Analogie :** si les composants kernel sont les verrous d'une porte, ExoShield est le vigile qui surveille qui passe.

---

## 2. Architecture des Modules

```
exo_shield/src/
├── main.rs          → point d'entrée Ring1, boucle IPC principale
├── engine/
│   ├── core.rs      → threat scoring, records, profils de risque
│   ├── scanner.rs   → signatures YARA, heuristiques, scan périodique
│   └── realtime.rs  → monitoring temps réel, rate tracking, alertes
├── behavioral/
│   ├── anomaly.rs   → détection d'anomalies comportementales
│   ├── heuristic.rs → heuristiques de menace
│   ├── profiler.rs  → profils de comportement par processus
│   └── sequence.rs  → détection de séquences d'appels suspects
├── hooks/
│   ├── syscall_hooks.rs → instrumentation syscalls Ring3
│   ├── exec_hooks.rs    → hooks execve/execveat
│   ├── memory_hooks.rs  → hooks mmap/mprotect suspects
│   └── net_hooks.rs     → hooks connect/bind/sendto
├── ipc_gate/
│   ├── policy.rs    → table de politiques, évaluation, default-deny
│   ├── audit.rs     → ring buffer audit 4096 entrées
│   └── access.rs    → classification capability requirements
├── network/
│   ├── firewall.rs          → règles stateful default-deny
│   ├── ids.rs               → détection patterns d'attaque
│   ├── dns_guard.rs         → filtrage DNS, blocage domaines
│   └── traffic_analysis.rs  → anomalie de trafic
├── sandbox/
│   ├── container.rs     → isolation Ring3 POSIX (`exo compat`)
│   ├── fs_restriction.rs → accès FS limités au manifest
│   ├── net_isolation.rs  → réseau par capability
│   └── syscall_filter.rs → allowlist syscalls par processus
├── signatures/
│   ├── database.rs → base de données signatures indexée
│   ├── matcher.rs  → moteur YARA simplifié
│   ├── yara.rs     → parser + évaluateur YARA
│   └── update.rs   → mise à jour signatures depuis ExoFS
├── ml/
│   ├── model.rs     → modèle embarqué (statique, pas de training)
│   ├── inference.rs → inférence sur features comportementales
│   ├── features.rs  → extraction features depuis events
│   └── update.rs    → réservé v0.3.0
├── forensics/
│   ├── memory_dump.rs → dump mémoire d'un processus suspect
│   ├── timeline.rs    → reconstruction timeline d'un incident
│   └── report.rs      → rapport forensique structuré
└── behavioral/
```

---

## 3. Protocole IPC (7 types de messages)

**Endpoint :** `"exo_shield"` (PID fixe, attribué par init_server)

**Format enveloppe :**
```rust
#[repr(C)]
struct ShieldRequest {
    sender_pid: u32,                              // PID de l'émetteur
    msg_type:   u32,                              // type de message
    payload:    [u8; IPC_INLINE_PAYLOAD_SIZE],    // 120 octets
}
// sizeof(ShieldRequest) == IPC_ENVELOPE_SIZE (128 octets)
```

### 3.1 — SCAN_REQUEST (0)

Demande un scan d'un processus ou d'une région mémoire.

```rust
// payload layout :
// [0..4]   : target_pid (u32)
// [4..8]   : scan_flags (u32) — SCAN_MEMORY | SCAN_SIGNATURES | SCAN_HEURISTIC
// [8..16]  : region_base (u64, optionnel si 0 = scan complet)
// [16..24] : region_size (u64)
```

Réponse : `scan_id` (u32) pour récupérer le résultat plus tard.
Requires : `CAP_EXOSHIELD_SCAN`

### 3.2 — EVENT_REPORT (1)

Rapport d'événement depuis les hooks kernel. Chemin critique — non bloquant.

```rust
// payload layout :
// [0..4]   : event_type (u32) — syscall, exec, mmap, net
// [4..8]   : source_pid (u32)
// [8..16]  : timestamp_ns (u64)
// [16..48] : event_data (32 octets, layout dépend de event_type)
// [48..56] : cap_token_hash (u64, pour audit)
```

Pas de capability requise (appelé par le kernel, pas par un processus Ring3).

### 3.3 — QUARANTINE_CMD (2)

Contenir ou libérer un processus.

```rust
// payload layout :
// [0..4]  : target_pid (u32)
// [4..8]  : action (u32) — CONTAIN | RELEASE | KILL
// [8..16] : reason_code (u64)
```

Requires : `CAP_EXOSHIELD_QUARANTINE` (privilege élevé)

### 3.4 — THREAT_QUERY (3)

Consulter les enregistrements de menaces.

```rust
// payload layout :
// [0..4]  : query_type (u32) — BY_PID | BY_LEVEL | ALL_ACTIVE
// [4..8]  : filter_pid (u32, si query_type == BY_PID)
// [8..12] : min_level (u32, si query_type == BY_LEVEL)
// [12..16]: max_results (u32)
```

Requires : `CAP_EXOSHIELD_QUERY`

### 3.5 — POLICY_UPDATE (4)

Mettre à jour les politiques de scanning.

Requires : `CAP_EXOSHIELD_ADMIN` (uniquement init_server ou crypto_server)

### 3.6 — HEARTBEAT (5)

Contrôle de vivacité. Réponse immédiate avec stats courantes.

```rust
// Réponse payload :
// [0..4]   : active_threats (u32)
// [4..8]   : events_last_sec (u32)
// [8..12]  : contained_processes (u32)
// [12..16] : uptime_seconds (u32)
```

### 3.7 — PMC_ANOMALY (6)

Rapport d'anomalie hardware performance counter (Spectre, cache timing).

```rust
// payload layout :
// [0..4]   : core_id (u32)
// [4..8]   : anomaly_type (u32)
// [8..16]  : counter_value (u64)
// [16..24] : baseline_value (u64)
// [24..28] : suspect_pid (u32)
```

---

## 4. Démarrage : Vague 5

ExoShield est le **dernier** serveur Ring1 à démarrer. Séquence :

```
Vague 1 : memory_server, scheduler_server, crypto_server
Vague 2 : device_server, virtio drivers, AHCI, NVMe, USB
Vague 3 : vfs_server
Vague 4 : tty_server, input_server, network_server, audio_server
Vague 5 : exo_shield  ← ICI
Vague 6 : exosh
```

**Séquence d'initialisation interne exo_shield :**

```rust
// main.rs → exo_shield_init()
1. engine_init()           // core + scanner + realtime
2. ipc_gate::policy_init() // charger politiques depuis ExoFS /etc/exoshield/
3. ipc_gate::audit_init()  // ring buffer 4096 entrées
4. signatures::load_db()   // charger /etc/exoshield/signatures.ydb
5. ml::load_model()        // charger modèle statique embarqué
6. hooks::register_all()   // IPC kernel → register syscall/exec/mem/net hooks
7. network::firewall_init()// politiques réseau par défaut
8. sandbox::init()         // préparer infrastructure isolation
9. // Scan initial de tous les processus Ring1 déjà actifs
   for pid in 2..current_pid { engine::assess_pid(pid); }
10. // Signal init_server : SHIELD_READY
    ipc_send(PID_INIT, MSG_SERVER_READY, "exo_shield");
11. // Boot chime si audio_server disponible
    ipc_send(PID_AUDIO, MSG_PLAY_SOUND, SOUND_BOOT_COMPLETE);
12. // Boucle principale IPC
    loop { handle_next_message(); }
```

---

## 5. Sandbox `exo compat` — Fonctionnement Détaillé

Chaque processus Ring3 installé via `exo compat install` reçoit automatiquement une sandbox au moment du premier `execve`.

**Manifest de capabilities (généré par exo-pkg, stocké dans ExoFS) :**
```
# /apps/calendar/.manifest
SYSCALLS: read write openat close fstat mmap brk exit_group
FS_ALLOW: /apps/calendar/ /tmp/calendar_tmp /var/calendar.db
FS_DENY:  /etc/ /var/exoledger/ /servers/
NET:      NONE
IPC:      NONE
CAPS:     CAP_NONE
```

**Application de la sandbox :**
```
execve("/apps/calendar", ...) intercepté par exec_hooks
  → exo_shield::sandbox::apply_from_manifest("/apps/calendar/.manifest")
  → syscall_filter installé sur le PID
  → fs_restriction : VFS mount namespace restreint
  → net_isolation : socket() → EPERM si NET=NONE dans manifest
  → cap token généré avec les constraints du manifest
  → ExoLedger : PROCESS_SANDBOXED event
```

---

## 6. PhoenixSafe — Conformité ExoPhoenix

```rust
impl PhoenixSafe for ExoShield {
    fn on_pre_switch(&mut self) {
        // 1. Flush alert ring buffer → ExoFS (snapshot)
        self.engine.realtime.flush_alerts();
        // 2. Sauvegarder état scoring processus actifs
        self.state_snapshot = self.engine.core.snapshot_risk_profiles();
        // 3. Suspendre hooks (pas de nouveaux events pendant bascule)
        self.hooks.suspend_all();
        // 4. Sauvegarder policy table checksum
        self.policy_checksum = self.ipc_gate.policy.checksum();
    }

    fn on_post_switch(&mut self) {
        // 1. Recharger signatures depuis ExoFS (peut avoir changé)
        self.signatures.reload_if_changed();
        // 2. Restaurer risk profiles
        self.engine.core.restore_risk_profiles(&self.state_snapshot);
        // 3. Réenregistrer hooks kernel (nouveau kernel B)
        self.hooks.register_all();
        // 4. Vérifier policy checksum
        assert_eq!(self.ipc_gate.policy.checksum(), self.policy_checksum);
        // 5. Rescan processus survivants
        for pid in surviving_pids() { self.engine.assess_pid(pid); }
        // 6. Reprendre surveillance
        self.hooks.resume_all();
    }
}
```

---

## 7. Alerte Sonore Sécurité

ExoShield est le seul processus autorisé à déclencher `SOUND_SECURITY_ALERT`.

```rust
// Dans engine/realtime.rs — generate_alert()
fn notify_audio_on_threat(level: ThreatLevel) {
    match level {
        ThreatLevel::High => {
            // 3 bips courts (300Hz, 150ms) espacés 200ms
            for _ in 0..3 {
                ipc_send(PID_AUDIO, BEEP, BeepParams { freq: 300, dur: 150 });
                spin_wait_ms(200);
            }
        }
        ThreatLevel::Critical => {
            // 1 bip long (200Hz, 1000ms)
            ipc_send(PID_AUDIO, PLAY_SYSTEM_SOUND, SOUND_SECURITY_ALERT);
        }
        _ => {} // Low/Medium : pas de son
    }
}
```

La politique sonore n'est pas configurable depuis Ring3 (DRV-ARCH-01).

---

## 8. Tests Requis

```
exoshield_test::engine_init_clean              PASS
exoshield_test::scan_clean_process             PASS
exoshield_test::scan_detects_yara_pattern      PASS
exoshield_test::sandbox_blocks_unauthorized_fs PASS
exoshield_test::sandbox_blocks_unauthorized_net PASS
exoshield_test::sandbox_allows_manifest_paths  PASS
exoshield_test::ipc_gate_audit_logged          PASS
exoshield_test::policy_deny_enforced           PASS
exoshield_test::phoenix_pre_switch_flushes     PASS
exoshield_test::phoenix_post_switch_rescans    PASS
exoshield_test::alert_high_three_beeps         PASS
exoshield_test::alert_critical_long_beep       PASS
security_test::exoshield_sandbox_escape_blocked PASS   ← intégration
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-EXOSHIELD-STRATA.md*
