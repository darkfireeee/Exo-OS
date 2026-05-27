# SPEC-EXOPHOENIX-STRATA — ExoPhoenix Parfait
## Bascule A↔B Reproductible < 500ms — ExoOS v0.2.0 Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXOPHOENIX-V0.2.md
**Base :** Validé release mode mai 2026 (IDT fix `nomem`→`readonly,preserves_flags`)

---

## 1. État de Départ (Mai 2026)

ExoPhoenix est déjà validé en release mode :
- IDT fix `nomem`→`readonly,preserves_flags` appliqué dans `gdt.rs` + `idt.rs`
- Make targets : `iso-release-phoenix-resurrection` + `qemu-release-phoenix-resurrection`
- TLC : 11,046 états vérifiés
- Tests : 2,975 unit PASS + 25 integration PASS
- QEMU_STATUS:33 (RESURRECTION_OK)

**Ce qui manque pour "parfait" en Strata :**
- Bascule avec processus Ring3 actifs (pas juste kernel seul)
- Préservation des sockets réseau actives (reconnexion transparente)
- Validation capabilities survivantes après bascule
- Recovery < 500ms mesurable sous charge 80%
- PhoenixSafe implémenté par exo_shield (cf. SPEC-EXOSHIELD-STRATA)
- SSR bitmask `[u64; CORE_MASK_WORDS]` pour 256 cores (CORR-75)

---

## 2. Définition de "Parfait" — Critères Strata

| Critère | Seuil | Test |
|---|---|---|
| Recovery A→B (bascule normale) | < 500ms | `phoenix_perf_test` |
| Recovery B→A (après crash) | < 500ms | `phoenix_crash_recovery_test` |
| Capabilities survivantes | 100% | `phoenix_cap_survival_test` |
| Processus Ring3 non-éphémères relancés | 100% | `phoenix_process_restore_test` |
| ExoFS atomicité pendant écriture | commit ou rollback | `phoenix_exofs_atomicity_test` |
| SSR cohérence | 0 champ invalide | `phoenix_ssr_integrity_test` |
| Bascule sous charge 80% CPU | < 500ms | `phoenix_stress_test` |
| ExoShield: 0 événement perdu | 100% | `phoenix_shield_continuity_test` |
| ExoShield: 0 process non-surveillé post-bascule | 100% | `phoenix_shield_rescan_test` |
| Stress 1000 bascules | 0 échec | `phoenix_stress_1000_test` |
| Bips audio: silence pendant bascule | OK | `phoenix_audio_silence_test` |

---

## 3. Architecture de la Bascule

### 3.1 Vue d'Ensemble

```
KERNEL A (actif)                        KERNEL B (en attente)
─────────────────────────               ────────────────────────
ExoPhoenix::Sentinel                    ExoPhoenix::forge.rs
  │  NMI heartbeat monitoring             (prêt à prendre le relais)
  │  SSR mise à jour continue
  │
  ▼ [Déclencheur : crash / manuel / watchdog]
  │
  ├─[1]  Pre-switch : tous PhoenixSafe notifiés
  │       exo_shield   : flush alerts, suspendre hooks, snapshot scoring
  │       exo-net      : invalider sockets TCP
  │       exo-crypto   : évincer cache clés
  │       exo-fs       : flush write cache, finir epoch
  │       exo-runtime  : compléter futures en vol
  │       audio_server : silence (stop tout)
  │
  ├─[2]  SSR snapshot atomique dans zone physique partagée
  │       { capabilities_table, process_list, ipc_endpoints, epoch_id,
  │         active_cores:[u64;4], shield_state_hash }
  │
  ├─[3]  ExoFS : epoch commit ou rollback
  │
  ├─[4]  Handoff → Kernel B
  │
KERNEL B reprend
  ├─[5]  Lire SSR + valider BLAKE3
  ├─[6]  Restaurer capabilities depuis SSR
  ├─[7]  Redémarrer Ring1 (vagues parallèles si possible)
  ├─[8]  exo_shield PhoenixSafe::on_post_switch()
  │       → recharger signatures YARA
  │       → réenregistrer hooks
  │       → rescan processus survivants
  ├─[9]  Relancer processus Ring3 non-éphémères
  ├─[10] Post-switch : libs notifiées
  │       exo-net : reconnecter sockets persistent
  │       exo-crypto : rouvrir caps crypto
  │       audio_server : reprendre état normal
  └─[11] PHOENIX_READY → RING1_COMPLETE → chime boot
```

### 3.2 SSR (System State Record) — Format Strata

```rust
/// Zone physique partagée A↔B — 4 KiB, page non-swappée.
/// Adresse physique : [0x0100_0000..0x0110_0000) — 64 KiB réservés.
#[repr(C, align(4096))]
pub struct SystemStateRecord {
    // Header (16 octets)
    pub magic:              u32,              // 0xEXO_PHXF
    pub version:            u32,              // = 2 pour Strata
    pub ssr_hash:           [u8; 32],         // BLAKE3 du reste du SSR

    // État kernel (48 octets)
    pub active_kernel:      KernelId,         // A ou B
    pub epoch_id:           EpochId,          // Epoch ExoFS courante
    pub boot_count:         u64,
    pub switch_reason:      SwitchReason,
    pub switch_start_ns:    u64,
    pub switch_end_ns:      u64,              // Rempli par Kernel B

    // Cores actifs — 256-core (CORR-75)
    pub active_cores:       [u64; CORE_MASK_WORDS], // [u64; 4]

    // Capabilities
    pub cap_table_ptr:      PhysAddr,
    pub cap_table_len:      u32,
    pub cap_table_hash:     [u8; 32],

    // ExoShield state (nouveau Strata)
    pub shield_scoring_ptr: PhysAddr,         // Risk profiles snapshot
    pub shield_scoring_len: u32,
    pub shield_state_hash:  [u8; 32],

    // Processus (max SSR_MAX_PROCESSES = 24)
    pub process_count:      u32,
    pub processes:          [ProcessRecord; 24],

    // IPC endpoints
    pub endpoint_count:     u32,
    pub endpoints:          [EndpointRecord; 128],

    // Audio state
    pub audio_was_playing:  bool,
    pub _reserved:          [u8; /* complète à 4096 */ _],
}

// Invariants vérifiés à la compilation
const_assert!(size_of::<SystemStateRecord>() <= 4096);
const_assert!(CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT); // O-05

pub struct ProcessRecord {
    pub pid:           u32,
    pub ring:          u8,
    pub restore_mode:  RestoreMode,
    pub binary_hash:   [u8; 32],
    pub cap_bitmap:    [u64; 4],      // 256 caps par process
    pub restart_args:  [u8; 64],
}

pub enum RestoreMode {
    Restart,   // Ring1 servers, apps simples
    Resume,    // Checkpoint disponible
    Abandon,   // Éphémère — ne pas relancer
}
```

### 3.3 Règle PhoenixSafe — Implémentations Requises

```rust
pub trait PhoenixSafe {
    fn on_pre_switch(&mut self) -> Result<(), PhoenixError>;
    fn on_post_switch(&mut self) -> Result<(), PhoenixError>;
    fn is_stateless(&self) -> bool { false }
    fn get_restore_mode(&self) -> RestoreMode { RestoreMode::Restart }
}
```

| Composant | `on_pre_switch()` | `on_post_switch()` |
|---|---|---|
| `exo-alloc` | stateless — rien | Réinitialiser arènes |
| `exo-net` | Invalider sockets TCP | Reconnecter sockets `persistent` |
| `exo-crypto` | Évincer cache clés local | Rouvrir caps crypto |
| `exo-fs` | Flush write cache + epoch commit | Rouvrir descripteurs ExoFS |
| `exo-runtime` | Compléter futures en vol | Redémarrer executor |
| `exo_shield` | Flush alerts + suspendre hooks + snapshot scoring | Recharger YARA + réenregistrer hooks + rescan |
| `audio_server` | Stop tout son | Reprendre état normal (pas de chime si bascule non-crash) |
| `network_server` | Flush buffers tx/rx | Reprendre smoltcp |

---

## 4. Déclencheurs de Bascule

### 4.1 Automatique (Crash)

Sentinel détecte :
- Absence heartbeat NMI > 2s
- Exception non-rattrapable (double fault, MCE)
- Watchdog hardware
- Canary kernel corrompu

### 4.2 Manuelle (Mise à Jour Live)

```bash
exo phoenix switch --reason update --new-kernel /path/kernel-B.elf
# 1. Vérification signature
# 2. Chargement zone kernel-B
# 3. Pre-switch (notif libs)
# 4. Handoff
# 5. Kernel B → PHOENIX_READY
# Pas de chime (bascule volontaire, pas de reboot)
```

### 4.3 Test de Validation

```bash
exo phoenix test-switch
# Sortie attendue :
# [00:00:000]  Pre-switch : notifications...
# [00:00:012]  exo_shield : alerts flushed, hooks suspended
# [00:00:013]  exo-net : 3 sockets TCP invalidées
# [00:00:014]  SSR snapshot : 2304 bytes (version 2)
# [00:00:015]  ExoFS epoch 43 : commit ✓
# [00:00:016]  Handoff → Kernel B
# [00:00:419]  Kernel B : SSR BLAKE3 valid ✓
# [00:00:423]  ExoShield : YARA rechargées, hooks réenregistrés
# [00:00:471]  Ring1 restored : 5/5
# [00:00:482]  Ring3 restored : 2/2
# [00:00:484]  Capabilities : 128/128 intact
# [SUCCESS]  Recovery : 484ms  (< 500ms ✓)
```

---

## 5. Correction SSR Bitmask (CORR-75 — Phase 0)

```rust
// forge.rs, handoff.rs, isolate.rs, ssr.rs

// AVANT (incorrect, limite 64 cores) :
pub active_cores: u64,

// APRÈS (correct, 256 cores) :
pub active_cores: [u64; CORE_MASK_WORDS],

// Helpers :
impl SsrCoreMask {
    pub fn set_core(&mut self, id: usize) {
        self.active_cores[id / 64] |= 1u64 << (id % 64);
    }
    pub fn is_active(&self, id: usize) -> bool {
        self.active_cores[id / 64] & (1u64 << (id % 64)) != 0
    }
}
```

---

## 6. Tests de Validation

```
phoenix_test::ssr_bitmask_256_cores          PASS
phoenix_test::ssr_hash_valid_after_write     PASS
phoenix_test::switch_no_load                 PASS  < 500ms
phoenix_test::switch_50pct_load              PASS  < 500ms
phoenix_test::switch_80pct_load              PASS  < 500ms
phoenix_test::cap_survival_basic             PASS  100%
phoenix_test::cap_survival_with_ring3        PASS  100%
phoenix_test::exofs_atomicity_during_write   PASS
phoenix_test::ring1_restore_all              PASS  N/N
phoenix_test::ring3_restore_restart          PASS
phoenix_test::exo_net_reconnect              PASS
phoenix_test::exo_crypto_reopen             PASS
phoenix_test::shield_continuity             PASS  0 events lost
phoenix_test::shield_rescan_post_switch     PASS  all PIDs covered
phoenix_test::audio_silence_during_switch   PASS
phoenix_test::stress_1000_switches          PASS  0 failures

Total cible : 16 PASS / 0 FAIL / 0 SKIP
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-EXOPHOENIX-STRATA.md*
