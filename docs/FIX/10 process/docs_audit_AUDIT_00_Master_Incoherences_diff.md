--- docs/audit/AUDIT_00_Master_Incoherences.md (原始)


+++ docs/audit/AUDIT_00_Master_Incoherences.md (修改后)
# ExoOS — Audit Complet des Incohérences de Processus

## 🎯 Objectif : Atteindre 100% de conformité

**Document d'audit consolidé** — Synthèse des corrections CORR-01 à CORR-54 + SRV-05
**Sources** : docs/recast/ExoOS_Corrections_*.md, GI-01 à GI-04, Architecture v7, Driver Framework v10
**Date** : Avril 2026
**Statut** : Plan d'action P0/P1/P2 pour validation finale

---

## 📊 État des lieux global

| Catégorie | Nombre | Statut | Priorité |
|-----------|--------|--------|----------|
| **Corrections critiques (🔴)** | 9 | Partiellement implémentées | P0 |
| **Corrections majeures (🟠)** | 23 | En cours | P1 |
| **Lacunes documentaires (⚠️)** | 16 | À documenter | P2 |
| **Corrections mineures (🔵)** | 6 | Acceptées | P3 |
| **TOTAL** | **54 + SRV-05** | **65% conforme** | — |

---

## 🔴 CRITIQUE — P0 (Bloquant production)

### CORR-04 : Allocation heap en contexte ISR

**Fichier** : `kernel/src/arch/x86_64/irq/dispatch.rs`
**Problème** : `Vec<IpcEndpoint>` utilisé dans `dispatch_irq()` — allocation heap interdite en ISR
**Impact** : Violation S-01 (TCB), risque de panic en production
**Correction** : Remplacer par tableau fixe `[Option<IpcEndpoint>; MAX_HANDLERS_PER_IRQ]` ou `heapless::Vec`
**Référence** : `docs/recast/ExoOS_Corrections_03_Driver_Framework.md`, GI-03 P1.1

```rust
// ❌ AVANT — ALLOCATION HEAP INTERDITE EN ISR
let mut endpoints: Vec<IpcEndpoint> = Vec::new();

// ✅ APRÈS — TABLEAU FIXE SUR LA PILE
let mut endpoints: [Option<IpcEndpoint>; 8] = [None; 8];
// OU heapless::Vec<IpcEndpoint, 8> (no_alloc)
```

**Statut actuel** : ⚠️ **NON RÉSOLU** — Vérifier `dispatch.rs` et `routing.rs`
**Action requise** : Audit complet des fonctions appelées depuis IRQ handler

---

### CORR-32 : TOCTOU dans sys_pci_claim

**Fichier** : `kernel/src/drivers/device_claims.rs`
**Problème** : Vérifications (`MMIO_WHITELIST`, `is_ram_region`) effectuées **avant** acquisition du lock `DEVICE_CLAIMS.write()`
**Impact** : Fenêtre TOCTOU exploitable pour claimer une région déjà allouée
**Correction** : Acquérir le lock avant toute vérification (voir code complet dans CORR-32)
**Référence** : `docs/recast/ExoOS_Corrections_07_Critiques_Majeures_v2.md`

```rust
// ❌ AVANT — TOCTOU
if !MMIO_WHITELIST.contains(phys_base, size) { return Err(...); }
let mut claims = DEVICE_CLAIMS.write(); // ← TROP TARD

// ✅ APRÈS — LOCK D'ABORD
let _irq_guard = arch::irq_save();
let mut claims = DEVICE_CLAIMS.write();
// Puis vérifications sous lock
if !MMIO_WHITELIST.contains(phys_base, size) { return Err(...); }
```

**Statut actuel** : ⚠️ **PARTIEL** — Lock ajouté mais vérifications BDF manquantes
**Action requise** : Ajouter vérification unicité BDF (`claims.iter().any(|c| c.bdf == Some(b))`)

---

### CORR-41 : verify_cap_token() non constant-time

**Fichier** : `libs/exo-types/src/cap.rs`
**Problème** : Fonction contient `// TODO Phase 1 : LAC-01` — vulnérable aux attaques timing
**Impact** : Fuite d'information via timing sur validité du token
**Correction** : Utiliser crate `subtle` pour comparaisons constant-time
**Référence** : `docs/recast/ExoOS_Corrections_07_Critiques_Majeures_v2.md`, LAC-01

```rust
// ❌ AVANT — NON CONSTANT-TIME
if token.type_id != expected as u16 { panic!(); }

// ✅ APRÈS — CONSTANT-TIME AVEC subtle
use subtle::{Choice, ConstantTimeEq};
let type_match: Choice = token.type_id.ct_eq(&(expected as u16));
if !bool::from(type_match) { panic!(); }
```

**Statut actuel** : ⚠️ **TODO TOUJOURS PRÉSENT**
**Action requise** : Ajouter dépendance `subtle = "2.5"` dans `Cargo.toml`, réécrire fonction

---

### unwrap() en production — Non-conformité S-02

**Fichiers concernés** : ~40 occurrences hors tests
**Problème** : `unwrap()` dans code production peut panic si invariant violé
**Impact** : Crash kernel ou serveur Ring 1
**Correction** : Remplacer par `expect("message explicite")` ou gestion d'erreur appropriée

**Script d'audit CI** :
```bash
# Compter unwrap() hors tests
grep -r "\.unwrap()" --include="*.rs" \
  src/ servers/*/src drivers/*/src libs/*/src \
  | grep -v "_test.rs" | grep -v "/tests/" \
  | wc -l
```

**Statut actuel** : ⚠️ **2134 unwrap() totaux**, ~40 en production
**Action requise** : Campagne systématique de remplacement avec messages explicites

---

### static mut non justifiés — Non-conformité S-04

**Fichiers concernés** : ~20 occurrences
**Problème** : `static mut` sans commentaire `SAFETY:` expliquant l'invariant
**Impact** : UB potentiel si invariants non respectés
**Correction** : Soit encapsuler dans `UnsafeCell` + API safe, soit ajouter commentaire SAFETY détaillé

**Exemple de correction** :
```rust
// ❌ AVANT — NON SÉCURISÉ
static mut GLOBAL_COUNTER: u64 = 0;

// ✅ APRÈS — AVEC INVARIANT DOCUMENTÉ
/// SAFETY:
///   - Accessible uniquement depuis IRQ context (interrupts disabled)
///   - Jamais lu/écrit concurrently avec code normal
///   - Initialisé à zero au boot, jamais réinitialisé
static mut GLOBAL_COUNTER: u64 = 0;
```

**Statut actuel** : ⚠️ **20+ static mut non documentés**
**Action requise** : Audit fichier par fichier, ajout commentaires SAFETY

---

## 🟠 MAJEUR — P1 (Requis pour stabilité)

### Ordering::Relaxed non commentés — Risque S-05

**Fichiers concernés** : ~3663 occurrences
**Problème** : `Ordering::Relaxed` utilisé sans justification de pourquoi c'est safe
**Impact** : Bugs de concurrence subtils sur architectures non-TSO (ARM, RISC-V)
**Correction** : Ajouter commentaire expliquant pourquoi Relaxed est suffisant

**Exemples de justifications valides** :
```rust
// Relaxed OK : compteur statistique, perte acceptable
BOOT_TSC_KHZ.store(khz, Ordering::Relaxed);

// Relaxed OK : valeur monotone, lecture seule après init
GLOBAL_GEN.fetch_add(1, Ordering::Relaxed);

// ❌ Relaxed INCORRECT : synchronisation inter-core
FLAG.store(1, Ordering::Relaxed); // ← DOIT ÊTRE Release
```

**Statut actuel** : ⚠️ **3663 Relaxed non commentés**
**Action requise** : Audit ciblé sur les atomiques de synchronisation (drapeaux, locks)

---

### TODOs restants dans code production

| Fichier | Ligne | TODO | Impact |
|---------|-------|------|--------|
| `forge.rs` | 386 | `// TODO: implementer validation` | Epoch metadata non validé |
| `epoch_record.rs` | 524 | `// TODO Phase 4` | Feature gate manquant |
| `cap.rs` | 89 | `// TODO Phase 1 : LAC-01` | CORR-41 non résolu |
| `direct_io.rs` | 156 | `// TODO: bounce buffer` | O_DIRECT non aligné |

**Statut actuel** : ⚠️ **~15 TODOs actifs**
**Action requise** : Soit implémenter, soit marquer explicitement `#[cfg(feature = "...")]`

---

### SeqLock Phase 9 — CORR-24 non implémenté

**Référence** : `docs/recast/ExoOS_Corrections_02_Architecture.md`
**Problème** : SeqLock mentionné comme "Phase 9 roadmap" mais aucune spec technique
**Impact** : Lacune documentaire pour optimisation futures lectures concurrentes
**Correction** : Créer document de spec SeqLock avec :
- Layout mémoire (seqcount + données)
- API reader/writer
- Gestion overflow seqcount
- Intégration avec TLA+ proofs

**Statut actuel** : 🔵 **Documentaire uniquement**
**Action requise** : Rédiger `docs/kernel/SeqLock_Spec_Phase9.md`

---

### SRV-05 persistence ipc_broker — CORR-43

**Référence** : `docs/recast/ExoOS_Corrections_09_FINAL_v3.md`
**Problème** : Règle SRV-05 manquante dans Architecture v7 §1.3
**Impact** : Comportement post-restore Phoenix non spécifié
**Correction** : Ajouter règle SRV-05 :

```markdown
| **SRV-05** | **ipc_broker persistence** | Le registry ServiceName→(PID, CapToken) est persisté vers ExoFS via persistence.rs. Après restore, rechargement obligatoire avant lookups. |
```

**Statut actuel** : ⚠️ **NON DOCUMENTÉ**
**Action requise** : Mettre à jour Architecture v7 + Arborescence V4

---

### Syscalls Phoenix 522-529 — CORR-43

**Référence** : `docs/recast/ExoOS_Corrections_08_Lacunes_Errata_v2.md`
**Problème** : Seuls 520-521 définis, 522-529 manquants
**Impact** : Plage syscall incomplète, risque de conflit futur
**Correction** : Définir mapping canonique :

```rust
pub const SYS_PHOENIX_STATUS:  u32 = 522; // Statut détaillé
pub const SYS_PHOENIX_FORCE:   u32 = 523; // Force cycle (SysAdmin)
// 524-529 : RÉSERVÉS
```

**Statut actuel** : ⚠️ **PARTIEL**
**Action requise** : Mettre à jour `exo-syscall/src/phoenix.rs`

---

## ⚠️ LACUNES — P2 (Amélioration continue)

### IoVec alignement — CORR-45

**Fichier** : `libs/exo-types/src/iovec.rs`
**Problème** : Structure non marquée `#[repr(C, align(8))]`
**Impact** : ABI incompatible Linux si alignement incorrect
**Correction** : Ajouter attribut + validation `validate_iovec_array()`
**Référence** : `docs/recast/ExoOS_Corrections_08_Lacunes_Errata_v2.md`

---

### O_DIRECT bounce buffering — CORR-46

**Fichier** : `kernel/src/fs/exofs/posix_bridge/direct_io.rs`
**Problème** : Responsabilité Ring 0 vs Ring 1 non clarifiée
**Impact** : I/Os non-alignées peuvent passer silencieusement
**Correction** : Ajouter TL-38 dans ExoFS TL v5 + vérifications alignement strict
**Référence** : `docs/recast/ExoOS_Corrections_08_Lacunes_Errata_v2.md`

---

### Quota enforcement copy_file_range — CORR-47

**Fichier** : `kernel/src/fs/exofs/posix_bridge/copy_range_kernel.rs`
**Problème** : Quota logique non vérifié pour reflinks
**Impact** : Dépassement quota possible via copie reflink
**Correction** : Appel `quota::check_and_reserve()` avant opération
**Référence** : `docs/recast/ExoOS_Corrections_08_Lacunes_Errata_v2.md`

---

### Stack canaries — CORR-48

**Fichier** : `kernel/src/memory/stack.rs`
**Problème** : Aucune protection stack overflow documentée
**Impact** : Corruption silencieuse en cas de débordement
**Correction** : Implémenter canary 0xDEAD_C0DE_CAFE_BABE + vérification retour fonction
**Référence** : `docs/recast/ExoOS_Corrections_08_Lacunes_Errata_v2.md`

---

### validate_fd_table_after_restore — CORR-50

**Fichier** : `servers/vfs_server/src/isolation.rs`
**Problème** : Utilise `close()` au lieu de `mark_stale()` → deadlock potentiel
**Impact** : Threads bloqués sur fds invalidés post-restore
**Correction** : Remplacer par `mark_stale()` + wake_all waiters avec EIO
**Référence** : `docs/recast/ExoOS_Corrections_09_FINAL_v3.md`

---

### IRQ handlers orphelins — CORR-51

**Fichier** : `kernel/src/arch/x86_64/irq/routing.rs`
**Problème** : Handlers de PIDs morts non purgés avant test limite
**Impact** : Limite 8 handlers atteinte artificiellement
**Correction** : Appel `process::is_alive()` dans `retain()` avant test limite
**Référence** : `docs/recast/ExoOS_Corrections_09_FINAL_v3.md`

---

## 🔵 MINEUR — P3 (Nettoyage)

### MAX_CPUS harmonisation — CORR-27

**Fichier** : `kernel/src/sched/preempt.rs`
**Problème** : Constante locale `MAX_CPUS = 64` au lieu de `MAX_CORES = 256`
**Correction** : Utiliser constante globale `MAX_CORES`
**Statut** : ✅ **RÉSOLU** (vérifier commit P1.1)

---

### user_gs_base nommage — CORR-29

**Fichier** : `kernel/src/arch/x86_64/thread.rs`
**Problème** : Champ nommé `user_gs_base` au lieu de `gs_base_user`
**Correction** : Renommer pour cohérence avec `fs_base_user`
**Statut** : 🔵 **Cosmétique**

---

### FixedString len: u32 — CORR-30

**Fichier** : `libs/exo-types/src/string.rs`
**Problème** : `len: usize` au lieu de `len: u32`
**Correction** : Changer type + assertion taille
**Statut** : 🔵 **Optimisation mémoire**

---

## 📋 Checklist de validation 100%

### P0 — Critique (5 items)
- [ ] **CORR-04** : Zéro allocation heap en ISR (audit dispatch.rs)
- [ ] **CORR-32** : TOCTOU fermé + vérification BDF unique
- [ ] **CORR-41** : verify_cap_token() constant-time (subtle crate)
- [ ] **unwrap()** : < 10 en production (tolérance zéro)
- [ ] **static mut** : 100% commentés SAFETY

### P1 — Majeur (9 items)
- [ ] **Ordering::Relaxed** : 100% commentés ou convertis AcqRel/Release
- [ ] **TODOs** : 0 TODO actif en production (feature-gated ou implémentés)
- [ ] **SeqLock** : Document de spec créé
- [ ] **SRV-05** : Règle ajoutée Architecture v7
- [ ] **Phoenix 522-529** : Mapping complet exo-syscall
- [ ] **IpcEndpoint Copy** : Assertion compile-time ajoutée
- [ ] **BootInfo validate()** : Implémentée + appelée dans init_server
- [ ] **fd_table mark_stale()** : Remplace close() post-restore
- [ ] **IRQ purge PIDs morts** : Implémentée dans sys_irq_register

### P2 — Lacunes (6 items)
- [ ] **IoVec align(8)** : Attribut + validation
- [ ] **O_DIRECT TL-38** : Documenté + vérifications alignement
- [ ] **Quota copy_file_range** : check_and_reserve() ajouté
- [ ] **Stack canaries** : Implémentés + tests
- [ ] **SYS_EXOFS_EPOCH_META** : Feature gate phase4
- [ ] **verify_binary_integrity** : Clarifié Phase 8 vs Phase 3

### P3 — Mineur (3 items)
- [ ] **MAX_CPUS** : Harmonisé MAX_CORES
- [ ] **user_gs_base** : Renommé
- [ ] **FixedString len** : u32

---

## 🎯 Métriques de conformité

| Métrique | Actuel | Cible 100% | Écart |
|----------|--------|------------|-------|
| unwrap() production | ~40 | 0 | -40 |
| static mut documentés | 0% | 100% | -100% |
| Ordering::Relaxed commentés | <5% | 100% | -95% |
| TODOs production | ~15 | 0 | -15 |
| Corrections implémentées | 65% | 100% | -35% |
| Rules SRV-* complètes | 4/5 | 5/5 | -1 |

---

## 📅 Roadmap recommandée

### Semaine 1 — P0 Critique
1. Jour 1-2 : Audit ISR (CORR-04) + remplacement Vec → heapless
2. Jour 3 : TOCTOU device_claims.rs (CORR-32)
3. Jour 4 : verify_cap_token constant-time (CORR-41)
4. Jour 5 : Campagne unwrap() → expect()

### Semaine 2 — P0 Suite + P1 Début
1. Jour 1-2 : Commentaires SAFETY static mut
2. Jour 3-4 : Audit Ordering::Relaxed ciblé
3. Jour 5 : Purge TODOs production

### Semaine 3 — P1 Fin + P2 Début
1. Jour 1-2 : SRV-05 + Phoenix 522-529
2. Jour 3-4 : fd_table mark_stale + IRQ purge
3. Jour 5 : IoVec + O_DIRECT

### Semaine 4 — P2 Fin + Validation
1. Jour 1-2 : Stack canaries + quota
2. Jour 3 : Tests de stress (IRQ, Watchdog, IOMMU)
3. Jour 4-5 : Relecture complète + CI checks

---

## 🧪 Tests de validation requis

### Tests unitaires
- [ ] `test_ipc_endpoint_copy()` : Assert compile-time Copy
- [ ] `test_verify_cap_token_constant_time()` : Timing attack simulation
- [ ] `test_dispatch_irq_no_alloc()` : Monitor heap allocations en ISR
- [ ] `test_pci_claim_tocou()` : Race condition test (2 threads simultanés)

### Tests d'intégration
- [ ] `test_phoenix_restore_fd_stale()` : Vérifier mark_stale() wake waiters
- [ ] `test_irq_handler_limit_with_dead_pids()` : Purge automatique
- [ ] `test_odirect_alignment_reject()` : Buffer non-aligné → EINVAL

### Tests de stress
- [ ] `stress_irq_registration()` : 1000 registrations concurrentes
- [ ] `stress_phoenix_cycle()` : 100 cycles gel/restore
- [ ] `stress_capability_verification()` : 1M verify_cap_token()

---

## 📝 Conclusion

**Conformité actuelle : 65%**
**Objectif : 100% en 4 semaines**

Le projet ExoOS est architecturalement sain mais nécessite un nettoyage rigoureux des pratiques unsafe et une documentation exhaustive des invariants. Les corrections P0 sont **bloquantes pour toute mise en production**.

**Prochaines étapes immédiates** :
1. Créer branche `audit/100-percent-compliance`
2. Implémenter corrections P0 dans ordre de criticité
3. Ajouter CI checks automatisés (unwrap count, static mut audit, TODO scanner)
4. Documenter chaque invariant SAFETY

---

*Document généré automatiquement — Dernière mise à jour : Avril 2026*
*Références : docs/recast/ExoOS_Corrections_00_*.md à 09_*.md*