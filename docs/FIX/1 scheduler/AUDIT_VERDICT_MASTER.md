# AUDIT VERDICT MASTER — ExoOS commit 93616537
**Analysé par : Claude (analyse à froid, code réel lu)**  
**Date : 2026-04-19**  
**Sources : Audit_scheduler_by_qwen (×2 doublon), RAPPORT_CLAUDE3_SECURITY_EXOSHIELD_CRYPTO**

---

## NOTA BENE : Les deux fichiers Qwen sont IDENTIQUES (doublon pur)

Les fichiers `Audit_scheduer_by_qwen.md` et `Audit_scheduer_by_qwen__1_.md` ont le même
contenu octet pour octet. Un seul audit scheduler Qwen existe.

---

## PARTIE 1 — SCHEDULER (Audit Qwen)

### ✅ CONFIRMÉ — P0-01 : Incohérence MAX_CPUS (atténuée)

**Claim Qwen :** topology.rs=512, percpu.rs=512 vs preempt.rs=256.  
**Verdict code réel :** PARTIELLEMENT FAUX sur les valeurs citées.

Code réel vérifié :
- `scheduler/smp/topology.rs:14` → `MAX_CPUS = 256` ✓
- `arch/x86_64/smp/percpu.rs:24` → `MAX_CPUS = 256` ✓
- `scheduler/core/preempt.rs:31` → `MAX_CPUS = 256` ✓
- `memory/arch_iface.rs:43` → `MAX_CPUS = 256` ✓
- **OUTLIERS réels :** `virt/stolen_time.rs:37` → `512` (local, KVM only), `frame/reclaim.rs:42` → `512` (local, memory reclaim)

**Bug réel :** Les deux modules outliers définissent une constante locale `MAX_CPUS = 512`
au lieu d'importer la canonique. En pratique ces deux modules accèdent à des tableaux
sur-alloués par rapport aux structures scheduler (256). Risque de confusion structurelle
si on étend à 512 un jour.

**Correction assignée : CORR-55** — uniformiser via import depuis topology.rs

---

### ✅ CONFIRMÉ — P0-02 : Double stockage TCB per-CPU

**Verdict :** CONFIRMÉ EXACT.

`switch.rs:39` : `pub static CURRENT_THREAD_PER_CPU: [AtomicUsize; MAX_CPUS]`  
`switch.rs:238` : écrit dans `CURRENT_THREAD_PER_CPU[cpu]`  
`switch.rs:243` : écrit dans `percpu::set_current_tcb()` (GS:[0x20])

**Nuance importante :** `ipc/sync/sched_hooks.rs:187` lit `CURRENT_THREAD_PER_CPU`
pour observer le thread courant d'un CPU distant. Ce n'est donc PAS purement redondant —
le tableau sert d'interface de visibilité cross-CPU. La correction n'est PAS de supprimer
le tableau mais de garantir la cohérence entre les deux sources de vérité.

**Correction assignée : CORR-56**

---

### ✅ CONFIRMÉ — P1-03 : Affinité limitée à 64 CPUs

**Verdict :** CONFIRMÉ.  
`affinity.rs:58` : `affinity & (1u64 << cpu.0)` — masque u64 = 64 bits max.  
Aucun check `cpu.0 < 64` explicite mais le masque overflow silencieusement pour cpu >= 64.

**Correction assignée : CORR-57** — étendre CpuSet à `[u64; 4]` (256 bits = 256 CPUs)

---

### ⚠️ PARTIELLEMENT CONFIRMÉ — P1-04 : Orderings Relaxed

**Verdict :** NUANCÉ.

Les `Relaxed` sur les compteurs de stats (`nr_running`, `picks_total`, `load_avg`) sont
**acceptables** — ce sont des statistiques de monitoring non-critiques.

Les `Relaxed` sur `PREEMPT_COUNT` et `vruntime` sont **problématiques** sur SMP car
un CPU peut ne pas voir la mise à jour de préemption d'un autre CPU avant de tenter
une migration. Requires Release/Acquire sur les chemins migration.

**Correction assignée : CORR-58** — ciblée (pas de remplacement global)

---

### ❌ REJETÉ — P2-05 : schedule_block() ne retire pas de la runqueue

**Verdict :** INCORRECT — Design intentionnel correctement implémenté.

Le contrat de `schedule_block()` exige que l'appelant positionne l'état du thread
sur `Sleeping`/`Uninterruptible` AVANT l'appel. `pick_next_task` exclut les threads
non-Runnable. Si aucun autre thread disponible, le thread est remis en Runnable
(comportement de dégradation gracieuse documenté). Ce n'est pas un bug.

**Document de rejet : REJECTED-P2-05.md**

---

### ✅ CONFIRMÉ — P2-06 : assert_preempt_disabled inactive en release

**Verdict :** CONFIRMÉ.  
`preempt.rs:234` : `debug_assert!(...)` — disparaît en release.

**Correction assignée : CORR-59**

---

## PARTIE 2 — SECURITY / EXOSHIELD / CRYPTO (Audit Claude3)

### 🔴 P0

| ID | Claim | Verdict | CORR |
|----|-------|---------|------|
| BUG-S1 | #CP handler IDT déconnecté de cp_handler | ✅ CONFIRMÉ EXACT | CORR-60 |
| BUG-S2 | ExoCordon bypass kernel IPC path | ✅ CONFIRMÉ EXACT | CORR-61 |
| BUG-S3 | PID mapping incohérent en exocordon | ✅ CONFIRMÉ (avec correction) | CORR-62 |

### 🟡 P1

| ID | Claim | Verdict | CORR |
|----|-------|---------|------|
| BUG-S4 | ct_u64_gte non constant-time (||) | ✅ CONFIRMÉ EXACT | CORR-63 |
| BUG-S5 | fetch_sub underflow SMP calls_left | ✅ CONFIRMÉ EXACT | CORR-64 |
| BUG-S6 | KERNEL_SECRET static mut sans sync | ✅ CONFIRMÉ EXACT | CORR-65 |
| BUG-S7 | CPUID edx clobber exoveil_init | ✅ CONFIRMÉ EXACT | CORR-66 |
| BUG-S8 | ExoLedger P0 race prev_hash SMP | ✅ CONFIRMÉ EXACT | CORR-67 |
| BUG-S9 | NIC IOMMU ordre boot incorrect | ✅ CONFIRMÉ EXACT | CORR-68 |
| BUG-S10 | exo_shield stub vide | ✅ CONFIRMÉ (connu) | Phase 3.1 |

### 🔵 P2

| ID | Claim | Verdict | CORR |
|----|-------|---------|------|
| BUG-S11 | CAP-05 early return timing diff | ✅ CONFIRMÉ | CORR-69 |
| BUG-S12 | XChaCha20+AES-GCM indisponibles | ✅ CONFIRMÉ | CORR-70 |
| BUG-S13 | Deadline table O(N) | ✅ CONFIRMÉ (Phase 3.2) | Doc only |
| BUG-S14 | Quota ExoCordon non rechargeable | ✅ CONFIRMÉ | CORR-71 |

---

## Tableau des propriétés TLA+ impactées

| Propriété | Status avant corrections | Bugs bloquants |
|-----------|--------------------------|----------------|
| CetNoRop | ❌ FAUSSE en production | CORR-60 |
| BudgetMonotonicity | ❌ VIOLÉE sur SMP | CORR-64 |
| BootSafety (NIC window) | ⚠️ Fenêtre boot | CORR-68 |
| P0Immutability | ⚠️ Race SMP | CORR-67 |
| IPC lateral movement blocked | ❌ FAUSSE (bypass) | CORR-61 |

---

## Ordre de priorité absolu

```
PHASE 0 (avant tout commit SMP) :
  CORR-60  — #CP IDT fix  (CetNoRop false = zero CET protection)
  CORR-61  — ExoCordon kernel path  (IPC security = illusoire)
  CORR-56  — Double TCB  (corruption mémoire SMP)
  CORR-64  — fetch_sub underflow  (budget contournable)
  CORR-68  — Boot order NIC  (fenêtre exfiltration)

PHASE 1 (avant tests SMP étendus) :
  CORR-62  — PID mapping
  CORR-63  — ct_u64_gte
  CORR-65  — KERNEL_SECRET
  CORR-67  — ExoLedger race
  CORR-58  — Memory ordering migration
  CORR-66  — CPUID clobber

PHASE 2 (qualité / robustesse) :
  CORR-55  — MAX_CPUS import
  CORR-57  — Affinité 256 bits
  CORR-59  — assert_preempt release
  CORR-69  — CAP-05 timing
  CORR-71  — Quota recharge
  CORR-70  — AEAD (Phase 3.2)
```
