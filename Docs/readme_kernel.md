
Cette structure est inspirée de projets industriels et de recherche de pointe, où chaque langage est utilisé là où il excelle.

### 📁 Arborescence Complète du Projet

```
exo-kernel/
├── .cargo/
│   └── config.toml                  # Configuration Cargo pour le noyau
├── build.rs                         # Script de build pour compiler le code C
├── Cargo.toml                       # Fichier de configuration du projet
├── x86_64-unknown-none.json         # Cible de compilation personnalisée
└── linker.ld                        # Script de l'éditeur de liens
src/
├── main.rs                          # Binaire pour les tests unitaires
├── lib.rs                           # Point d'entrée de la bibliothèque du noyau
├── lib/
│   ├── mod.rs                       # Déclaration des sous-modules
│   ├── collections/                 # Structures de données sans allocation
│   │   ├── mod.rs
│   │   ├── vec.rs                   # Vecteur simple (no_std)
│   │   └── string.rs                # Gestion des chaînes de caractères
│   ├── sync/                        # Primitifs de synchronisation
│   │   ├── mod.rs
│   │   ├── mutex.rs                 # Mutex adapté au noyau
│   │   └── once.rs                  # Initialisation unique
│   ├── memory/                      # Abstractions mémoire bas niveau
│   │   ├── mod.rs
│   │   ├── paging.rs                # Wrappers autour des tables de pages
│   │   └── address.rs               # Types pour les adresses (Virtuelle/Physique)
│   ├── arch/                        # Abstractions spécifiques à l'architecture
│   │   ├── mod.rs
│   │   └── x86_64/
│   │       ├── mod.rs
│   │       ├── registers.rs         # Accès aux registres CPU
│   │       └── interrupts.rs        # Gestion des interruptions
│   ├── macros/                      # Macros utilitaires
│   │   ├── mod.rs
│   │   ├── println.rs               # Macro `kprintln!` pour le debug
│   │   └── lazy_static.rs           # Version `no_std` de lazy_static
│   └── ffi/                         # Wrappers sûrs pour les appels C
│       ├── mod.rs
│       ├── c_str.rs                 # Gestion des chaînes C
│       └── va_list.rs               # Support des listes d'arguments variables
├── arch/
│   ├── mod.rs                       # Abstraction d'architecture
│   └── x86_64/
│       ├── mod.rs
│       ├── boot.asm                 # Entrée assembleur
│       ├── boot.c                   # Logique de boot en C
│       ├── gdt.rs                   # Global Descriptor Table
│       ├── idt.rs                   # Interrupt Descriptor Table
│       └── interrupts.rs            # Gestionnaires d'interruptions
├── c_compat/                        # Interopérabilité C (FFI)
│   ├── mod.rs
│   ├── serial.c                     # Pilote série simple
│   └── pci.c                        # Pilote PCI basique
├── memory/
│   ├── mod.rs                       # Module mémoire
│   ├── frame_allocator.rs           # Allocateur de frames physiques
│   ├── page_table.rs                # Tables de pages (mémoire virtuelle)
│   └── heap_allocator.rs            # Allocateur de tas (buddy system)
├── scheduler/
│   ├── mod.rs                       # Module de l'ordonnanceur
│   ├── thread.rs                    # Structure de thread (TCB)
│   ├── scheduler.rs                 # Logique work-stealing, NUMA-aware
│   └── context_switch.S             # Changement de contexte (ASM)
├── sync/
│   └── mod.rs                       # Primitives de synchronisation
├── ipc/
│   ├── mod.rs                       # Module IPC
│   ├── message.rs                   # Messages rapides via registres
│   └── channel.rs                   # Canaux lock-free (MPSC queues)
├── syscall/
│   ├── mod.rs                       # Interface des appels système
│   └── dispatch.rs                  # Distribution des appels
└── drivers/
    ├── mod.rs                       # Abstraction des pilotes
    └── block/
        └── mod.rs                   # Interface périphériques bloc


---

### 🔧 Fichiers de Configuration et de Build

#### `Cargo.toml`
Configure les dépendances et les options de compilation pour la performance.

```toml
[package]
name = "exo-kernel"
version = "0.1.0"
edition = "2021"

[dependencies]
# Pas de std, on utilise le core de Rust
bootloader = { version = "0.9", features = ["map_physical_memory"] }
x86_64 = "0.14.10"
volatile = "0.4.6"
spin = "0.9.8"
lazy_static = { version = "1.4.0", features = ["spin_no_std"] }
crossbeam-queue = "0.3.11" # Pour les structures lock-free (MPSC queue)

[profile.dev]
panic = "abort"

[profile.release]
# Options pour la performance ultime
panic = "abort"               # Pas de déroulement de pile
lto = true                   # Link-Time Optimization (optimise tout le programme)
codegen-units = 1            # Meilleures optimisations, compilation plus lente
overflow-checks = false      # À utiliser avec prudence en production
strip = true                 # Supprime les symboles de debug de l'exécutable

[build-dependencies]
cc = "1.0"                   # Pour compiler le code C
```

#### `build.rs`
Le script qui orchestre la compilation du code C et son intégration avec Rust.

```rust
// build.rs
fn main() {
    // Ne compiler le code C que pour la cible du noyau
    if !std::env::var("TARGET").unwrap().contains("unknown-none") {
        return;
    }

    // Utilise la crate `cc` pour compiler les fichiers C
    cc::Build::new()
        .file("src/arch/x86_64/boot.c") // Compile le code de boot C
        .file("src/c_compat/serial.c")   // Compile le pilote série
        .file("src/c_compat/pci.c")      // Compile le pilote PCI
        .compile("c_compat");             // Crée une bibliothèque statique `libc_compat.a`

    // Indique à Cargo de lier cette bibliothèque statique
    println!("cargo:rustc-link-lib=static=c_compat");
    // Indique à Cargo où trouver cette bibliothèque
    println!("cargo:rustc-link-search=native={}", std::env::var("OUT_DIR").unwrap());
}
```

#### `.cargo/config.toml`
Configure la chaîne de compilation pour une cible "bare metal".

```toml
[build]
target = "x86_64-unknown-none.json"

[unstable]
# Construire `core` et `alloc` nous-mêmes pour un contrôle total
build-std-features = ["compiler-builtins-mem"]
build-std = ["core", "compiler_builtins", "alloc"]

[target.x86_64-unknown-none]
# Définir un runner pour lancer facilement avec QEMU
runner = "bootimage runner"
```

#### `linker.ld`
Script de l'éditeur de liens pour positionner le noyau en mémoire.

```ld
ENTRY(start) /* Le point d'entrée est dans boot.asm */

SECTIONS
{
    . = 1M; /* Le noyau est chargé à l'adresse 1MiB */

    .text :
    {
        *(.text .text.*)
    }

    .rodata :
    {
        *(.rodata .rodata.*)
    }

    .data :
    {
        *(.data .data.*)
    }

    .bss :
    {
        *(.bss .bss.*)
        *(COMMON)
    }

    _end = .; /* Marqueur de fin de la section .bss */

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.comment)
    }
}
```

---

### 🧠 Modules du Noyau avec README

#### `src/arch/x86_64/`

**README.md**
```
# Architecture x86_64

## Objectif
Fournir une couche d'abstraction pour l'architecture x86_64. Ce module est responsable de la configuration la plus basse du processeur et du matériel juste après le boot.

## Architecture pour Performance Maximale
- **Boot en deux étapes** : `boot.asm` fait le minimum (pile, interruptions) et appelle `boot.c`. Le code C effectue les initialisations matérielles complexes (ex: détection de la mémoire via les structures de l'UEFI) avant de passer la main à Rust, qui est plus sûr mais nécessite un environnement déjà initialisé.
- **Interruptions optimisées** : Les gestionnaires d'interruptions sont écrits en Rust pour la sécurité, mais le prologue/épilogue est en assembleur pour un changement de contexte le plus rapide possible.

## Fichiers Clés
- `boot.asm`: Point d'entrée assembleur. Configure la pile et appelle `kmain`.
- `boot.c`: Pont C vers Rust. Initialise les matériels de base.
- `gdt.rs`: Met en place la Global Descriptor Table.
- `idt.rs`: Met en place l'Interrupt Descriptor Table.
- `interrupts.rs`: Définit les handlers pour les exceptions matérielles.

## Comment Construire
Ce module est compilé avec le reste du noyau. Le code C est compilé par `build.rs` et le code assembleur par le compilateur Rust.
```

**`src/arch/x86_64/boot.asm`**
```asm
.section .text
.global start

start:
    # Désactiver les interruptions pour un boot propre
    cli

    # Charger le pointeur de pile (défini dans linker.ld)
    mov rsp, stack_top

    # Appeler la fonction C `kmain`
    extern kmain
    call kmain

.hang:
    # Si kmain retourne, on attend
    hlt
    jmp .hang

.section .bss
.align 16
stack_bottom:
    .skip 8192 # 8 KiB pour la pile
stack_top:
```

**`src/arch/x86_64/boot.c`**
```c
#include <stddef.h>

// Déclaration de la fonction Rust principale
extern void rust_main(void);

// Fonction principale du noyau, appelée depuis boot.asm
void kmain(void) {
    // Initialisations bas niveau qui peuvent être plus simples en C
    // Par exemple, une configuration très précoire du matériel.
    
    // Appeler le point d'entrée principal du noyau écrit en Rust
    rust_main();
    
    // Ne devrait jamais arriver
    while (1) {
        asm volatile ("hlt");
    }
}
```

---

#### `src/scheduler/`

**README.md**
```
# Scheduler Multi-Agent

## Objectif
Fournir un ordonnanceur haute performance, scalable et capable de gérer plus de 10 000 threads (agents) avec une surcharge minimale (< 5µs pour 10K threads).

## Architecture pour Performance Maximale
- **Work-Stealing Lock-Free** : Chaque CPU possède sa propre file de threads. Les files sont des structures de données lock-free (ex: `crossbeam-queue::SegQueue`). Un CPU inactif peut "voler" du travail depuis la file d'un autre CPU sans verrou, éliminant les goulots d'étranglement.
- **Conscience NUMA** : Le scheduler alloue les threads et leur mémoire préférentiellement sur le nœud NUMA local, réduisant la latence d'accès à la mémoire, ce qui est crucial sur les systèmes >32 cœurs.
- **Changement de Contexte Optimisé** : La routine de changement de contexte est écrite en assembleur pur (`context_switch.S`) pour minimiser le nombre d'instructions et garantir une latence < 2µs.

## Fichiers Clés
- `scheduler.rs`: Logique de l'ordonnanceur, gestion des files de travail.
- `thread.rs`: Structure de contrôle de thread (TCB) avec état, pile, priorité.
- `context_switch.S`: Routine assembleur pour le changement de contexte entre threads.

## Comment Construire
Le code Rust est compilé normalement. Le fichier assembleur `.S` est pré-assemblé par le compilateur Rust et lié avec le reste du noyau.
```

**`src/scheduler/context_switch.S`**
```asm
// src/scheduler/context_switch.S
.global switch_context

# RDI = pointeur vers l'ancienne structure Thread
# RSI = pointeur vers la nouvelle structure Thread
switch_context:
    # Sauvegarder tous les registres généraux et le pointeur de pile
    push %rax
    push %rbx
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %rbp
    push %r8
    push %r9
    push %r10
    push %r11
    push %r12
    push %r13
    push %r14
    push %r15

    # Sauvegarder le stack pointer dans l'ancienne structure de thread
    mov %rsp, (%rdi)

    # Charger le stack pointer du nouveau thread
    mov (%rsi), %rsp

    # Restaurer tous les registres généraux depuis la nouvelle pile
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rdi
    pop %rsi
    pop %rbp
    pop %rdx
    pop %rcx
    pop %rbx
    pop %rax

    # Retourner au nouveau thread, restaure aussi RIP
    ret
```

---

#### `src/c_compat/`

**README.md**
```
# C Compatibility Layer (FFI)

## Objectif
Fournir une interface propre et sûre pour intégrer du code C et des pilotes existants dans le noyau Rust. Ce module agit comme une passerelle (Foreign Function Interface - FFI).

## Architecture pour Performance Maximale
- **Wrappers Sûrs** : Les fonctions C `unsafe` sont enveloppées dans des fonctions Rust sûres qui valident les arguments et gèrent les types, offrant une API ergonomique sans sacrifier la performance.
- **Pilotes en C** : Les pilotes pour matériels complexes (ex: PCIe, WiFi) sont souvent disponibles en C. Les intégrer ici est beaucoup plus rapide que de les réécrire en Rust, tout en isolant leur `unsafe` dans ce module bien défini.

## Fichiers Clés
- `mod.rs`: Déclare les fonctions C externes et fournit les wrappers Rust sûrs.
- `serial.c`: Pilote pour port série, très utile pour le debug précoce.
- `pci.c`: Pilote PCI basique pour l'énumération des périphériques.

## Comment Construire
Les fichiers `.c` sont compilés automatiquement par le script `build.rs` en une bibliothèque statique `libc_compat.a`, qui est ensuite liée au noyau.
```

**`src/c_compat/mod.rs`**
```rust
// src/c_compat/mod.rs

// Lie la bibliothèque statique compilée par build.rs
#[link(name = "c_compat", kind = "static")]
extern "C" {
    // Déclare les fonctions C que nous voulons utiliser
    pub fn serial_init();
    pub fn serial_write_char(c: u8);
    pub fn pci_enumerate_buses();
}

// Fournit une API Rust sûre et agréable
pub fn serial_write_str(s: &str) {
    for byte in s.bytes() {
        unsafe {
            serial_write_char(byte);
        }
    }
}
```

---

### 🚀 Guide de Construction Complet

Pour construire ce projet et obtenir les meilleurs résultats, suivez ces étapes.

#### 1. Prérequis

```bash
# Installer Rust Nightly (requis pour certaines fonctionnalités)
rustup default nightly
rustup component add rust-src

# Installer les outils nécessaires
cargo install cargo-binutils
rustup component add llvm-tools-preview
```

#### 2. Cloner le Projet

```bash
# (Assumez que vous avez cloné le dépôt avec la structure ci-dessus)
cd exo-kernel
```

#### 3. Compiler le Noyau

La commande `build` exécutera `build.rs` pour compiler le code C, puis compilera le code Rust et liera le tout.

```bash
cargo build --release
```

#### 4. Lancer avec QEMU

Le plus simple est d'utiliser `bootimage`, qui automatise la création d'une image disque bootable.

```bash
# Installer bootimage
cargo install bootimage

# Lancer le noyau dans QEMU
cargo bootimage --run
```

**Sortie Attendue :**
```
Exo-OS Kernel v0.1.0 (from Rust)
Serial port initialized (from C)
PCI buses enumerated (from C)
Scheduler initialized.
Memory manager initialized.
...
```

Cette structure hybride vous donne le meilleur des deux mondes : la performance et le contrôle du C et de l'assembleur pour les interactions matérielles, et la sécurité et la productivité de Rust pour la logique complexe du noyau. C'est la voie royale pour un OS moderne visant l'excellence.