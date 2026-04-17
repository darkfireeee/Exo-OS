# Exo-OS — Corrections intégrales GI-01 / GI-02 (WSL) + préparation GI-03

Date: 2026-03-29  
Auteur: GitHub Copilot (mise à jour post-corrections + audit recast)

---

## 1) Objet du document

Ce rapport remplace l’ancienne version d’audit « read-only » et trace l’état **après corrections appliquées** sur GI-01/GI-02, puis la préparation de GI-03 via relecture approfondie de `docs/recast/`.

Périmètre :

- Code modifié : `kernel/`, `libs/`, script `run_tests.sh`.
- Documentation relue : `docs/recast/GI-00..GI-04-05-06`, `ExoOS_Architecture_v7.md`, `ExoOS_Driver_Framework_v10.md`, `ExoPhoenix_Spec_v6.md`, série `ExoOS_Corrections_00..09`.
- Validation runtime : exécution WSL du runner projet.

---

## 2) Résumé exécutif

| Domaine | Statut | Commentaire |
|---|---|---|
| GI-01 (types/TCB/SSR) | ✅ Corrigé côté code | SSR runtime unifié via crate partagée, `ObjectId::is_valid()` corrigé |
| GI-02 (boot/switch/security) | ✅ Corrigé côté code | init syscall AP, handshake `SECURITY_READY`, MAJ GS `current_tcb` + `kernel_rsp` |
| Validation WSL | ✅ PASS | `PASS 25 / FAIL 0 / SKIP 6 / WARN 0` |
| GI-03 (drivers/IRQ/DMA) | ❌ Pas prêt en implémentation immédiate | Base documentaire riche, mais points bloquants restants (cf. §6) |

Verdict court:

- **GI-01/GI-02 : erreurs traitées et validées techniquement.**
- **GI-03 : prêt pour lot de corrections préparatoires, pas encore prêt pour exécution complète.**

---

## 3) Corrections effectivement appliquées (GI-01 / GI-02)

### 3.1 GI-01

1. **Unification SSR runtime sur la crate partagée**  
   - `kernel/src/exophoenix/ssr.rs`  
   - `kernel/src/exophoenix/handoff.rs`  
   - `kernel/src/exophoenix/interrupts.rs`

2. **Correction workspace/libs**  
   - `libs/Cargo.toml` (members invalides retirés)  
   - `libs/exo_allocator/Cargo.toml` (`exo_types` → package `exo-types`)

3. **Fix `ObjectId::is_valid()`**  
   - `libs/exo_types/src/object_id.rs`  
   - `ObjectId::ZERO` invalide explicitement, exception `ZERO_BLOB_ID_4K` conservée.

### 3.2 GI-02

1. **Init SYSCALL sur AP**  
   - `kernel/src/arch/x86_64/smp/init.rs`

2. **Câblage sécurité boot + garde anti double-init**  
   - `kernel/src/arch/x86_64/boot/early_init.rs`  
   - `kernel/src/lib.rs`

3. **Écriture explicite GS `current_tcb` + rafraîchissement GS `kernel_rsp` au switch**  
   - `kernel/src/arch/x86_64/smp/percpu.rs`  
   - `kernel/src/scheduler/core/switch.rs`

4. **Ajout de tests de préparation P2-7 (host-safe + gated Ring0)**  
   - `kernel/src/security/mod.rs`  
   - `kernel/src/scheduler/fpu/lazy.rs`  
   - `kernel/src/arch/x86_64/smp/percpu.rs`

5. **Fiabilisation runner**  
   - `run_tests.sh` (agrégation multi-lignes `test result`, filtres GI, résumé robuste)

---

## 4) Validation WSL (post-corrections)

Exécution : `run_tests.sh` sous WSL.

Résultats observés :

- Kernel bare-metal check: ✅
- Tests GI-02 host (`p2_7_`): ✅ `4 passed / 3 ignored`
- Tests `exo-types` host: ✅ `20 passed / 3 ignored`

Résumé global:

- **PASS: 25**
- **FAIL: 0**
- **SKIP: 6**
- **WARN: 0**

Conclusion runtime: **validation propre** pour le périmètre GI-01/GI-02 couvert par le runner.

---

## 5) Cohérence GI-01/GI-02 avec la chaîne « création modules → corrections »

### 5.1 Constat global

La chaîne est **globalement cohérente côté guides GI + code**, mais avec des poches documentaires anciennes encore divergentes.

### 5.2 Ce qui est aligné

1. **GI-01 ↔ corrections types**  
   - `GI-01_Types_TCB_SSR.md` + `ExoOS_Corrections_01_Kernel_Types.md`  
   - Alignement avec implémentation actuelle (`TCB`, `ObjectId`, SSR partagé).

2. **GI-02 ↔ corrections architecture**  
   - `GI-02_Boot_ContextSwitch.md` + `ExoOS_Corrections_02_Architecture.md`  
   - Les fixes clés (SYSCALL AP, sécurité boot, invariants switch/GS) sont désormais codés.

### 5.3 Divergences documentaires restantes à arbitrer

1. **Boot init_server (`argv[1]` vs `boot_info_virt`)**  
   - `ExoOS_Arborescence_V4(Phase server Et Lib).md` mentionne encore `argv[1]`  
   - `ExoOS_Architecture_v7.md` et `ExoOS_Corrections_02_Architecture.md` imposent `boot_info_virt`.

2. **SSR MAX_CORES (64 vs 256) selon documents**  
   - `ExoPhoenix_Spec_v6.md` reste orienté MAX_CORES=64  
   - `GI-01` / `Architecture_v7` / `exo-phoenix-ssr` convergent sur layout partagé 256.

3. **Index des corrections non entièrement resynchronisé**  
   - Exemples de numérotation/redescription qui divergent entre `ExoOS_Corrections_00_Master_Index.md` et `ExoOS_Corrections_09_FINAL_v3.md` (notamment CORR-51).

### 5.4 Verdict de cohérence

- **Techniquement (code): OUI, GI-01/GI-02 sont en accord avec la trajectoire de correction.**
- **Documentairement (recast): PARTIEL, reste un ménage de cohérence à faire avant GI-03.**

---

## 6) Préparation GI-03 — audit recast et backlog

### 6.1 Prérequis GI-03 identifiés dans la doc

- GI-01 et GI-02 finalisés comme fondation (`GI-03_Drivers_IRQ_DMA.md`).
- Séquence boot/sécurité stable (`ExoOS_Architecture_v7.md`).
- Contrats IRQ/ISR et DMA/IOMMU cadrés (`GI-03_Drivers_IRQ_DMA.md`, `ExoOS_Driver_Framework_v10.md`).

### 6.2 Bloquants principaux (P0) avant implémentation GI-03 complète

1. **`domain_of_pid()` / registre de domaines IOMMU** : documenté mais non matérialisé dans le code.  
   Références: `GI-03_Drivers_IRQ_DMA.md`, `ExoOS_Corrections_03_Driver_Framework.md`, `ExoOS_Corrections_00_Master_Index.md`.

2. **Normalisation documentaire des invariants de boot et d’interface init_server** (`argv[1]` vs `boot_info_virt`) pour éviter des implémentations contradictoires.

3. **Resynchronisation du Master Index des corrections** (id/catégories) pour éviter ambiguïtés de suivi en phase drivers.

### 6.3 Priorisation GI-03 proposée

#### P0 (immédiat)

1. Spécifier/implémenter `IOMMU_DOMAIN_REGISTRY` + `domain_of_pid()/release_domain()`.  
2. Fermer les contradictions doc `argv[1]` / `boot_info_virt`.  
3. Resynchroniser `ExoOS_Corrections_00_Master_Index.md` avec `*_FINAL_v3.md`.

#### P1 (démarrage GI-03)

4. Valider explicitement les chemins ISR sans alloc (`dispatch_irq`) et les règles d’ACK/watchdog.  
5. Vérifier la chaîne `do_exit()` complète côté drivers (IRQ/MMIO/DMA/domain release).

#### P2 (durcissement)

6. Ajouter des tests de stress IOMMU fault queue + contention IRQ sur cible de test.

---

## 7) Conclusion

Le lot demandé est clôturé sur GI-01/GI-02 :

- ✅ corrections codées,
- ✅ tests WSL passés,
- ✅ rapport renommé et repositionné en « corrections ».

Pour la suite GI-03 :

- le socle technique est prêt,
- mais la phase doit commencer par un **lot de normalisation documentaire + primitives IOMMU manquantes** afin d’éviter les écarts de trajectoire.

Statut final:

- **GI-01/GI-02 : GO (corrigé et validé).**
- **GI-03 : GO conditionnel après P0 de préparation ci-dessus.**
