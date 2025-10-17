
# Exo-Kernel Core Library

[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://docs.rust-embedded.org/book/intro/no-std.html)

> **Briques fondamentales haute performance, sûres et modulaires pour les systèmes d'exploitation de nouvelle génération.**

Cette bibliothèque est le cœur de notre projet de noyau hybride, conçue avec une philosophie simple : atteindre une performance absolue sans sacrifier la sécurité et l'ergonomie que Rust offre. Elle fournit un ensemble de modules réutilisables et optimisés pour les défis uniques du développement de noyau dans un environnement `no_std`.

## 📖 Table des Matières

- [Philosophie](#-philosophie)
- [✨ Fonctionnalités Clés](#-fonctionnalités-clés)
- [🚀 Démarrage Rapide](#-démarrage-rapide)
- [📚 Documentation des Modules](#-documentation-des-modules)
  - [`sync`](#sync-primitifs-de-synchronisation)
  - [`memory`](#memory-abstractions-mémoire)
  - [`arch`](#arch-abstractions-spécifiques-à-larchitecture)
  - [`macros`](#macros-macros-utilitaires)
  - [`ffi`](#ffi-interopérabilité-avec-c)
- [🔬 Objectifs de Performance](#-objectifs-de-performance)
- [🤝 Contribuer](#-contribuer)
- [📄 Licence](#-licence)

## 🧠 Philosophie

Le développement d'un noyau moderne exige un équilibre délicat. D'un côté, le contrôle absolu et la vitesse brute du C et de l'assembleur sont indispensables pour interagir directement avec le matériel. De l'autre, la complexité croissante des systèmes d'exploitation nécessite des garanties de sécurité et une productivité que seul Rust peut fournir.

L'**Exo-Kernel Core Library** est notre réponse à ce défi. Elle est construite sur les principes suivants :

1.  **Performance d'abord** : Chaque fonction est écrite avec la latence et le débit comme objectifs primaires. Nous utilisons des instructions atomiques, des structures de données lock-free et des chemins d'exécution optimisés.
2.  **Sûreté par l'abstraction** : Nous n'évitons pas le code `unsafe` ; nous le *confinons*. Toutes les opérations dangereuses sont encapsulées derrière des API sûres et ergonomiques, vous protégeant des erreurs subtiles de bas niveau.
3.  **Modularité extrême** : Chaque module est indépendant et peut être utilisé séparément. Que vous construisiez un micro-noyau, un monolithe ou un exo-noyau, vous pouvez n'intégrer que les composants dont vous avez besoin.
4.  **Conçue pour `no_std`** : Cette bibliothèque ne dépend d'aucun système d'exploitation sous-jacent. Elle est elle-même la fondation.

## ✨ Fonctionnalités Clés

-   **✅ `no_std` Compatible** : Fonctionne dans un environnement bare-metal.
-   **⚡ Abstractions à Coût Nul** : Toutes les abstractions de haut niveau se compilent en code aussi efficace que du code écrit à la main.
-   **🔒 Primitives de Synchronisation Optimisées** : Mutex spinlock et `Once` pour l'initialisation unique, conçus pour le noyau.
-   **🧠 Gestion Mémoire Sécurisée** : Types forts pour les adresses (virtuelle/physique) et wrappers pour les tables de pages.
-   **🖥️ Abstractions Matérielles Propres** : Accès aux registres CPU et gestion des interruptions via une API Rust idiomatique.
-   **🛠️ Macros Utilitaires Puissantes** : `kprintln!` pour le debug sur port série et `lazy_static!` adapté pour `no_std`.
-   **🌉 Interopérabilité C Sans Douleur** : Wrappers sûrs pour les chaînes C (`CStr`) et les listes d'arguments variables (`VaList`).

## 🚀 Démarrage Rapide

### Prérequis

-   Rust Nightly
-   `cargo-binutils` pour les outils de bas niveau

```bash
rustup default nightly
rustup component add rust-src
cargo install cargo-binutils
rustup component add llvm-tools-preview
```

### Ajout à votre projet

Ajoutez ce qui suit à votre fichier `Cargo.toml` :

```toml
[dependencies]
exo-kernel-lib = { path = "chemin/vers/exo-kernel/src/lib" } # ou le nom de votre crate si publiée
```

### Exemple : "Hello, Kernel!"

Le "Hello, World!" du développement de noyau est souvent un simple message de debug sur le port série.

```rust
// Dans votre fichier principal (ex: src/main.rs)
#![no_std]
#![no_main]

use exo_kernel_lib::{kprintln, arch::x86_64::registers};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // La macro kprintln! utilise automatiquement le port série pour le debug.
    kprintln!("Exo-Kernel Core Library - Initialisation...");
    kprintln!("Le noyau fonctionne !");

    // Vous pouvez aussi utiliser les abstractions de bas niveau
    let cr3_value = registers::read_cr3();
    kprintln!("Adresse du répertoire de pages (CR3) : {:#x}", cr3_value);

    loop {
        // On ne quitte jamais le noyau
        registers::hlt();
    }
}
```

## 📚 Documentation des Modules

### `sync` - Primitifs de Synchronisation

Ce module fournit les outils essentiels pour gérer la concurrence au sein du noyau, où les threads standards et les mutex du système d'exploitation ne sont pas disponibles.

-   **`Mutex<T>`** : Un spinlock simple et efficace. Il utilise une attente active (`spin_loop_hint`) pour verrouiller une ressource, ce qui est idéal pour les sections critiques de très courte durée dans un noyau.
    ```rust
    use exo_kernel_lib::sync::Mutex;

    static MY_DATA: Mutex<u64> = Mutex::new(0);

    fn increment_counter() {
        let mut data = MY_DATA.lock(); // Verrouille le mutex
        *data += 1; // Accès sécurisé aux données
    } // Le mutex est automatiquement déverrouillé à la fin de la portée
    ```

-   **`Once<T>`** : Garantit qu'une initialisation coûteuse n'est exécutée qu'une seule fois, même si plusieurs threads tentent d'y accéder simultanément. Parfait pour initialiser des structures globales complexes.
    ```rust
    use exo_kernel_lib::sync::Once;

    static EXPENSIVE_RESOURCE: Once<MyComplexStruct> = Once::new();

    fn get_resource() -> &'static MyComplexStruct {
        EXPENSIVE_RESOURCE.call_once(|| {
            // Ce code ne s'exécutera qu'une seule fois
            MyComplexStruct::new()
        })
    }
    ```

### `memory` - Abstractions Mémoire

La gestion de la mémoire est la tâche la plus critique et la plus dangereuse dans un noyau. Ce module utilise le système de types de Rust pour éliminer une classe entière d'erreurs.

-   **`VirtualAddress` / `PhysicalAddress`** : Des wrappers autour de `usize` qui garantissent que vous ne mélangerez jamais accidentellement une adresse virtuelle avec une adresse physique. Ils fournissent des méthodes utiles pour l'alignement des pages.
    ```rust
    use exo_kernel_lib::memory::{VirtualAddress, PhysicalAddress};

    let vaddr = VirtualAddress::new(0xdeadbeef);
    let aligned_vaddr = vaddr.align_down_to_page(); // Arrondit à 4KiB

    let paddr = PhysicalAddress::new(0x1000);
    assert!(paddr.is_page_aligned());
    ```

-   **`Page<S>` et `PageTable`** : Des abstractions de haut niveau pour travailler avec la pagination. `Page` représente une plage d'adresses virtuelles, tandis que `PageTable` et `PageTableEntry` permettent de manipuler les structures de pagination du CPU de manière sécurisée.
    ```rust
    use exo_kernel_lib::memory::{Page, Size4KiB, PageTable, PageTableFlags};

    let page = Page::<Size4KiB>::containing_address(VirtualAddress::new(0x123456));
    kprintln!("La page contenant l'adresse commence à : {:#x}", page.start_address());

    let mut table = PageTable::new();
    let entry = table.entry_mut(0).unwrap();
    entry.set_flags(PageTableFlags::new().present().writable());
    ```

### `arch` - Abstractions Spécifiques à l'Architecture

Ce module isole le code spécifique à une architecture matérielle. Actuellement, il se concentre sur **x86_64**, mais est conçu pour être extensible à d'autres architectures comme ARM64.

-   **`registers`** : Fournit des fonctions sûres pour lire et écrire dans les registres de contrôle du CPU (CR0, CR3, CR4, etc.) et pour exécuter des instructions spéciales (`hlt`, `invlpg`).
    ```rust
    use exo_kernel_lib::arch::x86_64::registers;

    // Invalider une entrée spécifique dans le TLB
    registers::invlpg(0xdead0000);

    // Lire le compteur de cycles CPU pour du benchmarking
    let cycles = registers::read_tsc();
    ```

-   **`interrupts`** : Offre un cadre pour gérer les interruptions et les exceptions CPU. Il inclut des structures pour le contexte d'interruption et un gestionnaire pour enregistrer et distribuer les handlers.
    ```rust
    use exo_kernel_lib::arch::x86_64::{interrupts::{InterruptManager, InterruptContext}, registers};

    // Handler pour les défauts de page
    extern "C" fn page_fault_handler(context: &mut InterruptContext) {
        kprintln!("Défaut de page à l'adresse : {:#x}", registers::read_cr2());
        // ... gérer le défaut ...
    }

    // Enregistrer le handler (à faire lors de l'initialisation de l'IDT)
    let mut int_manager = InterruptManager::new();
    int_manager.register_handler(14, page_fault_handler);
    ```

### `macros` - Macros Utilitaires

Les macros dans Rust permettent de générer du code au moment de la compilation, réduisant la répétition et ajoutant des fonctionnalités qui seraient impossibles autrement dans un contexte `no_std`.

-   **`kprintln!`** : Votre meilleur ami pour le debug. Elle fonctionne comme `println!` mais écrit sur le port série (COM1 par défaut), qui est disponible très tôt dans le processus de boot.
    ```rust
    use exo_kernel_lib::kprintln;

    let some_value = 42;
    kprintln!("La valeur est : {}", some_value);
    kprintln!("Structure complexe : {:?}", my_struct);
    ```

-   **`lazy_static!`** : Une version adaptée de la crate populaire. Elle permet de créer des variables statiques qui ne sont initialisées qu'à leur première utilisation, ce qui est essentiel pour les structures qui nécessitent une allocation dynamique ou une configuration complexe.
    ```rust
    use exo_kernel_lib::{lazy_static, sync::Mutex};

    lazy_static! {
        static ref GLOBAL_HEAP: Mutex<Heap> = {
            // L'initialisation du tas n'a lieu qu'ici, au premier accès.
            Mutex::new(Heap::new())
        };
    }

    fn allocate(size: usize) -> Option<usize> {
        GLOBAL_HEAP.lock().alloc(size)
    }
    ```

### `ffi` - Interopérabilité avec C

Même dans un noyau majoritairement Rust, il est souvent nécessaire d'interagir avec du code C existant (pilotes, bibliothèques) ou de suivre des conventions d'appel C (ex: syscalls).

-   **`CStr`** : Une représentation sûre des chaînes de caractères terminées par un byte nul, le format standard en C. Elle permet de convertir facilement entre les chaînes Rust et C.
    ```rust
    use exo_kernel_lib::ffi::{CStr, cstr};

    // Créer une CStr à partir d'un littéral
    let name = cstr!("Exo-Kernel");

    // Convertir une chaîne reçue du code C
    unsafe {
        let c_string = CStr::from_ptr(ptr_from_c);
        kprintln!("Message du pilote C : {}", c_string.to_string_lossy());
    }
    ```

-   **`VaList`** : Permet à Rust de gérer les fonctions C qui acceptent un nombre variable d'arguments (comme `printf`). C'est crucial pour implémenter des handlers de syscall ou des fonctions de formatage compatibles.
    ```rust
    use exo_kernel_lib::ffi::VaList;

    // Fonction qui imite le comportement de printf
    unsafe extern "C" fn my_printf(format: *const u8, mut args: VaList) {
        // Logique pour parser `format` et extraire les arguments de `args`
        let arg1 = args.usize();
        let arg2 = args.i32();
        // ...
    }
    ```

## 🔬 Objectifs de Performance

Cette bibliothèque est conçue pour atteindre des benchmarks de classe mondiale. Voici nos cibles "ultimes" mais réalistes pour les composants du noyau qui l'utiliseront.

| Composant | Objectif "Ultime" (Réaliste) | Comment l'aborder | Compromis Acceptable |
| :--- | :--- | :--- | :--- |
| **Latence IPC** | < 500 ns (nanosecondes) | Optimiser le chemin rapide, éviter les copies. | Légère augmentation de la complexité du code. |
| **Context Switch** | < 1 µs (microseconde) | Minimiser l'état à sauvegarder, utiliser des instructions spécifiques au CPU. | Support de moins d'architectures exotiques au début. |
| **Scheduler** | > 1M threads/scalable | Scheduler lock-free, conscience des NUMA nodes. | Latence légèrement plus élevée pour les threads peu prioritaires. |
| **Syscall** | > 5M appels/sec | Passage en mode utilisateur le plus rapide possible (ex: `sysenter`/`syscall`). | Interface d'appel système moins riche fonctionnellement au départ. |
| **Démarrage** | < 500 ms jusqu'au shell | Paralléliser le boot, initramfs minimal, pilotes asynchrones. | Moins de messages de debug au démarrage. |
| **Compatibilité** | Large (x86_64, ARM64) | Abstractions matérielles bien définies (HAL). | Performances non optimales sur les architectures moins communes. |

## 🤝 Contribuer

Nous sommes ouverts aux contributions ! Que ce soit pour corriger un bug, améliorer la documentation, ou proposer une nouvelle optimisation, votre aide est la bienvenue.

1.  Fork le projet.
2.  Créez une branche pour votre fonctionnalité (`git checkout -b feature/amazing-feature`).
3.  Commitez vos changements (`git commit -m 'Add some amazing feature'`).
4.  Pushez vers la branche (`git push origin feature/amazing-feature`).
5.  Ouvrez une Pull Request.

Veuillez vous assurer que votre code respecte le style existant et que les tests passent.

## 📄 Licence

Ce projet est sous double licence MIT ou Apache-2.0, à votre convenance.

---

