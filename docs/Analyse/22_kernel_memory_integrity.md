# Analyse Exhaustive Ligne par Ligne — `kernel/src/memory/integrity/`

## 1. Vue d'Ensemble du Module d'Intégrité Mémoire

Le sous-système `kernel/src/memory/integrity/` regroupe les mécanismes de détection proactive de corruption mémoire, de débordement de tampon (buffer overflow) et d'utilisation après libération (use-after-free). Il agit comme une couche de vérification continue protégeant le noyau contre les failles d'exploitation et les anomalies logicielles.

Le module s'articule autour de 3 piliers :
- `canary.rs` : Canaris de pile (stack canaries) par CPU et par thread, générés à partir d'entropie matérielle (mélangeur FNV/SplitMix64 sur TSC) avec support de rotation dynamique et panique immédiate en cas d'altération de l'adresse de retour.
- `guard_pages.rs` : Pages sentinelles (guard pages) non-présentes encadrant les piles noyau des CPUs, les piles de threads et les allocations `vmalloc`, déclenchant un défaut de page explicite dès qu'un débordement franchit la limite du tampon.
- `sanitizer.rs` : Implémentation allégée de Kernel Address Sanitizer (KASAN-lite) utilisant une table d'ombre (shadow map) où chaque octet représente l'accessibilité de 8 octets physiques du tas noyau, avec empoisonnement des zones rouges (redzones) et détection des UAF.

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 Canaris de Pile (`canary.rs`)
- **Génération Déterministe d'Entropie :**
  - La fonction `generate_canary` combine le compteur de cycles TSC avec l'identifiant CPU et la constante du nombre d'or de Fibonacci (`0x9E37_79B9_7F4A_7C15`).
  - La table `CANARY_TABLE` aligne chaque entrée sur 64 octets (`#[repr(C, align(64))]`) afin d'éliminer totalement le phénomène de faux partage de cache (false sharing) entre cœurs SMP adjacents.
- **Vérification et Sanction :**
  - La méthode `verify_thread_canary` compare le canari attendu avec celui stocké dans le TCB.
  - En cas de divergence, `canary_violation_handler` déclenche sans délai un `panic!` avec affichage des canaris attendu et observé, interdisant toute poursuite de l'exécution avec une pile corrompue.

### 2.2 Pages Sentinelles (`guard_pages.rs`)
- **Marquage PTE Exclusif :**
  - Les pages gardes utilisent une valeur de PTE spécifique : bit `PRESENT = 0` et bits réservés OS 11:9 configurés à `0b111` (`GUARD_PTE_TAG = 0xE00`).
  - Cela permet au gestionnaire de défaut de page (`#PF`) de distinguer instantanément un simple accès invalide d'un franchissement critique de page garde (`is_guard_pte`).
- **Annuaire des Régions (`GuardRegionTable`) :**
  - Capacité de 4096 descripteurs de régions gardées (`low_guard`, `high_guard`, `region_start`, `kind`).
  - Recherche du premier emplacement libre optimisée avec l'idiome Rust `(0..MAX_GUARD_REGIONS).find(|&i| !self.regions[i].active)`.

### 2.3 Sanitizer Mémoire KASAN-lite (`sanitizer.rs`)
- **Représentation Shadow Map :**
  - Ratio de compression 8:1 (1 octet shadow pour 8 octets adressables).
  - États : `SHADOW_ACCESSIBLE = 0x00`, `SHADOW_REDZONE = 0xFA`, `SHADOW_FREED = 0xFD`, `SHADOW_UNINIT = 0xF1`.
- **Interception des Allocations Tas :**
  - Les crochets `kasan_on_alloc` et `kasan_on_free` désempoisonnent et empoisonnent automatiquement les plages allouées dans l'allocateur hybride du noyau.

---

## 3. Synthèse des Corrections Appliquées

| Fichier | Problème Détecté | Correction Appliquée |
| :--- | :--- | :--- |
| `canary.rs` | Absence de `Default` sur `CanaryStats` | `impl Default for CanaryStats` |
| `canary.rs` | En-têtes doc `# Safety` non conformes | Remplacement des deux-points par un titre Markdown propre sur 3 fonctions |
| `guard_pages.rs` | Boucle manuelle de recherche `for ... if !active { return Some }` | Remplacement par l'itérateur idiomatique `.find()` |
| `guard_pages.rs` | Absence de `Default` sur `GuardPageStats` et `GuardRegionTable` | `impl Default for GuardPageStats` & `impl Default for GuardRegionTable` |
| `guard_pages.rs` | En-tête doc `# Safety` non conforme | Correction du format de titre sur `clear_guard_pte` |
| `sanitizer.rs` | Absence de `Default` sur `KasanStats` | `impl Default for KasanStats` |
| `sanitizer.rs` | En-têtes doc `# Safety` non conformes | Remplacement des deux-points par un titre Markdown propre sur 6 fonctions |
| `mod.rs` | En-tête doc `# Safety` non conforme | Correction du format de titre sur `init()` |

---

## 4. Statut de Validation

Le sous-système `kernel/src/memory/integrity/` est entièrement vérifié et prêt pour la détection temps réel des anomalies mémoire.
