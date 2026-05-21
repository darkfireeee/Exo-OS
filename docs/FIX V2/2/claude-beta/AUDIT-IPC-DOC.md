# ExoOS v0.2.0 — Audit IPC : Désynchronisation Code/Documentation (P2)
## Constantes IPC : 4 divergences confirmées entre docs/ et kernel/src/

**Auteur :** claude-beta  
**Date :** 2026-05-20  
**Sévérité :** P2 — Documentation mensongère, source de bugs futurs  
**Fichiers concernés :**  
- `docs/kernel/ipc/README.md` (documentation)  
- `kernel/src/ipc/core/constants.rs` (source de vérité)

---

## Méthode d'audit

Chaque constante listée dans `docs/kernel/ipc/README.md` section
"Constantes clés" a été croisée avec la valeur réelle dans
`kernel/src/ipc/core/constants.rs`.

---

## IPC-DOC-01 — IPC_MAX_CHANNELS : 4 096 (doc) vs 65 536 (code)

### Dans la documentation

```markdown
<!-- docs/kernel/ipc/README.md -->
| `IPC_MAX_CHANNELS` | 4 096 | Canaux simultanés max |
```

### Dans le code source

```rust
// kernel/src/ipc/core/constants.rs

pub const MAX_CHANNELS: usize = 65_536;
pub const IPC_MAX_CHANNELS: usize = MAX_CHANNELS;  // alias = 65 536
```

### Impact

Un développeur lisant la doc planifiera des structures de données ou des
tests avec la limite à 4 096 canaux. À 4 097 canaux ouverts, le comportement
réel est différent de ce qu'il anticipait (pas d'erreur côté kernel avant
65 536). Des tests de stress basés sur la doc seront incorrects.

### Correction

Mettre à jour `docs/kernel/ipc/README.md` :

```markdown
| `IPC_MAX_CHANNELS` | 65 536 | Canaux simultanés max |
```

---

## IPC-DOC-02 — SYNC_CHANNEL_TIMEOUT_NS : 100 ms (doc) vs 5 ms (code)

### Dans la documentation

```markdown
<!-- docs/kernel/ipc/README.md -->
| `SYNC_CHANNEL_TIMEOUT_NS` | 100 ms | Timeout canal synchrone |
```

### Dans le code source

```rust
// kernel/src/ipc/core/constants.rs

/// Timeout par défaut d'un canal synchrone (spin-wait) en nanosecondes = 5 ms.
pub const SYNC_CHANNEL_TIMEOUT_NS: u64 = 5_000_000;  // 5 ms, pas 100 ms
```

### Impact

Facteur 20× de différence. Un serveur Ring1 dimensionné pour tolérer des
attentes de 100 ms côté IPC synchrone verra des `Timeout` inattendus à 5 ms
sous charge. Des boucles de retry trop agressives peuvent résulter de cette
confusion.

### Correction

Mettre à jour `docs/kernel/ipc/README.md` :

```markdown
| `SYNC_CHANNEL_TIMEOUT_NS` | 5 ms (5 000 000 ns) | Timeout canal synchrone |
```

---

## IPC-DOC-03 — MSG_HEADER_MAGIC : confusion avec IPC_FUTEX_MAGIC

### Dans la documentation

```markdown
<!-- docs/kernel/ipc/README.md -->
| `MSG_HEADER_MAGIC` | 0x1FCF_07E0 | Validité de l'en-tête BdB |
```

### Dans le code source

```rust
// kernel/src/ipc/core/constants.rs

/// Valeur magique dans l'en-tête de frame de message.
/// 0x4D534748 = 'M','S','G','H'
pub const MSG_HEADER_MAGIC: u32 = 0x4D53_4748;   // ← valeur réelle

/// Valeur magique pour les futex IPC.
pub const IPC_FUTEX_MAGIC: u32 = 0x1FCF_07E0;    // ← c'est CETTE valeur dans la doc
```

### Analyse

La valeur `0x1FCF_07E0` documentée sous le nom `MSG_HEADER_MAGIC` correspond
en réalité à `IPC_FUTEX_MAGIC`. La documentation a **interverti les deux
constantes**.

### Conséquence

Du code de validation de frame IPC implémenté d'après la documentation
utiliserait `0x1FCF_07E0` comme magic de header, ce qui ne correspondrait
jamais aux frames réelles (qui utilisent `0x4D534748`). Tout parser de
messages ou outil de debug basé sur la doc rejetterait des frames valides
ou accepterait des frames invalides.

### Correction

```markdown
<!-- docs/kernel/ipc/README.md — version corrigée -->
| `MSG_HEADER_MAGIC`  | 0x4D53_4748 | Magic en-tête frame message ('M','S','G','H') |
| `IPC_FUTEX_MAGIC`   | 0x1FCF_07E0 | Magic futex IPC (distingue des futex mémoire)  |
```

---

## IPC-DOC-04 — IPC_MAX_PROCESSES : 512 (doc) vs 65 536 (code)

### Dans la documentation

```markdown
<!-- docs/kernel/ipc/README.md -->
| `IPC_MAX_PROCESSES` | 512 | Processus IPC simultanés max |
```

### Dans le code source

```rust
// kernel/src/ipc/core/constants.rs

/// Nombre maximal de processus pouvant détenir des ressources IPC.
pub const IPC_MAX_PROCESSES: usize = 65_536;
```

### Impact

Facteur 128× de différence. Des structures de données dimensionnées à 512
slots pour la table des processus IPC serait 128× trop petites. Le vrai
système peut gérer 65 536 processus concurrents détenant des ressources IPC.

### Correction

Mettre à jour `docs/kernel/ipc/README.md` :

```markdown
| `IPC_MAX_PROCESSES` | 65 536 | Processus IPC simultanés max |
```

---

## Récapitulatif des divergences IPC

| Constante | Valeur documentée | Valeur réelle | Ratio |
|---|---|---|---|
| `IPC_MAX_CHANNELS` | 4 096 | 65 536 | ×16 |
| `SYNC_CHANNEL_TIMEOUT_NS` | 100 ms | 5 ms | ×20 |
| `MSG_HEADER_MAGIC` | `0x1FCF_07E0` | `0x4D53_4748` | confusion |
| `IPC_MAX_PROCESSES` | 512 | 65 536 | ×128 |

### Recommandation systémique

Mettre en place `tools/audit_constants.py` (O-06) qui compare
automatiquement les constantes dans les fichiers `README.md` des sous-modules
avec leurs valeurs dans les fichiers `.rs` correspondants. Ce script devrait
être intégré au pre-commit hook (O-12) pour prévenir toute re-divergence.

---

*claude-beta — ExoOS v0.2.0 Audit — AUDIT-IPC-DOC.md*
