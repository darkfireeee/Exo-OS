# Checklist Delta v0.2.0 — État Point par Point

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Source :** `MASTER-CHECKLIST-V0.2-REV2.md`  
**Légende :** ✅ Conforme · ❌ Non-conforme · ⚠️ Partiel · 🔍 Non vérifiable (dans lib.zip)

---

## BLOC -1 — Bugs Bloquants (0/7)

| ID | Description | Statut | Correction |
|---|---|---|---|
| B-01 | VirtIO BAR lu depuis PCI config space (pas hardcodé) | ❌ | CORR-IOTA-01 |
| B-02 | ExoFS persiste sur disque après reboot | ❌ | CORR-IOTA-01 |
| B-03 | Boot OK avec `-m 2G` (ACPI parser > 1 GiB) | ❌ | CORR-IOTA-03 |
| B-04 | `phys_to_virt()` guard dynamique (pas fixe 1 GiB) | ❌ | CORR-IOTA-03 |
| B-05 | `cgroup::init()` avant `runqueue_init()` | ❌ | CORR-IOTA-02 |
| B-06 | IPC send : cap token requis si len >= 200 | ❌ | CORR-IOTA-04 |
| B-07 | `exosh` démarre sans réseau en < 5s | ❌ | CORR-IOTA-05 |

---

## BLOC 0 — Outillage (0/13)

| ID | Description | Statut | Correction |
|---|---|---|---|
| O-01 | `arch/constants.rs` centralisé existe | ❌ | CORR-IOTA-06 |
| O-02 | `const_assert!(SSR_SIZE <= 4096)` | ❌ | CORR-IOTA-07 |
| O-03 | `const_assert!(KAIROS_WINDOW_NS > 0)` | ❌ | CORR-IOTA-07 |
| O-04 | `const_assert!(PHYSMAP_INITIAL_COVERAGE == 1GiB)` | ❌ | CORR-IOTA-03 |
| O-05 | `const_assert!(CORE_MASK_WORDS * 64 >= MAX_CORES_LAYOUT)` | ❌ | CORR-IOTA-06 |
| O-06 | `tools/audit_constants.py` existe | ❌ | CORR-IOTA-09 |
| O-07 | `audit_constants.py` s'exécute sans erreur | ❌ | CORR-IOTA-09 |
| O-08 | Règles Semgrep ExoOS créées | ❌ | CORR-IOTA-10 |
| O-09 | Semgrep passe sans erreur sur `kernel/src/` | ❌ | CORR-IOTA-10 |
| O-10 | `deny.toml` configuré (tokio, dbus, libc bannis) | ❌ | CORR-IOTA-08 |
| O-11 | `cargo deny check` passe | ❌ | CORR-IOTA-08 |
| O-12 | Pre-commit hook actif | ❌ | Manuel |
| O-13 | Workflow CI `audit.yml` vert | ❌ | CORR-IOTA-10 |

---

## BLOC 1 — ExoPhoenix (0/22)

| ID | Description | Statut | Correction |
|---|---|---|---|
| P-01 | `const_assert!(SSR_SIZE <= 4096)` dans kernel | ❌ | CORR-IOTA-17 |
| P-02 | `SSR_MAX_PROCESSES >= 12` (crate) | 🔍 | CORR-IOTA-17 |
| P-03 | `CORE_MASK_WORDS × 64 >= MAX_CORES_LAYOUT` (crate) | 🔍 | CORR-IOTA-17 |
| P-04 | SSR en zone E820 réservée `[0x1000000..0x1100000]` | ⚠️ | CORR-IOTA-19 |
| P-05 | Stage0 lit SSR depuis MMIO physique | 🔍 | — |
| P-06 | Stage0 vérifie checksum BLAKE3 SSR | 🔍 | — |
| P-07 | Ring1 servers démarrent en parallèle après bascule | ❌ | CORR-IOTA-18 |
| P-08 | SLA recovery bascule < 500ms mesuré | ❌ | CORR-IOTA-18 |
| P-09 | Bascule A→B préserve les IPC en vol | 🔍 | — |
| P-10 | Bascule B→A (rollback) fonctionne | 🔍 | — |
| P-11 | Phoenix watchdog armé avant Ring0→Ring1 | ⚠️ | — |
| P-12 | PID 1 (`init_server`) survit à la bascule | 🔍 | — |
| P-13 | ExoLedger enregistre chaque bascule | ❌ | CORR-IOTA-12 |
| P-14 | Typo TLA+ corrigée (`0x110000` → `0x1100000`) | ❌ | CORR-IOTA-19 |
| P-15 | `PHOENIX_ENABLED` activé par défaut | 🔍 | — |
| P-16 | Stress 100 bascules en 10min sans perte de données | ❌ | Test QEMU |
| P-17 | Handoff : registres CPU sauvegardés | 🔍 | — |
| P-18 | Handoff : mapping mémoire validé | 🔍 | — |
| P-19 | `BOOT_SEQUENCE_V0.2.md` créé et complet | ❌ | CORR-IOTA-20 |
| P-20 | TLA+ spec SSR layout cohérente avec code | ⚠️ | CORR-IOTA-19 |
| P-21 | Smoke test `exo_phoenix_test` passe | ❌ | Test QEMU |
| P-22 | Documentation bascule dans ROADMAP v0.2.0 | ❌ | CORR-IOTA-20 |

---

## BLOC 2 — Séquence Boot Sécurité (0/22)

| ID | Description | Statut | Correction |
|---|---|---|---|
| S-01 | `memory_init()` Phase 0 en premier | ⚠️ | CORR-IOTA-20 |
| S-02 | `arch_init()` + APIC Phase 1 (avant NMI) | ⚠️ | CORR-IOTA-20 |
| S-03 | ExoCage CR4/MSR Phase 2 (avant heap) | ⚠️ | CORR-IOTA-20 |
| S-04 | ExoNMI Phase 3 (LAPIC disponible) | ⚠️ | CORR-IOTA-20 |
| S-05 | ExoSeal `verify_chain()` Phase 6 | 🔍 | — |
| S-06 | ExoShield IOMMU Phase 7 (avant Ring1) | ⚠️ | — |
| S-07 | SMEP actif avant premier accès heap | ⚠️ | CORR-IOTA-20 |
| S-08 | SMAP actif avant premier accès heap | ⚠️ | CORR-IOTA-20 |
| S-09 | KPTI actif sur les deux chemins de boot | ⚠️ | CORR-IOTA-20 |
| S-10 | NX/XD actif sur toutes les pages data | ⚠️ | CORR-IOTA-20 |
| S-11 | CET shadow stack initialisé | ⚠️ | — |
| S-12 | Zero Trust fast path bitmask Ring1↔Ring1 | ❌ | ERR-09 |
| S-13 | KASLR offset > 0 sur tout boot non-debug | ⚠️ | — |
| S-14 | ExoShield IOMMU protège tous les DMA Ring1 | ⚠️ | — |
| S-15 | ExoArgos PMC valide sur AP et BSP | 🔍 | — |
| S-16 | ExoKairos : budget reset par `KAIROS_WINDOW_NS` | ❌ | CORR-IOTA-11 |
| S-17 | ExoKairos : throttle à 100% du budget fenêtre | ❌ | CORR-IOTA-11 |
| S-18 | ExoKairos : kill à 200% cumulé (2 fenêtres) | ❌ | CORR-IOTA-11 |
| S-19 | `is_immutable()` check dans `object_write` | ❌ | CORR-IOTA-12 |
| S-20 | ExoLedger log sur écriture bloquée | ❌ | CORR-IOTA-12 |
| S-21 | ExoShield IOMMU avant Ring0→Ring1 handoff | ⚠️ | — |
| S-22 | `phase5-tests` suite passe à 100% | ❌ | Test kernel |

---

## BLOC 3 — Drivers (1/8)

| ID | Description | Statut | Correction |
|---|---|---|---|
| D-01 | VirtIO-blk BAR0 lu depuis PCI | ❌ | CORR-IOTA-01 |
| D-02 | VirtIO-net BAR0 lu depuis PCI | ⚠️ | À vérifier |
| D-03 | VirtioDmaEngine sans adresse hardcodée | ❌ | CORR-IOTA-01 |
| D-04 | IOMMU domain créé pour chaque Ring1 DMA | ⚠️ | — |
| D-05 | IOMMU fault_handler log ExoLedger | ⚠️ | — |
| D-06 | PCI énumération complète avant `drivers::init()` | ✅ | — |
| D-07 | VirtIO-blk : lecture/écriture secteur 0 réussit | ❌ | CORR-IOTA-01 |
| D-08 | VirtIO-net : TX/RX ICMP réussit | ⚠️ | — |

---

## BLOC 4 — IPC (1/10)

| ID | Description | Statut | Correction |
|---|---|---|---|
| I-01 | `MAX_MSG_SIZE = 240` (ring slot) | ✅ | — |
| I-02 | Seuil inline ≤ 192 octets dans syscall_abi | ❌ | CORR-IOTA-21 |
| I-03 | SHM pour payload > 200 octets | ⚠️ | — |
| I-04 | Cap token check si `len >= IPC_ENVELOPE_SIZE` | ❌ | CORR-IOTA-04 |
| I-05 | `IPC_FLAG_INJECT_SRC_PID` requiert cap valide | ❌ | CORR-IOTA-04 |
| I-06 | Ring IPC : pas de famine (priority aging) | 🔍 | — |
| I-07 | IPC router lookup < 1ms | ⚠️ | — |
| I-08 | IPC Ring1↔Ring1 validé par exo_shield | ⚠️ | — |
| I-09 | Zero Trust bitmask Ring1↔Ring1 | ❌ | ERR-09 |
| I-10 | Test injection PID bloquée (EACCES) | ❌ | CORR-IOTA-04 |

---

## BLOC 5 — Bibliothèques (0/8)

| ID | Description | Statut | Correction |
|---|---|---|---|
| L-01 | `exo-alloc` backend = dlmalloc no_std | 🔍 | CORR-IOTA-22 |
| L-02 | `exo-alloc` sans libc transitif | 🔍 | CORR-IOTA-22 |
| L-03 | `exo-rt` panic handler no_std | 🔍 | — |
| L-04 | Seuil inline 192 octets dans syscall_abi | ❌ | CORR-IOTA-21 |
| L-05 | Seuil SHM > 200 octets dans kernel IPC | ❌ | CORR-IOTA-21 |
| L-06 | `exo-libc` sans malloc système | 🔍 | — |
| L-07 | `cargo deny check` passe sur libs/ | ❌ | CORR-IOTA-08 |
| L-08 | `exo-crypto` implémentation interne | 🔍 | — |

---

## BLOC 6 — ExoFS (2/10)

| ID | Description | Statut | Correction |
|---|---|---|---|
| F-01 | ExoFS monte depuis disque VirtIO valide | ❌ | CORR-IOTA-01 |
| F-02 | Lecture fichier après reboot | ❌ | CORR-IOTA-01 |
| F-03 | `is_immutable()` bloque les écritures | ❌ | CORR-IOTA-12 |
| F-04 | ExoLedger log sur écriture bloquée | ❌ | CORR-IOTA-12 |
| F-05 | Quota par cgroup appliqué | ⚠️ | — |
| F-06 | `cp`, `mv`, `rm` POSIX minimaux | ⚠️ | — |
| F-07 | `ls -la` fonctionne | ✅ | v0.1.0 acquis |
| F-08 | Chemin absolu + relatif résolus | ✅ | v0.1.0 acquis |
| F-09 | Création de répertoire imbriqué | ⚠️ | — |
| F-10 | Superblock checksum valide | 🔍 | — |

---

## BLOC 7 — Compatibilité ELF (1/6)

| ID | Description | Statut | Correction |
|---|---|---|---|
| E-01 | ELF base 0x400000 accepté par loader | ✅ | v0.1.0 acquis |
| E-02 | `const_assert!(USER_ELF_BASE_MIN <= 0x400000)` | ❌ | CORR-IOTA-06 |
| E-03 | PT_LOAD aligné 4 KiB validé | ⚠️ | — |
| E-04 | ELF dynamique (INTERP) refusé proprement | ⚠️ | — |
| E-05 | `exo compat install busybox` passe | ❌ | Dépend F-01 |
| E-06 | Binaire musl statique s'exécute | ⚠️ | — |

---

## BLOC 8 — Shell & Utilitaires (1/8)

| ID | Description | Statut | Correction |
|---|---|---|---|
| SH-01 | `exosh` accessible en < 5s après boot | ❌ | CORR-IOTA-05 |
| SH-02 | `exosh` accessible sans réseau | ❌ | CORR-IOTA-05 |
| SH-03 | `top` : liste process via syscall réel | ❌ | — |
| SH-04 | `ps` : PID, nom, état, CPU% | ❌ | — |
| SH-05 | Pipe `cmd1 \| cmd2` fonctionne | ⚠️ | — |
| SH-06 | Redirection `>`, `>>`, `<` | ⚠️ | — |
| SH-07 | Glob `*` dans `ls`, `rm` | ⚠️ | — |
| SH-08 | `Ctrl+C` interrompt la commande | ✅ | v0.1.0 acquis |

---

## BLOC 9 — Graphisme (reporté v0.3.0)

| ID | Description | Statut |
|---|---|---|
| G-01 à G-07 | Wayland, wgpu, iced | ❌ Reporté v0.3.0 |
| G-08 | Report wgpu documenté dans ROADMAP | ❌ |
| G-09 | Report iced documenté dans ROADMAP | ❌ |

---

## BLOC 10 — Installation (reporté v0.3.0)

| ID | Description | Statut |
|---|---|---|
| IN-01 à IN-10 | Installeur visuel, partitionnement | ❌ Reporté v0.3.0 |

---

## BLOC 11 — exo_shield (0/12)

| ID | Description | Statut | Correction |
|---|---|---|---|
| ES-01 | `lib.rs` déclare les 9 modules | ❌ | CORR-IOTA-13 |
| ES-02 | `_start()` initialise les 9 modules | ❌ | CORR-IOTA-14 |
| ES-03 | hooks branchés dans `handle_event_report()` | ❌ | CORR-IOTA-15 |
| ES-04 | `handle_quarantine_cmd()` containment sandbox réel | ❌ | CORR-IOTA-16 |
| ES-05 | ML score refinement actif | ❌ | CORR-IOTA-15 |
| ES-06 | Forensics timeline enregistrée sur menace | ❌ | CORR-IOTA-16 |
| ES-07 | Bridge ExoArgos → exo_shield PMC | ⚠️ | — |
| ES-08 | DNS guard actif sur toutes les requêtes Ring1 | ❌ | CORR-IOTA-14 |
| ES-09 | IDS : règles minimales chargées | ❌ | CORR-IOTA-14 |
| ES-10 | Sandbox : isolation réseau réelle | ❌ | CORR-IOTA-16 |
| ES-11 | Sandbox : isolation FS réelle | ❌ | CORR-IOTA-16 |
| ES-12 | Test complet exo_shield : 0 erreur | ❌ | — |

---

## Résumé Global

| Bloc | Total | ✅ | ⚠️ | ❌ | 🔍 |
|---|---|---|---|---|---|
| BLOC -1 | 7 | 0 | 0 | 7 | 0 |
| BLOC 0 | 13 | 0 | 0 | 13 | 0 |
| BLOC 1 | 22 | 0 | 2 | 12 | 8 |
| BLOC 2 | 22 | 0 | 10 | 9 | 3 |
| BLOC 3 | 8 | 1 | 4 | 3 | 0 |
| BLOC 4 | 10 | 1 | 3 | 6 | 0 |
| BLOC 5 | 8 | 0 | 0 | 3 | 5 |
| BLOC 6 | 10 | 2 | 3 | 5 | 0 |
| BLOC 7 | 6 | 1 | 3 | 2 | 0 |
| BLOC 8 | 8 | 1 | 4 | 3 | 0 |
| BLOC 9/10 | 12 | 0 | 0 | 12 | 0 |
| BLOC 11 | 12 | 0 | 1 | 11 | 0 |
| **TOTAL** | **138** | **6** | **30** | **86** | **16** |

**Taux de conformité actuel : 4.3%** (6/138)  
**Cible v0.2.0 : 100%** des BLOCS -1 à 8 + 11 (BLOCS 9/10 reportés v0.3.0)

---

*claude iota — CHECKLIST_DELTA_V0.2.0_CLAUDE_IOTA.md — 2026-05-20*
