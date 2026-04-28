# Rapport — Correctifs `docs/FIX/3 arch-memory/1`
Date: 2026-04-19
Répertoire: `C:\Users\xavie\Desktop\Exo-OS`

## Périmètre strict

Corrections appliquées **uniquement** à partir de:

- `docs/FIX/3 arch-memory/1/CORRECTIONS_00_MASTER_INDEX.md`
- `docs/FIX/3 arch-memory/1/CORRECTIONS_01_EXOPHOENIX_CRITIQUES.md`
- `docs/FIX/3 arch-memory/1/CORRECTIONS_02_LINKAGE_RUNTIME.md`
- `docs/FIX/3 arch-memory/1/CORRECTIONS_03_MAJEURES_SECURITE_SCHEDULER.md`

## Synthèse d’exécution

- Correctifs codés: ExoPhoenix (forge/isolate/handoff/sentinel/stage0), KPTI, swap fat-pointer, CET shadow-stack alloc/free, HPET fixmap, CFS division-safe, commentaires de conformité, hygiene rustc-ICE.
- Validation statique: diagnostic éditeur propre sur fichiers modifiés (hors warning attendu `OUT_DIR` côté `forge.rs`).
- Validation build WSL (déjà exécutée pendant cette passe):
  - `kernel`: `cargo check -q --target x86_64-unknown-none` → `EXIT:0`
  - `libs`: `cargo check -q -p generic-rt -p exo-phoenix-ssr` → `EXIT:0`
  - tentative cible JSON sans flag nightly dédié → `.json target specs require -Zjson-target-spec`

## Détail des points traités

### CRIT / NEW

| ID | Statut | Implémentation |
|---|---|---|
| CRIT-01 | ⚠️ Partiel sécurisé | `kernel/build.rs` génère les blobs hash (`OUT_DIR`), `forge.rs` les charge via `include_bytes!`, `stage0.rs` force `Degraded` si hash nul. |
| CRIT-02 | ✅ | `isolate.rs::mark_a_pages_not_present()` implémenté via walk des tables et clear `PRESENT`. |
| CRIT-03 | ✅ | `isolate.rs::override_a_idt_with_b_handlers()` implémenté + helpers handlers dans `arch/x86_64/idt.rs`. |
| CRIT-04 | ✅ | `handoff.rs::mask_all_msi_msix()` implémente scan capabilities PCI + mask MSI/MSI-X réel. |
| CRIT-05 | ⚠️ Partiel | `forge.rs`: FLR PCIe réel implémenté; rechargement driver vérifie cache ExoFS + lookup `stage0::driver_blob_id()`, mais mapping BDF→BlobId reste non alimenté en pratique. |
| CRIT-07 | ✅ | Offset liveness rendu contractuel via `libs/exo-phoenix-ssr::A_LIVENESS_MIRROR_OFFSET`, utilisé par `sentinel.rs`. |
| CRIT-10 | ✅ | `kpti_split.rs::build_user_shadow_pml4()` ajouté; `spectre/kpti.rs::init_kpti()` alloue/enregistre/active seulement en cas de succès. |
| NEW-01 | ✅ (conforme doc fournie) | `swap_in.rs` remplace tuple implicite par `FatPtr` explicite pour sérialisation/reconstruction du provider. |
| NEW-03 | ⚠️ Partiel | Fichiers `rustc-ice-*.txt` supprimés + ignore Git ajouté; cause racine toolchain/ICE non éliminée dans ce passage. |

### MAJ / MIN

| ID | Statut | Implémentation |
|---|---|---|
| MAJ-01 | ✅ | `handoff.rs::send_init_ipi_to_resistant_cores()` accepte `TLB_ACK_DONE` comme ACK coopératif. |
| MAJ-02 | ✅ | `exocage.rs`: allocation/libération shadow stacks branchées sur buddy (`ZEROED|PIN`). |
| MAJ-03 | ⚠️ Partiel | `exoledger.rs`: fallback OID non-zéro en contexte sans TCB; extraction complète CapToken/OID non implémentée ici. |
| MAJ-05 | ✅ | `acpi/hpet.rs`: mapping HPET via fixmap (`FIXMAP_HPET`) + flags MMIO + flush TLB local. |
| MAJ-08 | ✅ | Commentaire offsets TCB `_cold_reserve` corrigé dans `exocage.rs`. |
| MAJ-13 | ✅ | Commentaire EmergencyPool réaligné sur `EMERGENCY_POOL_SIZE=256`. |
| MIN-05 | ✅ | `libs/generic-rt::panic_notls()` écrit désormais le message sur port debug `0xE9` avant abort. |
| MIN-07 | ✅ | `cfs.rs::timeslice_for()` protégé via `nr_tasks.max(1)` avant division. |
| MIN-08 | ✅ | `sentinel.rs::pmc_anomaly_score()` bascule sur deltas CTR + baseline (évite faux positifs EVTSEL). |
| MIN-09 | ✅ | `stage0.rs`: taille minimale pool R3 forcée à 8 MiB. |

## Points déjà couverts avant cette passe (non retouchés ici)

- CRIT-06, CRIT-08, CRIT-09, CRIT-11, CRIT-12
- MAJ-04, MAJ-07, MAJ-09, MAJ-10, MAJ-11, MAJ-12
- NEW-02, NEW-04, MIN-03

## Fichiers modifiés (cette passe)

- `.gitignore`
- `kernel/build.rs`
- `kernel/src/exophoenix/forge.rs`
- `kernel/src/exophoenix/stage0.rs`
- `kernel/src/exophoenix/isolate.rs`
- `kernel/src/exophoenix/handoff.rs`
- `kernel/src/arch/x86_64/idt.rs`
- `libs/exo-phoenix-ssr/src/lib.rs`
- `kernel/src/exophoenix/sentinel.rs`
- `kernel/src/memory/virtual/page_table/kpti_split.rs`
- `kernel/src/arch/x86_64/spectre/kpti.rs`
- `kernel/src/memory/virtual/fault/swap_in.rs`
- `kernel/src/security/exocage.rs`
- `kernel/src/security/exoledger.rs`
- `kernel/src/arch/x86_64/acpi/hpet.rs`
- `kernel/src/scheduler/policies/cfs.rs`
- `kernel/src/memory/physical/frame/emergency_pool.rs`
- `libs/generic-rt/src/lib.rs`
- suppression: `rustc-ice-2026-04-19T16_25_42-760.txt`, `rustc-ice-2026-04-19T16_26_46-774.txt`, `rustc-ice-2026-04-19T16_27_39-788.txt`

## Résidu explicite

1. **CRIT-05** reste partiel tant que la table BDF→BlobId n’est pas peuplée par une source autoritative.
2. **MAJ-03** reste partiel tant que l’OID n’est pas extrait du CapToken courant.
3. **NEW-03** traité côté hygiene Git, pas côté bug compilateur nightly.
4. **CRIT-01** dépend de l’injection réelle des hashes (`KERNEL_A_IMAGE_HASH`, `KERNEL_A_MERKLE_ROOT`) par pipeline build/release.

## Conclusion

Le lot `docs/FIX/3 arch-memory/1` a été traité de façon ciblée sur le code actif, avec compilation WSL verte sur les cibles validées pendant la passe. Les résidus sont explicitement tracés et limités aux points nécessitant une source de vérité/pipeline externe ou un câblage de données non présent dans l’état courant.
