# Analyse Exhaustive Ligne par Ligne — `kernel/src/memory/cow/`

## 1. Vue d'Ensemble du Module CoW (Copy-on-Write)

Le module `kernel/src/memory/cow/` fournit le mécanisme fondamental de copie sur écriture au sein du sous-système de mémoire virtuelle d'Exo-OS. Il intervient lors du clonage d'espaces d'adressage (notamment l'appel système `fork` / `clone`) afin d'éviter la duplication immédiate et coûteuse des pages physiques utilisateur.

Le module se compose de :
- `mod.rs` : Point d'entrée, réexportations publiques et fonction d'initialisation (`init()`).
- `tracker.rs` : Table de hachage statique en accès rapide (`CowTracker`, table de 65 536 entrées) associant les index de cadres physiques (`Frame`) à leurs compteurs de références partagées (`refcount`).
- `breaker.rs` : Logique de rupture de partage (`break_cow`, `try_break_cow`), réalisant l'allocation d'un cadre exclusif, la duplication physique via la région d'accès direct au noyau (`physmap`), et la décrémentation atomique du compteur de l'ancien cadre.

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 Suivi des Références CoW (`tracker.rs`)
- **Structure des Entrées (`CowEntry`) :**
  - Entrée alignée sur 16 octets contenant `frame_idx: AtomicU64` et `ref_count: AtomicU32`.
  - Sentinelles : `SLOT_EMPTY = 0` (non utilisé), `SLOT_DELETED = u64::MAX` (tombstone).
- **Résolution des Collisions (Sondage Linéaire avec Tombrating) :**
  - Le hachage utilise l'algorithme FNV-1a 64 bits (`hash = (h ^ frame_idx) * FNV_PRIME`).
  - Lors de l'insertion (`try_inc`), le premier slot marqué `SLOT_DELETED` rencontré est mémorisé pour réutilisation optimale, limitant la dispersion de la chaîne.
  - La boucle de sondage linéaire suit l'idiome Rust `for (collisions, probe) in (0..COW_TABLE_SIZE).enumerate()`.
- **Synchronisation :**
  - Un verrou `spin::Mutex<()>` protège les opérations `try_inc`, `dec`, et `tracked_ref_count` contre les accès concurrents TOCTOU (Time of Check to Time of Use).
  - Remplacement de l'initialisation non sûre `MaybeUninit::zeroed().assume_init()` par l'expression constante inline sûre `[const { CowEntry::new() }; COW_TABLE_SIZE]`.

### 2.2 Mécanisme de Rupture (`breaker.rs`)
- **Évaluation du Refcount :**
  - Si le cadre possède un refcount $\le 1$, aucun nouveau cadre n'est alloué (`CowBreakOutcome::AlreadyExclusive(frame)`). Le bit d'écriture peut être rétabli directement dans la table de pages.
- **Duplication Physique :**
  - En cas de refcount $\ge 2$, allocation d'une nouvelle page physique (`alloc_page(AllocFlags::NONE)`).
  - Copie exacte de 4096 octets via l'espace direct `PHYS_MAP_BASE` à l'aide de `core::ptr::copy_nonoverlapping`.
  - **Invariance de Sécurité :** La source et la destination résident dans la mémoire physique mappée et sont strictement disjointes.
  - La décrémentation (`COW_TRACKER.dec(frame)`) est opérée immédiatement ; si le compteur atteint zéro, l'ancien cadre est restitué à l'allocateur de cadres (`free_page`).

---

## 3. Synthèse des Corrections Appliquées

| Fichier | Problème Détecté | Correction Appliquée |
| :--- | :--- | :--- |
| `cow/tracker.rs` | `needless_range_loop` / compteur explicite de collisions | Remplacement par `for (collisions, probe) in (0..COW_TABLE_SIZE).enumerate()` |
| `cow/tracker.rs` | Initialisation non vérifiée via `MaybeUninit::zeroed().assume_init()` | Remplacement par `[const { CowEntry::new() }; COW_TABLE_SIZE]` |
| `cow/tracker.rs` | Absence de `Default` sur `CowTracker` | `impl Default for CowTracker` |
| `cow/breaker.rs` | Absence de `Default` sur `CowBreakerStats` | `impl Default for CowBreakerStats` |

---

## 4. Statut de Validation

Le module `kernel/src/memory/cow/` est vérifié, sécurisé contre les courses concurrentes, et conforme aux exigences no_std.
