# PATCH FIX — Commit 26e1b5ac "Partial fix terminal"
## ExoOS v0.2.0 — Strata

**Date :** 2026-06-02  
**Basé sur HEAD :** 26e1b5ac  
**Auteur patch :** claude-alpha  

---

## Fichiers modifiés (6)

| Fichier | Patch | Sévérité |
|---|---|---|
| `servers/init_server/src/service_table.rs` | SEC-01 : réordonne exo_shield avant exosh | **P0** |
| `kernel/src/syscall/table.rs` | SEC-02 : capability guard SYS_FRAMEBUFFER_INFO | P1 |
| `kernel/src/memory/virtual/address_space/user.rs` | MEM-01 : debug_assert → assert dur | P1 |
| `kernel/src/ipc/channel/mpmc.rs` | IPC-01 : doc RING_SIZE 4096→16 | P2 |
| `kernel/src/syscall/fs_bridge.rs` | TTY-01 : timeout 5 s → 500 ms | P2 |
| `servers/fb_server/src/main.rs` | FB-01/02 : doc CONSOLE + font path | P2 |

## Outils Python (3)

| Script | Usage |
|---|---|
| `tools/verify_patch_26e1b5ac.py` | Vérifie que tous les patches sont appliqués |
| `tools/check_service_order.py` | Valide la topologie du service_table |
| `tools/scan_unsafe_patterns.py` | Scanne unwrap/panic/debug_assert hors tests |

---

## Application du patch

```bash
# Depuis la racine du dépôt ExoOS
cp patch/servers/init_server/src/service_table.rs   servers/init_server/src/
cp patch/kernel/src/syscall/table.rs                kernel/src/syscall/
cp patch/kernel/src/memory/virtual/address_space/user.rs \
                                                    kernel/src/memory/virtual/address_space/
cp patch/kernel/src/ipc/channel/mpmc.rs             kernel/src/ipc/channel/
cp patch/kernel/src/syscall/fs_bridge.rs            kernel/src/syscall/
cp patch/servers/fb_server/src/main.rs              servers/fb_server/src/
cp patch/tools/verify_patch_26e1b5ac.py             tools/
cp patch/tools/check_service_order.py               tools/
cp patch/tools/scan_unsafe_patterns.py              tools/
```

## Vérification

```bash
# Vérifier les patches appliqués
python3 tools/verify_patch_26e1b5ac.py --repo .

# Valider la topologie de démarrage Ring1
python3 tools/check_service_order.py --repo .

# Scanner les unwrap/panic en production (kernel)
python3 tools/scan_unsafe_patterns.py --repo . --dir kernel --severity P1
```

---

## Détail des patches

### SEC-01 — Régression sécurité : ordre exo_shield/exosh [P0]

**Problème :** Le commit 26e1b5ac avait ajouté `"exosh"` dans `DEPS_EXO_SHIELD`
et retiré `"exo_shield"` de `DEPS_EXOSH`. Résultat : exosh pouvait démarrer
avant exo_shield, rendant le shell interactif sans surveillance NGAV active.

**Correction :**
- `DEPS_EXOSH` → ajoute `"exo_shield"` (exosh attend SHIELD_READY)
- `DEPS_EXO_SHIELD` → retire `"exosh"` (exo_shield démarre sans exosh)
- `CANONICAL_SERVICES` → exo_shield placé avant exosh dans la table

**Invariant Strata restauré :**
```
Vague 5 : exo_shield  ← scan initial tous PID 1..N → SHIELD_READY
Vague 6 : exosh       ← prompt interactif après SHIELD_READY
```

### SEC-02 — SYS_FRAMEBUFFER_INFO sans contrôle d'accès [P1]

**Problème :** `sys_framebuffer_info` retournait l'adresse physique du
framebuffer à n'importe quel processus, y compris Ring3.

**Correction :** `AtomicU32 FB_INFO_AUTHORIZED_PID` — le premier appelant
(fb_server au boot) est autorisé ; tout PID différent reçoit `EACCES`.

### MEM-01 — debug_assert silencieux en release [P1]

**Problème :** `map_page_unflushed` utilisait `debug_assert!` pour vérifier
que l'adresse virtuelle est dans l'espace utilisateur. En build release, cette
vérification est supprimée : une adresse incorrecte corromprait silencieusement
l'espace noyau.

**Correction :** Remplacement par `assert!` dur avec message d'erreur formaté.

### IPC-01 — Documentation mpmc.rs incorrecte [P2]

**Problème :** Ligne 11 de `mpmc.rs` indiquait "4096 slots" alors que
`RING_SIZE = 16` dans `ipc/core/constants.rs`. Documentation trompeuse.

**Correction :** Documentation mise à jour avec la valeur réelle (16 slots)
et référence à la constante canonique.

### TTY-01 — Timeout TTY trop long [P2]

**Problème :** `TTY_SEND_TIMEOUT_NS = 5_000_000_000` (5 secondes).
Un `write()` shell sur un tty_server lent gèle l'interface 5 secondes.

**Correction :** Réduit à `500_000_000` (500 ms) — suffisant pour absorber
la contention IPC normale.

### FB-01/02 — Documentation fb_server [P2]

**FB-01 :** `CONSOLE` utilise `UnsafeCell` sans mutex ni documentation de
l'invariant mono-thread requis. Commentaire SAFETY ajouté.

**FB-02 :** Chemin relatif `#[path = "../../../exo-boot/src/display/font.rs"]`
fragile (se casse si un répertoire est déplacé). TODO ajouté pour migration
vers un crate `exo-font` dédié.

---

*claude-alpha — ExoOS v0.2.0 — Strata — 2026-06-02*
