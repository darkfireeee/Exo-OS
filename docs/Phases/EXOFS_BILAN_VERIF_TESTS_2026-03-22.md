# Rapport de Résolution: Tests Intégration ExoFS et SIGSEGV (Bilan Final)
**Date** : 22 Mars 2026

## 1. Contexte et Objectif
Lors de l'implémentation de la batterie de tests des 3 Tiers (Simple, Moyen, Stress) pour le module `ObjectCache` d'ExoFS, l'exécution `cargo test` sous l'hôte WSL/Linux se heurtait à plusieurs problèmes bloquants, allant du crash silencieux brutal (`SIGSEGV`) à de multiples erreurs API lors de la compilation.

L'objectif était d'obtenir un "vert final" concluant tout en validant le fonctionnement d'allocation et d'éviction du cache.

## 2. Problème 1 : Crash `SIGSEGV` (Conflits Bare-metal vs Host OS)
Les tests binaires craschaient car l'environnement hôte tentait d'interpréter des instructions noyaux de très bas niveau qui étaient injectées globalement.

### A. Court-circuit de l'Allocateur Global
*   **Fichier touché** : `kernel/src/memory/heap/allocator/global.rs`
*   **Cause** : Le `KernelAllocator` (spécifique au noyau) était déclaré via `#[global_allocator]`. Lors de l'exécution des tests sur l'environnement hôte, il corrompait la pile d'allocation Linux au lieu d'utiliser celle de la lib standard.
*   **Résolution** : Protéger la directive pour qu'elle ne s'applique qu'au code hors test.
    ```rust
    #[cfg_attr(not(test), global_allocator)]
    ```

### B. Intrusion du Script de Linker (`linker.ld`)
*   **Fichier touché** : `kernel/build.rs`
*   **Cause** : Le script de compilation forçait inconditionnellement l'incorporation de `linker.ld` modifiant le format exécutable (ELF). Le point d'entrée `main` des tests disparaissait et l'OS hôte ne reconnaissait plus le fichier de test.
*   **Résolution** : Ajout d'une détection dynamique de la cible (cible native OS `x86_64-unknown-linux-gnu` exclue) pour restreindre l'usage de ce script aux cibles bare-metal.
    ```rust
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("none") || target.contains("exo") {
        println!("cargo:rustc-link-arg=-Tkernel/linker.ld");
    }
    ```

## 3. Problème 2 : Compilation des Tests (Désaccords d'API)
Avec le SIGSEGV réparé, les tests nécessitaient d'être strictement alignés aux signatures internes du noyau pour valider leur compilation.

### A. Méthodes virtuelles et Wrappers (`tier_1_simple.rs`)
*   **Erreurs** : Appels à des méthodes introuvables (`.hits()`, `.misses()`, `.len()`) et erreur de validation sur `BlobId` instancié tel quel.
*   **Correctifs** : 
    * L'initialisation de la taille se fait via la fonction `cache.n_entries()`.
    * L'objet statique utilise son constructeur `BlobId::from_raw([0; 32])`.

### B. Collision d'Énumérations `ObjectKind` (`tier_2_moyen.rs`, `tier_3_stress.rs`)
*   **Erreurs** : Le compilateur retournait `E0308 : expected ObjectKind, found ObjectKind`. L'environnement de test importait le mauvais type d'enum du module `core` au lieu du module `cache`.
*   **Correctifs** : 
    * Reciblage des directives d'imports (`use crate::fs::exofs::cache::object_cache::ObjectKind;`).
    * Encapsulation locale stricte des data tests via `.into_boxed_slice()` pour passer du vecteur statique au format pointeur alloué du conteneur.

## 4. Bilan de l'Exécution (Le Vert Final)
Grâce à la levée complète des erreurs de mémoire partagée et aux ajustements d'API, l'environnement natif réussit la passe avec succès. Le système d'éviction réagit et valide toutes les conditions du "Storage Stress".

**Résultat Console Verbatim** :
```text
running 5 tests
test fs::exofs::tests::integration::tier_1_simple::test_cache_init ... ok
test fs::exofs::tests::integration::tier_1_simple::test_cache_miss ... ok
test fs::exofs::tests::integration::tier_2_moyen::test_cache_overwrite ... ok
test fs::exofs::tests::integration::tier_2_moyen::test_cache_insertion ... ok
test fs::exofs::tests::integration::tier_3_stress::test_cache_stress_and_eviction ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2805 filtered out; finished in 0.10s
```