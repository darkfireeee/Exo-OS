# MASTER-CHECKLIST-V0.2-REV2 — Checklist Révisée Post-Audit Beta+Gamma
## Intègre les Corrections CORR-76 à CORR-86 + Outillage

**Auteur :** claude-alpha  
**Date :** 2026-05-16  
**Remplace :** `MASTER-CHECKLIST-V0.2.md` (version initiale)  
**Statut :** DOCUMENT VIVANT — cocher au fur et à mesure

> **Convention :** `[ ]` = à faire / `[x]` = validé / `[~]` = partiel / `[-]` = hors périmètre v0.2.0

---

## BLOC -1 — Bugs Kernel Bloquants (NOUVEAU — à faire en tout premier)

Ces bugs existaient en v0.1.0 et bloquent **tout le reste**. Aucun travail de v0.2.0 n'est possible sans les résoudre.

| # | Critère | CORR | Test | Statut |
|---|---------|------|------|--------|
| B-01 | VirtIO BAR lu depuis PCI config space (plus d'adresse hardcodée) | CORR-86 | Log BAR0 MMIO base != 0x10000000 | `[ ]` |
| B-02 | ExoFS persiste sur disque après reboot | CORR-86 | cat /test.txt après reboot | `[ ]` |
| B-03 | Boot avec -m 2G sans panique kernel | CORR-76 | Log "physmap étendue : 2 GiB" | `[ ]` |
| B-04 | phys_to_virt() sur adresse > 1 GiB retourne adresse valide | CORR-76 | Test unitaire | `[ ]` |
| B-05 | cgroup::init() appelé avant runqueue_init() | CORR-77 | Log "root cgroup valide" | `[ ]` |
| B-06 | Ring1 servers attachés au root cgroup sans crash | CORR-77 | exo ps → tous running | `[ ]` |
| B-07 | ELF base 0x400000 accepté par l'ELF loader | CORR-80 | Binary hello charge | `[ ]` |
| B-08 | USER_ELF_BASE_MIN ≤ 0x400000 (const_assert!) | CORR-80 | cargo build → pass | `[ ]` |
| B-09 | MSG len==128 sans cap → PolicyDenied (pas d'injection PID) | CORR-78 | Test pénétration | `[ ]` |
| B-10 | exosh démarre sans network_server (QEMU -net none) | CORR-79 | Boot sans réseau → exosh visible | `[ ]` |

**Seuil de passage BLOC -1 → BLOC 0 : 10/10**

---

## BLOC 0 — Outillage d'Audit (NOUVEAU)

| # | Critère | Statut |
|---|---------|--------|
| O-01 | `arch/constants.rs` créé avec toutes les constantes canoniques | `[ ]` |
| O-02 | `const_assert!` dans ssr.rs (SSR size ≤ 4096) | `[ ]` |
| O-03 | `const_assert!` dans exokairos.rs (KAIROS_WINDOW_NS) | `[ ]` |
| O-04 | `const_assert!` dans physmap.rs (PHYSMAP_INITIAL_COVERAGE) | `[ ]` |
| O-05 | `const_assert!` cohérence CORE_MASK_WORDS × 64 == MAX_CORES_LAYOUT | `[ ]` |
| O-06 | `tools/audit_constants.py` créé et fonctionnel | `[ ]` |
| O-07 | `audit_constants.py` → 0 erreurs sur kernel/ | `[ ]` |
| O-08 | `tools/semgrep-rules/exoos.yaml` créé | `[ ]` |
| O-09 | Semgrep → 0 violations sur kernel/ | `[ ]` |
| O-10 | `deny.toml` configuré (libsodium, dbus, zbus, tokio-runtime interdits) | `[ ]` |
| O-11 | `cargo deny check` → 0 violations | `[ ]` |
| O-12 | Pre-commit hook installé et fonctionnel | `[ ]` |
| O-13 | `.github/workflows/audit.yml` créé | `[ ]` |

**Seuil de passage BLOC 0 → BLOC 1 : 13/13**

---

## BLOC 1 — ExoPhoenix (RÉVISÉ)

| # | Critère | Correction | Statut |
|---|---------|-----------|--------|
| P-01 | SSR struct ≤ 4096 octets (const_assert! vérifié) | CORR-81 | `[ ]` |
| P-02 | SSR_MAX_PROCESSES = 24 avec politique de priorisation documentée | CORR-81 | `[ ]` |
| P-03 | SSR bitmask [u64; CORE_MASK_WORDS] dans forge/handoff/isolate/ssr | CORR-75-A | `[ ]` |
| P-04 | Bascule A→B sans charge : < 500ms | phoenix_perf_test | `[ ]` |
| P-05 | Bascule A→B sous charge 80% CPU : < 500ms | phoenix_stress_test | `[ ]` |
| P-06 | Recovery B→A après crash simulé : < 500ms | phoenix_crash_recovery | `[ ]` |
| P-07 | Ring1 servers démarrés EN PARALLÈLE après bascule | CORR-81 (ERR-11) | `[ ]` |
| P-08 | Capabilities survivantes préservées : 100% | phoenix_cap_survival | `[ ]` |
| P-09 | ExoFS atomicité pendant écriture | phoenix_exofs_atomicity | `[ ]` |
| P-10 | SSR cohérent : 0 champ invalide | phoenix_ssr_integrity | `[ ]` |
| P-11 | Tests unitaires ExoPhoenix dédiés créés (min 4) | C-GAMMA-02 | `[ ]` |
| P-12 | exo-net reconnexion après bascule | phoenix_exo_net_reconnect | `[ ]` |
| P-13 | Stress 1000 bascules : 0 échec | phoenix_stress_1000 | `[ ]` |
| P-14 | SSR typo README corrigé (0x110000 → 0x1100000) | C-GAMMA-03 | `[ ]` |

---

## BLOC 2 — Séquence de Boot Sécurité (RÉVISÉ)

La séquence de boot a été reséquencée (CORR-82). Checklist mise à jour en conséquence.

| # | Critère | Phase Boot | Statut |
|---|---------|-----------|--------|
| S-01 | memory_init() en Phase 0 (premier) | Phase 0 | `[ ]` |
| S-02 | arch_init() + APIC en Phase 1 (avant NMI) | Phase 1 | `[ ]` |
| S-03 | ExoCage (CR4, MSR) en Phase 2 — pas de heap requis | Phase 2 | `[ ]` |
| S-04 | ExoNMI watchdog en Phase 3 (LAPIC disponible) | Phase 3 | `[ ]` |
| S-05 | ExoSeal verify_chain() en Phase 6 (heap + PCI disponibles) | Phase 6 | `[ ]` |
| S-06 | ExoShield IOMMU en Phase 7 (avant Ring1) | Phase 7 | `[ ]` |
| S-07 | ExoCage : SMEP activé BSP + APs | Phase 2 | `[ ]` |
| S-08 | ExoCage : SMAP activé BSP + APs | Phase 2 | `[ ]` |
| S-09 | ExoCage : KPTI actif | Phase 2 | `[ ]` |
| S-10 | ExoCage : CET Shadow Stack Ring3 | Phase 2 | `[ ]` |
| S-11 | ExoCage : NX/XD actif | Phase 2 | `[ ]` |
| S-12 | Zero Trust : fast path bitmask Ring1↔Ring1 (ERR-09) | Phase 5 | `[ ]` |
| S-13 | Zero Trust : slow path complet Ring3→Ring1 | Phase 5 | `[ ]` |
| S-14 | CapToken : verify() sur chaque accès ressource | Phase 5 | `[ ]` |
| S-15 | CapToken : révocation immédiate propagée | Phase 5 | `[ ]` |
| S-16 | ExoKairos : budget avec reset fenêtre (ERR-07) | Phase 4 | `[ ]` |
| S-17 | ExoKairos : throttle à 100% budget fenêtre | Phase 4 | `[ ]` |
| S-18 | ExoKairos : kill à 200% budget fenêtre | Phase 4 | `[ ]` |
| S-19 | ExoLedger : is_immutable() vérifié dans blob_write (ERR-04) | Phase 5 | `[ ]` |
| S-20 | ExoLedger : test écriture sur objet immutable → Deny + audit | CORR-84 | `[ ]` |
| S-21 | ExoShield : IOMMU activé avant les drivers | Phase 7 | `[ ]` |
| S-22 | ExoShield : DMA hors plage → fault + DMA stoppé | Phase 7 | `[ ]` |
| S-23 | ExoNMI : watchdog armé toutes les 200ms | Phase 3 | `[ ]` |
| S-24 | Suite complète 13/13 tests sécurité PASS | security_integration | `[ ]` |

---

## BLOC 3 — Drivers (INCHANGÉ sauf B-01/B-02 déplacés en BLOC -1)

| # | Critère | Statut |
|---|---------|--------|
| D-01 | virtio-net : TX/RX fonctionnel | `[ ]` |
| D-02 | virtio-net : IOMMU domain séparé | `[ ]` |
| D-03 | virtio-blk : BAR lu dynamiquement depuis PCI config space | `[ ]` |
| D-04 | virtio-blk : lecture/écriture fonctionnelle sur disque | `[ ]` |
| D-05 | virtio-console : console fonctionnelle | `[ ]` |
| D-06 | ps2-keyboard : saisie texte stable | `[ ]` |
| D-07 | ps2-mouse : mouvements et clics | `[ ]` |
| D-08 | fb-gop : framebuffer GOP stable | `[ ]` |
| D-09 | rtl8139 : MVP réseau QEMU | `[ ]` |
| D-10 | e1000e : MVP réseau Intel | `[ ]` |
| D-11 | nvme : MVP lecture/écriture blocks | `[ ]` |
| D-12 | ahci : MVP lecture/écriture SATA | `[ ]` |
| D-13 | IommuFaultQueue : CAS-strong actif | `[ ]` |
| D-14 | Zéro panic! ni alloc dans les ISRs (Semgrep vérifié) | `[ ]` |

---

## BLOC 4 — Kernel Core (RÉVISÉ)

| # | Critère | Correction | Statut |
|---|---------|-----------|--------|
| K-01 | Fork : VMA tree cloné correctement | Existant | `[ ]` |
| K-02 | Fork : TLB shootdown deadlock résolu | Existant | `[ ]` |
| K-03 | CoW : KERNEL_FAULT_ALLOC sur bonne address space | Existant | `[ ]` |
| K-04 | Scheduler : SMP fonctionnel tous APs | Existant | `[ ]` |
| K-05 | APs SYSCALL MSRs set avant STI | Existant | `[ ]` |
| K-06 | IPC SpscRing : 0 drop sous charge nominale | Existant | `[ ]` |
| K-07 | SYS_GETDENTS64 implémenté | Existant | `[ ]` |
| K-08 | SYS_FUTEX partiel (FUTEX_WAIT, FUTEX_WAKE) | Existant | `[ ]` |
| K-09 | SYS_CLONE avec CLONE_THREAD | Existant | `[ ]` |
| K-10 | Stress test 2h+ : 0 panic kernel | Stress test | `[ ]` |
| K-11 | Memory leak test 2h : 0 fuite | Balloon test | `[ ]` |

---

## BLOC 5 — Bibliothèques ExoOS (RÉVISÉ)

| # | Critère | Correction | Statut |
|---|---------|-----------|--------|
| L-01 | exo-alloc : `dlmalloc` backend principal no_std | ERR-08 | `[ ]` |
| L-02 | exo-alloc : align_up() correct (Kani prouvé) | Existant | `[ ]` |
| L-03 | exo-alloc : 5/5 tests PASS | Existant | `[ ]` |
| L-04 | exo-net : données ≤ 200B → inline IPC | ERR-05 | `[ ]` |
| L-05 | exo-net : données > 200B → SHM + IPC référence | CORR-85 | `[ ]` |
| L-06 | exo-net : TcpStream connect/read/write testé | Existant | `[ ]` |
| L-07 | exo-net : DNS via hickory-dns | Existant | `[ ]` |
| L-08 | exo-crypto : AES-GCM, ChaCha20, Argon2id, Ed25519 | Existant | `[ ]` |
| L-09 | exo-fs : open/read_at/write_at | Existant | `[ ]` |
| L-10 | exo-fs : snapshot_create / rollback | Existant | `[ ]` |
| L-11 | exo-runtime : async executor fonctionnel | Existant | `[ ]` |
| L-12 | PhoenixSafe : toutes les libs à état implémentent le trait | Existant | `[ ]` |

---

## BLOC 6 — musl-exo & Compatibilité POSIX

| # | Critère | Statut |
|---|---------|--------|
| M-01 | POSIX coverage documenté comme "0/127 actuel, 95% cible" | `[ ]` |
| M-02 | open/read/write/close fonctionnels | `[ ]` |
| M-03 | fork/exec/wait fonctionnels | `[ ]` |
| M-04 | socket/connect/send/recv TCP | `[ ]` |
| M-05 | getuid() retourne 0, setuid() no-op + ExoLedger | `[ ]` |
| M-06 | /dev/urandom → crypto_server TRNG | `[ ]` |
| M-07 | Sandbox POSIX (/bin/, /usr/, /home/) montée | `[ ]` |
| M-08 | 14/14 tests musl_exo PASS | `[ ]` |

---

## BLOC 7 — Gestionnaire de Paquets `exo`

| # | Critère | Statut |
|---|---------|--------|
| E-01 | `exo compat install calendar` → fonctionne (persisté sur disque) | `[ ]` |
| E-02 | `exo compat install curl` → fonctionne | `[ ]` |
| E-03 | `exo compat install vlc` → installe avec avertissement Wayland | `[ ]` |
| E-04 | `exo remove <pkg>` → caps révoquées + ExoFS nettoyé | `[ ]` |
| E-05 | `exo doctor` → 0 erreur critique sur système sain | `[ ]` |
| E-06 | `exo audit` → 50 dernières entrées ExoLedger affichées | `[ ]` |

---

## BLOC 8 — Affichage & Protocole ExoOS

| # | Critère | Statut |
|---|---------|--------|
| A-01 | `exo ls -l` : format capability natif (b/d/r/s/x/c + rwxlksd + @token) | `[ ]` |
| A-02 | `exo ps` : colonnes RING / CAPS / STATE | `[ ]` |
| A-03 | Erreurs : format EXO-XXXX + ExoLedger#ID | `[ ]` |
| A-04 | Aucun outil natif n'affiche rwx / uid / gid | `[ ]` |
| A-05 | Documentation "POSIX 95% = cible, pas état actuel" | `[ ]` |

---

## BLOC 9 — Graphisme & Shell (RÉVISÉ — sans wgpu)

| # | Critère | Correction | Statut |
|---|---------|-----------|--------|
| G-01 | fb_server : framebuffer GOP stable | Existant | `[ ]` |
| G-02 | fb_server : blit depuis SHM Ring3 | Existant | `[ ]` |
| G-03 | fb_server : événements PS/2 routés vers Ring3 | Existant | `[ ]` |
| G-04 | fontdue : rendu texte ASCII complet no_std | CORR-83 | `[ ]` |
| G-05 | exosh : prompt $ interactif et stable sur framebuffer | CORR-83 | `[ ]` |
| G-06 | exosh : exo ls → format capability correct | Existant | `[ ]` |
| G-07 | ExoPhoenix : exosh redémarre proprement après bascule | Existant | `[ ]` |
| G-08 | wgpu → reporté v0.3.0 (documenté) | CORR-83 | `[-]` |
| G-09 | iced → reporté v0.3.0 (documenté) | CORR-83 | `[-]` |

---

## BLOC 10 — Observabilité

| # | Critère | Statut |
|---|---------|--------|
| O2-01 | monitor_server : logs Ring1 + Ring3 | `[ ]` |
| O2-02 | monitor_server : persistance dans ExoFS | `[ ]` |
| O2-03 | `exo log` avec filtres (level, pid) | `[ ]` |
| O2-04 | `exo metrics` : CPU / mémoire / IPC / réseau | `[ ]` |

---

## BLOC 11 — exo_shield Complet (NOUVEAU — post-audit sécurité)

| # | Critère | CORR | Statut |
|---|---------|------|--------|
| ES-01 | lib.rs : 5 modules orphelins ajoutés (hooks, sandbox, network, ml, forensics) | CORR-75-A | `[ ]` |
| ES-02 | main.rs : inits des 5 modules dans _start() | CORR-75-B | `[ ]` |
| ES-03 | hooks branchés dans handle_event_report() | CORR-75-C | `[ ]` |
| ES-04 | Containment réel (sandbox) dans handle_quarantine_cmd() | CORR-75-D | `[ ]` |
| ES-05 | YARA patterns 8→64 bytes | CORR-75-E | `[ ]` |
| ES-06 | Bridge ExoArgos→exo_shield (PMC_ANOMALY_REPORT) | CORR-75-F | `[ ]` |

---

## BLOC 12 — Tests Globaux (RÉVISÉ)

| # | Critère | Résultat |
|---|---------|---------|
| T-01 | `cargo test --all` : 0 FAIL | `[ ]` |
| T-02 | `cargo test --test integration` : 0 FAIL | `[ ]` |
| T-03 | Security integration tests : 13/13 PASS | `[ ]` |
| T-04 | ExoPhoenix tests : 14/14 PASS (incluant 4 nouveaux) | `[ ]` |
| T-05 | musl-exo tests : 14/14 PASS | `[ ]` |
| T-06 | Driver tests : 14/14 PASS | `[ ]` |
| T-07 | Kani proofs : 5/5 PASS | `[ ]` |
| T-08 | audit_constants.py : 0 erreurs | `[ ]` |
| T-09 | Semgrep : 0 violations | `[ ]` |
| T-10 | cargo deny check : 0 violations | `[ ]` |
| T-11 | Stress test kernel 2h : 0 panic | `[ ]` |
| T-12 | Memory leak test 2h : 0 fuite | `[ ]` |
| T-13 | ExoLedger chain verify : intact depuis boot | `[ ]` |
| T-14 | Test pénétration injection PID : 0 succès | `[ ]` |

---

## Compteur de Progression Révisé

```
BLOC -1  Bugs Kernel Bloquants    :  0 / 10   [ 0%]  ← NOUVEAU, PRIORITÉ ABSOLUE
BLOC 0   Outillage d'Audit        :  0 / 13   [ 0%]  ← NOUVEAU
BLOC 1   ExoPhoenix               :  0 / 14   [ 0%]
BLOC 2   Sécurité Boot            :  0 / 24   [ 0%]
BLOC 3   Drivers                  :  0 / 14   [ 0%]
BLOC 4   Kernel Core              :  0 / 11   [ 0%]
BLOC 5   Libs ExoOS               :  0 / 12   [ 0%]
BLOC 6   musl-exo                 :  0 / 8    [ 0%]
BLOC 7   PKG exo                  :  0 / 6    [ 0%]
BLOC 8   Affichage                :  0 / 5    [ 0%]
BLOC 9   Graphisme & Shell        :  0 / 7    [ 0%]  (2 reportés v0.3.0)
BLOC 10  Observabilité            :  0 / 4    [ 0%]
BLOC 11  exo_shield Complet       :  0 / 6    [ 0%]  ← NOUVEAU
BLOC 12  Tests Globaux            :  0 / 14   [ 0%]
────────────────────────────────────────────────────
TOTAL                             :  0 / 158  [ 0%]

Seuil de release v0.2.0 : 158/158  (100%)
Seuil RC1                : 145/158  (92%)
```

**Delta vs v1 de la checklist :**
- 143 critères → 158 critères (+15)
- Ajout BLOC -1 : 10 bugs kernel bloquants
- Ajout BLOC 0 : 13 critères d'outillage
- Ajout BLOC 11 : 6 critères exo_shield complet
- Révision BLOC 1 : +3 critères (SSR redesign, tests dédiés, Ring1 parallèle)
- Révision BLOC 2 : séquence de boot reordonnée
- wgpu/iced/winit : reportés v0.3.0 (2 critères marqués `[-]`)

---

*claude-alpha — ExoOS v0.2.0 — MASTER-CHECKLIST-V0.2-REV2.md*
