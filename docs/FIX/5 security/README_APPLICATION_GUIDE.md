# Guide d'Application des Correctifs — ExoShield v1.0
## Ensemble de Correctifs Complet

---

## 📊 Résumé des Correctifs

| Patch | Fichier(s) Cible(s) | Vulnérabilité | Sévérité | Temps Estimé |
|-------|---------------------|---------------|----------|--------------|
| patch_01 | `arch/x86_64/smp/init.rs` + `security/mod.rs` | CVE-EXO-001 Race SMP | CRITIQUE | 4h |
| patch_02 | `security/exocage.rs` + `scheduler/core/task.rs` | CET per-thread + WRSSQ | CRITIQUE | 6h |
| patch_03 | `security/exoseal.rs` | verify_p0_fixes() + ordre PKS | CRITIQUE | 3h |
| patch_04 | `security/exoledger.rs` | DoS P0 overflow | MAJEUR | 3h |
| patch_05 | `security/exoargos.rs` | Fuite info PMC | MAJEUR | 2h |
| patch_06 | `security/exonmi.rs` | Watchdog hardcoded | MAJEUR | 2h |
| patch_07 | `capability/mod.rs` + `exophoenix/isolate.rs` + `mod.rs` | Side-channel + KPTI + dead code | MAJEUR | 4h |

**Total estimé : 24 heures**

---

## ⚠️ Vérification : Tous les Findings Sont CONFIRMÉS

L'analyse croisée des 4 passes d'audit (Grok + équipe) avec les extraits
verbatim du code source confirme que **toutes les vulnérabilités** signalées
dans l'audit sont **réelles et non corrigées** dans le commit `ef58e5c`
(« Prepare fix security »).

Points saillants :
- Le commit FIX 3.1 et « Prepare fix security » modifient **uniquement** la
  documentation (`docs/FIX/`) sans toucher `kernel/src/security/`
- CVE-EXO-001 est auto-documentée dans le code lui-même (le commentaire
  dit « DOIT spin-wait » mais le code ne le fait pas)
- CET per-thread : les 4 passes s'accordent sur l'absence d'appel à
  `enable_cet_for_thread()` dans `task::new_thread()`

---

## 🗓️ Plan d'Application (3 Semaines)

### Semaine 1 — Correctifs CRITIQUES (P0)

```
Jour 1-2 : patch_01 (CVE-EXO-001)
  - Implémenter ap_wait_security_ready() dans smp/init.rs
  - Ajouter appels dans chaque AP entry point
  - Tests : cargo test security:: + simulation QEMU 4 cœurs

Jour 3-4 : patch_02 (ExoCage CET per-thread)
  - Compléter/vérifier exocage.rs
  - Câbler enable_cet_for_thread() dans task::new_thread()
  - Ajouter static_assert! TCB 256 bytes
  - Tests : QEMU avec -cpu qemu64,+cet

Jour 5 : patch_03 (ExoSeal verify_p0_fixes)
  - Implémenter verify_p0_fixes()
  - Corriger l'ordre exokairos/pks_restore
  - Tests : boot complet QEMU
```

### Semaine 2 — Correctifs MAJEURS (P1)

```
Jour 1-2 : patch_04 + patch_05 (ExoLedger + ExoArgos)
  - Graceful overflow handler
  - Validation TCB dans pmc_snapshot()
  - Tests unitaires

Jour 3-4 : patch_06 + patch_07 partie KPTI (ExoNmi + ExoPhoenix)
  - Timeout watchdog adaptatif
  - Compléter isolate.rs (mark_a_pages_not_present + override IDT)
  - Tests handoff QEMU dual-kernel

Jour 5 : patch_07 partie constant-time + dead code
  - Ajouter subtle::ConstantTimeEq dans capability/
  - Nettoyer re-exports orphelins
  - cargo clippy --fix --all-targets
```

### Semaine 3 — Validation & Hardening

```
- Re-run TLA+ SmpBoot.tla (12 modules, ~1.2 milliard d'états)
- cargo audit (dépendances CVE)
- Fuzzing IPC (afl-fuzz ou cargo-fuzz)
- Smoke test CET complet : forcer une #CP violation en QEMU et vérifier
  que cp_handler → ledger → freeze_req → Kernel B fonctionne
- Test de charge P0 : générer intentionnellement 65000+ événements P0
  et vérifier le graceful handoff
- Audit externe (optionnel)
```

---

## 🔧 Ordre d'Application des Patches

**IMPORTANT** : Respecter cet ordre pour éviter les dépendances circulaires.

```
1. patch_04 (exoledger.rs) ← Requis par tous les autres patches
                              (ActionTag étendu)
2. patch_03 (exoseal.rs)   ← Requis par patch_01 (verify_p0_fixes)
3. patch_01 (smp/init.rs)  ← CVE-EXO-001 : priorité absolue
4. patch_02 (exocage.rs)   ← CET per-thread
5. patch_05 (exoargos.rs)  ← PMC validation
6. patch_06 (exonmi.rs)    ← Watchdog
7. patch_07 (divers)       ← Nettoyage + constant-time + KPTI
```

---

## 🧪 Checklist de Validation Post-Correction

### Compilation
- [ ] `cargo build --workspace` sans erreurs ni warnings
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace` tous tests passent
- [ ] `cargo audit` aucune CVE connue dans les dépendances

### Tests Fonctionnels
- [ ] Boot QEMU 1 cœur : SECURITY_READY levé correctement
- [ ] Boot QEMU 4 cœurs : APs attendent bien SECURITY_READY
- [ ] Test CET : `qemu-system-x86_64 -cpu qemu64,+cet` → pas de faux #CP
- [ ] Test #CP intentionnel → cp_handler → ledger → freeze_req → Kernel B OK
- [ ] Test overflow P0 : 65000 entrées → graceful handoff (pas de panic)
- [ ] Test PMC : appel pmc_snapshot() depuis thread non-propriétaire → PmcError::TcbMismatch
- [ ] Test watchdog : timeout < 500ms → clamped à 500ms
- [ ] Test constant-time : verify_cap_token() résiste au timing (criterion bench)

### Vérification Formelle
- [ ] Re-run TLA+ SmpBoot.tla → 0 violation de BootSafety
- [ ] Re-run TLA+ ExoPhoenix_Spec_v6.tla → dual-kernel exclusivity maintenue
- [ ] Re-run TLA+ ExoCage → shadow stack invariants

### Sécurité
- [ ] Aucun `unwrap()` non justifié dans security/
- [ ] Tous les `unsafe` blocs documentés avec justification
- [ ] verify_p0_fixes() appelé dans exoseal_boot_phase0() ET boot_complete()
- [ ] static_assert! TCB 256 bytes présent dans task.rs
- [ ] subtle::ConstantTimeEq dans verify_cap_token()

---

## 🔄 Plan de Rollback

En cas de régression critique lors de l'application des patches :

```bash
# Retour au dernier état stable
git stash
git checkout ef58e5c48e52e36136b51cc525ce5d07958841e6

# Ou par patch individuel
git revert <commit_patch_N>
```

**Note** : Les patches sont conçus pour être atomiques (un commit par patch).
En cas d'échec de patch_02 (CET), les patches 01/03 restent valides et
applicables indépendamment.

---

## 📈 Amélioration Prévue de la Cohérence Code/Spec

| État | Cohérence Code/Spec |
|------|---------------------|
| Avant corrections (commit ef58e5c) | ~68–82 % |
| Après Semaine 1 (P0 CRITIQUES) | ~88 % |
| Après Semaine 2 (MAJEURS) | ~95 % |
| Après Semaine 3 (validation complète) | ~98 % |

**Objectif : Production-ready ExoShield v1.0**

---

*Patches générés le 20 avril 2026 — Basés sur l'analyse de 4 passes d'audit
du commit ef58e5c48e52e36136b51cc525ce5d07958841e6 (« Prepare fix security »)*
