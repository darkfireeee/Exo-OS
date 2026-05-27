# ROADMAP-PHASE-0-CORRIGEE — Phase 0 Reséquencée
## Ordre d'Implémentation Corrigé — Avant Tout le Reste

**Auteur :** claude-alpha  
**Date :** 2026-05-16  
**Remplace :** Section "PHASE 0" de `ROADMAP-IMPLEMENTATION-V0.2.md`

---

## Pourquoi Phase 0 a Changé

La Phase 0 du corpus initial supposait que le kernel v0.1.0 était suffisamment stable pour construire dessus. L'audit claude-beta + claude-gamma a révélé 5 bugs kernel qui rendent **inaccessible tout le reste du corpus** :

- **CRIT-01** : physmap limitée à 1 GiB → panique sur 2+ GiB de RAM
- **CRIT-02** : cgroup::init() omis → scheduler défaillant
- **HIGH-01** : injection PID via longueur message → faille sécurité
- **HIGH-02** : service optionnel bloque exosh → UX cassée
- **HIGH-03** : USER_ELF_BASE_MIN=1TiB → aucun binaire ELF standard ne charge
- **C-GAMMA-01** : VirtIO BAR hardcodé → ExoFS RAM-only, persistance nulle

**Sans corriger ces six bugs, rien de la v0.2.0 ne peut fonctionner.**

---

## Phase 0 Corrigée — 6 Étapes Séquentielles Strictes

### Étape 0.0 — VirtIO BAR Dynamique (CORR-86)

**Bloque :** Toute persistance de données.

```bash
# Vérification du fix
qemu-system-x86_64 -m 256M -device virtio-blk-pci,drive=d \
  -drive id=d,file=exofs-root.img,format=raw,if=none -serial stdio

# Dans le log série, chercher :
# "virtio-blk PCI 00:03.0 — BAR0 MMIO base: 0xC0000000"
# Si l'adresse est 0x10000000 → CORR-86 non appliqué
```

**Test de validation :**
```
exosh:/$ echo "persistence" > /test.txt
exosh:/$ sync
# Reboot QEMU
exosh:/$ cat /test.txt
persistence   ← doit afficher ça
```

**Ne pas passer à 0.1 tant que ce test échoue.**

---

### Étape 0.1 — physmap Complète (CORR-76)

**Bloque :** Boot sur toute machine > 1 GiB.

```bash
# Test avec 2 GiB de RAM
qemu-system-x86_64 -m 2G ...
# Si panique kernel → CORR-76 non appliqué
# Si log : "physmap étendue : 2 GiB total" → OK
```

---

### Étape 0.2 — cgroup::init() (CORR-77)

**Bloque :** Démarrage des serveurs Ring1 avec attachement cgroup.

```bash
# Test : démarrer tous les Ring1 servers
# Si un serveur Ring1 crash à l'attachement cgroup → CORR-77 non appliqué
# Dans exosh : `exo ps` doit montrer tous les PIDs Ring1 en running
```

---

### Étape 0.3 — ELF Base Min (CORR-80)

**Bloque :** Chargement de tout binaire ELF standard.

```rust
// Test minimal : compiler et charger un binaire ELF Ring3
// Le binaire "hello" avec base 0x400000 doit charger sans erreur
// Si ErreurELF::BaseTooLow → CORR-80 non appliqué
```

---

### Étape 0.4 — Injection PID (CORR-78) + Service Non-Bloquant (CORR-79)

**Ordre :** CORR-78 d'abord (sécurité), puis CORR-79 (UX).

**Test CORR-78 :**
```rust
// Test de pénétration : envoyer un message de 128 octets exactement
// Si le kernel traite ce message comme un PID message sans vérification cap
// → CORR-78 non appliqué
let malicious_msg = [0u8; 128];
ipc_send(target_pid, &malicious_msg);
// Doit retourner Err(IpcError::PolicyDenied) ou Err(IpcError::InvalidType)
```

**Test CORR-79 :**
```bash
# Démarrer ExoOS sans carte réseau (-net none)
qemu-system-x86_64 -m 256M -net none ...
# Si exosh ne démarre pas → CORR-79 non appliqué
# Si exosh démarre avec message "network_server non disponible" → OK
```

---

### Étape 0.5 — Setup Outillage d'Audit (TOOLS-AUDIT-EXOOS.md)

**Bloque :** Toute la suite du développement (éviter la réapparition des bugs).

```bash
# 1. Créer arch/constants.rs avec toutes les constantes canoniques
# 2. Ajouter const_assert! dans les fichiers critiques
# 3. Créer tools/audit_constants.py
# 4. Créer tools/semgrep-rules/exoos.yaml
# 5. Installer le pre-commit hook
# 6. Installer cargo-deny avec deny.toml

# Vérification :
python3 tools/audit_constants.py  # → 0 erreurs
semgrep --config tools/semgrep-rules/exoos.yaml kernel/ --error  # → 0 erreurs
cargo check --all  # → 0 erreurs de compilation
```

**Seulement après les 6 étapes validées :** Commencer la Phase 1 (sécurité complète).

---

## Checklist Phase 0 Corrigée

| # | Critère | Test | Statut |
|---|---------|------|--------|
| 0.0-A | VirtIO BAR lu depuis PCI config space | Log "BAR0 MMIO base: 0xC000..." | `[ ]` |
| 0.0-B | ExoFS persiste sur disque après reboot | cat /test.txt → "persistence" | `[ ]` |
| 0.1-A | Boot avec -m 2G sans panique | Log "physmap étendue : 2 GiB" | `[ ]` |
| 0.1-B | phys_to_virt() sur addr > 1 GiB → OK | Test unitaire | `[ ]` |
| 0.2-A | cgroup::init() appelé avant runqueue | Log "root cgroup valide" | `[ ]` |
| 0.2-B | Ring1 servers attachés au root cgroup | exo ps → tous running | `[ ]` |
| 0.3-A | ELF base 0x400000 accepté par le loader | Binary hello charge | `[ ]` |
| 0.3-B | const_assert!(USER_ELF_BASE_MIN ≤ 0x400000) | cargo build → pass | `[ ]` |
| 0.4-A | MSG de 128 octets sans cap → PolicyDenied | Test pénétration | `[ ]` |
| 0.4-B | exosh démarre sans network_server | QEMU -net none → exosh visible | `[ ]` |
| 0.5-A | audit_constants.py → 0 erreurs | python3 tools/audit_constants.py | `[ ]` |
| 0.5-B | Semgrep → 0 erreurs sur kernel/ | semgrep --error kernel/ | `[ ]` |
| 0.5-C | const_assert! dans ssr.rs, exokairos.rs | cargo build → pass | `[ ]` |
| 0.5-D | cargo-deny → 0 violations | cargo deny check | `[ ]` |
| 0.5-E | pre-commit hook installé | git commit → audit avant commit | `[ ]` |

**Seuil de passage Phase 0 → Phase 1 : 15/15**

---

*claude-alpha — ExoOS v0.2.0 — ROADMAP-PHASE-0-CORRIGEE.md*
