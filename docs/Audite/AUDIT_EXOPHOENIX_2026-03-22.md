# Audit approfondi du module `exophoenix` (Exo-OS)

Date: 2026-03-22
Périmètre principal: `kernel/src/exophoenix/**`
Périmètre secondaire d’intégration: `arch/x86_64/{idt.rs,exceptions.rs,tss.rs,boot/memory_map.rs,apic/io_apic.rs}` + `kernel/src/lib.rs`
Objectif: base de refonte/correction de Kernel-B sentinelle

---

## 1) Résumé exécutif

`exophoenix` implémente le noyau sentinelle (Kernel B) chargé de surveiller, isoler, puis reconstruire Kernel A en cas de compromission.
Le module est structuré, déjà intégré à `kernel/src/lib.rs`, et connecte de manière forte l’architecture x86_64, l’IOMMU, la mémoire virtuelle et ExoFS.

Le cœur fonctionnel se décompose en 8 fichiers:
- `mod.rs`: état global `PhoenixState` + exports modules
- `ssr.rs`: région SSR partagée A<->B
- `stage0.rs`: bootstrap complet 1→13
- `interrupts.rs`: handlers IPI réservés `0xF1/0xF2/0xF3`
- `sentinel.rs`: boucle de détection continue
- `handoff.rs`: transitions soft/hard isolation
- `forge.rs`: reconstruction A + checklist post-reconstruction
- `isolate.rs`: cage mémoire côté A

Forces observées:
- architecture claire en phases.
- usage atomique systématique (Acquire/Release explicites).
- contraintes lock-free respectées dans les handlers critiques.
- intégration IDT/IST cohérente avec vecteurs réservés.
- pattern “étapes explicites” dans `stage0` très auditable.

Risques observés:
- plusieurs blocs `ADAPT`/placeholder dans `forge.rs` et `isolate.rs`.
- robustesse dépendante d’hypothèses hardware (APIC/IOMMU/CPUID).
- logique critique fortement couplée aux invariants SSR et mapping APIC→slot.
- chemins best-effort sur certains aspects PCIe/MSI-X.

---

## 2) Positionnement architectural

`exophoenix` est un module transverse de résilience noyau.
Il dépend directement de:
- `arch::x86_64` (APIC, IDT, exceptions, TSS, CPUID/MSR, PIT)
- `memory` (paging, tables, guard pages, buddy alloc, IOMMU)
- `security` (hash BLAKE3)
- `fs::exofs` (blob cache et image de reconstruction)

Il n’est pas un “service” utilisateur.
Il opère exclusivement ring0.
Il impose des conventions strictes:
- no-alloc/no-lock dans handlers IPI critiques.
- ordering mémoire explicite.
- transitions d’état contrôlées via `PHOENIX_STATE`.

---

## 3) Fichiers principaux (exhaustif)

- `kernel/src/exophoenix/mod.rs`
- `kernel/src/exophoenix/ssr.rs`
- `kernel/src/exophoenix/stage0.rs`
- `kernel/src/exophoenix/interrupts.rs`
- `kernel/src/exophoenix/sentinel.rs`
- `kernel/src/exophoenix/handoff.rs`
- `kernel/src/exophoenix/forge.rs`
- `kernel/src/exophoenix/isolate.rs`

Total module principal: 8 fichiers.

---

## 4) Fichiers secondaires d’intégration (exhaustif)

- `kernel/src/lib.rs` (déclaration `pub mod exophoenix;`)
- `kernel/src/arch/x86_64/idt.rs` (vecteurs réservés ExoPhoenix)
- `kernel/src/arch/x86_64/exceptions.rs` (dispatch handlers ExoPhoenix quand vecteurs actifs)
- `kernel/src/arch/x86_64/tss.rs` (IST ExoPhoenix)
- `kernel/src/arch/x86_64/boot/memory_map.rs` (réservation SSR)
- `kernel/src/arch/x86_64/apic/io_apic.rs` (interdiction routage IRQ sur vecteurs ExoPhoenix)

---

## 5) API et types structurants

### 5.1 `mod.rs`

`PhoenixState` (`repr(u8)`):
- `BootStage0`
- `Normal`
- `Threat`
- `IsolationSoft`
- `IsolationHard`
- `Certif`
- `Restore`
- `Degraded`
- `Emergency`

Global:
- `PHOENIX_STATE: AtomicU8`

### 5.2 `ssr.rs`

Constantes:
- `SSR_BASE`, `SSR_SIZE`, `MAX_CORES`
- offsets `SSR_HANDOFF_FLAG`, `SSR_LIVENESS_NONCE`, `SSR_SEQLOCK`, `SSR_CMD_B2A`, `SSR_FREEZE_ACK`, `SSR_PMC_SNAPSHOT`, `SSR_LOG_AUDIT`, `SSR_METRICS_PUSH`
- `FREEZE_ACK_DONE`, `TLB_ACK_DONE`

Fonctions:
- `freeze_ack_offset(slot_index)`
- `pmc_snapshot_offset(slot_index)`
- `unsafe fn ssr_atomic(offset) -> &'static AtomicU64`

### 5.3 `stage0.rs`

Fonctions pivots:
- `stage0_init_all_steps()`
- `stage0_init() -> !`
- `install_b_page_tables()`
- `setup_b_stack_with_guard_page()`
- `init_b_tss(stack_top)`
- `setup_b_idt_with_stubs()`
- `parse_stage0_acpi()`
- `enumerate_pci_devices()`
- `build_apic_to_slot_from_real_madt()`
- `calibrate_apic_timer_via_pit_ch2()`
- `init_local_apic_dispatch()`
- `setup_iommu_stage0(pool_r3_size)`
- `mark_facs_ro_in_a_pts(facs_phys)`
- `hash_and_store_madt(madt_phys)`
- `init_pool_r3_from_stage0_size(...)`
- `arm_apic_watchdog(ms)`
- `send_sipi_once(core_slot, entry_vector)`

State atoms:
- `EXOPHOENIX_VECTORS_ACTIVE`
- `TICKS_PER_US`
- `IOMMU_POLICY_READY`
- `IOMMU_BLOCKED_DOMAIN_ID`
- `FACS_RO_MARKED`
- `MADT_HASH_QWORDS`
- `SIPI_SENT`

### 5.4 `interrupts.rs`

Handlers:
- `handle_freeze_ipi() -> !` (`0xF1`)
- `handle_pmc_snapshot_ipi()` (`0xF2`)
- `handle_tlb_flush_ipi()` (`0xF3`)

### 5.5 `sentinel.rs`

Fonctions:
- `run_forever() -> !`
- `run_introspection_cycle()`
- `walk_a_page_tables_iterative()`
- `check_liveness_nonce()`
- `pmc_anomaly_score()`

### 5.6 `handoff.rs`

Fonctions:
- `begin_isolation_soft()`
- `begin_isolation_hard()`
- fonctions internes freeze/acks/revoke/forge policy

### 5.7 `forge.rs`

Fonction principale:
- `reconstruct_kernel_a() -> Result<(), ForgeError>`

Étapes:
- load image ExoFS
- parse ELF
- verify Merkle
- reset drivers ring1
- post-checklist G9

### 5.8 `isolate.rs`

Fonction principale:
- `isolate_kernel_a_memory()`

Sous-étapes:
- marquage pages A !PRESENT (ADAPT)
- shootdown TLB global
- hard revoke IOMMU
- override IDT A (ADAPT)

---

## 6) Flux opérationnel (boot → normal → threat → restore)

1. Stage0 configure Kernel B (tables, TSS/IDT, ACPI, PCI, APIC, IOMMU, watchdog).
2. `PHOENIX_STATE` passe en `Normal`.
3. SIPI one-shot vers A (garde-fou anti double émission).
4. Sentinelle boucle et score les anomalies.
5. Si score ≥ seuil: `Threat`, puis `begin_isolation_soft()`.
6. Freeze coopératif (`0xF1`) + soft revoke IOMMU.
7. Si timeout/échec: fallback hard isolation.
8. Reconstruction `forge` (jusqu’à 3 tentatives).
9. Si succès: `Restore`, retour `Normal`.
10. Si échecs: `Degraded`.

---

## 7) Vecteurs ExoPhoenix et intégration IDT

Réservations dans `idt.rs`:
- `0xF1` = `VEC_EXOPHOENIX_FREEZE`
- `0xF2` = `VEC_EXOPHOENIX_PMC`
- `0xF3` = `VEC_EXOPHOENIX_TLB`

Routage IST:
- vecteurs ExoPhoenix mappés sur `IST_EXOPHOENIX_IPI`.

Intégration `exceptions.rs`:
- en mode ExoPhoenix actif, les handlers IPI standards redirigent vers `exophoenix::interrupts::*`.

Protection `io_apic.rs`:
- refus du routage IRQ hardware vers les vecteurs réservés ExoPhoenix.

---

## 8) Concurrence, atomiques, locks

Observation majeure:
- le module privilégie les atomiques.
- très peu de locks “classiques” dans les chemins critiques.
- politique lock-free explicite pour handlers.

Exemples:
- `PHOENIX_STATE: AtomicU8`
- `APIC_TO_SLOT: [AtomicU8;256]`
- `IOMMU_*` policy flags atomiques
- SSR lue/écrite via `AtomicU64` et ordering explicite.

Points de vigilance:
- cohérence des fences SeqCst dans chemins IOMMU fallback.
- discipline Acquire/Release sur handoff flags SSR.
- robustesse du mapping APIC sparse.

---

## 9) Usage `&str`

Usage relativement limité et utile:
- erreurs de handoff: `Result<(), &'static str>`
- `ForgeError::ChecklistFailed(&'static str)`

C’est cohérent pour no_std et chemins critiques.

---

## 10) TODO / stubs / placeholders / ADAPT

### 10.1 `forge.rs`

Blocs `[ADAPT]` détectés:
- constantes hash image/merkle à connecter au vrai référentiel.
- API PCI/driver reset selon implémentation réelle.
- intégration DMA exacte selon stack existante.
- lecture IDT A via primitives finalisées.

### 10.2 `isolate.rs`

Blocs `[ADAPT]` détectés:
- marquage pages A !PRESENT via API page-table réelle.
- override IDT A via accès physique final.

### 10.3 `handoff.rs`

Placeholder explicite:
- `scan_and_release_spinlocks()` best-effort, sans table lock-owner exportée.

Implication:
- ce module est avancé mais pas totalement “close-world final”.
- la refonte doit fermer ces ADAPT avant validation prod.

---

## 11) Intégration mémoire/IOMMU

Le module touche:
- CR3/TLB
- mappings kernel
- guard pages
- pool R3
- domaines IOMMU bloqués
- flush IOTLB (Intel/AMD)

Risques:
- erreurs d’ordre flush/transition.
- faux positifs isolation si acks SSR incomplets.
- dépendance forte à la qualité des informations ACPI/PCI.

---

## 12) Intégration sécurité

`stage0.rs` s’appuie sur BLAKE3 (`security::crypto::blake3`).
`forge.rs` vérifie l’intégrité via hash/merkle.

Ce couplage impose:
- clés/hashes de référence fiables.
- pipeline de mise à jour strictement contrôlé.

---

## 13) Intégration ExoFS

`forge.rs` lit l’image A via `BLOB_CACHE`/`BlobId`.
C’est un point critique:
- disponibilité ExoFS pendant incident.
- cohérence blob + metadata.
- gestion des erreurs `ExoFsLoadFailed`.

---

## 14) Risques majeurs de refonte

- dérive ABI entre handlers/IDT/TSS.
- régression ordering SSR.
- mauvaise gestion APIC mode xAPIC/x2APIC.
- erreurs dans fallback IOMMU AMD.
- dérive des invariants stage0 (1→13).
- ADAPT non résolus laissés en prod.

---

## 15) Recommandations prioritaires

P0:
- fermer tous les `[ADAPT]` de `forge.rs` et `isolate.rs`.
- formaliser table lock-owner pour `scan_and_release_spinlocks`.
- ajouter tests d’intégration handoff soft/hard sur topologies variées.

P1:
- renforcer télémétrie SSR (debug + audit).
- tests chaos sur timeout/SMI/freeze ack partiels.
- valider résistance APIC sparse IDs.

P2:
- outillage de replay incident.
- bench latence cycle sentinelle.

---

## 16) Checklist détaillée (EXOPHX-CHK)

- EXOPHX-CHK-001 vérifier `PHOENIX_STATE` init BootStage0.
- EXOPHX-CHK-002 vérifier transitions état sans trou.
- EXOPHX-CHK-003 vérifier transitions état sans cycle mort.
- EXOPHX-CHK-004 vérifier valeur `repr(u8)` stable.
- EXOPHX-CHK-005 vérifier SSR_BASE réservé mémoire map.
- EXOPHX-CHK-006 vérifier SSR_SIZE cohérent mapping.
- EXOPHX-CHK-007 vérifier offsets SSR non chevauchants.
- EXOPHX-CHK-008 vérifier `freeze_ack_offset` borné.
- EXOPHX-CHK-009 vérifier `pmc_snapshot_offset` borné.
- EXOPHX-CHK-010 vérifier `ssr_atomic` uniquement offsets valides.
- EXOPHX-CHK-011 vérifier étape 1 install page tables B.
- EXOPHX-CHK-012 vérifier étape 2 guard page non présente.
- EXOPHX-CHK-013 vérifier étape 3 TSS chargé correctement.
- EXOPHX-CHK-014 vérifier étape 4 IDT stubs inactifs.
- EXOPHX-CHK-015 vérifier étape 5 parse ACPI robuste.
- EXOPHX-CHK-016 vérifier étape 5.5 enum PCI robuste.
- EXOPHX-CHK-017 vérifier étape 6 map APIC->slot.
- EXOPHX-CHK-018 vérifier étape 7 calibration APIC via PIT.
- EXOPHX-CHK-019 vérifier étape 8 init APIC dispatch.
- EXOPHX-CHK-020 vérifier étape 9 IOMMU deny-by-default.
- EXOPHX-CHK-021 vérifier étape 10 FACS RO.
- EXOPHX-CHK-022 vérifier étape 10 hash MADT.
- EXOPHX-CHK-023 vérifier étape 11 pool R3 init.
- EXOPHX-CHK-024 vérifier étape 12 watchdog arm.
- EXOPHX-CHK-025 vérifier étape 13 SIPI one-shot.
- EXOPHX-CHK-026 vérifier `SIPI_SENT` anti double envoi.
- EXOPHX-CHK-027 vérifier fallback APIC xAPIC.
- EXOPHX-CHK-028 vérifier fallback APIC x2APIC.
- EXOPHX-CHK-029 vérifier `detect_apic_mode` cohérent.
- EXOPHX-CHK-030 vérifier CPUID probe fiable.
- EXOPHX-CHK-031 vérifier invariance TSC détectée.
- EXOPHX-CHK-032 vérifier PMU version détectée.
- EXOPHX-CHK-033 vérifier VMXOFF défensif sûr.
- EXOPHX-CHK-034 vérifier `EXOPHOENIX_VECTORS_ACTIVE` ordering.
- EXOPHX-CHK-035 vérifier `activate/deactivate` vector flag.
- EXOPHX-CHK-036 vérifier `apic_slot` fallback.
- EXOPHX-CHK-037 vérifier APIC sparse IDs.
- EXOPHX-CHK-038 vérifier table APIC_TO_SLOT sans collision.
- EXOPHX-CHK-039 vérifier `build_apic_to_slot` fallback BSP.
- EXOPHX-CHK-040 vérifier watch-dog ticks non nuls.
- EXOPHX-CHK-041 vérifier `POOL_R3_SIZE_BYTES` calcul borné.
- EXOPHX-CHK-042 vérifier `calc_pool_r3_size` alignement.
- EXOPHX-CHK-043 vérifier allocations buddy order.
- EXOPHX-CHK-044 vérifier guard region registration.
- EXOPHX-CHK-045 vérifier `IOMMU_POLICY_READY` bascule.
- EXOPHX-CHK-046 vérifier blocked domain id valide.
- EXOPHX-CHK-047 vérifier flush IOTLB Intel.
- EXOPHX-CHK-048 vérifier fallback AMD fence.
- EXOPHX-CHK-049 vérifier root ports ACS comptés.
- EXOPHX-CHK-050 vérifier régions protégées mises à jour.
- EXOPHX-CHK-051 vérifier FACS mark RO succès.
- EXOPHX-CHK-052 vérifier MADT hash stock/load cohérent.
- EXOPHX-CHK-053 vérifier hash MADT recalcul stable.
- EXOPHX-CHK-054 vérifier PIT ch2 timeout géré.
- EXOPHX-CHK-055 vérifier APIC timer register access.
- EXOPHX-CHK-056 vérifier `apic_timer_write/read` mode split.
- EXOPHX-CHK-057 vérifier stack B top alignment.
- EXOPHX-CHK-058 vérifier stack guard effectivement unmapped.
- EXOPHX-CHK-059 vérifier no recursion stage0 critique.
- EXOPHX-CHK-060 vérifier no alloc handlers IPI.
- EXOPHX-CHK-061 vérifier no lock handlers IPI.
- EXOPHX-CHK-062 vérifier `handle_freeze_ipi` ACK release.
- EXOPHX-CHK-063 vérifier `handle_freeze_ipi` loop pause.
- EXOPHX-CHK-064 vérifier `handle_pmc_snapshot_ipi` bounds.
- EXOPHX-CHK-065 vérifier `handle_tlb_flush_ipi` CR3 reload.
- EXOPHX-CHK-066 vérifier ACK TLB écrit SSR.
- EXOPHX-CHK-067 vérifier APIC EOI dans handlers.
- EXOPHX-CHK-068 vérifier `sentinel::run_forever` no panic.
- EXOPHX-CHK-069 vérifier `walk_a_page_tables_iterative` itératif.
- EXOPHX-CHK-070 vérifier max_steps anti-boucle.
- EXOPHX-CHK-071 vérifier PF flood score.
- EXOPHX-CHK-072 vérifier PA remap score.
- EXOPHX-CHK-073 vérifier liveness nonce release/acquire.
- EXOPHX-CHK-074 vérifier timeout liveness.
- EXOPHX-CHK-075 vérifier fallback nonce sans RDRAND.
- EXOPHX-CHK-076 vérifier pmc anomaly score positif seul.
- EXOPHX-CHK-077 vérifier threat threshold.
- EXOPHX-CHK-078 vérifier SMI cycle long skip.
- EXOPHX-CHK-079 vérifier `SMI_COUNTER` incrément.
- EXOPHX-CHK-080 vérifier `THREAT_COUNTER` incrément.
- EXOPHX-CHK-081 vérifier transition Threat sur score.
- EXOPHX-CHK-082 vérifier appel `begin_isolation_soft`.
- EXOPHX-CHK-083 vérifier handoff flag release.
- EXOPHX-CHK-084 vérifier reset freeze ACK targets.
- EXOPHX-CHK-085 vérifier broadcast freeze except self.
- EXOPHX-CHK-086 vérifier soft revoke IOMMU même fenêtre.
- EXOPHX-CHK-087 vérifier attente ACK + drain 100us.
- EXOPHX-CHK-088 vérifier fallback hard isolation timeout.
- EXOPHX-CHK-089 vérifier hard revoke après ACK.
- EXOPHX-CHK-090 vérifier passage IsolationHard.
- EXOPHX-CHK-091 vérifier handoff flag B active.
- EXOPHX-CHK-092 vérifier tentative forge max 3.
- EXOPHX-CHK-093 vérifier passage Restore sur succès.
- EXOPHX-CHK-094 vérifier passage Degraded sur échecs.
- EXOPHX-CHK-095 vérifier begin_isolation_hard path.
- EXOPHX-CHK-096 vérifier mask MSI/MSI-X best effort.
- EXOPHX-CHK-097 vérifier INIT IPI cores résistants.
- EXOPHX-CHK-098 vérifier scan/release spinlocks placeholder.
- EXOPHX-CHK-099 vérifier `forge::reconstruct_kernel_a` pipeline.
- EXOPHX-CHK-100 vérifier load image ExoFS.
- EXOPHX-CHK-101 vérifier parse ELF magic.
- EXOPHX-CHK-102 vérifier sections `.text/.rodata/.data`.
- EXOPHX-CHK-103 vérifier `.bss` bounds.
- EXOPHX-CHK-104 vérifier entry point non nul.
- EXOPHX-CHK-105 vérifier Merkle compare strict.
- EXOPHX-CHK-106 vérifier reset drivers sequence.
- EXOPHX-CHK-107 vérifier FLR pour chaque device.
- EXOPHX-CHK-108 vérifier drain DMA timeout.
- EXOPHX-CHK-109 vérifier IOTLB flush post FLR.
- EXOPHX-CHK-110 vérifier reload driver binaire.
- EXOPHX-CHK-111 vérifier checklist FACS RO.
- EXOPHX-CHK-112 vérifier checklist MADT hash.
- EXOPHX-CHK-113 vérifier checklist TLB shootdown.
- EXOPHX-CHK-114 vérifier checklist IDT vecteurs.
- EXOPHX-CHK-115 vérifier isole memory !PRESENT ADAPT.
- EXOPHX-CHK-116 vérifier override IDT ADAPT.
- EXOPHX-CHK-117 vérifier no deadlock handoff.
- EXOPHX-CHK-118 vérifier no deadlock sentinel.
- EXOPHX-CHK-119 vérifier no deadlock stage0.
- EXOPHX-CHK-120 vérifier no reentrant panic path.
- EXOPHX-CHK-121 vérifier imports no_std conformes.
- EXOPHX-CHK-122 vérifier absence `std`.
- EXOPHX-CHK-123 vérifier cfg target cohérent.
- EXOPHX-CHK-124 vérifier interactions tss/idt.
- EXOPHX-CHK-125 vérifier interactions exceptions.
- EXOPHX-CHK-126 vérifier interactions io_apic réservations.
- EXOPHX-CHK-127 vérifier interactions memory_map SSR.
- EXOPHX-CHK-128 vérifier interactions lib.rs export.
- EXOPHX-CHK-129 vérifier ordering atomique SSR flags.
- EXOPHX-CHK-130 vérifier ordering atomique PHOENIX_STATE.
- EXOPHX-CHK-131 vérifier ordering atomique ack freeze.
- EXOPHX-CHK-132 vérifier ordering atomique ack tlb.
- EXOPHX-CHK-133 vérifier fence usage justifié.
- EXOPHX-CHK-134 vérifier SeqCst usage minimum.
- EXOPHX-CHK-135 vérifier Acquire/Release correctness.
- EXOPHX-CHK-136 vérifier Relaxed usage metrics.
- EXOPHX-CHK-137 vérifier unsafe blocks commentés.
- EXOPHX-CHK-138 vérifier pointer arithmetic bornée.
- EXOPHX-CHK-139 vérifier read/write volatile justifiés.
- EXOPHX-CHK-140 vérifier alignement structures.
- EXOPHX-CHK-141 vérifier overflow saturating ops.
- EXOPHX-CHK-142 vérifier constants et limites.
- EXOPHX-CHK-143 vérifier MAGIC values documentées.
- EXOPHX-CHK-144 vérifier timeouts réalistes.
- EXOPHX-CHK-145 vérifier SMI multiplier réaliste.
- EXOPHX-CHK-146 vérifier threat score calibration.
- EXOPHX-CHK-147 vérifier watchdog default valeur.
- EXOPHX-CHK-148 vérifier start ticks fallback.
- EXOPHX-CHK-149 vérifier APIC timer reading robust.
- EXOPHX-CHK-150 vérifier x2APIC MSR usage.
- EXOPHX-CHK-151 vérifier xAPIC MMIO usage.
- EXOPHX-CHK-152 vérifier CR4 VMXE bit handling.
- EXOPHX-CHK-153 vérifier VMXOFF erreurs tolérées.
- EXOPHX-CHK-154 vérifier APIC vectors active gating.
- EXOPHX-CHK-155 vérifier handlers return/diverge correct.
- EXOPHX-CHK-156 vérifier liveness mirror phys mapping.
- EXOPHX-CHK-157 vérifier A mirror offset validité.
- EXOPHX-CHK-158 vérifier physmap base usage.
- EXOPHX-CHK-159 vérifier A region bounds.
- EXOPHX-CHK-160 vérifier B region bounds.
- EXOPHX-CHK-161 vérifier SSR cmd region bounds.
- EXOPHX-CHK-162 vérifier pool R3 bounds.
- EXOPHX-CHK-163 vérifier ACL/routing IRQ réservés.
- EXOPHX-CHK-164 vérifier IST assignment vecteurs Phoenix.
- EXOPHX-CHK-165 vérifier panic broadcast interactions.
- EXOPHX-CHK-166 vérifier compat SMP bringup.
- EXOPHX-CHK-167 vérifier compat CPU offline.
- EXOPHX-CHK-168 vérifier compat AP hotplug.
- EXOPHX-CHK-169 vérifier compat no HPET.
- EXOPHX-CHK-170 vérifier compat no PMU.
- EXOPHX-CHK-171 vérifier compat no RDRAND.
- EXOPHX-CHK-172 vérifier compat no IOMMU.
- EXOPHX-CHK-173 vérifier compat Intel VT-d.
- EXOPHX-CHK-174 vérifier compat AMD IOMMU.
- EXOPHX-CHK-175 vérifier compat hyperviseur.
- EXOPHX-CHK-176 vérifier bare-metal path.
- EXOPHX-CHK-177 vérifier test regression vecteurs.
- EXOPHX-CHK-178 vérifier test regression handoff.
- EXOPHX-CHK-179 vérifier test regression forge.
- EXOPHX-CHK-180 vérifier test regression isolate.
- EXOPHX-CHK-181 vérifier test stress cycles sentinel.
- EXOPHX-CHK-182 vérifier test stress faux positifs.
- EXOPHX-CHK-183 vérifier test stress faux négatifs.
- EXOPHX-CHK-184 vérifier test corruption SSR.
- EXOPHX-CHK-185 vérifier test timeout freeze ack.
- EXOPHX-CHK-186 vérifier test timeout liveness.
- EXOPHX-CHK-187 vérifier test timeout watchdog.
- EXOPHX-CHK-188 vérifier test parse ACPI invalide.
- EXOPHX-CHK-189 vérifier test MADT incohérent.
- EXOPHX-CHK-190 vérifier test FACS absent.
- EXOPHX-CHK-191 vérifier test PIT indisponible.
- EXOPHX-CHK-192 vérifier test APIC mode mismatch.
- EXOPHX-CHK-193 vérifier test IOMMU domain create fail.
- EXOPHX-CHK-194 vérifier test pool R3 alloc fail.
- EXOPHX-CHK-195 vérifier test BLOB cache miss.
- EXOPHX-CHK-196 vérifier test ELF parse fail.
- EXOPHX-CHK-197 vérifier test Merkle mismatch.
- EXOPHX-CHK-198 vérifier test driver reset fail.
- EXOPHX-CHK-199 vérifier test checklist post fail.
- EXOPHX-CHK-200 vérifier test degrade transition.
- EXOPHX-CHK-201 vérifier test restore transition.
- EXOPHX-CHK-202 vérifier test emergency transition.
- EXOPHX-CHK-203 vérifier test continuous normal mode.
- EXOPHX-CHK-204 vérifier test repeated threats.
- EXOPHX-CHK-205 vérifier test repeated SIPI blocked.
- EXOPHX-CHK-206 vérifier test APIC slot duplicate.
- EXOPHX-CHK-207 vérifier test APIC slot missing.
- EXOPHX-CHK-208 vérifier test ACK write visibility.
- EXOPHX-CHK-209 vérifier test handoff flag visibility.
- EXOPHX-CHK-210 vérifier test liveness nonce visibility.
- EXOPHX-CHK-211 vérifier test pmc snapshot visibility.
- EXOPHX-CHK-212 vérifier test command SSR consistency.
- EXOPHX-CHK-213 vérifier test seqlock SSR usage.
- EXOPHX-CHK-214 vérifier test race sentinel/handoff.
- EXOPHX-CHK-215 vérifier test race isolate/forge.
- EXOPHX-CHK-216 vérifier test race vectors active.
- EXOPHX-CHK-217 vérifier test race idt updates.
- EXOPHX-CHK-218 vérifier test race iotlb flush.
- EXOPHX-CHK-219 vérifier test race pool updates.
- EXOPHX-CHK-220 vérifier test race madt hash load.
- EXOPHX-CHK-221 vérifier test race facs mark ro.
- EXOPHX-CHK-222 vérifier test race APIC timer read.
- EXOPHX-CHK-223 vérifier test race cpuid probe.
- EXOPHX-CHK-224 vérifier test race vmxoff path.
- EXOPHX-CHK-225 vérifier test race acpi parser.
- EXOPHX-CHK-226 vérifier test race pci scan.
- EXOPHX-CHK-227 vérifier test race buddy alloc.
- EXOPHX-CHK-228 vérifier test race iommu domains.
- EXOPHX-CHK-229 vérifier test race exofs blob load.
- EXOPHX-CHK-230 vérifier test race idt reserved vectors.
- EXOPHX-CHK-231 vérifier test race exceptions redirection.
- EXOPHX-CHK-232 vérifier test race tss IST.
- EXOPHX-CHK-233 vérifier test race io_apic route.
- EXOPHX-CHK-234 vérifier test race panic ipi.
- EXOPHX-CHK-235 vérifier test race smp startup.
- EXOPHX-CHK-236 vérifier test race AP halt.
- EXOPHX-CHK-237 vérifier test race scheduler wake.
- EXOPHX-CHK-238 vérifier test race memory shootdown.
- EXOPHX-CHK-239 vérifier test race process switching.
- EXOPHX-CHK-240 vérifier test race syscall return.
- EXOPHX-CHK-241 vérifier test race interrupt nesting.
- EXOPHX-CHK-242 vérifier test race NMI arrival.
- EXOPHX-CHK-243 vérifier test race MC exception.
- EXOPHX-CHK-244 vérifier test race PF exception.
- EXOPHX-CHK-245 vérifier test race GP exception.
- EXOPHX-CHK-246 vérifier test race double fault.
- EXOPHX-CHK-247 vérifier test race debug trap.
- EXOPHX-CHK-248 vérifier test race breakpoints.
- EXOPHX-CHK-249 vérifier test race ctrl-protection.
- EXOPHX-CHK-250 vérifier test race virtualization exception.
- EXOPHX-CHK-251 vérifier test robustesse docs-code alignement.
- EXOPHX-CHK-252 vérifier test robustesse index global.
- EXOPHX-CHK-253 vérifier test robustesse naming coherence.
- EXOPHX-CHK-254 vérifier test robustesse module exports.
- EXOPHX-CHK-255 vérifier test robustesse compile warnings.
- EXOPHX-CHK-256 vérifier test robustesse clippy hotspots.
- EXOPHX-CHK-257 vérifier test robustesse lint unsafe.
- EXOPHX-CHK-258 vérifier test robustesse format rustfmt.
- EXOPHX-CHK-259 vérifier test robustesse binary size.
- EXOPHX-CHK-260 vérifier clôture audit ExoPhoenix.

---

## 17) Annexes techniques “fichiers secondaires”

### 17.1 `arch/x86_64/idt.rs`
- Déclare explicitement les vecteurs ExoPhoenix.
- Garde-fou `is_exophoenix_reserved_vector`.
- Stack IST dédiée.

### 17.2 `arch/x86_64/exceptions.rs`
- Redirection runtime vers handlers ExoPhoenix conditionnée par `exophoenix_vectors_active()`.

### 17.3 `arch/x86_64/tss.rs`
- IST ExoPhoenix dédié et guardé.

### 17.4 `boot/memory_map.rs`
- Réservation SSR explicite dans la map mémoire.

### 17.5 `apic/io_apic.rs`
- protection contre routage IRQ sur vecteurs `0xF1/0xF2/0xF3`.

### 17.6 `kernel/src/lib.rs`
- module exposé globalement.

---

## 18) Conclusion

ExoPhoenix est un composant stratégique déjà bien avancé, avec des invariants clairs et une intégration profonde dans l’architecture x86_64.
Le chantier prioritaire de refonte/correction consiste à:
1) fermer les blocs `[ADAPT]`,
2) renforcer les tests de transitions et timeouts,
3) verrouiller la robustesse des chemins IOMMU/APIC/SSR.

Ce document constitue une base de travail complète pour la prochaine itération de hardening.
