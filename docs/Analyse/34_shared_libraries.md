# Rapport d'Analyse : Shared Libraries (libs/exo_types et libs/exo-phoenix-ssr)

## 1. Vue d'Ensemble

Les crates situés sous `libs/` constituent les fondations partagées entre le Ring 0 (noyau bare-metal `exo-os-kernel`), le Ring 1 (serveurs système micro-noyau), et le Ring 3 (applications et bibliothèques utilisateurs).

### Modules Analysés :
1. `libs/exo_types` : Types universels stricts, adresses mémoires typées (`PhysAddr`, `VirtAddr`, `IoVirtAddr`), identifiants système (`Pid`, `Tid`, `Bdf`, `EndpointId`, `ObjectId`), structures sans allocation pour chemins et listes fixes (`FixedString`, `FixedVec`), capabilities et tokens cryptographiques (`CapToken`, `Rights`), messages IPC typés (`IpcMessage`), et ABI POSIX/Linux (`EpollEventAbi`, `PollFd`, `IoVec`).
2. `libs/exo-phoenix-ssr` : Contrat et layout mémoire de la *Shared Status Region* (SSR) d'ExoPhoenix (Kernel B), région physique immuablement positionnée à `0x0100_0000` (64 KiB) partagée entre Kernel A et Kernel B pour orchestrer le basculement atomique de noyau sans perte de session.

---

## 2. Analyse Détaillée des Composants

### 2.1 `libs/exo_types`

- **Adresses Physiques et Virtuelles (`src/addr.rs`) :**
  - Encapsule les types `PhysAddr(pub u64)`, `VirtAddr(pub u64)` et `IoVirtAddr(pub u64)`.
  - Invariants : Empêche toute confusion à la compilation entre adresses physiques et virtuelles.
  - Alignement canonique x86-64 : `VirtAddr::is_canonical()` vérifie que les bits 47..63 sont la réplication du bit 47 (sign-extension 48-bit).
  - Fournit les méthodes d'alignement `align_down(align)` et `align_up(align)` avec vérification stricte de puissance de deux.

- **Capabilities & Tokens (`src/cap.rs`) :**
  - `CapToken` : Structure de 64 octets comprenant le type de capability, les droits (`Rights` représentés par un bitmask sur 64 bits), le sujet (`Pid`), la cible (`ObjectId`), un numéro de séquence (monotone anti-replay) et une signature BLAKE3 de 32 octets.
  - Fonction pure `verify_cap_token` vérifiant en temps constant l'authenticité sans allocation dynamique.
  - Invariant SRV-02 : `exo_types` ne dépend directement d'aucune bibliothèque cryptographique lourde ; la signature est validée par comparaison de digests prédéterminés.

- **Structures de Données sans Allocation (`src/fixed_string.rs` et `src/fixed_vec.rs`) :**
  - `FixedString<N>` : Tableau d'octets de taille fixe sur la pile ou dans les structures statiques, sans indirection mémoire, garantissant une absence totale de fragmentation du heap.
  - `FixedVec<T, N>` : Vecteur no_std avec capacité statique `N`, stockant les éléments contigus avec un compteur `len: usize`. Sécurisé contre les débordements avec des vérifications de bornes explicites `push()` retournant `Result<(), ()>`.
  - Typages spécialisés : `PathBuf` (chemin de 256 octets) et `ServiceName` (identifiant de service de 64 octets).

- **Messages IPC Typés (`src/ipc_msg.rs`) :**
  - Structure `IpcMessage` d'exactement 64 octets :
    - `sender: EndpointId`
    - `receiver: EndpointId`
    - `msg_type: u32`
    - `flags: u32`
    - `payload: [u8; 40]`
    - `token: u64`
  - Invariant IPC-02 : Tous les messages de contrôle du micro-noyau sont de taille fixe, sérialisables par valeur et transférables dans les registres ou via des anneaux SPSC lock-free sans copie de données lourdes.

- **Identifiants Stricts (`src/ids.rs`) :**
  - Nouveaux types forts (`Pid`, `Tid`, `Bdf`, `EndpointId`) interdisant toute conversion implicite vers des entiers primitifs, éliminant les bogues de typage croisé.

---

### 2.2 `libs/exo-phoenix-ssr`

- **Contrat PhoenixSafe :**
  - Trait universel `PhoenixSafe` :
    ```rust
    pub trait PhoenixSafe {
        const RECOVERY: PhoenixRecovery;
        fn on_pre_switch(&self) -> Result<(), PhoenixError>;
        fn on_post_switch(&self) -> Result<(), PhoenixError>;
    }
    ```
  - Permet à tout sous-système (IPC, VFS, drivers matériels) de sérialiser ou d'invalider son état avant que Kernel B ne prenne le contrôle de la machine.

- **Layout Physique de la SSR (CORR-02) :**
  - Située à l'adresse fixe `SSR_BASE_PHYS = 0x0100_0000` (16 MiB physique, hors de portée de la stack de boot BSP et du code noyau à 1 MiB).
  - Taille : `SSR_SIZE = 0x1_0000` (64 KiB).
  - `SSR_MAX_CORES_LAYOUT = 256` : Capacité dimensionnée pour 256 cœurs CPU logiques.
  - Partitionnement strict vérifié à la compilation par des assertions statiques (`assert!` en const context) :
    - `[0x0000]` : Magic `0x4558_5048` (`EXPH`) et version majeure v7.
    - `[0x0008]` : Handoff flag atomique Kernel B $\rightarrow$ Kernel A.
    - `[0x0010]` : Nonce de liveness (détection de blocage ou gel du BSP).
    - `[0x0018]` : Seqlock inter-noyaux.
    - `[0x0040]` : File de commandes B $\rightarrow$ A (64 octets).
    - `[0x0080]` : `freeze_ack` par cœur (u32 $\times$ 256 = 1 KiB).
    - `[0x4000]` : Snapshots des compteurs de performance (PMC) par cœur (64 B $\times$ 256 = 16 KiB).
    - `[0x8000]` : Journal d'audit (16 KiB).
    - `[0xC000]` : Métriques système globales (16 KiB).

- **Sûreté Concurrente :**
  - Synchronisation sans verrou (`lock-free`) à l'aide d'opérations atomiques (`Acquire`/`Release`) et de lectures seqlock garantissant l'absence de lecture corrompue en cas de bascule d'urgence.

---

## 3. Synthèse de Conformité

- **Dépendances :** Aucun cycle de dépendance, conformité totale `no_std`.
- **Mémoire :** Zéro allocation dynamique dans le chemin critique.
- **Invariants :** Validation statique des alignements, des offsets et des tailles par assertions à la compilation.
