

Absolument ! Voici un README complet qui documente les modules `x86_64` et `c_compat`, expliquant leur rôle, leur fonctionnement et la manière dont ils interagissent pour former le socle de votre noyau.

---

# 📚 Documentation des Modules Principaux : Architecture et Compatibilité

Ce document fournit une vue d'approfondie des deux modules fondamentaux du noyau Exo-Kernel : `src/arch/x86_64/` et `src/c_compat/`. Ensemble, ils établissent l'environnement d'exécution de base, depuis la toute première instruction exécutée par le processeur jusqu'à l'interface permettant de réutiliser du code C existant.

## Table des Matières

1.  [Philosophie Générale : Le Meilleur des Deux Mondes](#philosophie-générale)
2.  [Module `src/arch/x86_64/` : La Couche d'Abstraction Matérielle](#module-x86_64)
    *   [La Chaîne de Démarrage (Boot Chain)](#la-chaîne-de-démarrage)
    *   [Détail des Fichiers](#détail-des-fichiers-x86_64)
3.  [Module `src/c_compat/` : Le Pont vers le Monde C (FFI)](#module-c_compat)
    *   [Le Concept FFI (Foreign Function Interface)](#le-concept-ffi)
    *   [Détail des Fichiers](#détail-des-fichiers-c_compat)
4.  [Comment Tout est Assemblé : Le Processus de Build](#comment-tout-est-assemblé)
5.  [Conclusion et Prochaines Étapes](#conclusion)

---

<a name="philosophie-générale"></a>
## 🧠 Philosophie Générale : Le Meilleur des Deux Mondes

Ce noyau est conçu avec une architecture hybride délibérée :

*   **Assembleur (`boot.asm`)** : Utilisé pour le travail le plus bas niveau qui ne peut être abstrait. Configuration de la pile, désactivation des interruptions, saut initial. C'est le code qui a le contrôle total et aucune abstraction.
*   **C (`boot.c`, `serial.c`, `pci.c`)** : Choisit pour sa simplicité dans les interactions matérielles directes et pour capitaliser sur l'écosystème immense de pilotes et de code de bas niveau existants. Il sert de pont pratique entre l'assembleur brut et la sécurité de Rust.
*   **Rust (`*.rs`)** : Le cœur de la logique du noyau. Sa gestion de la mémoire, son système de types et son approche de la concurrence sans `data races` en font le choix idéal pour construire un système d'exploitation complexe, fiable et performant.

Cette approche permet de maximiser la performance là où cela compte (assembleur), de faciliter le développement et l'intégration (C), tout en garantissant la sécurité et la robustesse de la logique métier du noyau (Rust).

---

<a name="module-x86_64"></a>
## 🏗️ Module `src/arch/x86_64/` : La Couche d'Abstraction Matérielle

Ce module est le fondement sur lequel repose tout le reste. Il est responsable de la configuration initiale du processeur x86_64 pour qu'il puisse exécuter du code Rust de manière sûre.

<a name="la-chaîne-de-démarrage"></a>
### La Chaîne de Démarrage (Boot Chain)

Le démarrage du noyau suit une séquence précise, conçue pour mettre en place l'environnement progressivement :

1.  **`boot.asm` (Point d'entrée)** : Le bootloader charge ce code à l'adresse `1M` et saute à l'étiquette `start`. Il effectue les tâches minimales :
    *   Désactive les interruptions (`cli`).
    *   Configure un pointeur de pile (`rsp`) pour que les appels de fonction fonctionnent.
    *   Appelle la fonction `kmain` écrite en C.

2.  **`boot.c` (Le Pont)** : Cette fonction C prend le relais. À ce stade, nous avons une pile, mais pas encore d'environnement Rust complet. Son rôle est de :
    *   Effectuer des initialisations matérielles complexes qui sont plus simples à écrire en C (par exemple, interroger l'UEFI pour la carte mémoire).
    *   Appeler `rust_main()`, le point d'entrée principal de notre noyau Rust.

3.  **`rust_main()` (Le Cœur Rust)** : Une fois dans Rust, nous pouvons utiliser tout le pouvoir du langage. La première étape est d'appeler `arch::init()`, qui orchestre la configuration de l'architecture.

<a name="détail-des-fichiers-x86_64"></a>
### Détail des Fichiers

#### `mod.rs`
Le chef d'orchestre du module. Il déclare les sous-modules (`gdt`, `idt`, `interrupts`) et expose une fonction `init()` publique qui est appelée par `rust_main()` pour initialiser tous les composants de l'architecture dans le bon ordre.

#### `boot.asm`
Le point d'entrée absolu. C'est du code assembleur pur qui s'exécute en mode 64 bits long. Sa simplicité est sa force : il ne fait que le strict nécessaire pour passer le contrôle à un langage de plus haut niveau.

#### `boot.c`
Le code C qui sert de pont. Il est compilé séparément et lié au noyau. Il déclare la fonction Rust `extern void rust_main(void)` et l'appelle.

#### `gdt.rs`
Met en place la **Global Descriptor Table**. Bien qu'en mode 64 bits la segmentation soit largement désactivée, la GDT est toujours requise pour des raisons de compatibilité et, surtout, pour définir une **TSS (Task State Segment)**. La TSS est cruciale car elle indique au processeur où trouver la pile à utiliser lors des changements de privilège (par exemple, lors d'une interruption).

#### `idt.rs`
Configure l'**Interrupt Descriptor Table**. C'est une structure essentielle qui mappe chaque numéro d'interruption ou d'exception à une fonction gestionnaire (handler). Nous utilisons `lazy_static!` pour construire cette table au moment de l'exécution, car nous ne pouvons pas allouer de mémoire statique complexe avant que l'allocateur ne soit prêt. Ce fichier définit des handlers pour les exceptions CPU (ex: page fault, divide by zero) et pour les interruptions matérielles (IRQs) comme le timer ou le clavier.

#### `interrupts.rs`
Fournit des fonctions de contrôle pour les interruptions. La fonction `init()` active les interruptions matérielles avec l'instruction `sti`, permettant au processeur de répondre aux événements externes. Il offre aussi des utilitaires comme `without_interrupts` pour exécuter du code de manière atomique.

---

<a name="module-c_compat"></a>
## 🌉 Module `src/c_compat/` : Le Pont vers le Monde C (FFI)

Ce module encapsule tout le code C et l'interface nécessaire pour l'appeler depuis Rust. Son objectif est d'isoler le code `unsafe` inhérent à l'inter-opérabilité et de fournir une API Rust sûre et ergonomique.

<a name="le-concept-ffi"></a>
### Le Concept FFI (Foreign Function Interface)

Une FFI est un mécanisme qui permet à un programme écrit dans un langage (Rust) d'appeler des fonctions écrites dans un autre langage (C). En Rust, cela se fait via le bloc `extern "C"`. L'appel de code étranger est intrinsèquement `unsafe` car le compilateur Rust ne peut pas vérifier la validité de la mémoire ou des conventions d'appel de l'autre langage. Le rôle de ce module est de créer des **wrappers** sûrs autour de ces appels `unsafe`.

<a name="détail-des-fichiers-c_compat"></a>
### Détail des Fichiers

#### `mod.rs`
La façade Rust du module. Il utilise l'attribut `#[link(name = "c_compat", kind = "static")]` pour indiquer au linker de lier la bibliothèque statique `libc_compat.a` (générée par `build.rs`). Il déclare les signatures des fonctions C dans un bloc `extern "C"`. Enfin, il définit des fonctions Rust sûres (comme `serial_write_str`) qui appellent les fonctions C `unsafe` en interne, validant les arguments et gérant les types pour le reste du noyau.

#### `serial.c`
Un pilote pour le port série (COM1). C'est l'outil de débogage le plus fondamental. Il communique directement avec le matériel en utilisant des instructions d'entrée/sortie (`inb`, `outb`) pour écrire des caractères. Ce code est simple et efficace, et il permet d'afficher des messages très tôt dans le processus de boot, même sans écran.

#### `pci.c`
Un pilote basique pour le bus **PCI** (Peripheral Component Interconnect). Ce bus est utilisé pour connecter la plupart des périphériques haut débit (cartes réseau, graphiques, etc.). Ce code démontre une interaction matérielle plus complexe : il accède à l'espace de configuration PCI en écrivant une adresse sur un port (`0xCF8`) et en lisant les données depuis un autre port (`0xCFC`). Il parcourt tous les bus et appareils pour lister ce qui est disponible sur la machine.

---

<a name="comment-tout-est-assemblé"></a>
## 🔧 Comment Tout est Assemblé : Le Processus de Build

La magie qui transforme ces fichiers disparates en un unique exécutable de noyau se passe dans le processus de build, orchestré par Cargo et `build.rs`.

1.  **`build.rs` s'exécute en premier** : Avant même que Rust ne compile votre code, Cargo exécute `build.rs`. Ce script utilise la crate `cc` pour invoquer un compilateur C (comme `gcc` ou `clang`).
    *   Il compile `src/arch/x86_64/boot.c`, `src/c_compat/serial.c` et `src/c_compat/pci.c`.
    *   Il les archive dans une bibliothèque statique nommée `libc_compat.a` dans le répertoire de build (`OUT_DIR`).

2.  **Cargo compile le code Rust** :
    *   Le compilateur Rust (`rustc`) compile tous vos fichiers `.rs`.
    *   Lorsqu'il arrive dans `src/c_compat/mod.rs`, il voit les déclarations `extern "C"`. Il ne vérifie pas leur existence, mais il note que ces symboles doivent être fournis par une bibliothèque liée.
    *   Il compile aussi `src/arch/x86_64/boot.asm` (souvent via un plugin comme `rustc` avec un support intégré pour l'assembleur).

3.  **L'Édition de Liens (Linking)** :
    *   Enfin, le linker (souvent `lld` ou `ld`) est appelé.
    *   Il prend tous les fichiers objets Rust, le fichier objet de `boot.asm`, et la bibliothèque `libc_compat.a`.
    *   Il résout les symboles : l'appel à `kmain` dans `boot.asm` est lié à la fonction `kmain` dans `boot.c`. L'appel à `serial_init` dans `mod.rs` est lié à la fonction `serial_init` dans `libc_compat.a`.
    *   Il utilise le script `linker.ld` pour positionner chaque section de code et de données à la bonne adresse en mémoire, en commençant à `1M`.

Le résultat est un fichier binaire unique, `exo-kernel`, prêt à être chargé et exécuté par un bootloader.

---

<a name="conclusion"></a>
## 🚀 Conclusion et Prochaines Étapes

Les modules `x86_64` et `c_compat` forment un socle robuste et performant pour le noyau Exo-Kernel. Ils démontrent une architecture pragmatique qui n'a pas peur d'utiliser le bon outil pour chaque tâche.

**Les prochaines étapes logiques du développement seraient :**


*   **Étoffer les `drivers/`** : Utiliser le module `c_compat` pour intégrer des pilotes plus complexes (ex: stockage, réseau) et créer des abstractions Rust par-dessus.

Cette base solide permet de construire un système d'exploitation moderne, sûr et extrêmement performant.