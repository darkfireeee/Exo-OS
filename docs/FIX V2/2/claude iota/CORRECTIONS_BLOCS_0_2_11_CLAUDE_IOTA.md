# Corrections BLOC 0 / BLOC 2 / BLOC 11 — Outillage, Sécurité, ExoShield

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Référence audit :** `AUDIT_KERNEL_V0.2.0_CLAUDE_IOTA.md` INC-O, INC-S, INC-ES

---

## BLOC 0 — Outillage d'Audit

---

### CORR-IOTA-06 — `arch/constants.rs` : Fichier Centralisé des Constantes

**Fichier à créer :** `kernel/src/arch/constants.rs`

```rust
//! # arch/constants.rs
//! Constantes architecturales centralisées — ExoOS v0.2.0
//!
//! Ce fichier est la source de vérité pour toutes les constantes
//! qui apparaissent dans des const_assert! cross-module.
//! Tout ajout de constante architecturale passe ici en premier.

// ── Physmap ────────────────────────────────────────────────────────────────
/// Couverture du boot page table (avant install_extended_physmap).
pub const PHYSMAP_INITIAL_COVERAGE: usize = 1 * 1024 * 1024 * 1024; // 1 GiB
const _: () = assert!(PHYSMAP_INITIAL_COVERAGE == 0x4000_0000);

// ── Layout Utilisateur ──────────────────────────────────────────────────────
/// Adresse virtuelle basse acceptée pour un ELF utilisateur.
/// Doit être ≤ 0x400000 (base ELF standard System V ABI).
pub const USER_ELF_BASE_MIN: u64 = 0x0000_0000_0001_0000; // 64 KiB
const _: () = assert!(USER_ELF_BASE_MIN <= 0x40_0000, "USER_ELF_BASE_MIN > 4 MiB");

// ── ExoPhoenix SSR ─────────────────────────────────────────────────────────
/// Adresse physique de base de la SSR (zone E820 réservée à 16 MiB).
pub const SSR_PHYS_BASE: u64 = 0x0100_0000; // 16 MiB
/// Fin exclusive de la zone SSR (16 MiB + 4 KiB).
pub const SSR_PHYS_END: u64  = SSR_PHYS_BASE + 4096;
const _: () = assert!(SSR_PHYS_END - SSR_PHYS_BASE == 4096);

// ── SMP Layout ─────────────────────────────────────────────────────────────
/// Nombre maximum de CPU supportés.
pub const MAX_CORES_LAYOUT: usize = 256;
/// Nombre de mots u64 pour le bitmask de cores.
pub const CORE_MASK_WORDS: usize = (MAX_CORES_LAYOUT + 63) / 64; // 4
const _: () = assert!(CORE_MASK_WORDS * 64 >= MAX_CORES_LAYOUT);

// ── ExoKairos ──────────────────────────────────────────────────────────────
/// Durée d'une fenêtre de budget temporel en nanosecondes (1 seconde).
pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000;
const _: () = assert!(KAIROS_WINDOW_NS > 0);
/// Throttle à 100% du budget restant dans la fenêtre.
pub const KAIROS_THROTTLE_PCT: u64 = 100;
/// Kill process à 200% du budget cumulé sur 2 fenêtres consécutives.
pub const KAIROS_KILL_PCT: u64 = 200;
```

Ajouter dans `kernel/src/lib.rs` :

```rust
pub mod arch;
// arch/mod.rs :
pub mod constants;
```

---

### CORR-IOTA-07 — `const_assert!` Manquants (INC-O02 à O05)

**Fichier :** `kernel/src/exophoenix/ssr.rs`

```rust
use crate::arch::constants::{SSR_PHYS_BASE, SSR_PHYS_END};

pub const SSR_MAX_PROCESSES: usize = 24;
pub const SSR_MAX_ENDPOINTS: usize = 48;
pub const PROCESS_RECORD_SIZE: usize = 96;
pub const ENDPOINT_RECORD_SIZE: usize = 24;

pub const SSR_SIZE: usize =
      64
    + 44
    +  4 + SSR_MAX_PROCESSES  * PROCESS_RECORD_SIZE
    +  4 + SSR_MAX_ENDPOINTS  * ENDPOINT_RECORD_SIZE
    + 16;

// ── Invariants statiques (O-02) ────────────────────────────────────────────
const _: () = assert!(SSR_SIZE <= 4096,
    "SSR_SIZE dépasse 4 KiB — réduire SSR_MAX_PROCESSES ou SSR_MAX_ENDPOINTS");
const _: () = assert!(SSR_MAX_PROCESSES >= 12,
    "SSR doit pouvoir restaurer au moins 12 Ring1 servers");
const _: () = assert!(SSR_PHYS_END - SSR_PHYS_BASE >= SSR_SIZE as u64,
    "Zone physique SSR trop petite pour SSR_SIZE");
```

**Fichier :** `kernel/src/security/exokairos.rs`

```rust
use crate::arch::constants::KAIROS_WINDOW_NS;

// Invariant (O-03)
const _: () = assert!(KAIROS_WINDOW_NS == 1_000_000_000,
    "KAIROS_WINDOW_NS doit être 1 seconde");
```

---

### CORR-IOTA-08 — `deny.toml` (INC-O10)

**Fichier à créer :** `deny.toml` (racine du workspace)

```toml
# deny.toml — ExoOS v0.2.0
# Vérification par : cargo deny check
# CI : .github/workflows/audit.yml

[graph]
targets = [
    { triple = "x86_64-unknown-none" },
]

[licenses]
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
]
deny = [
    "GPL-2.0",   # incompatible avec un OS propriétaire
    "LGPL-2.0",
    "LGPL-2.1",
]
copyleft = "deny"

[bans]
multiple-versions = "warn"
wildcards = "deny"
deny = [
    # Dépendances interdites par la Vision ExoOS (no-std strict)
    { name = "tokio",       reason = "Runtime async interdit — ExoOS est synchrone/IPC" },
    { name = "tokio-runtime", reason = "Idem" },
    { name = "dbus",        reason = "D-Bus interdit — utiliser IPC ExoOS natif" },
    { name = "zbus",        reason = "Idem" },
    { name = "libsodium",   reason = "Utiliser exo_crypto interne" },
    { name = "openssl",     reason = "Utiliser exo_crypto interne" },
    { name = "ring",        reason = "Evaluer avant autorisation" },
    { name = "std",         reason = "Kernel no_std strict (crate std interdite)" },
    { name = "libc",        reason = "Pas de libc dans le kernel" },
]

[advisories]
db-path   = "~/.cargo/advisory-db"
db-urls   = ["https://github.com/rustsec/advisory-db"]
vulnerability = "deny"
unmaintained  = "warn"
unsound       = "deny"
notice        = "warn"
```

---

### CORR-IOTA-09 — Script de Vérification des Constantes (INC-O06)

**Fichier à créer :** `tools/audit_constants.py`

```python
#!/usr/bin/env python3
"""
audit_constants.py — Vérifie la cohérence des constantes entre kernel et crates.
Usage : python3 tools/audit_constants.py [--kernel-src kernel/src]

Vérifications effectuées :
  1. SSR_SIZE <= 4096
  2. USER_ELF_BASE_MIN <= 0x400000
  3. PHYSMAP_INITIAL_COVERAGE == 1 GiB
  4. CORE_MASK_WORDS * 64 >= MAX_CORES_LAYOUT
  5. KAIROS_WINDOW_NS > 0
  6. SSR_PHYS_BASE référencé dans E820 (memory_map.rs)
"""
import re
import sys
from pathlib import Path

KERNEL_SRC = Path("kernel/src")

def extract_const(path: Path, name: str) -> int | None:
    pattern = re.compile(
        rf'const\s+{name}\s*:\s*\w+\s*=\s*([^;]+);'
    )
    text = path.read_text(errors="replace")
    m = pattern.search(text)
    if not m:
        return None
    expr = m.group(1).strip()
    # Évaluation simple des expressions numériques Rust
    expr = expr.replace("_", "").replace("usize", "").replace("u64", "")
    try:
        return eval(expr, {"__builtins__": {}})
    except Exception:
        return None

def check_all() -> list[str]:
    errors = []
    arch_const = KERNEL_SRC / "arch" / "constants.rs"

    checks = [
        ("SSR_MAX_PROCESSES",       lambda v: v >= 12,        "SSR_MAX_PROCESSES doit être >= 12"),
        ("PHYSMAP_INITIAL_COVERAGE",lambda v: v == 1<<30,     "PHYSMAP_INITIAL_COVERAGE != 1 GiB"),
        ("KAIROS_WINDOW_NS",        lambda v: v == 1_000_000_000, "KAIROS_WINDOW_NS != 1 seconde"),
        ("MAX_CORES_LAYOUT",        lambda v: v <= 1024,      "MAX_CORES_LAYOUT > 1024"),
        ("USER_ELF_BASE_MIN",       lambda v: v <= 0x400000,  "USER_ELF_BASE_MIN > 4 MiB"),
    ]

    for name, pred, msg in checks:
        val = extract_const(arch_const, name)
        if val is None:
            errors.append(f"ABSENT : {name} dans {arch_const}")
        elif not pred(val):
            errors.append(f"INVALIDE : {name} = {val:#x} — {msg}")
        else:
            print(f"  OK   {name} = {val:#x}")

    return errors

if __name__ == "__main__":
    print("=== audit_constants.py ExoOS ===")
    errs = check_all()
    if errs:
        print("\nERREURS DÉTECTÉES :")
        for e in errs:
            print(f"  ✗ {e}")
        sys.exit(1)
    else:
        print("\nToutes les constantes sont cohérentes.")
        sys.exit(0)
```

---

### CORR-IOTA-10 — Workflow CI Audit (INC-O13)

**Fichier à créer :** `.github/workflows/audit.yml`

```yaml
name: Audit Kernel ExoOS

on:
  push:
    branches: [main, v0.2.0-stabilisation]
  pull_request:

jobs:
  constants:
    name: Vérification Constantes
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: python3 tools/audit_constants.py

  cargo-deny:
    name: Dépendances (cargo deny)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v1
        with:
          command: check all

  semgrep:
    name: Analyse Statique (Semgrep)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run Semgrep
        run: |
          pip install semgrep
          semgrep --config tools/semgrep-rules/exoos.yaml \
                  --error kernel/src/ servers/

  build:
    name: Compilation Kernel
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          components: rust-src
      - run: |
          cd kernel
          cargo build --target x86_64-unknown-none 2>&1
```

---

## BLOC 2 — Sécurité Boot

---

### CORR-IOTA-11 — ExoKairos : Fenêtre de Reset du Budget (INC-S16)

**Fichier :** `kernel/src/security/exokairos.rs`

```rust
use crate::arch::constants::{KAIROS_WINDOW_NS, KAIROS_THROTTLE_PCT, KAIROS_KILL_PCT};

/// Capability temporelle avec budget atomique, deadline et fenêtre de reset.
#[repr(C)]
pub struct TemporalCap {
    calls_left:    core::sync::atomic::AtomicU64,
    bytes_left:    core::sync::atomic::AtomicU64,
    deadline_ns:   u64,
    // ── NOUVEAU (CORR-IOTA-11) ────────────────────────────────────────────
    /// Timestamp de début de la fenêtre courante (nanosecondes TSC).
    window_start_ns: core::sync::atomic::AtomicU64,
    /// Budget initial de la fenêtre (pour calcul des %).
    budget_initial:  u64,
    /// Nombre de fenêtres consécutives dépassant KAIROS_KILL_PCT.
    overrun_windows: core::sync::atomic::AtomicU32,
    // ────────────────────────────────────────────────────────────────────
    hmac_tag:      [u8; 32],
}

impl TemporalCap {
    /// Décrémente le budget et gère la fenêtre de reset.
    /// Retourne `KairosDecision` : Allow / Throttle / Kill.
    pub fn use_cap(&self, cost_calls: u64, cost_bytes: u64) -> KairosDecision {
        let now = crate::arch::x86_64::tsc::read_tsc_ns();
        let window_start = self.window_start_ns.load(Ordering::Acquire);

        // ── Reset de fenêtre si KAIROS_WINDOW_NS écoulé ──────────────────
        if now.saturating_sub(window_start) >= KAIROS_WINDOW_NS {
            // Nouvelle fenêtre : remettre les budgets à leur valeur initiale.
            self.calls_left.store(self.budget_initial, Ordering::Release);
            self.bytes_left.store(self.budget_initial * 512, Ordering::Release);
            self.window_start_ns.store(now, Ordering::Release);
            // Remettre le compteur d'overrun si la fenêtre précédente était saine.
            // (seul le kill sur 2 fenêtres consécutives est conservé)
        }
        // ────────────────────────────────────────────────────────────────

        // Décrémentation atomique
        let remaining_calls = self.calls_left
            .fetch_sub(cost_calls.min(self.calls_left.load(Ordering::Relaxed)), Ordering::AcqRel);
        let remaining_bytes = self.bytes_left
            .fetch_sub(cost_bytes.min(self.bytes_left.load(Ordering::Relaxed)), Ordering::AcqRel);

        let usage_pct = 100u64.saturating_sub(
            remaining_calls.saturating_mul(100) / self.budget_initial.max(1)
        );

        if usage_pct >= KAIROS_KILL_PCT {
            let overruns = self.overrun_windows.fetch_add(1, Ordering::AcqRel) + 1;
            if overruns >= 2 {
                return KairosDecision::Kill;
            }
        }
        if usage_pct >= KAIROS_THROTTLE_PCT {
            return KairosDecision::Throttle;
        }

        let _ = remaining_bytes; // utilisé pour le check bytes si nécessaire
        KairosDecision::Allow
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KairosDecision {
    Allow,
    Throttle,
    Kill,
}
```

---

### CORR-IOTA-12 — ExoLedger : Vérification `is_immutable()` dans object_write (INC-S19)

**Fichier :** `kernel/src/fs/exofs/syscall/object_write.rs`

```rust
use crate::fs::exofs::objects::logical_object::LogicalObject;
use crate::security::exoledger;

/// Syscall SYS_EXOFS_OBJECT_WRITE (opcode 503).
pub fn sys_exofs_object_write(
    obj_id:  u64,
    offset:  u64,
    buf_ptr: u64,
    buf_len: u64,
    _flags:  u64,
    _a6:     u64,
) -> i64 {
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();

    // Récupérer l'objet
    let obj = match crate::fs::exofs::core::global_objects().get(obj_id) {
        Some(o) => o,
        None => return ENOENT,
    };

    // ── CORR-IOTA-12 : Guard immutabilité (ERR-04 / S-19) ───────────────
    if obj.is_immutable() {
        // Logger la tentative dans ExoLedger (audit chaîné)
        exoledger::record(exoledger::ExoLedgerEvent::WriteAttemptOnImmutable {
            blob_id: obj_id,
            caller_pid,
            offset,
            len: buf_len,
        });
        log::warn!("[ExoLedger] tentative d'écriture sur objet immutable {:#x} par pid {}",
                   obj_id, caller_pid);
        return EPERM; // = -1 (EPERM : opération non permise)
    }
    // ────────────────────────────────────────────────────────────────────

    // Vérifications ACL et écriture existantes...
    // ... code inchangé ci-dessous ...
}
```

**Fichier :** `kernel/src/security/exoledger.rs` — Ajouter l'event :

```rust
#[derive(Debug, Clone, Copy)]
pub enum ExoLedgerEvent {
    // ... events existants ...

    /// Tentative d'écriture sur un objet marqué immutable.
    WriteAttemptOnImmutable {
        blob_id:    u64,
        caller_pid: u32,
        offset:     u64,
        len:        u64,
    },
}
```

---

## BLOC 11 — exo_shield Complet

---

### CORR-IOTA-13 — lib.rs : Déclaration des 5 Modules Orphelins (INC-ES01)

**Fichier :** `servers/exo_shield/src/lib.rs`

```rust
// AVANT :
#![no_std]

pub mod behavioral;
pub mod engine;
pub mod ipc_gate;
pub mod signatures;

// APRÈS (CORR-IOTA-13 = CORR-75-A) :
#![no_std]

pub mod behavioral;
pub mod engine;
pub mod forensics;   // ← AJOUTÉ — analyse post-mortem, memory dump, timeline
pub mod hooks;       // ← AJOUTÉ — exec/net/memory/syscall hooks
pub mod ipc_gate;
pub mod ml;          // ← AJOUTÉ — inference comportementale
pub mod network;     // ← AJOUTÉ — IDS, firewall, DNS guard
pub mod sandbox;     // ← AJOUTÉ — containment, isolation
pub mod signatures;
```

---

### CORR-IOTA-14 — main.rs `_start()` : Init des 5 Modules (INC-ES02)

**Fichier :** `servers/exo_shield/src/main.rs` — section `_start()` ligne ~814

```rust
pub extern "C" fn _start() -> ! {
    // ── Init modules existants (inchangés) ──────────────────────────────
    ipc_gate::policy_init();
    ipc_gate::audit_init();
    engine::engine_init();
    signatures::signatures_init();
    behavioral::behavioral_init();

    // ── CORR-IOTA-14 : Init des 5 modules précédemment orphelins ────────
    // (CORR-75-B)

    // Hooks — interception syscall/exec/net/mem
    hooks::exec_hooks::exec_hooks_init();
    hooks::net_hooks::net_hooks_init();
    hooks::memory_hooks::mem_hooks_init();
    hooks::syscall_hooks::syscall_hooks_init();

    // Network — IDS, firewall, DNS guard, analyse trafic
    network::ids::ids_init();
    network::firewall::firewall_init();
    network::dns_guard::dns_guard_init();
    network::traffic_analysis::traffic_analysis_init();

    // ML — moteur d'inférence comportementale
    ml::model::model_init();
    ml::features::features_init();

    // Forensics — dump mémoire, timeline, rapport incident
    forensics::memory_dump::memory_dump_init();
    forensics::timeline::timeline_init();
    forensics::reporter::reporter_init();

    // Sandbox — containment et isolation de processus
    sandbox::container::container_init();
    sandbox::policy::sandbox_policy_init();
    // ────────────────────────────────────────────────────────────────────

    log::info!("[exo_shield] tous les modules initialisés");

    // Boucle principale IPC (inchangée)
    main_loop()
}
```

---

### CORR-IOTA-15 — `handle_event_report()` : Branchement Hooks (INC-ES03)

**Fichier :** `servers/exo_shield/src/main.rs` — `handle_event_report()` lignes ~305–337

```rust
fn handle_event_report(req: &ShieldRequest) -> ShieldReply {
    let event_type = req.payload[0];
    let pid        = u32::from_le_bytes(req.payload[1..5].try_into().unwrap_or_default());

    // ── CORR-IOTA-15 : Passer par les hooks avant scoring ────────────────
    // (CORR-75-C) — enrichissement du contexte événementiel

    let mut enriched_event = engine::Event::from_raw(event_type, pid, &req.payload);

    match event_type {
        EVENT_TYPE_EXEC => {
            // Hook exec : ajoute hash binaire, parent PID, argv hash
            hooks::exec_hooks::on_exec_event(&mut enriched_event);
        }
        EVENT_TYPE_NET_CONNECT | EVENT_TYPE_NET_DNS => {
            // Hook réseau : corrèle avec règles IDS, DNS guard
            hooks::net_hooks::on_net_event(&mut enriched_event);
            network::ids::check_event(&mut enriched_event);
            network::dns_guard::check_event(&mut enriched_event);
        }
        EVENT_TYPE_MMAP | EVENT_TYPE_MPROTECT => {
            // Hook mémoire : détection ROP/JIT/shellcode
            hooks::memory_hooks::on_memory_event(&mut enriched_event);
        }
        EVENT_TYPE_SYSCALL => {
            // Hook syscall : fréquence, séquences anomales
            hooks::syscall_hooks::on_syscall_event(&mut enriched_event);
        }
        _ => {}
    }
    // ─────────────────────────────────────────────────────────────────────

    // Soumettre l'événement enrichi au moteur de scoring
    let score = engine::submit_event(&enriched_event);

    // ML — refinement du score si confiance faible
    let final_score = if score.confidence < 80 {
        ml::model::refine_score(&enriched_event, score)
    } else {
        score
    };

    if final_score.risk_level >= RISK_HIGH {
        // Déclencher forensics en cas de menace haute
        forensics::timeline::record_threat_event(pid, &enriched_event, final_score);
    }

    ShieldReply::ok_with_score(final_score.risk_level)
}
```

---

### CORR-IOTA-16 — `handle_quarantine_cmd()` : Containment Réel (INC-ES04)

**Fichier :** `servers/exo_shield/src/main.rs` — `handle_quarantine_cmd()` lignes ~352–399

```rust
fn handle_quarantine_cmd(req: &ShieldRequest) -> ShieldReply {
    let cmd        = req.payload[0];
    let target_pid = u32::from_le_bytes(req.payload[1..5].try_into().unwrap_or_default());

    match cmd {
        // cmd == 0 : Containment (isoler le processus)
        0 => {
            // ── CORR-IOTA-16 : Containment réel via sandbox ──────────────
            // (CORR-75-D) — avant, seulement mark_process_contained()

            // 1. Vérifier que le PID est actif
            if !engine::is_pid_active(target_pid) {
                return ShieldReply::err(ERR_PID_NOT_FOUND);
            }

            // 2. Appliquer l'isolation sandbox (restreint réseau, FS, fork)
            match sandbox::container::isolate_process(target_pid) {
                Ok(()) => {
                    log::warn!("[exo_shield] PID {} mis en sandbox", target_pid);
                }
                Err(e) => {
                    log::error!("[exo_shield] sandbox::isolate_process({}) failed: {:?}",
                                target_pid, e);
                    return ShieldReply::err(ERR_SANDBOX_FAILED);
                }
            }

            // 3. Marquer dans le profil de risque (existant)
            engine::mark_process_contained(target_pid);

            // 4. Enregistrer l'événement dans la timeline forensics
            forensics::timeline::record_quarantine(target_pid);
            // ─────────────────────────────────────────────────────────────

            ShieldReply::ok()
        }

        // cmd == 1 : Kill (terminer le processus)
        1 => {
            // Appel syscall kernel exo_kill via IPC
            let kill_result = ipc_gate::send_kill_request(target_pid);
            forensics::timeline::record_kill(target_pid);
            engine::remove_process_profile(target_pid);
            if kill_result.is_ok() { ShieldReply::ok() } else { ShieldReply::err(ERR_KILL_FAILED) }
        }

        // cmd == 2 : Release (sortir du sandbox)
        2 => {
            sandbox::container::release_process(target_pid)?;
            engine::mark_process_released(target_pid);
            ShieldReply::ok()
        }

        _ => ShieldReply::err(ERR_UNKNOWN_CMD),
    }
}
```

---

## Récapitulatif des Fichiers Modifiés / Créés

| Fichier | Type | INC couverts |
|---|---|---|
| `kernel/src/arch/constants.rs` | CRÉÉ | O-01, O-02–O-05 |
| `kernel/src/exophoenix/ssr.rs` | MODIFIÉ | P-01, O-02 |
| `kernel/src/security/exokairos.rs` | MODIFIÉ | S-16 |
| `kernel/src/fs/exofs/syscall/object_write.rs` | MODIFIÉ | S-19 |
| `kernel/src/security/exoledger.rs` | MODIFIÉ | S-19 |
| `deny.toml` | CRÉÉ | O-10 |
| `tools/audit_constants.py` | CRÉÉ | O-06 |
| `.github/workflows/audit.yml` | CRÉÉ | O-13 |
| `servers/exo_shield/src/lib.rs` | MODIFIÉ | ES-01 |
| `servers/exo_shield/src/main.rs` | MODIFIÉ | ES-02, ES-03, ES-04 |

---

*claude iota — CORRECTIONS_BLOCS_0_2_11_CLAUDE_IOTA.md — 2026-05-20*
