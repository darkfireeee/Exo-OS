# Analyse Exhaustive Ligne par Ligne — `kernel/src/memory/utils/`

## 1. Vue d'Ensemble des Utilitaires Mémoire Noyau

Le sous-système `kernel/src/memory/utils/` fournit les services transversaux de gestion critique et de synchronisation fine au sein de l'architecture mémoire d'Exo-OS. Il prend en charge la synchronisation rapide en espace utilisateur sans transition noyau inutile (table de futex), la gestion des situations de pénurie sévère de mémoire physique (Out-Of-Memory Killer) et le compactage dynamique de caches (shrinkers).

Le module se compose de :
- `futex_table.rs` : Table de hachage de fast user-space mutexes (`FutexHashTable`, 4096 seaux protégés par des `spin::Mutex<BucketInner>`), hachage SipHash-1-3 avec clé secrète initialisée au boot, files de threads endormis (`FutexWaiter`), support complet de `WAIT`, `WAKE`, `REQUEUE` et hooks de suspension vers l'ordonnanceur.
- `oom_killer.rs` : Mécanisme d'arbitrage OOM sélectionnant et neutralisant les processus consommateurs excessifs selon une heuristique pondérée (`oom_score`), avec intervalle de refroidissement (cooldown) pour éviter les cascades de terminaisons.
- `shrinker.rs` : Interface d'enregistrement et d'invocation de shrinkers (réducteurs de cache), permettant au gestionnaire de mémoire de réclamer des pages de caches d'objets (SLUB, dentry, page cache) avant d'engager des mesures d'urgence OOM.

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 Table des Futex (`futex_table.rs`)
- **Isolation par Seaux et Hash Sécurisé (Règle MEM-FUTEX / V-34) :**
  - La table est fragmentée en `FUTEX_HASH_BUCKETS = 4096` seaux indépendants. Chaque seau possède son propre verrou `spin::Mutex`.
  - La fonction de hachage `bucket_index` exploite SipHash-1-3 avec une graine aléatoire de 128 bits (`FUTEX_SEED_K0`, `FUTEX_SEED_K1`) configurée une seule fois au boot par le BSP (`init_futex_seed`), immunisant la table contre les attaques par déni de service ciblées sur les collisions de hachage (Hash DoS).
- **Opérations sur les Files d'Attente (`futex_wait`, `futex_wake`, `futex_requeue`) :**
  - Chaque `FutexWaiter` est alloué sur la pile du thread en attente et inséré dans la liste simplement chaînée du seau correspondant.
  - La vérification de valeur atomique (`*uaddr == val`) sous verrou empêche les réveils perdus (lost wakeups).
  - L'opération `futex_requeue` déplace atomiquement des waiters d'un seau source vers un seau de destination (essentiel pour `pthread_cond_broadcast`), en gérant l'ordre d'acquisition des verrous pour écarter tout risque d'interblocage.
  - La liaison vers le scheduler via `FUTEX_SLEEP_HOOK` est maintenant typée explicitement lors de la conversion de pointeur brut : `core::mem::transmute::<*mut (), FutexSleepHook>(ptr)`.

### 2.2 Out-Of-Memory Killer (`oom_killer.rs`)
- **Sélection de la Victime (`select_oom_victim`) :**
  - Évaluation des processus candidats via `OomScorer` ou par échantillonnage dans `OOM_CANDIDATE_BUF`.
  - Tri rapide instable par clé inversée : `slice.sort_unstable_by_key(|b| core::cmp::Reverse(b.oom_score))`.
  - Application d'un temps de garde (`LAST_KILL_TSC`) pour laisser au processus visé le temps de libérer ses pages physiques avant de déclencher une nouvelle éviction.

### 2.3 Régulation de Pression Mémoire (`shrinker.rs`)
- Enregistrement de fonctions de rappel réclamant la mémoire jetable des sous-systèmes du noyau lors du franchissement des seuils d'alerte des zones physiques.

---

## 3. Synthèse des Corrections Appliquées

| Fichier | Problème Détecté | Correction Appliquée |
| :--- | :--- | :--- |
| `futex_table.rs` | Absence de `Default` sur `FutexBucket` et `FutexStats` | `impl Default for FutexBucket` & `impl Default for FutexStats` |
| `futex_table.rs` | En-têtes doc `# Safety` non conformes et manquants | Remplacement des deux-points et ajout de la doc sur 4 fonctions |
| `futex_table.rs` | `transmute` sans annotation de type explicite | `core::mem::transmute::<*mut (), FutexSleepHook>(ptr)` |
| `oom_killer.rs` | Absence de `Default` sur `OomStats` | `impl Default for OomStats` |
| `oom_killer.rs` | Tri redondant `sort_unstable_by` avec comparateur de tuple | `sort_unstable_by_key` avec `Reverse(b.oom_score)` |

---

## 4. Statut de Validation

Le sous-système `kernel/src/memory/utils/` est désormais parfaitement aligné sur les critères de robustesse et de sûreté du noyau Exo-OS.
