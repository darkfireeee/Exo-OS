# CORR-75 — exo_shield : Modules Orphelins + YARA Pattern Fix
## Corrections Prioritaires P0 + P1

**Auteur :** claude-alpha  
**Date :** 2026-05-15  
**Fichiers affectés :**
- `exo_shield/src/lib.rs`
- `exo_shield/src/main.rs`
- `exo_shield/src/signatures/yara.rs`
- `exo_shield/src/engine/realtime.rs` (hook branching)

---

## CORR-75-A — lib.rs : Déclaration des 5 Modules Manquants

### AVANT (incorrect)
```rust
#![no_std]

pub mod behavioral;
pub mod engine;
pub mod ipc_gate;
pub mod signatures;
```

### APRÈS (correct)
```rust
#![no_std]

pub mod behavioral;
pub mod engine;
pub mod forensics;   // ← AJOUTÉ
pub mod hooks;       // ← AJOUTÉ
pub mod ipc_gate;
pub mod ml;          // ← AJOUTÉ
pub mod network;     // ← AJOUTÉ
pub mod sandbox;     // ← AJOUTÉ
pub mod signatures;
```

---

## CORR-75-B — main.rs : Initialisation Complète dans `_start()`

### Section à remplacer dans `_start()`

```rust
// AVANT :
ipc_gate::policy_init();
ipc_gate::audit_init();
engine::engine_init();
signatures::signatures_init();
behavioral::behavioral_init();

// APRÈS :
ipc_gate::policy_init();
ipc_gate::audit_init();
engine::engine_init();
signatures::signatures_init();
behavioral::behavioral_init();

// ── Modules précédemment orphelins ─────────────────────────────────────────

// Hooks — interception d'événements exec/net/memory/syscall
hooks::exec_hooks::exec_hooks_init();
hooks::net_hooks::net_hooks_init();
hooks::memory_hooks::mem_hooks_init();
hooks::syscall_hooks::syscall_hooks_init();

// Network — IDS signatures, firewall, DNS guard, traffic analysis
network::ids::ids_init();
network::firewall::firewall_init();
network::dns_guard::dns_guard_init();
network::traffic_analysis::traffic_analysis_init();

// ML — inference engine (réseau de neurones comportemental)
ml::model::model_init();
ml::features::features_init();

// Forensics — memory dump, timeline, rapport
forensics::memory_dump::memory_dump_init();
forensics::timeline::timeline_init();
forensics::report::report_init();

// Sandbox — containment réel des processus
sandbox::container::container_manager_init();
sandbox::fs_restriction::fs_restriction_init();
sandbox::net_isolation::net_isolation_init();
sandbox::syscall_filter::syscall_filter_init();
```

---

## CORR-75-C — main.rs : Branchement des Hooks dans handle_event_report()

### Après l'appel à `engine::submit_event()`, ajouter :

```rust
fn handle_event_report(req: &ShieldRequest) -> ShieldReply {
    let target_pid = read_u32_le(&req.payload, 0);
    let event_type = engine::EventType::from_u8(req.payload[4]);
    let opcode     = read_u32_le(&req.payload, 8);
    let arg0       = read_u64_le(&req.payload, 12);
    let arg1       = read_u64_le(&req.payload, 20);
    let severity   = engine::ThreatLevel::from_u8(req.payload[28]);
    let tick       = current_tick();

    if target_pid == 0 { return ShieldReply::new(SHIELD_ERR_ARGS); }

    let event = engine::MonitoredEvent {
        pid: target_pid, event_type, opcode, arg0, arg1,
        timestamp: tick, severity,
    };

    // ── Routage vers les hooks spécialisés ───────────────────────────────────
    // (NOUVEAU — était absent)
    match event.event_type {
        engine::EventType::Exec => {
            // Validation pre-exec : blacklist, chaîne d'exécution suspecte
            let exec_action = hooks::pre_exec_validate(
                target_pid,
                &req.payload[10..], // path bytes dans le payload
            );
            hooks::record_exec_event(target_pid, &event, exec_action);
        }
        engine::EventType::Network => {
            // src_ip = arg0[32..63], src_port = arg0[0..15]
            // dst_ip = arg1[32..63], dst_port = arg1[0..15]
            let _net_action = hooks::pre_connect_check(
                target_pid,
                arg0,       // addr encodée
                arg1 as u16, // port
            );
            // Vérifier exfiltration DNS
            hooks::detect_exfiltration(target_pid, arg0, arg1, tick);
            // Vérifier port scan
            hooks::detect_port_scan(target_pid, tick);
        }
        engine::EventType::Memory => {
            let alloc_size = arg0 as usize;
            let alloc_addr = arg1;
            hooks::pre_alloc_check(target_pid, alloc_size, opcode);
            hooks::detect_buffer_overflow(target_pid, alloc_addr, alloc_size, tick);
            hooks::detect_use_after_free(target_pid, alloc_addr, tick);
        }
        engine::EventType::Syscall => {
            hooks::pre_syscall_check(target_pid, opcode, arg0, arg1);
            hooks::post_syscall_monitor(target_pid, opcode, 0); // retval = 0
            hooks::detect_dangerous_syscall(target_pid, opcode, arg0);
            hooks::analyze_syscall_sequence(target_pid, opcode, tick);
        }
        _ => {}
    }

    // ── Engine principal (inchangé) ──────────────────────────────────────────
    let result = engine::submit_event(&event, tick);
    engine::stat_scan_executed(result.matched);

    // ── Inférence ML sur la base des features comportementales ──────────────
    // (NOUVEAU)
    let proc_data = ml::features::ProcessBehaviourData {
        pid: target_pid,
        syscall_count: hooks::get_syscall_stats().total_syscalls,
        net_connections: hooks::get_net_stats().total_connections,
        // ... autres champs depuis les hooks stats
        ..Default::default()
    };
    let features = ml::features::FeatureExtractor::extract(&proc_data);
    let weights  = ml::model::current_weights();
    let inference = ml::inference::infer(&weights, &features);
    
    // Si ML classifie comme Malicious et engine confirme → auto-containment
    if inference.classification == ml::inference::Classification::Malicious
       && result.max_severity >= engine::ThreatLevel::High
    {
        engine::mark_process_contained(target_pid, tick);
        engine::stat_containments_inc();
        
        // Appliquer le sandbox réel
        sandbox::container::apply_profile(
            target_pid,
            &sandbox::ContainerProfile::default_quarantine(),
        );
        
        // Enregistrer la timeline forensique
        forensics::timeline::record_timeline_event(
            target_pid,
            forensics::TimelineEventType::ContainmentStart,
            tick,
        );
    }

    // ── Construction de la réponse (inchangée) ───────────────────────────────
    let mut reply = ShieldReply::new(SHIELD_OK);
    reply.data[0..4].copy_from_slice(&result.alert_id.to_le_bytes());
    reply.data[4] = result.action as u8;
    reply.data[5] = if result.rate_exceeded { 1 } else { 0 };
    reply.data[6] = if result.contained { 1 } else { 0 };
    reply
}
```

---

## CORR-75-D — main.rs : Containment Réel dans handle_quarantine_cmd()

```rust
fn handle_quarantine_cmd(req: &ShieldRequest) -> ShieldReply {
    let cmd        = req.payload[0];
    let target_pid = read_u32_le(&req.payload, 1);
    let tick       = current_tick();

    if target_pid == 0 { return ShieldReply::new(SHIELD_ERR_ARGS); }

    match cmd {
        0 => {
            // Contain process
            let ok = engine::mark_process_contained(target_pid, tick);
            engine::stat_containments_inc();

            if ok {
                // NOUVEAU : Appliquer le containment réel (pas juste un flag)
                
                // 1. Profil sandbox de quarantaine
                sandbox::container::apply_profile(
                    target_pid,
                    &sandbox::ContainerProfile::default_quarantine(),
                );
                
                // 2. Bloquer le réseau du processus
                network::firewall::block_pid(target_pid);
                
                // 3. Démarrer la timeline forensique
                forensics::timeline::record_timeline_event(
                    target_pid,
                    forensics::TimelineEventType::ContainmentStart,
                    tick,
                );
                
                // 4. Initier un memory dump de l'état
                forensics::memory_dump::store_dump(target_pid, tick);
            }

            let mut reply = ShieldReply::new(if ok { SHIELD_OK } else { SHIELD_ERR_NOT_FOUND });
            reply.data[0] = if ok { 1 } else { 0 };
            reply
        }
        1 => {
            // Release process from containment
            let ok = engine::release_process(target_pid);
            
            if ok {
                // NOUVEAU : Retirer le containment réel
                sandbox::container::release_profile(target_pid);
                network::firewall::unblock_pid(target_pid);
                
                forensics::timeline::record_timeline_event(
                    target_pid,
                    forensics::TimelineEventType::ContainmentEnd,
                    tick,
                );
                
                // Générer le rapport forensique final
                let _ = forensics::report::generate_report(target_pid, tick);
            }

            let mut reply = ShieldReply::new(if ok { SHIELD_OK } else { SHIELD_ERR_NOT_CONTAINED });
            reply.data[0] = if ok { 1 } else { 0 };
            reply
        }
        // ... cmd=2 inchangé
        _ => ShieldReply::new(SHIELD_ERR_ARGS),
    }
}
```

---

## CORR-75-E — signatures/yara.rs : Extension Pattern 8→64 Bytes

```rust
// AVANT :
pub struct Condition {
    pub cond_type: ConditionType,
    pub field:     FieldType,
    pub offset:    u16,
    pub length:    u8,
    pub value:     [u8; 8],    // ← LIMITÉ À 8 BYTES
    pub threshold: u64,
    pub logic_op:  LogicOp,
    pub enabled:   bool,
    _reserved:     [u8; 3],
}

// APRÈS :
pub const MAX_PATTERN_LEN: usize = 64;  // ← NOUVEAU

pub struct Condition {
    pub cond_type: ConditionType,
    pub field:     FieldType,
    pub offset:    u16,
    pub length:    u8,          // maintenant peut aller jusqu'à 64
    pub value:     [u8; MAX_PATTERN_LEN],  // ← 8 → 64 bytes
    pub threshold: u64,
    pub logic_op:  LogicOp,
    pub enabled:   bool,
    _reserved:     [u8; 3],     // padding à ajuster si nécessaire
}

impl Condition {
    pub const fn empty() -> Self {
        Self {
            cond_type: ConditionType::Equals,
            field:     FieldType::RawData,
            offset:    0,
            length:    0,
            value:     [0u8; MAX_PATTERN_LEN],  // ← mise à jour
            threshold: 0,
            logic_op:  LogicOp::And,
            enabled:   false,
            _reserved: [0; 3],
        }
    }

    pub fn equals(field: FieldType, offset: u16, value: &[u8]) -> Self {
        let len = value.len().min(MAX_PATTERN_LEN);  // ← était .min(8)
        let mut val = [0u8; MAX_PATTERN_LEN];        // ← mise à jour
        val[..len].copy_from_slice(&value[..len]);
        Self {
            cond_type: ConditionType::Equals,
            field,
            offset,
            length: len as u8,
            value: val,
            threshold: 0,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }
    
    // ... même pattern pour contains(), not_equals()
}
```

### Parseur binaire — recalcul des offsets après extension

```rust
// parse_condition() — offsets mis à jour pour MAX_PATTERN_LEN = 64

pub fn parse_condition(data: &[u8]) -> Condition {
    // Format binaire étendu :
    // [cond_type:1, field:1, offset:2, length:1, value:64, threshold:8, logic_op:1]
    // Total : 78 octets (était 29)
    const PARSE_SIZE: usize = 1 + 1 + 2 + 1 + MAX_PATTERN_LEN + 8 + 1; // 78
    
    if data.len() < PARSE_SIZE { return Condition::empty(); }

    let cond_type = match ConditionType::from_u8(data[0]) {
        Some(ct) => ct,
        None => return Condition::empty(),
    };
    let field = match FieldType::from_u8(data[1]) {
        Some(f) => f,
        None => return Condition::empty(),
    };

    let offset = u16::from_le_bytes([data[2], data[3]]);
    let length = data[4].min(MAX_PATTERN_LEN as u8);  // ← borner à 64

    let mut value = [0u8; MAX_PATTERN_LEN];
    value.copy_from_slice(&data[5..5 + MAX_PATTERN_LEN]);  // ← 64 bytes

    let threshold_start = 5 + MAX_PATTERN_LEN;  // = 69
    let threshold = u64::from_le_bytes([
        data[threshold_start],     data[threshold_start + 1],
        data[threshold_start + 2], data[threshold_start + 3],
        data[threshold_start + 4], data[threshold_start + 5],
        data[threshold_start + 6], data[threshold_start + 7],
    ]);

    let logic_op = match LogicOp::from_u8(data[threshold_start + 8]) {
        Some(lo) => lo,
        None => LogicOp::And,
    };

    Condition { cond_type, field, offset, length, value, threshold, logic_op, enabled: true, _reserved: [0; 3] }
}
```

---

## CORR-75-F — Nouveau Message IPC : PMC_ANOMALY (ExoArgos Bridge)

Ajouter un nouveau type de message pour recevoir les anomalies PMC du kernel :

```rust
// main.rs — AJOUT d'un nouveau type de message
const PMC_ANOMALY_REPORT: u32 = 6;  // ← NOUVEAU (était 5 = HEARTBEAT)
// Renommer HEARTBEAT à 7 ou garder 5 et mettre PMC en 6

// Dans handle_request() :
PMC_ANOMALY_REPORT => handle_pmc_anomaly(req),

// Nouveau handler :
/// Handle PMC_ANOMALY_REPORT (msg_type 6) — envoyé par le kernel via ExoArgos.
///
/// Payload layout:
///   [0..4]   pid (LE)
///   [4..12]  inst_retired (LE)
///   [12..20] clk_unhalted (LE)
///   [20..28] l3_miss (LE)
///   [28..36] br_mispred (LE)
///   [36..44] tsc (LE)
///   [44..48] discordance_score (LE, fixed-point u32)
fn handle_pmc_anomaly(req: &ShieldRequest) -> ShieldReply {
    let pid          = read_u32_le(&req.payload, 0);
    let inst_retired = read_u64_le(&req.payload, 4);
    let clk_unhalted = read_u64_le(&req.payload, 12);
    let l3_miss      = read_u64_le(&req.payload, 20);
    let br_mispred   = read_u64_le(&req.payload, 28);
    let discordance  = read_u32_le(&req.payload, 44);
    let tick         = current_tick();

    if pid == 0 { return ShieldReply::new(SHIELD_ERR_ARGS); }

    // Convertir l'anomalie PMC en événement engine
    let event = engine::MonitoredEvent {
        pid,
        event_type: engine::EventType::Behavioral,
        opcode:     0xPMC,  // opcode spécial PMC
        arg0:       l3_miss,
        arg1:       br_mispred,
        timestamp:  tick,
        severity:   if discordance > 5000 {
            engine::ThreatLevel::Critical
        } else if discordance > 3500 {
            engine::ThreatLevel::High
        } else {
            engine::ThreatLevel::Medium
        },
    };

    let result = engine::submit_event(&event, tick);

    // Si discordance élevée → enregistrer dans la timeline forensique
    if discordance > 3500 {  // > DECEPTION_THRESHOLD
        forensics::timeline::record_timeline_event(
            pid,
            forensics::TimelineEventType::PmcAnomaly,
            tick,
        );
        
        engine::generate_manual_alert(
            pid,
            event.severity,
            engine::ThreatCategory::SideChannel,
            1,  // confidence
            b"pmc_deception_threshold_exceeded",
            tick,
        );
    }

    ShieldReply::new(SHIELD_OK)
}
```

---

## Résumé des Corrections

| ID | Fichier | Changement | Effort |
|----|---------|-----------|--------|
| CORR-75-A | `lib.rs` | +5 `pub mod` manquants | 5 lignes |
| CORR-75-B | `main.rs` | +11 appels d'init | 15 lignes |
| CORR-75-C | `main.rs` | Branchement hooks dans event_report | ~50 lignes |
| CORR-75-D | `main.rs` | Containment réel dans quarantine | ~30 lignes |
| CORR-75-E | `yara.rs` | Pattern 8→64 bytes + parseur | ~20 lignes modifiées |
| CORR-75-F | `main.rs` | Nouveau handler PMC_ANOMALY | ~40 lignes |

**Effort total estimé : ~2-3 heures de développement.  
Impact : activation de 5 modules entiers + détection side-channel + YARA plus puissant.**

---

*claude-alpha — ExoOS v0.2.0 — CORR-75.md*
