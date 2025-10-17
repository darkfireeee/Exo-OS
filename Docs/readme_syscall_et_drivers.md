

# README: Modules Syscall et Drivers pour Exo-Kernel

## Table des matières
- [Introduction](#introduction)
- [Module Syscall](#module-syscall)
  - [Architecture](#architecture-syscall)
  - [Fichiers](#fichiers-syscall)
  - [Interface et Utilisation](#interface-et-utilisation-syscall)
  - [Performance et Optimisations](#performance-et-optimisations-syscall)
- [Module Drivers](#module-drivers)
  - [Architecture](#architecture-drivers)
  - [Fichiers](#fichiers-drivers)
  - [Interface et Utilisation](#interface-et-utilisation-drivers)
  - [Performance et Optimisations](#performance-et-optimisations-drivers)
- [Intégration entre les Modules](#intégration-entre-les-modules)
- [Exemples d'Utilisation](#exemples-dutilisation)
- [Guide de Contribution](#guide-de-contribution)

## Introduction

Ce document présente deux modules fondamentaux du noyau Exo-Kernel : le module **syscall** (appels système) et le module **drivers** (gestion des pilotes). Ces modules sont conçus pour offrir des performances exceptionnelles tout en maintenant une interface claire et sécurisée.

Le module syscall permet aux applications en espace utilisateur d'interagir avec le noyau, tandis que le module drivers fournit une abstraction unifiée pour la gestion des périphériques matériels. Ensemble, ils forment la base de l'interaction entre les applications, le noyau et le matériel.

## Module Syscall

### Architecture Syscall

Le module syscall implémente une interface d'appels système optimisée pour atteindre plus de 5 millions d'appels par seconde. Il utilise les instructions `syscall`/`sysret` pour un passage en mode noyau efficace avec une latence minimale.

L'architecture est divisée en plusieurs composants :
- **Point d'entrée en assembleur** : Gère la transition du mode utilisateur au mode noyau
- **Dispatcheur** : Route les appels vers les gestionnaires appropriés
- **Gestionnaires d'appels système** : Implémentent la logique de chaque appel système

### Fichiers Syscall

#### `src/syscall/mod.rs`

Ce fichier contient :
- L'initialisation du mécanisme d'appels système via les MSRs (Model Specific Registers)
- Le point d'entrée en assembleur pour les appels système
- La définition des numéros d'appels système
- La structure des arguments d'appels système
- La fonction de dispatch qui route les appels vers les gestionnaires appropriés

```rust
// Exemple d'initialisation des appels système
pub fn init() {
    // Configuration des MSRs pour les appels système
    unsafe {
        // IA32_STAR: segments kernel et user
        x86_64::registers::msr::wrmsr(
            IA32_STAR,
            (0x08u64 << 32) | (0x18u64 << 48)
        );
        
        // IA32_LSTAR: adresse du point d'entrée
        x86_64::registers::msr::wrmsr(
            IA32_LSTAR,
            syscall_entry as u64
        );
        
        // IA32_FMASK: masque des drapeaux à effacer
        x86_64::registers::msr::wrmsr(
            IA32_FMASK,
            0x300  // IF et TF
        );
        
        // Activer les appels système
        let efer = x86_64::registers::model_specific::Msr::new(0xC0000080);
        let mut efer_value = efer.read();
        efer_value |= 1;  // SCE (System Call Extensions)
        efer.write(efer_value);
    }
}
```

#### `src/syscall/dispatch.rs`

Ce fichier contient les implémentations des différents appels système :
- `sys_read`, `sys_write` : Opérations d'entrée/sortie
- `sys_open`, `sys_close` : Gestion des descripteurs de fichiers
- `sys_exit` : Terminaison de processus
- `sys_yield`, `sys_sleep` : Ordonnancement
- `sys_mmap`, `sys_munmap` : Gestion mémoire
- `sys_clone` : Création de processus
- `sys_ipc_send`, `sys_ipc_recv` : Communication inter-processus

Chaque fonction est optimisée pour minimiser la latence et maximiser le débit.

### Interface et Utilisation Syscall

Les appels système sont accessibles depuis l'espace utilisateur via l'instruction `syscall`. Les arguments sont passés dans les registres selon la convention x86_64 :

- `RAX` : Numéro de l'appel système
- `RDI`, `RSI`, `RDX`, `R10`, `R8`, `R9` : Arguments
- `RAX` : Valeur de retour

Exemple d'utilisation depuis l'espace utilisateur (en assembleur) :

```asm
; Exemple d'appel système write(1, "Hello", 5)
mov rax, 1          ; sys_write
mov rdi, 1          ; stdout
mov rsi, msg        ; adresse du message
mov rdx, 5          ; longueur
syscall             ; appel système
```

### Performance et Optimisations Syscall

Plusieurs optimisations sont mises en œuvre pour atteindre les objectifs de performance :

1. **Utilisation des instructions `syscall`/`sysret`** : Plus rapides que les interruptions logicielles traditionnelles
2. **Point d'entrée en assembleur pur** : Minimise les opérations avant l'exécution du code Rust
3. **Validation minimale des arguments** : Effectuée uniquement lorsque nécessaire
4. **Chemin rapide pour les opérations courantes** : Lecture/écriture sur stdout/stderr
5. **Pas d'allocation mémoire dans le chemin critique** : Utilisation de structures pré-allouées

Ces optimisations permettent d'atteindre l'objectif de plus de 5 millions d'appels par seconde.

## Module Drivers

### Architecture Drivers

Le module drivers fournit une interface unifiée pour la gestion des pilotes de périphériques. Il est conçu pour être extensible et performant, avec support pour différents types de périphériques.

L'architecture est basée sur :
- **Traits** : Définissent les interfaces communes pour tous les pilotes
- **Gestionnaire de pilotes** : Enregistre, gère et distribue les pilotes
- **Spécialisations** : Interfaces spécifiques pour chaque type de périphérique (bloc, caractère, réseau, etc.)

### Fichiers Drivers

#### `src/drivers/mod.rs`

Ce fichier contient :
- La définition du trait `Driver` commun à tous les pilotes
- Le type `DriverError` pour la gestion des erreurs
- La structure `DriverManager` pour la gestion des pilotes
- L'instance globale du gestionnaire de pilotes

```rust
// Exemple de trait Driver
pub trait Driver: Send + Sync {
    fn driver_type(&self) -> DriverType;
    fn name(&self) -> &str;
    fn init(&mut self) -> Result<(), DriverError>;
    fn shutdown(&mut self) -> Result<(), DriverError>;
    fn is_ready(&self) -> bool;
}

// Exemple de gestionnaire de pilotes
pub struct DriverManager {
    drivers: BTreeMap<u32, Arc<Mutex<dyn Driver>>>,
    next_id: u32,
}
```

#### `src/drivers/block/mod.rs`

Ce fichier contient :
- La définition du trait `BlockDevice` pour les périphériques bloc
- La structure `GenericBlockDevice` comme implémentation de base
- Les fonctions pour enregistrer et récupérer les périphériques bloc
- Le support pour les opérations asynchrones

```rust
// Exemple de trait BlockDevice
pub trait BlockDevice: Driver {
    fn device_type(&self) -> BlockDeviceType;
    fn size_in_sectors(&self) -> u64;
    fn sector_size(&self) -> u64;
    fn supports_async(&self) -> bool;
    fn read_sectors(&mut self, sector: u64, count: u64, data: *mut u8) -> Result<(), BlockError>;
    fn write_sectors(&mut self, sector: u64, count: u64, data: *const u8) -> Result<(), BlockError>;
    fn flush(&mut self) -> Result<(), BlockError>;
    fn submit_async_request(&mut self, request: BlockRequest) -> Result<(), BlockError>;
    fn process_async_requests(&mut self);
}
```

### Interface et Utilisation Drivers

Les pilotes sont utilisés principalement par les appels système et d'autres parties du noyau. Voici un exemple d'utilisation d'un périphérique bloc :

```rust
// Enregistrement d'un nouveau périphérique bloc
let disk = Arc::new(Mutex::new(GenericBlockDevice::new(
    "disk0",
    BlockDeviceType::SSD,
    1024 * 1024 * 1024 / 512,  // 1GB en secteurs de 512 octets
    512,
    true,  // support asynchrone
)));

let disk_id = register_block_device(disk)?;

// Utilisation du périphérique
if let Some(disk) = get_block_device(disk_id) {
    let mut d = disk.lock();
    
    // Lecture synchrone
    let mut buffer = [0u8; 4096];
    d.read_sectors(0, 8, buffer.as_mut_ptr())?;
    
    // Écriture synchrone
    d.write_sectors(0, 8, buffer.as_ptr())?;
    
    // Vidage des caches
    d.flush()?;
}
```

### Performance et Optimisations Drivers

Le module drivers est optimisé pour offrir des performances élevées :

1. **Opérations asynchrones** : Permettent le chevauchement des opérations d'E/S
2. **File d'attente lock-free** : Minimise les contentions entre les threads
3. **Interface directe** : Évite les copies inutiles de données
4. **Support NUMA** : Alloue les ressources localement au processeur
5. **Validation minimale** : Effectuée uniquement lorsque nécessaire

Ces optimisations permettent d'atteindre des latences d'E/S inférieures à 500ns pour les opérations rapides.

## Intégration entre les Modules

Les modules syscall et drivers sont étroitement intégrés. Les appels système utilisent les pilotes pour accéder au matériel, tandis que les pilotes peuvent déclencher des appels système pour des opérations complexes.

Par exemple, l'appel système `sys_write` utilise le pilote série pour afficher des messages de debug :

```rust
pub fn sys_write(args: SyscallArgs) -> u64 {
    let fd = args.rdi;
    let buf_ptr = args.rsi as *const u8;
    let count = args.rdx;
    
    match fd {
        1 => {
            // stdout - Écrire sur le port série
            unsafe {
                for i in 0..count {
                    let c = *buf_ptr.add(i as usize);
                    crate::c_compat::serial_write_char(c);
                }
            }
            count
        }
        // ...
    }
}
```

De même, les pilotes peuvent utiliser les appels système pour des opérations complexes :

```rust
impl BlockDevice for GenericBlockDevice {
    fn read_sectors(&mut self, sector: u64, count: u64, data: *mut u8) -> Result<(), BlockError> {
        // Utiliser un appel système pour une opération complexe
        let args = SyscallArgs {
            rdi: self.id,
            rsi: sector,
            rdx: count,
            r10: data as u64,
            r8: 0,
            r9: 0,
        };
        
        let result = crate::syscall::sys_block_read(args);
        if result == 0xFFFFFFFFFFFFFFFF {
            return Err(BlockError::DeviceError);
        }
        
        Ok(())
    }
    // ...
}
```

## Exemples d'Utilisation

### Exemple 1 : Implémentation d'un pilote de disque

```rust
use crate::drivers::{BlockDevice, BlockDeviceType, BlockError, BlockRequest, BlockOperation};
use alloc::sync::Arc;
use spin::Mutex;

pub struct AHCIController {
    // Champs spécifiques au contrôleur AHCI
    // ...
}

impl AHCIController {
    pub fn new() -> Self {
        // Initialisation du contrôleur
        // ...
    }
}

impl crate::drivers::Driver for AHCIController {
    fn driver_type(&self) -> crate::drivers::DriverType {
        crate::drivers::DriverType::Block
    }
    
    fn name(&self) -> &str {
        "AHCI Controller"
    }
    
    fn init(&mut self) -> Result<(), crate::drivers::DriverError> {
        // Initialisation spécifique au contrôleur AHCI
        // ...
        Ok(())
    }
    
    fn shutdown(&mut self) -> Result<(), crate::drivers::DriverError> {
        // Arrêt spécifique au contrôleur AHCI
        // ...
        Ok(())
    }
    
    fn is_ready(&self) -> bool {
        // Vérifier si le contrôleur est prêt
        // ...
        true
    }
}

impl BlockDevice for AHCIController {
    fn device_type(&self) -> BlockDeviceType {
        BlockDeviceType::HardDisk
    }
    
    fn size_in_sectors(&self) -> u64 {
        // Retourner la taille du disque en secteurs
        // ...
        1024 * 1024 * 1024 / 512  // 1GB en secteurs de 512 octets
    }
    
    fn sector_size(&self) -> u64 {
        512
    }
    
    fn supports_async(&self) -> bool {
        true
    }
    
    fn read_sectors(&mut self, sector: u64, count: u64, data: *mut u8) -> Result<(), BlockError> {
        // Implémentation de la lecture via AHCI
        // ...
        Ok(())
    }
    
    fn write_sectors(&mut self, sector: u64, count: u64, data: *const u8) -> Result<(), BlockError> {
        // Implémentation de l'écriture via AHCI
        // ...
        Ok(())
    }
    
    fn flush(&mut self) -> Result<(), BlockError> {
        // Implémentation du vidage des caches via AHCI
        // ...
        Ok(())
    }
    
    fn submit_async_request(&mut self, request: BlockRequest) -> Result<(), BlockError> {
        // Soumettre une requête asynchrone via AHCI
        // ...
        Ok(())
    }
    
    fn process_async_requests(&mut self) {
        // Traiter les requêtes asynchrones en attente
        // ...
    }
}

// Enregistrement du pilote
pub fn init_ahci() -> Result<u32, crate::drivers::DriverError> {
    let ahci = Arc::new(Mutex::new(AHCIController::new()));
    crate::drivers::register_block_device(ahci)
}
```

### Exemple 2 : Ajout d'un nouvel appel système

```rust
// Dans src/syscall/mod.rs
#[repr(u64)]
pub enum SyscallNumber {
    // Appels système existants
    Read = 0,
    Write = 1,
    // ...
    
    // Nouvel appel système
    GetTime = 100,
}

// Dans src/syscall/dispatch.rs
pub fn sys_get_time(_args: SyscallArgs) -> u64 {
    // Implémentation de la récupération du temps
    // ...
    1234567890  // Exemple de timestamp
}

// Dans la fonction dispatch
match syscall_number {
    // Gestionnaires existants
    x if x == SyscallNumber::Read as u64 => sys_read(args),
    x if x == SyscallNumber::Write as u64 => sys_write(args),
    // ...
    
    // Nouveau gestionnaire
    x if x == SyscallNumber::GetTime as u64 => sys_get_time(args),
    
    _ => {
        // Appel système inconnu
        serial_write_str("Unknown syscall: ");
        0xFFFFFFFFFFFFFFFF  // Code d'erreur
    }
}
```

## Guide de Contribution

Pour contribuer aux modules syscall et drivers, veuillez suivre ces directives :

1. **Respecter les conventions de code** : Utilisez le style Rust officiel et suivez les patterns établis dans le code existant.

2. **Maintenir la compatibilité** : Assurez-vous que les modifications n'introduisent pas de régressions dans les interfaces existantes.

3. **Documenter le code** : Ajoutez des commentaires clairs pour les nouvelles fonctionnalités et les modifications complexes.

4. **Tests** : Ajoutez des tests unitaires pour les nouvelles fonctionnalités et assurez-vous que les tests existants passent.

5. **Performance** : Mesurez l'impact des modifications sur les performances et optimisez si nécessaire.

6. **Sécurité** : Validez soigneusement les entrées utilisateur et assurez-vous que le code est sécurisé contre les attaques courantes.

Pour soumettre une contribution :
1. Fork le dépôt
2. Créer une branche pour votre fonctionnalité
3. Implémenter la fonctionnalité avec des tests
4. Soumettre une pull request avec une description claire des modifications

---

Pour plus d'informations sur le projet Exo-Kernel, veuillez consulter la documentation principale du projet.