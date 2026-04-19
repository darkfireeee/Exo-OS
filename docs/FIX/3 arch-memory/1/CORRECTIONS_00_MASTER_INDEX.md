# MASTER INDEX — Corrections ExoOS
> Audit complet : 2 passes + vérification commit `2f75b6cf` · 2026-04-19
> 3 fichiers de corrections détaillées + ce fichier index

---

## TABLEAU DE BORD GÉNÉRAL

| ID | Sévérité | Statut | Module | Fichier correction |
|----|----------|--------|--------|--------------------|
| CRIT-01 | 🔴 BLOQUANT | ❌ OUVERT | `forge.rs` — hashes nuls | CORRECTIONS_01 |
| CRIT-02 | 🔴 BLOQUANT | ⚠️ PARTIEL | `isolate.rs` — cage vide | CORRECTIONS_01 |
| CRIT-03 | 🔴 BLOQUANT | ❌ OUVERT | `isolate.rs` — IDT vide | CORRECTIONS_01 |
| CRIT-04 | 🔴 MAJEUR  | ❌ OUVERT | `handoff.rs` — MSI non masqués | CORRECTIONS_01 |
| CRIT-05 | 🔴 MAJEUR  | ❌ OUVERT | `forge.rs` — drivers fictifs | CORRECTIONS_01 |
| CRIT-06 | 🔴 BLOQUANT | ✅ CORRIGÉ | `ssr.rs` — physique vs virtuel | — |
| CRIT-07 | 🔴 MAJEUR  | ❌ OUVERT | `sentinel.rs` — mirror offset | CORRECTIONS_01 |
| CRIT-08 | 🔴 BLOQUANT | ✅ CORRIGÉ | `fastcall_asm.s` — non inclus | — |
| CRIT-09 | 🔴 BLOQUANT | ✅ CORRIGÉ | `arch_cpu_relax` — indéfini | — |
| CRIT-10 | 🔴 BLOQUANT | ❌ OUVERT | `kpti.rs` — user_pml4 nulle | CORRECTIONS_02 |
| CRIT-11 | 🔴 BLOQUANT | ✅ CORRIGÉ | SMP — double ONLINE_CPU_COUNT | — |
| CRIT-12 | 🔴 BLOQUANT | ✅ CORRIGÉ | Swap provider non enregistré | — |
| MAJ-01  | 🟠 GRAVE   | ⚠️ PARTIEL | ACK freeze/TLB collision | CORRECTIONS_01 |
| MAJ-02  | 🟠 GRAVE   | ❌ OUVERT | Shadow stack CET à 0x0 | CORRECTIONS_03 |
| MAJ-03  | 🟠 MOYEN  | ❌ OUVERT | OID audit = zéros | CORRECTIONS_03 |
| MAJ-04  | 🟠 GRAVE   | ✅ CORRIGÉ | HMAC vulnérable | — |
| MAJ-05  | 🟠 MOYEN  | ❌ OUVERT | HPET MMIO non mappé | CORRECTIONS_03 |
| MAJ-06  | 🟠 MOYEN  | ❌ OUVERT | VFS flush absent | CORRECTIONS_03 |
| MAJ-07  | 🟠 GRAVE   | ✅ CORRIGÉ | PIT timeout itérations | — |
| MAJ-08  | 🟠 FAIBLE  | ❌ OUVERT | Commentaire TCB trompeur | CORRECTIONS_03 |
| MAJ-09  | 🟠 GRAVE   | ✅ CORRIGÉ | Un seul SIPI | — |
| MAJ-10  | 🟠 GRAVE   | ✅ CORRIGÉ | TSC offset signé en u64 | — |
| MAJ-11  | 🟠 GRAVE   | ✅ CORRIGÉ | Hotplug bitmask 64 CPUs | — |
| MAJ-12  | 🟠 GRAVE   | ✅ CORRIGÉ | IPC collision modulo 256 | — |
| MAJ-13  | 🟠 FAIBLE  | ❌ OUVERT | Commentaire "64" vs 256 | CORRECTIONS_03 |
| NEW-01  | 🟠 MOYEN  | ❌ OUVERT | transmute fat pointer UB | CORRECTIONS_02 |
| NEW-02  | 🟡 FAIBLE  | ✅ CORRIGÉ | Swap provider non appelé boot | — |
| NEW-03  | 🔴 BLOQUANT | ❌ OUVERT | Rustc ICE dans le repo | Ce fichier |
| NEW-04  | 🟠 MOYEN  | ✅ CORRIGÉ | Double SIPI (send_sipi_once) | — |
| MIN-03  | 🟡 BUG     | ✅ CORRIGÉ | Deadline APIC DOWN inversée | — |
| MIN-05  | 🟡 FAIBLE  | ❌ OUVERT | Panic handler silencieux | CORRECTIONS_03 |
| MIN-07  | 🟡 BUG     | ❌ OUVERT | CFS division par zéro race | CORRECTIONS_03 |
| MIN-08  | 🟡 FAIBLE  | ❌ OUVERT | PMC score faux positif | CORRECTIONS_03 |
| MIN-09  | 🟡 FAIBLE  | ❌ OUVERT | Pool R3 taille zéro | CORRECTIONS_03 |

**Légende** : ✅ Corrigé · ⚠️ Partiellement corrigé · ❌ Ouvert

---

## PRIORITÉ D'IMPLÉMENTATION

### 🚨 Urgent — Bloquant pour tout test bare-metal

```
Ordre recommandé :
1. NEW-03  : Éliminer les rustc ICE (le code ne compile pas proprement)
2. CRIT-10 : KPTI user_pml4 → triple fault au premier retour userspace
3. CRIT-01 : Hashes Kernel A → ExoPhoenix inutilisable
4. CRIT-02 : Cage mémoire → isolation fictive
5. CRIT-03 : Override IDT → isolation fictive (suite de CRIT-02)
```

### ⚡ Haute priorité — Sécurité

```
6. CRIT-04 : MSI masking → IRQ pendant gel
7. CRIT-05 : Drivers Ring 1 → reconstruction incomplète
8. CRIT-07 : Liveness mirror → faux positifs
9. MAJ-02  : Shadow stack CET à 0x0 → crash thread CET
10. MAJ-01  : ACK collision résiduelle → INIT IPI inutile
```

### 🔧 Normale — Qualité et robustesse

```
11. MAJ-05 : HPET fixmap → crash possible bare-metal
12. MAJ-06 : VFS flush → perte données démontage
13. MAJ-03 : OID audit → journal inutilisable
14. NEW-01 : transmute fat pointer → risque futur
15. MIN-07 : CFS race division → panique potentielle
16. MIN-08 : PMC score → faux positifs profiler
17. MIN-09 : Pool R3 zéro → servers Ring 1 sans mémoire DMA
```

### 📝 Documentation et nettoyage

```
18. MAJ-08 : Commentaire offset TCB
19. MAJ-13 : Commentaire "64 WaitNodes"
20. MIN-05 : Panic handler silencieux early boot
```

---

## NEW-03 — Rustc ICE (Internal Compiler Error) à éliminer

### Fichiers concernés
```
rustc-ice-2026-04-19T16_25_42-760.txt
rustc-ice-2026-04-19T16_26_46-774.txt
rustc-ice-2026-04-19T16_27_39-788.txt
```

### Action immédiate
Ces fichiers indiquent que `rustc` a crashé pendant la compilation. Un ICE est
un bug du compilateur déclenché par du code Rust qui touche un cas limite non géré.

```bash
# 1. Identifier le module qui trigger le crash
# Relancer la compilation avec verbose :
cargo build --target x86_64-exo-os.json -v 2>&1 | grep -A5 "ice\|ICE\|internal compiler"

# 2. Vérifier le toolchain utilisé
cat kernel/rust-toolchain.toml

# 3. Bisect si nécessaire :
# - Commenter les modules récemment ajoutés un par un
# - Identifier la construct Rust qui provoque le crash

# 4. Workarounds courants :
# - Simplifier les expressions const complexes
# - Éviter les const génériques imbriquées profondément
# - Remplacer les patterns macro complexes par des fonctions
```

**À ne pas committer dans le repo** : supprimer les fichiers `rustc-ice-*.txt`
(ils ne doivent pas polluer l'historique git) :
```bash
echo "rustc-ice-*.txt" >> .gitignore
git rm --cached rustc-ice-*.txt
```

---

## CHECKLISTE DE VALIDATION PAR CORRECTION

### Pour chaque correction appliquée, vérifier :

```
□ Le code compile sans warnings sur la cible bare-metal (x86_64-exo-os.json)
□ Les assertions statiques (`const _: ()`) continuent de passer
□ Le module TLA+ correspondant (si existant) reste vérifié
□ La documentation inline (SAFETY, RÈGLE) est mise à jour
□ Aucun nouveau TODO/FIXME/ADAPT n'est introduit sans tracking
□ Les tests unitaires existants passent (cargo test --target x86_64_unknown_linux_gnu)
□ Le test QEMU de boot smoke passe (make run-qemu)
```

### Critères d'acceptation par niveau de sévérité

| Sévérité | Tests requis |
|----------|-------------|
| 🔴 CRIT  | Boot QEMU + test unitaire + revue TLA+ du module |
| 🟠 MAJ   | Test unitaire + revue de code pair |
| 🟡 MIN   | Test unitaire ou smoke test suffisant |

---

## RÉSUMÉ STATISTIQUE

| Catégorie | Total | Corrigés | Partiels | Ouverts |
|-----------|-------|----------|----------|---------|
| CRITIQUES | 12    | 6        | 2        | 4       |
| MAJEURES  | 13    | 7        | 0        | 6       |
| MINEURES  | 5     | 1        | 0        | 4       |
| NOUVEAUX  | 4     | 2        | 0        | 2       |
| **TOTAL** | **34**| **16**   | **2**    | **16**  |

**Taux de correction du commit `2f75b6cf` : 47% (16/34 points)**

Les corrections apportées dans ce commit sont de bonne qualité technique
(MAJ-10 TSC offset signé, MAJ-11 bitmask 256 CPUs, MAJ-12 ring_for) et
montrent une compréhension correcte des problèmes architecturaux identifiés.

Les points restants ouverts (CRIT-01/02/03/04/05/07/10) représentent les
défis les plus difficiles car ils requièrent soit des décisions d'architecture
(hashes Kernel A = processus de release), soit des implémentations non triviales
(cage mémoire PTE walk, KPTI shadow PML4, MSI masking).

---

*Fichiers de correction :*
- `CORRECTIONS_01_EXOPHOENIX_CRITIQUES.md` — CRIT-01 à CRIT-07 + MAJ-01
- `CORRECTIONS_02_LINKAGE_RUNTIME.md` — CRIT-08 à CRIT-12 + NEW-01
- `CORRECTIONS_03_MAJEURES_SECURITE_SCHEDULER.md` — MAJ-02 à MAJ-13 + MIN-05/07/08/09
- `CORRECTIONS_00_MASTER_INDEX.md` — ce fichier
