# Roadmap Implémentation v0.2.0 — Stabilisation Complète

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Objectif :** Amener le kernel ExoOS de 4.3% à 100% de conformité checklist  
**Durée estimée :** 5 sprints de 2 semaines

---

## Principes de Cette Roadmap

1. **Aucune feature nouvelle** avant 100% checklist — v0.2.0 est la stabilisation, pas l'extension.
2. **BLOC -1 en premier** — tout le reste dépend d'un disque qui fonctionne et d'un boot sain.
3. **CI/CD en place dès sprint 1** — les outils d'audit évitent les régressions dans les sprints suivants.
4. **Tests QEMU à chaque sprint** — pas de merge sans smoke test réussi.
5. **Un fichier par correction** — chaque CORR-IOTA-XX correspond à une PR atomique.

---

## Sprint 1 — Fondations (Semaines 1–2)

> **Critère de sortie :** Boot OK avec 2 GiB · ExoFS persiste · cgroup propre · CI active

### S1-01 · CORR-IOTA-01 — VirtIO BAR Dynamique
**Priorité :** P0  
**Effort :** 1 jour  
**Fichiers :** `fs/exofs/storage/virtio_adapter.rs` · `drivers/pci_topology.rs`  
**Test de sortie :**
```bash
# Boot -m 2G avec virtio-blk-pci → ExoFS monte
# echo test > /tmp/probe && reboot && cat /tmp/probe affiche "test"
qemu-system-x86_64 -m 2G \
  -drive file=test.img,if=none,id=d0 \
  -device virtio-blk-pci,drive=d0 \
  -kernel exoos.elf -nographic
```
**Valide :** B-01, B-02, D-01, D-03, F-01, F-02

---

### S1-02 · CORR-IOTA-02 — cgroup Avant runqueue
**Priorité :** P0  
**Effort :** 0.5 jour  
**Fichiers :** `scheduler/mod.rs` · `process/mod.rs`  
**Test de sortie :**
```rust
#[test]
fn cgroup_before_runqueue() {
    let root = process::resource::cgroup::root();
    assert!(root.is_valid());
    let idle0 = scheduler::idle::idle_pid_for_cpu(0);
    assert_eq!(process::resource::cgroup::pid_cgroup(idle0), root);
}
```
**Valide :** B-05

---

### S1-03 · CORR-IOTA-03 — Physmap 2 GiB + ACPI Parser
**Priorité :** P0  
**Effort :** 1 jour  
**Fichiers :** `arch/x86_64/acpi/parser.rs` · `memory/core/layout.rs` · `arch/x86_64/boot/memory_map.rs`  
**Test de sortie :**
```bash
# Boot -m 2G sans panic kernel
qemu-system-x86_64 -m 2G -kernel exoos.elf -nographic 2>&1 | \
  grep -v "PANIC" | grep "physmap étendue"
# Attendu : "[physmap] étendue à 0x80000000 (2 GiB)"
```
**Valide :** B-03, B-04, O-04

---

### S1-04 · CORR-IOTA-05 — exosh Sans Réseau
**Priorité :** P0  
**Effort :** 0.5 jour  
**Fichiers :** `init_server/src/service_table.rs` · `init_server/src/boot_sequence.rs`  
**Test de sortie :**
```bash
# Boot sans -net → exosh disponible en < 5s
qemu-system-x86_64 -m 1G -net none -kernel exoos.elf -nographic 2>&1 | \
  grep -E "exosh.*prêt|shell.*ready"
# timestamp doit être < 5000ms depuis le boot
```
**Valide :** B-07, SH-01, SH-02

---

### S1-05 · CORR-IOTA-06 + CORR-IOTA-07 — Outillage Constantes
**Priorité :** P1  
**Effort :** 1 jour  
**Fichiers :** `kernel/src/arch/constants.rs` (CRÉER) · `exophoenix/ssr.rs` · `security/exokairos.rs`  
**Test de sortie :**
```bash
python3 tools/audit_constants.py
# Attendu : "Toutes les constantes sont cohérentes."
cargo build --target x86_64-unknown-none 2>&1 | grep "error\[E0080\]"
# Attendu : aucun (les const_assert! ne paniquent pas)
```
**Valide :** O-01 à O-05, P-01, E-02

---

### S1-06 · CORR-IOTA-08 + CORR-IOTA-10 — deny.toml + CI
**Priorité :** P1  
**Effort :** 0.5 jour  
**Fichiers :** `deny.toml` (CRÉER) · `.github/workflows/audit.yml` (CRÉER)  
**Test de sortie :**
```bash
cargo deny check
# Attendu : 0 erreurs
```
**Valide :** O-10, O-11, O-13

---

**Bilan Sprint 1 — Items validés attendus :**

| Bloc | Avant | Après S1 | Delta |
|---|---|---|---|
| BLOC -1 | 0/7 | 7/7 | +7 ✅ |
| BLOC 0 | 0/13 | 7/13 | +7 ✅ |
| BLOC 1 | 0/22 | 1/22 | +1 ✅ |
| BLOC 3 | 1/8 | 4/8 | +3 ✅ |
| BLOC 6 | 2/10 | 4/10 | +2 ✅ |
| BLOC 8 | 1/8 | 3/8 | +2 ✅ |

---

## Sprint 2 — Sécurité Boot + IPC (Semaines 3–4)

> **Critère de sortie :** `phase5-tests` passe · cap token check actif · ExoKairos fenêtré

### S2-01 · CORR-IOTA-04 — Cap Token IPC (Injection PID)
**Priorité :** P0 Sécurité  
**Effort :** 1 jour  
**Fichiers :** `syscall/table.rs` · `ipc/core/constants.rs`  
**Test de sortie :**
```rust
#[test]
fn ipc_injection_blocked() {
    // Un processus Ring3 envoie len=200 sans cap token
    let result = sys_exo_ipc_send(ep, buf_ptr, 200, 0, 0, 0);
    assert_eq!(result, EACCES, "doit retourner EACCES");
}
```
**Valide :** B-06, I-04, I-05, I-10

---

### S2-02 · CORR-IOTA-11 — ExoKairos Fenêtre Reset
**Priorité :** P1  
**Effort :** 1 jour  
**Fichiers :** `security/exokairos.rs`  
**Test de sortie :**
```rust
#[test]
fn kairos_window_reset() {
    let cap = TemporalCap::new(10, 10_240, 5_000_000_000);
    // Épuiser le budget
    for _ in 0..10 { cap.use_cap(1, 1024); }
    assert_eq!(cap.use_cap(1, 1024), KairosDecision::Kill); // budget épuisé
    
    // Simuler 1 seconde écoulée
    mock_advance_tsc(KAIROS_WINDOW_NS);
    
    // Après reset de fenêtre, budget renouvelé
    assert_eq!(cap.use_cap(1, 1024), KairosDecision::Allow);
}
```
**Valide :** S-16, S-17, S-18

---

### S2-03 · CORR-IOTA-12 — ExoLedger Immutabilité
**Priorité :** P1  
**Effort :** 0.5 jour  
**Fichiers :** `fs/exofs/syscall/object_write.rs` · `security/exoledger.rs`  
**Test de sortie :**
```rust
#[test]
fn immutable_write_blocked() {
    let obj = create_immutable_object();
    let result = sys_exofs_object_write(obj.id, 0, buf_ptr, 64, 0, 0);
    assert_eq!(result, EPERM);
    // Vérifier log ExoLedger
    let events = exoledger::last_events(1);
    assert_eq!(events[0].kind, ExoLedgerEvent::WriteAttemptOnImmutable { .. });
}
```
**Valide :** S-19, S-20, F-03, F-04, P-13

---

### S2-04 · CORR-IOTA-20 — BOOT_SEQUENCE_V0.2.md
**Priorité :** P2 Doc  
**Effort :** 0.5 jour  
**Fichiers :** `docs/Vision v0.2.0/BOOT_SEQUENCE_V0.2.md` (CRÉER)  
**Test de sortie :** Relecture croisée avec le code — chaque phase documentée correspond à un appel réel dans `lib.rs`/`early_init.rs`.  
**Valide :** P-19, P-22, S-01 à S-11 (documentation)

---

### S2-05 · CORR-IOTA-21 — Seuil IPC Inline 192 Octets
**Priorité :** P2  
**Effort :** 0.5 jour  
**Fichiers :** `servers/syscall_abi/src/lib.rs` · `ipc/core/constants.rs`  
**Test de sortie :**
```rust
#[test]
fn ipc_inline_threshold() {
    // Message de 192 octets → inline (pas SHM)
    let stats = ipc_send_and_measure(192);
    assert!(!stats.used_shm, "192 octets doit être inline");
    // Message de 201 octets → SHM
    let stats = ipc_send_and_measure(201);
    assert!(stats.used_shm, "201 octets doit utiliser SHM");
}
```
**Valide :** I-02, L-04, L-05

---

**Bilan Sprint 2 — Items validés supplémentaires attendus :**

| Bloc | Delta S2 |
|---|---|
| BLOC 2 | +9 (S-16..S-22) |
| BLOC 4 | +4 (I-04, I-05, I-02, I-10) |
| BLOC 5 | +2 (L-04, L-05) |
| BLOC 6 | +2 (F-03, F-04) |

---

## Sprint 3 — ExoPhoenix Complet (Semaines 5–6)

> **Critère de sortie :** Bascule A↔B < 500ms · SSR layout vérifié · typos corrigées

### S3-01 · CORR-IOTA-17 — SSR Layout + const_assert!
**Effort :** 0.5 jour  
**Fichiers :** `exophoenix/ssr.rs`  
**Valide :** P-01, P-02 (si lib.zip accessible), P-04 (E820)

### S3-02 · CORR-IOTA-18 — Ring1 Parallèle
**Effort :** 1.5 jours  
**Fichiers :** `init_server/src/boot_sequence.rs`  
**Test de sortie :**
```bash
# Mesurer le temps de recovery
t0=$(date +%s%3N)
echo "failover" > /dev/exo_phoenix
# Attendre exosh
until exosh_ready; do sleep 0.01; done
echo "$(($(date +%s%3N) - t0))ms"  # doit être < 500ms
```
**Valide :** P-07, P-08

### S3-03 · CORR-IOTA-19 — Typo TLA+ + E820
**Effort :** 0.25 jour  
**Fichiers :** `docs/Exo-OS-TLA+/redme_final_test.md` · `exo-boot/src/e820.rs`  
**Valide :** P-04, P-14, P-20

### S3-04 · Stress Test 100 Bascules
**Effort :** 1 jour  
**Fichiers :** `tests/phoenix_smoke.sh` (CRÉER)  
```bash
#!/bin/bash
# phoenix_smoke.sh — 100 bascules en 10 minutes
PASS=0; FAIL=0
for i in $(seq 1 100); do
    result=$(trigger_failover_and_measure)
    ms=$(echo $result | awk '{print $1}')
    if [ "$ms" -lt 500 ]; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        echo "FAIL bascule $i : ${ms}ms > 500ms SLA"
    fi
done
echo "$PASS/100 bascules OK · $FAIL échouées"
[ "$FAIL" -eq 0 ]  # exit code 0 si tout passe
```
**Valide :** P-16, P-21

---

## Sprint 4 — exo_shield Complet (Semaines 7–8)

> **Critère de sortie :** exo_shield : 12/12 items · containment réel démontré

### S4-01 · CORR-IOTA-13 — lib.rs Modules Déclarés
**Effort :** 0.25 jour  
**Valide :** ES-01

### S4-02 · CORR-IOTA-14 — _start() Init Complète
**Effort :** 0.5 jour  
**Valide :** ES-02, ES-08, ES-09

### S4-03 · CORR-IOTA-15 — Hooks dans handle_event_report
**Effort :** 1 jour  
**Valide :** ES-03, ES-05, ES-06

### S4-04 · CORR-IOTA-16 — Containment Sandbox Réel
**Effort :** 1.5 jours  
**Test de sortie :**
```bash
# Lancer un processus "malicieux" qui tente des connexions réseau
# exo_shield doit le détecter et l'isoler
exosh> spawn /bin/test_malicious
# Dans les logs :
# [exo_shield] PID 42 détection menace haute (score 95)
# [exo_shield] PID 42 mis en sandbox
# [exo_shield] réseau PID 42 isolé
# Vérifier que le processus ne peut plus faire de connexions réseau
```
**Valide :** ES-04, ES-10, ES-11

### S4-05 · Test Complet exo_shield
**Effort :** 0.5 jour  
**Fichiers :** `tests/exo_shield_test.sh` (CRÉER)  
**Valide :** ES-12, ES-07 (bridge ExoArgos)

---

## Sprint 5 — Shell, ELF, Compatibilité & Clôture (Semaines 9–10)

> **Critère de sortie :** 100% checklist BLOCS -1 à 8 + 11 · zéro ❌ restant

### S5-01 · Shell : `top` et `ps` via Syscall Réel
**Effort :** 1.5 jours  
**Fichiers :** `userspace/exosh/cmd/top.rs` · `userspace/exosh/cmd/ps.rs`  
**Contexte :** Actuellement `top` utilise une table PID/nom connue côté shell (limite v0.1.0 documentée).
Implémenter `SYS_EXOOS_LIST_PROCESSES` syscall dans le kernel + consommation côté shell.  
**Valide :** SH-03, SH-04

### S5-02 · ELF : `const_assert` + Validation PT_LOAD
**Effort :** 0.5 jour  
**Fichiers :** `fs/elf_loader_impl.rs` · `arch/constants.rs`  
**Valide :** E-02, E-03, E-04

### S5-03 · `exo compat install busybox`
**Effort :** 1 jour  
**Contexte :** Dépend de F-01 (disque) et E-06 (musl). Télécharger busybox statique musl,
le stocker sur ExoFS, vérifier l'exécution.  
**Valide :** E-05, E-06, F-05

### S5-04 · Outillage Semgrep (O-08, O-09)
**Effort :** 0.5 jour  
**Fichiers :** `tools/semgrep-rules/exoos.yaml` (CRÉER)  

Règles minimales :
```yaml
rules:
  - id: exoos-no-hardcoded-mmio
    pattern: "const $NAME: usize = 0x1000_0000"
    message: "Adresse MMIO hardcodée détectée — utiliser PCI BAR dynamique"
    severity: ERROR
    languages: [rust]

  - id: exoos-missing-immutable-check
    pattern: |
      fn $FUNC(...) {
        ...
        // no is_immutable() call
        ...object_write(...)
        ...
      }
    message: "Écriture d'objet sans vérification is_immutable()"
    severity: WARNING
    languages: [rust]

  - id: exoos-forbidden-std
    pattern: "use std::"
    message: "Dépendance std interdite dans le kernel no_std"
    severity: ERROR
    languages: [rust]
    paths:
      include: ["kernel/src/**"]
```
**Valide :** O-08, O-09

### S5-05 · Pre-commit Hook (O-12)
**Effort :** 0.25 jour  
**Fichier :** `.git/hooks/pre-commit` (CRÉER)  
```bash
#!/bin/bash
set -e
echo "=== pre-commit ExoOS ==="
python3 tools/audit_constants.py
cargo deny check --quiet
semgrep --config tools/semgrep-rules/exoos.yaml \
        --quiet kernel/src/ servers/
echo "=== pre-commit OK ==="
```
**Valide :** O-12

### S5-06 · Roadmap BLOCS 9/10 Reportés (G-08, G-09)
**Effort :** 0.25 jour  
**Fichier :** `docs/Vision v0.2.0/ROADMAP-IMPLEMENTATION-V0.2.md` — section Wayland v0.3.0  
**Valide :** G-08, G-09

### S5-07 · Revue Finale + `phase5-tests` 100%
**Effort :** 1 jour  
**Fichiers :** `servers/phase5-tests/`  
**Valide :** S-22, I-06, I-07, I-08

---

## Tableau de Suivi Global

| Sprint | Semaines | Items ciblés | Cumul validés | % checklist |
|---|---|---|---|---|
| Départ (v0.1.0) | — | — | 6 | 4% |
| Sprint 1 | 1–2 | +22 | 28 | 20% |
| Sprint 2 | 3–4 | +17 | 45 | 33% |
| Sprint 3 | 5–6 | +12 | 57 | 41% |
| Sprint 4 | 7–8 | +15 | 72 | 52% |
| Sprint 5 | 9–10 | +24 | 96 | 70% |
| Tests QEMU finaux | 11 | +16 (mesures QEMU) | 112 | 81% |
| 🔍 lib.zip validés | 11 | +16 (exo-alloc etc.) | 128 | 93% |
| Zéro ❌ BLOCS -1→8+11 | 12 | +10 | 138 | **100%** |

> Les items 🔍 (dans `libs/`) représentent 16 points non vérifiables sans accès à `lib.zip`.
> Une fois le zip extrait et vérifié, ces points doivent être audités séparément.

---

## Dépendances Critiques Entre Corrections

```
CORR-IOTA-01 (VirtIO BAR)
    └─ F-01, F-02 (ExoFS disque)
        └─ E-05 (compat install)
        └─ F-05 (quota cgroup)

CORR-IOTA-02 (cgroup avant runqueue)
    └─ F-05, I-06 (cgroup + IPC sans famine)

CORR-IOTA-03 (physmap 2 GiB)
    └─ CORR-IOTA-01 (PCI BAR peut être > 1 GiB)

CORR-IOTA-04 (cap token IPC)
    └─ I-09 (Zero Trust bitmask)

CORR-IOTA-12 (ExoLedger immutabilité)
    └─ P-13 (ExoLedger bascule)
    └─ F-03, F-04

CORR-IOTA-13 à 16 (exo_shield)
    └─ ES-07 (bridge ExoArgos)
    └─ ES-12 (test complet)
```

---

## Critères de Sortie v0.2.0

Pour déclarer la version v0.2.0 stabilisée, **tous les critères suivants** doivent être verts :

- [ ] `cargo build --target x86_64-unknown-none` : 0 erreur, 0 warning `-D warnings`
- [ ] `cargo test -p kernel` : 100% pass
- [ ] `cargo deny check` : 0 erreur
- [ ] `python3 tools/audit_constants.py` : 0 erreur
- [ ] `semgrep --config tools/semgrep-rules/exoos.yaml kernel/src/` : 0 erreur
- [ ] Boot QEMU `-m 2G` avec `virtio-blk-pci` : ExoFS monte, shell disponible en < 5s
- [ ] Persistance fichier après reboot : OK
- [ ] Bascule Phoenix 100 fois : 100% < 500ms
- [ ] Injection PID IPC : bloquée (EACCES)
- [ ] Écriture objet immutable : bloquée (EPERM) + log ExoLedger
- [ ] `phase5-tests` : 100%
- [ ] `exo_shield_test.sh` : 0 erreur
- [ ] `BOOT_SEQUENCE_V0.2.md` : complet et vérifié
- [ ] `CHECKLIST_DELTA_V0.2.0_CLAUDE_IOTA.md` : 0 item ❌ sur BLOCS -1 à 8 + 11
- [ ] `docs/Vision v0.2.0/ROADMAP-IMPLEMENTATION-V0.2.md` : BLOCS 9/10 marqués reportés v0.3.0

Quand tous ces critères sont verts → tag `v0.2.0` · début de la session Wayland/Install (v0.3.0).

---

*claude iota — ROADMAP_IMPLEMENTATION_V0.2.0_CLAUDE_IOTA.md — 2026-05-20*
