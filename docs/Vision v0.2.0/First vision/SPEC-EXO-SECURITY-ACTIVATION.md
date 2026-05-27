# SPEC-EXO-SECURITY-ACTIVATION — Activation Complète de la Sécurité
## ExoOS v0.2.0 — Tous les Composants en Production

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0 — CRITIQUE

---

## 1. Objectif

En v0.1.x, les composants de sécurité ExoOS étaient présents dans le code mais partiellement activés — certains fonctionnaient en mode "audit-only" ou étaient bypassés à l'initialisation. La v0.2.0 exige que **tous** les composants soient **actifs en production**, non bypassables, et mutuellement cohérents.

**Critère de validation ultime :** Exécuter la suite de tests `security_integration_test` — 100% PASS, zéro bypass détecté.

---

## 2. Carte des Composants de Sécurité

```
                    ┌─────────────────────────────┐
                    │         ExoSeal             │  Couche 1 — Boot integrity
                    │  (inverted boot, hash chain) │
                    └──────────────┬──────────────┘
                                   │
                    ┌──────────────▼──────────────┐
                    │         ExoCage             │  Couche 2 — Isolation hardware
                    │  (CET shadow stack, SMEP,   │
                    │   SMAP, KPTI, CFI)          │
                    └──────────────┬──────────────┘
                                   │
          ┌────────────────────────▼──────────────────────────┐
          │                  Zero Trust Layer                  │  Couche 3 — Vérification
          │   (chaque IPC vérifié, chaque objet labelisé)      │  continue
          └────┬──────────────────────────────────────────┬───┘
               │                                          │
┌──────────────▼────────────┐            ┌───────────────▼─────────────┐
│       CapToken System     │            │         ExoKairos            │  Couche 4 — Droits
│  (chaque ressource =      │            │  (budgets temporels inline,  │  & budgets
│   token + droits précis)  │            │   anti-DoS par processus)    │
└──────────────┬────────────┘            └───────────────┬─────────────┘
               │                                          │
               └──────────────────┬───────────────────────┘
                                   │
                    ┌──────────────▼──────────────┐
                    │         ExoLedger           │  Couche 5 — Audit
                    │  (journal immutable,        │  immuable
                    │   signé, epoch-linked)      │
                    └──────────────┬──────────────┘
                                   │
          ┌────────────────────────▼──────────────────────────┐
          │                  ExoShield                         │  Couche 6 — Isolation
          │   (IOMMU statique NIC, DMA isolation,             │  physique
          │    PCI claim verification)                         │
          └────────────────────────┬──────────────────────────┘
                                   │
                    ┌──────────────▼──────────────┐
                    │          ExoNMI             │  Couche 7 — Watchdog
                    │  (NMI watchdog, kernel      │  permanent
                    │   integrity runtime check)  │
                    └─────────────────────────────┘
```

---

## 3. Spécification par Composant

### 3.1 ExoSeal — Boot Integrity

**Fichier :** `kernel/src/security/exoseal.rs`

**Principe :** Inverted boot — le kernel vérifie sa propre intégrité avant de démarrer quoi que ce soit. Si la chaîne de hash est invalide, le boot s'arrête.

**État cible v0.2.0 :**

```rust
// Dans early_init.rs — Phase 0 (avant tout le reste)
pub fn exoseal_verify_boot_chain() -> Result<(), SealError> {
    // 1. Hash du kernel binaire (BLAKE3)
    let kernel_hash = blake3_hash_kernel_image();
    
    // 2. Vérification contre la valeur stockée dans le TPM ou en flash
    let expected = tpm_read_expected_kernel_hash()?;
    if kernel_hash != expected {
        return Err(SealError::KernelTampered);
    }
    
    // 3. Hash des modules Ring1 (serveurs)
    for server in RING1_SERVER_LIST {
        let hash = blake3_hash_server(server);
        let exp = tpm_read_server_hash(server.id)?;
        if hash != exp { return Err(SealError::ServerTampered(server.id)); }
    }
    
    // 4. Marquer le système comme "sealed"
    BOOT_SEAL_STATE.store(SealState::Verified, Ordering::Release);
    Ok(())
}
```

**Checklist v0.2.0 :**
- [ ] `exoseal_verify_boot_chain()` appelé en Phase 0, avant `security_init()`
- [ ] Arrêt complet (panic + LED/beep) si hash invalide
- [ ] En mode QEMU/émulation : hash de référence stocké en mémoire (pas TPM)
- [ ] Mode développement : `EXOSEAL_DEV_BYPASS=1` uniquement pour debug, loggé dans ExoLedger

---

### 3.2 ExoCage — Isolation Matérielle

**Fichier :** `kernel/src/security/exocage.rs`

**Principe :** Activer tous les mécanismes matériels d'isolation disponibles sur x86_64.

**Mécanismes à activer en v0.2.0 :**

| Mécanisme | Registre/Instruction | Description |
|-----------|---------------------|-------------|
| SMEP | CR4.SMEP | Interdit l'exécution de pages user en mode kernel |
| SMAP | CR4.SMAP | Interdit l'accès aux pages user en mode kernel |
| KPTI | CR3 isolé | Tableaux de pages séparés user/kernel |
| CET SS | MSR_IA32_U_CET | Shadow stack pour Ring3 |
| CET IBT | MSR_IA32_S_CET | Indirect branch tracking |
| NX/XD | EFER.NXE | Pages données non exécutables |
| IBRS | MSR_IA32_SPEC_CTRL | Mitigation Spectre v2 |
| SSBD | MSR_IA32_SPEC_CTRL | Mitigation Spectre v4 |

**Validation :**
```rust
pub fn exocage_verify_active() -> ExocageStatus {
    ExocageStatus {
        smep: read_cr4().contains(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION),
        smap: read_cr4().contains(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION),
        kpti: KPTI_ACTIVE.load(Ordering::Acquire),
        cet_ss: rdmsr(MSR_IA32_U_CET) & CET_SS_EN != 0,
        cet_ibt: rdmsr(MSR_IA32_S_CET) & CET_IBT_EN != 0,
        nx: rdmsr(MSR_EFER) & EFER_NXE != 0,
        ibrs: rdmsr(MSR_IA32_SPEC_CTRL) & IBRS_ENABLE != 0,
        ssbd: rdmsr(MSR_IA32_SPEC_CTRL) & SSBD_ENABLE != 0,
    }
}
```

**Checklist v0.2.0 :**
- [ ] Tous les flags activés sur le BSP et tous les APs (après INIT IPI)
- [ ] `exocage_verify_active()` appelé après Phase 5 `security_init()` avec panic si incomplet
- [ ] CET shadow stack activé pour tous les threads Ring3
- [ ] KPTI actif sur toutes les transitions kernel↔user

---

### 3.3 Zero Trust Layer — Vérification Continue des IPC

**Fichier :** `kernel/src/security/zero_trust/`

**Principe :** Chaque message IPC est porteur d'un label de contexte. Le kernel vérifie que l'émetteur a le droit d'envoyer ce message à ce destinataire avec ce contenu.

**Labels Zero Trust :**
```rust
pub struct ZeroTrustLabel {
    pub sender_cap:   CapToken,    // Qui envoie ?
    pub receiver_cap: CapToken,    // Qui reçoit ?
    pub action:       IpcAction,   // Que demande-t-on ?
    pub scope:        ObjectId,    // Sur quel objet ?
    pub context_hash: u64,         // Hash du contexte (anti-replay)
}
```

**Politique v0.2.0 :**
- Un processus Ring3 ne peut jamais envoyer un IPC directement à un autre Ring3 sans passer par un serveur Ring1 (sauf SHM autorisé)
- Tout IPC Ring3 → Ring1 est vérifié contre le manifest de capabilities du processus
- Tout IPC Ring1 → Ring0 est vérifié contre la table des droits serveur (SRV-01/02/04)
- Les IPC non conformes sont : bloqués + loggés dans ExoLedger + comptés dans ExoKairos

**Checklist v0.2.0 :**
- [ ] `zero_trust::check_ipc()` appelé sur **chaque** `ipc_send` dans le fast path
- [ ] Pas de bypass possible (pas de flag "trusted" implicite)
- [ ] Les APs (processeurs secondaires) héritent de la politique du BSP

---

### 3.4 CapToken System — Chaque Ressource est un Token

**Fichiers :** `kernel/src/security/capability/`

**Principe :** Il n'existe aucune ressource accessible sans capability. Pas de fd global, pas de path global non restreint.

**Types de capabilities v0.2.0 :**

```rust
pub enum CapabilityDomain {
    Fs(FsRights, FsScope),         // Accès filesystem
    Net(NetRights, NetScope),      // Accès réseau
    Ipc(IpcRights, EndpointId),    // Envoi/réception IPC
    Mem(MemRights, VirtAddr, usize), // Région mémoire
    Time(TimeRights),              // Accès horloge
    Crypto(CryptoRights, KeyId),   // Opération crypto
    Device(DevRights, DeviceId),   // Accès périphérique
    Display(DisplayRights, Region), // Accès framebuffer
    Debug(DebugRights),            // Debug/trace (fortement restreint)
}
```

**Règles d'octroi v0.2.0 :**
1. Seul `security_server` peut octroyer des capabilities
2. Un processus ne peut pas déléguer plus que ce qu'il possède (RULE-CAP-01)
3. Toute dérivation de capability est loggée dans ExoLedger
4. Les capabilities ont une durée de vie : `session` / `permanent` / `n_uses`
5. La révocation est immédiate et propagée à tous les `CapToken` dérivés

**Checklist v0.2.0 :**
- [ ] `capability::verify()` sur chaque accès à une ressource (FS, IPC, net, mem)
- [ ] `capability::revoke()` propageant à tous les tokens enfants
- [ ] Zéro "root" ou accès total implicite — même les serveurs Ring1 ont des caps limitées
- [ ] Test de régression : tentative d'accès avec capability révoquée → `EXO-0410`

---

### 3.5 ExoKairos — Budgets Temporels

**Fichier :** `kernel/src/security/exokairos.rs`

**Principe :** Chaque processus a un budget de temps CPU par unité de temps. Dépassement = action configurable (throttle / kill / alert).

**Configuration par défaut v0.2.0 :**

```toml
[exokairos.defaults]
# Budget par fenêtre de 1 seconde
ring3_default_ms = 100       # App standard : 100ms/s = 10% CPU max
ring3_elevated_ms = 500      # App avec cap TIME_ELEVATED
ring1_server_ms = 800        # Serveurs Ring1 : 80% CPU max
ring0_unlimited = true       # Ring0 n'est pas limité

[exokairos.actions]
at_50pct_budget  = "log"         # À 50% du budget : log discret
at_90pct_budget  = "warn"        # À 90% du budget : avertissement
at_100pct_budget = "throttle"    # À 100% : throttle (pas kill)
at_200pct_budget = "kill"        # À 200% cumulé : kill + audit
```

**Inline dans le scheduler :**
```rust
// Dans scheduler/core/switch.rs — à chaque tick
fn update_kairos_budget(tcb: &mut Tcb, elapsed_ns: u64) {
    let budget = &mut tcb.kairos_budget;
    budget.used_ns += elapsed_ns;
    
    if budget.used_ns > budget.limit_ns {
        // Budget dépassé
        exokairos_enforce(tcb, budget.used_ns, budget.limit_ns);
        // exokairos_enforce : throttle ou kill selon la config
    }
}
```

**Checklist v0.2.0 :**
- [ ] Budget initialisé à la création de chaque processus Ring3
- [ ] Mise à jour à chaque context switch
- [ ] `exokairos_enforce()` appelé avant le retour en Ring3
- [ ] Dépassement loggé dans ExoLedger avec PID, budget, action

---

### 3.6 ExoLedger — Audit Immuable

**Fichier :** `kernel/src/security/exoledger.rs`

**Principe :** Journal immuable et signé de tous les événements de sécurité. Les entrées sont liées aux epochs ExoFS — on ne peut pas supprimer une entrée sans casser la chaîne.

**Format d'entrée :**
```rust
pub struct LedgerEntry {
    pub id:         u64,           // Monotone global
    pub epoch:      EpochId,       // Epoch ExoFS au moment de l'événement
    pub timestamp:  u64,           // ns depuis boot
    pub pid:        u32,           // Processus concerné
    pub event:      LedgerEvent,   // Type d'événement
    pub cap_token:  Option<u64>,   // Capability impliquée
    pub result:     LedgerResult,  // Allow / Deny
    pub chain_hash: [u8; 32],      // BLAKE3(prev_entry || this_entry)
}
```

**Événements audités v0.2.0 :**
- Octroi et révocation de capabilities
- Accès refusés (toutes raisons)
- Montages/démontages de volumes
- Démarrage/arrêt de processus Ring1
- Bascule ExoPhoenix (avec état SSR)
- Toute opération crypto dans `crypto_server`
- Installations/désinstallations de paquets

**Checklist v0.2.0 :**
- [ ] ExoLedger persisté dans ExoFS (objet sealed, type `secret`)
- [ ] Chaîne de hash vérifiée au boot (intégrité depuis le dernier arrêt)
- [ ] `exo audit` affiche les 50 dernières entrées avec vérification de chaîne
- [ ] Impossibilité de supprimer des entrées (objet ExoFS sealed + immuable)

---

### 3.7 ExoShield — Isolation Physique DMA/IOMMU

**Fichier :** `kernel/src/security/exoveil.rs` + drivers IOMMU

**Principe :** L'IOMMU est configuré statiquement à l'init. Aucun périphérique ne peut accéder à de la mémoire non autorisée via DMA.

**Configuration statique v0.2.0 :**

```
NIC (virtio-net) :
  IOMMU domain : NET_DMA_DOMAIN
  Plage autorisée : [net_rx_ring, net_tx_ring]  ← uniquement les rings
  Tout accès hors plage : fault → ExoLedger + arrêt DMA

SATA/NVMe (virtio-blk) :
  IOMMU domain : BLOCK_DMA_DOMAIN
  Plage autorisée : [block_dma_buffer]
  Tout accès hors plage : fault → ExoLedger

Tout autre périphérique non réclamé :
  IOMMU domain : BLACKHOLE_DOMAIN
  Aucun accès mémoire autorisé
```

**Checklist v0.2.0 :**
- [ ] IOMMU activé (Intel VT-d ou AMD-Vi) avant tout démarrage de driver Ring1
- [ ] Chaque `SYS_DMA_ALLOC=534` retourne un (virt, iova) pair dans le domaine correct
- [ ] `IommuFaultQueue` actif — les fautes sont enregistrées, pas ignorées
- [ ] Test : DMA out-of-bounds → fault détectée + loggée + DMA stoppé

---

### 3.8 ExoNMI — Watchdog Kernel

**Fichier :** `kernel/src/security/exonmi.rs`

**Principe :** Watchdog NMI qui vérifie périodiquement l'intégrité du kernel en cours d'exécution (stack canaries, guard pages, IDT integrity).

**Vérifications NMI v0.2.0 :**
```rust
pub fn nmi_watchdog_handler() {
    // 1. Vérifier les canaries de stack kernel
    verify_all_kernel_stack_canaries();
    
    // 2. Vérifier l'intégrité de l'IDT (pas de hook non autorisé)
    verify_idt_integrity();
    
    // 3. Vérifier que SMEP/SMAP sont toujours actifs
    assert!(read_cr4().contains(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION));
    assert!(read_cr4().contains(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION));
    
    // 4. Incrémenter le compteur heartbeat (vu par ExoPhoenix sentinel)
    NMI_HEARTBEAT.fetch_add(1, Ordering::Relaxed);
}
```

**Checklist v0.2.0 :**
- [ ] NMI watchdog armé toutes les 200ms
- [ ] Heartbeat visible par ExoPhoenix sentinel (utilisé pour détecter le freeze kernel)
- [ ] Canaries sur toutes les stacks kernel (via `memory/integrity/canary.rs`)
- [ ] IDT integrity check avec hash stocké au boot

---

## 4. Suite de Tests de Sécurité

Fichier : `tests/security_integration_tests.rs`

```
security_test::exoseal_verify_chain          PASS
security_test::exocage_all_mechanisms        PASS
security_test::zerotrust_ipc_blocked         PASS
security_test::captoken_access_denied        PASS
security_test::captoken_revocation_immediate PASS
security_test::captoken_no_privilege_escalation PASS
security_test::exokairos_throttle_at_100pct  PASS
security_test::exokairos_kill_at_200pct      PASS
security_test::exoledger_chain_integrity     PASS
security_test::exoledger_immutable           PASS
security_test::exoshield_dma_fault           PASS
security_test::exonmi_watchdog_fires         PASS
security_test::full_attack_simulation        PASS  ← tentative d'escalade

Total: 13 PASS / 0 FAIL / 0 SKIP
```

---

## 5. Activation dans le Boot (Séquence Complète)

```
Phase 0:  ExoSeal verify_boot_chain()          ← avant TOUT
Phase 1:  ExoCage activate_hardware()          ← CR4, MSR, KPTI
Phase 2:  ExoNMI arm_watchdog()               ← 200ms NMI
Phase 3:  memory_init()                        ← buddy, SLUB
Phase 4:  scheduler_init()                     ← avec ExoKairos
Phase 5:  security_init()                      ← CapToken, ZeroTrust, ExoLedger
Phase 6:  ExoShield configure_iommu()         ← AVANT les drivers
Phase 7:  ipc_init()                           ← avec ZeroTrust labels
Phase 8:  FS init (ExoFS mount)               ← ExoLedger persisté
Phase 9:  Ring1 servers start                  ← avec capabilities limitées
Phase 10: SECURITY_READY flag                  ← tout le monde peut l'interroger
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-SECURITY-ACTIVATION.md*
