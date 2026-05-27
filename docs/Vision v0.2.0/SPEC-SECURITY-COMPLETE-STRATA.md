# SPEC-SECURITY-COMPLETE-STRATA — Activation Complète de la Sécurité
## 7 Composants Kernel + ExoShield Ring1 — ExoOS v0.2.0 Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXO-SECURITY-ACTIVATION.md + SPEC-SECURITY-COMPLETE-V0.2.md

---

## 1. Objectif

En v0.1.x, les composants de sécurité étaient présents dans le code mais partiellement actifs — certains en mode audit-only, certains non câblés. Strata exige que **tous** les composants soient actifs en production, non bypassables, mutuellement cohérents.

**Critère de validation ultime :** `security_integration_test` → 13/13 PASS, zéro bypass.

---

## 2. Carte des Composants — Architecture en Couches

```
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 1 — Boot Integrity                                       │
│  ExoSeal  →  hash chain kernel + ring1 servers                   │
└─────────────────────────┬────────────────────────────────────────┘
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 2 — Isolation Hardware                                   │
│  ExoCage  →  CET shadow stack + IBT + SMEP + SMAP + KPTI + NX   │
└─────────────────────────┬────────────────────────────────────────┘
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 3 — Vérification Continue                               │
│  Zero Trust Layer  →  chaque IPC labelisé + vérifié             │
└──────────┬───────────────────────────────────────┬──────────────┘
           ▼                                       ▼
┌──────────────────────┐               ┌───────────────────────────┐
│  COUCHE 4a           │               │  COUCHE 4b                │
│  CapToken System     │               │  ExoKairos                │
│  → droits précis     │               │  → budgets temporels      │
│    par ressource     │               │    anti-DoS inline        │
└──────────┬───────────┘               └───────────┬───────────────┘
           └───────────────────┬───────────────────┘
                               ▼
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 5 — Audit Immuable                                       │
│  ExoLedger  →  journal BLAKE3 chaîné, epoch-linked, sealed       │
└─────────────────────────┬────────────────────────────────────────┘
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 6 — Isolation Physique (kernel)                          │
│  ExoShield-IOMMU  →  DMA isolation, domaines NET/BLOCK/BLACKHOLE │
└─────────────────────────┬────────────────────────────────────────┘
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 7 — Watchdog Permanent                                   │
│  ExoNMI  →  NMI watchdog 200ms, canary check, IDT integrity      │
└─────────────────────────┬────────────────────────────────────────┘
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  COUCHE 8 — Détection & Réponse (Ring1)                          │
│  ExoShield Server  →  EDR complet : hooks + YARA + sandbox + ML  │
│  Voir SPEC-EXOSHIELD-STRATA.md pour détail complet               │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. État Requis par Composant — Strata

### 3.1 ExoSeal — Boot Integrity

**Fichier :** `kernel/src/security/exoseal.rs`

**Principe :** Le kernel vérifie sa propre intégrité avant tout démarrage. Chaîne invalide = boot stop.

**État Strata :** Actif en production. En mode QEMU dev, `EXOSEAL_DEV_BYPASS` autorisé mais loggé dans ExoLedger obligatoirement.

```rust
// kernel/src/security/exoseal.rs
pub fn exoseal_verify_boot_chain() -> Result<(), SealError> {
    // 1. Hash kernel binaire (BLAKE3)
    let kernel_hash = blake3_hash_kernel_image();
    let expected = read_expected_kernel_hash()?; // TPM ou flash ou bootloader
    if kernel_hash != expected {
        #[cfg(not(feature = "dev-bypass"))]
        return Err(SealError::KernelTampered);
        #[cfg(feature = "dev-bypass")]
        exoledger_log(Event::ExosealDevBypass { hash: kernel_hash });
    }

    // 2. Hash de chaque server Ring1
    for server in RING1_SERVER_LIST {
        let hash = blake3_hash_server_image(server.id);
        let expected = read_server_hash(server.id)?;
        if hash != expected {
            #[cfg(not(feature = "dev-bypass"))]
            return Err(SealError::ServerTampered(server.id));
            #[cfg(feature = "dev-bypass")]
            exoledger_log(Event::ServerHashMismatch { id: server.id });
        }
    }

    BOOT_SEAL_STATE.store(SealState::Verified, Ordering::Release);
    Ok(())
}
```

**Validation :** `security_test::exoseal_verify_chain PASS`

---

### 3.2 ExoCage — Isolation Hardware

**Fichier :** `kernel/src/security/exocage.rs`

**Mécanismes requis (tous actifs sur BSP + tous APs) :**

```rust
pub fn exocage_init_cpu() {
    // SMEP : Supervisor Mode Execution Prevention
    unsafe { x86_64::registers::control::Cr4::update(|cr4| {
        cr4.insert(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
        cr4.insert(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION); // SMAP
        cr4.insert(Cr4Flags::OSXSAVE);
    }); }

    // NX/XD bit
    unsafe { Msr::new(0xC000_0080).write(
        Msr::new(0xC000_0080).read() | EFER_NXE
    ); }

    // IBRS + SSBD (Spectre/Meltdown)
    unsafe {
        Msr::new(IA32_SPEC_CTRL).write(SPEC_CTRL_IBRS | SPEC_CTRL_SSBD);
    }

    // CET Shadow Stack (si supporté par CPU)
    if cpu_has_cet_ss() {
        unsafe {
            Msr::new(IA32_U_CET).write(CET_SHSTK_EN | CET_WR_SHSTK_EN);
            Msr::new(IA32_S_CET).write(CET_SHSTK_EN | CET_ENDBR_EN); // IBT
        }
    }

    // KPTI : pages kernel non mappées en mode user
    // (géré dans paging::setup_kpti())
}

pub fn exocage_verify_active() -> Result<(), CageError> {
    let cr4 = Cr4::read();
    if !cr4.contains(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION) {
        return Err(CageError::SmepInactive);
    }
    if !cr4.contains(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION) {
        return Err(CageError::SmapInactive);
    }
    // ... vérifications EFER, CET, IBRS
    Ok(())
}
// Appelé en étape 18 (SECURITY_READY) — panic si un mécanisme manque
```

**Validation :** `security_test::exocage_all_mechanisms PASS`

---

### 3.3 Zero Trust Layer

**Fichier :** `kernel/src/security/zero_trust.rs`

**État v0.1.x :** `check_direct_ipc()` existait mais pas branché dans le fast path IPC.

**Fix Strata (CORR précédent + A3) :**

```rust
// kernel/src/ipc/core/send.rs — AVANT dispatch du message
pub fn ipc_send(sender: Pid, dest: EndpointId, msg: &Message) -> Result<(), IpcError> {
    // Fast path Zero Trust check (bitmask, ~3ns overhead)
    zero_trust::check_ipc(sender, dest, msg.len)?;

    // Suite normale...
    fast_path_dispatch(sender, dest, msg)
}

// zero_trust.rs
pub fn check_ipc(sender: Pid, dest: EndpointId, len: usize) -> Result<(), ZtError> {
    let sender_label = TRUST_LABELS.get(sender)?;
    let dest_label   = TRUST_LABELS.get_endpoint(dest)?;

    // Ring3 → Ring3 direct IPC bloqué (doit passer par Ring1)
    if sender_label.ring == 3 && dest_label.ring == 3 {
        return Err(ZtError::DirectRing3Blocked);
    }
    // Longueur max selon niveau de confiance
    if len > sender_label.max_msg_len {
        return Err(ZtError::MessageTooLong);
    }

    Ok(())
}
```

**Validation :** `security_test::zerotrust_ipc_blocked PASS`

---

### 3.4 CapToken System

**Fichier :** `kernel/src/security/capability.rs`

**État Strata :** Chaque accès FS, IPC Ring1, et réseau vérifié. Révocation immédiate.

```rust
pub fn capability_check(token: CapToken, required: Rights, object: ObjectId)
    -> Result<(), CapError>
{
    let entry = CAP_TABLE.get(token)?;
    if entry.object_id != object {
        return Err(CapError::WrongObject);
    }
    if !entry.rights.contains(required) {
        exoledger_log(Event::CapDenied { token, required, object });
        return Err(CapError::InsufficientRights);
    }
    if entry.revoked.load(Ordering::Acquire) {
        return Err(CapError::Revoked);
    }
    Ok(())
}

pub fn capability_revoke(token: CapToken) {
    if let Some(entry) = CAP_TABLE.get_mut(token) {
        entry.revoked.store(true, Ordering::Release);
        // Invalider tous les tokens dérivés
        for derived in &entry.derived_tokens {
            capability_revoke(*derived);
        }
        exoledger_log(Event::CapRevoked { token });
    }
}
```

**Validation :** `security_test::captoken_access_denied PASS`, `security_test::captoken_revocation_immediate PASS`

---

### 3.5 ExoKairos — Budgets Temporels

**Fichier :** `kernel/src/security/exokairos.rs`

**Règle corrigée (ERR-07) :** La fenêtre de reset est fixe : 1 seconde (KAIROS_WINDOW_NS = 1_000_000_000).

```rust
const KAIROS_WINDOW_NS: u64 = 1_000_000_000; // 1 seconde
const_assert!(KAIROS_WINDOW_NS == 1_000_000_000); // O-03

pub fn update_kairos_budget(pid: Pid, elapsed_ns: u64) {
    let budget = KAIROS_TABLE.get_mut(pid).expect("pid missing");
    budget.used_ns += elapsed_ns;

    let window_elapsed = ktime_get_ns() - budget.window_start_ns;
    if window_elapsed >= KAIROS_WINDOW_NS {
        // Reset fenêtre
        budget.window_start_ns = ktime_get_ns();
        budget.cumulative_overage_ns = budget.used_ns.saturating_sub(budget.limit_ns);
        budget.used_ns = 0;
    }

    // Throttle à 100% (ralentir sans tuer)
    if budget.used_ns >= budget.limit_ns {
        scheduler_throttle(pid);
        exoledger_log(Event::KairosThrottle { pid, used: budget.used_ns });
    }

    // Kill à 200% cumulé
    if budget.cumulative_overage_ns >= budget.limit_ns * 2 {
        process_kill(pid, Signal::KairosExceeded);
        exoledger_log(Event::KairosKill { pid });
    }
}
```

**Validation :** `security_test::exokairos_throttle_at_100pct PASS`, `security_test::exokairos_kill_at_200pct PASS`

---

### 3.6 ExoLedger — Journal Immuable

**Fichier :** `kernel/src/security/exoledger.rs`

**État Strata :** Journal persisté dans ExoFS partition ROOT, adresse fournie par BootInfo v2.

```rust
pub struct LedgerEntry {
    pub seq:        u64,            // Numéro de séquence monotone
    pub timestamp:  u64,            // ktime_get_ns()
    pub epoch_id:   EpochId,        // Epoch ExoFS courante
    pub event_type: EventType,
    pub payload:    [u8; 96],
    pub prev_hash:  [u8; 32],       // BLAKE3 de l'entrée précédente
    pub entry_hash: [u8; 32],       // BLAKE3 de cette entrée
}

pub fn ledger_append(event: Event) {
    let prev = LAST_ENTRY_HASH.load();
    let seq  = LEDGER_SEQ.fetch_add(1, Ordering::SeqCst);

    let entry = LedgerEntry {
        seq, timestamp: ktime_get_ns(),
        epoch_id: exofs_current_epoch(),
        event_type: event.kind(),
        payload: event.encode(),
        prev_hash: prev,
        entry_hash: [0; 32], // calculé ci-dessous
    };

    let hash = blake3::hash(bytes_of(&entry));
    let entry = LedgerEntry { entry_hash: hash.into(), ..entry };

    LAST_ENTRY_HASH.store(hash.into());
    exofs_sealed_append(LEDGER_OBJECT, bytes_of(&entry));
}

// Vérification chaîne (boot + exo audit --verify-chain)
pub fn ledger_verify_chain() -> Result<(), LedgerError> {
    let entries = exofs_read_all(LEDGER_OBJECT)?;
    let mut prev_hash = [0u8; 32];
    for entry in &entries {
        if entry.prev_hash != prev_hash {
            return Err(LedgerError::ChainBroken { seq: entry.seq });
        }
        let computed = blake3::hash(/* entry sans entry_hash */);
        if computed.as_bytes() != &entry.entry_hash {
            return Err(LedgerError::EntryCorrupted { seq: entry.seq });
        }
        prev_hash = entry.entry_hash;
    }
    Ok(())
}
```

**Validation :** `security_test::exoledger_chain_integrity PASS`, `security_test::exoledger_immutable PASS`

---

### 3.7 ExoShield-IOMMU — Isolation DMA

**Fichier :** `kernel/src/security/exoshield_iommu.rs`

*(Distinct du serveur exo_shield Ring1 — c'est la protection DMA kernel)*

```rust
// Domaines IOMMU créés avant tout driver
pub fn iommu_init_domains() {
    IOMMU.create_domain(DomainId::NET,       AccessPolicy::ReadWrite);
    IOMMU.create_domain(DomainId::BLOCK,     AccessPolicy::ReadWrite);
    IOMMU.create_domain(DomainId::BLACKHOLE, AccessPolicy::None);
}

// Chaque driver déclaré reçoit un domaine
// SYS_DMA_ALLOC = 534 : retourne (virt, iova) dans le domaine du driver
// Toute DMA hors domaine → IommuFaultQueue + abort
```

**Validation :** `security_test::exoshield_dma_fault PASS`

---

### 3.8 ExoNMI — Watchdog Permanent

**Fichier :** `kernel/src/security/exonmi.rs`

```rust
// NMI handler (IDT entrée 2)
pub extern "C" fn nmi_handler() {
    // 1. Incrémenter heartbeat (ExoPhoenix sentinel lit ce compteur)
    NMI_HEARTBEAT.fetch_add(1, Ordering::SeqCst);

    // 2. Vérifier canaries stack kernel de chaque CPU actif
    for cpu in active_cpus() {
        if !cpu.kernel_stack_canary_valid() {
            trigger_emergency_phoenix_switch(Reason::StackCorruption);
        }
    }

    // 3. IDT integrity check (hash première entrée)
    let idt_hash = blake3_hash_idt_entry_0();
    if idt_hash != EXPECTED_IDT_ENTRY_0_HASH.load() {
        trigger_emergency_phoenix_switch(Reason::IdtCorruption);
    }

    // 4. PMC anomaly → exo_shield IPC (non-bloquant)
    if let Some(anomaly) = pmc_detect_anomaly() {
        ipc_send_nonblocking(PID_EXOSHIELD,
            ShieldMsg::PmcAnomaly { core_id: current_cpu(), anomaly });
    }
}

// Armement watchdog
pub fn exonmi_arm(interval_ms: u64) {
    NMI_INTERVAL_MS.store(interval_ms, Ordering::Release);
    apic_set_nmi_timer(interval_ms);
}
```

**Validation :** `security_test::exonmi_watchdog_fires PASS`

---

## 4. Composants Supplémentaires — Tableau Complet

| Composant | Couche | État Strata | Action requise |
|---|---|---|---|
| ExoSeal | 1 | Actif | Hash chain kernel + ring1 |
| ExoCage (CET+IBT+SMEP+SMAP+KPTI) | 2 | Actif | Tous APs + verify_active() |
| ExoVeil (PKS) | 2 | Conditionnel | Ice Lake+ seulement, fallback SW |
| CFG Lock | 2 | Actif | `cfg_lock()` appelé à SECURITY_READY |
| SafeStack | 2 | Actif si CET absent | Auto |
| KASLR | 2 | Actif | BootInfo v2 entropy 64B |
| Zero Trust MLS | 3 | Actif | `check_ipc()` dans fast path |
| CapToken | 4a | Actif | Chaque accès FS + IPC + réseau |
| ExoKairos | 4b | Actif | Fenêtre 1s, throttle 100%, kill 200% |
| Pledge | 4a | Actif v0.2.0 | `PledgeSet` pour chaque app POSIX |
| ipc_policy | 3 | Actif | `check_direct_ipc()` câblé |
| ExoArgos (PMC) | 2 | Actif | Résultats → exo_shield PMC_ANOMALY |
| ExoNMI | 7 | Actif | 200ms interval |
| Stack Protector | 2 | Actif | — |
| ExoLedger | 5 | Actif | Journal ExoFS sealed |
| audit logger | 5 | Actif | — |
| integrity_check | 5 | Actif | — |
| code_signing (Ed25519) | 1 | Actif | Clés dev en QEMU, prod PKI en v0.2.0 final |
| ExoShield-IOMMU | 6 | Actif | Domaines NET/BLOCK/BLACKHOLE |
| exo_shield engine | 8 | Actif Ring1 | Voir SPEC-EXOSHIELD-STRATA |
| exo_shield hooks | 8 | **Actif** | CORR-75 câblé |
| exo_shield sandbox | 8 | **Actif** | CORR-75 câblé |
| exo_shield network | 8 | **Actif** | CORR-75 câblé |
| exo_shield ML | 8 | **Actif** | CORR-75 câblé, modèle v0 |
| exo_shield forensics | 8 | Sur demande | — |

Score cible Strata : **27/27 actifs → ~98%**

---

## 5. Actions P0 Restantes

### A1 — CORR-75 : Activer modules orphelins exo_shield (2-3h)

Les 5 modules (`hooks`, `sandbox`, `network`, `ml`, `forensics`) sont implémentés mais absents de `lib.rs`. À câbler dans `exo_shield_init()`.

### A2 — CFG Lock (30min)

```rust
// kernel/src/init.rs — étape 18 (SECURITY_READY)
security::cfg_lock(); // Aucune nouvelle cible CFI après ce point
```

### A3 — ipc_policy fast path (1h)

Voir section 3.3 ci-dessus.

### A4 — Pledge pour apps POSIX (2h)

```rust
// Dans exo-pkg, au moment de exo compat install :
let pledge_set = analyze_binary_syscalls(&binary)?;
install_pledge(pid, pledge_set);
```

### A5 — ExoArgos bridge vers exo_shield (1h)

```rust
// kernel/src/security/exoargos.rs
fn send_anomaly_to_shield(anomaly: PmcAnomaly) {
    ipc_send_nonblocking(PID_EXOSHIELD,
        ShieldMsg::PmcAnomaly { core_id: anomaly.core, .. });
}
```

---

## 6. Suite de Tests Sécurité — 13 Tests Requis

```
security_test::exoseal_verify_chain              PASS
security_test::exocage_all_mechanisms            PASS
security_test::zerotrust_ipc_blocked             PASS
security_test::captoken_access_denied            PASS
security_test::captoken_revocation_immediate     PASS
security_test::captoken_no_privilege_escalation  PASS
security_test::exokairos_throttle_at_100pct      PASS
security_test::exokairos_kill_at_200pct          PASS
security_test::exoledger_chain_integrity         PASS
security_test::exoledger_immutable               PASS
security_test::exoshield_dma_fault               PASS
security_test::exonmi_watchdog_fires             PASS
security_test::exoshield_sandbox_escape_blocked  PASS  ← nouveau Strata
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-SECURITY-COMPLETE-STRATA.md*
