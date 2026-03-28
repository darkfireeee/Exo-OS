# ExoOS — Corrections Servers, Libs & Arborescence
**Couvre : CORR-05, CORR-17, CORR-21, CORR-25, CORR-26, CORR-28**  
**Sources IAs : Kimi (§6 CORR-01), ChatGPT5 (§1.1 sender_pid), MiniMax (ES-06/07), Claude**

---

## CORR-05 🔴 — `CapabilityType` enum `#[repr(C)]` avec variantes à données : illégal Rust

### Problème
Arborescence V4 §2 `libs/exo-types/src/cap.rs` mentionne :
```rust
pub enum CapabilityType {
    IpcBroker,
    Driver { pci_id: u16 },  // ← ILLÉGAL avec #[repr(C)] ou #[repr(u16)]
    ...
}
```

**En Rust, un enum `#[repr(C)]` ou `#[repr(u*)]` ne peut PAS avoir de variantes avec données.**  
Cette restriction est vérifiée à la compilation → **erreur E0517**.

**Source** : Kimi §6 (CORR-01), Z-AI implicite dans la spec CapToken

### Correction — `libs/exo-types/src/cap.rs`

```rust
// libs/exo-types/src/cap.rs — CORR-05

/// Type de capacité — discriminant pur (pas de données inline).
/// Conforme à #[repr(u16)] : pas de variantes avec données.
///
/// Pour les capacités avec paramètre (ex: Driver PCI),
/// le paramètre est stocké dans cap.object_id ou dans un champ dédié de CapToken.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityType {
    /// Capacité de connexion à ipc_broker.
    IpcBroker       = 1,

    /// Capacité d'allocation mémoire via memory_server.
    MemoryServer     = 2,

    /// Capacité de driver PCI.
    /// Le BDF du device est encodé dans cap.object_id[0..6].
    /// cap.object_id[0..2] = bus:device:function (PciBdf compact).
    DriverPci        = 3,

    /// Capacité d'administration système (SysDeviceAdmin).
    SysDeviceAdmin   = 4,

    /// Capacité d'accès ExoFS.
    ExoFsAccess      = 5,

    /// Capacité de communication crypto_server.
    CryptoServer     = 6,

    /// Capacité de surveillance ExoPhoenix (exo_shield).
    ExoPhoenix       = 7,
    // Ajouter de nouvelles capacités ici avec des discriminants croissants
}

/// Token de capacité ExoOS.
/// Taille : à définir selon les contraintes de performance.
/// Vérification : verify_cap_token() via crate subtle (LAC-01).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CapToken {
    /// Génération anti-replay — incrémentée à chaque émission de token.
    pub generation: u64,

    /// Identifiant de la ressource cible.
    /// Pour DriverPci : bytes[0..2] = PciBdf compact (bus:dev:fn en u16).
    pub object_id: ObjectId,

    /// Bitmask des droits accordés.
    pub rights: u32,

    /// Discriminant du type (CapabilityType as u16).
    pub type_id: u16,

    /// Padding alignement.
    pub _pad: [u8; 2],
}

/// Vérifie qu'un CapToken est du type attendu.
/// DOIT être la première instruction de main.rs de chaque server (CAP-01).
/// Implémentation constant-time requise via crate subtle (LAC-01).
pub fn verify_cap_token(token: &CapToken, expected: CapabilityType) -> bool {
    // TODO Phase 1 : LAC-01 — implémentation constant-time via subtle::ct_eq
    token.type_id == expected as u16
}

// Helper pour créer un CapToken DriverPci avec BDF encodé
pub fn make_driver_pci_cap(generation: u64, bdf: PciBdf, rights: u32) -> CapToken {
    let mut object_id = ObjectId([0u8; 32]);
    // Encoder BDF dans les 3 premiers bytes
    object_id.0[0] = bdf.bus;
    object_id.0[1] = bdf.device;
    object_id.0[2] = bdf.function;
    CapToken {
        generation,
        object_id,
        rights,
        type_id: CapabilityType::DriverPci as u16,
        _pad: [0u8; 2],
    }
}
```

---

## CORR-17 🟠 — `sender_pid` : réutilisation PID après crash

### Problème
Le modèle IPC actuel : `reply via ipc_send(msg.sender_pid, response)`.

**Scénario de corruption** :
1. Process A (PID 42) envoie une requête à vfs_server
2. Process A crashe et est terminé avant la réponse
3. Process C reçoit le PID 42 (réutilisation par le scheduler)
4. vfs_server répond à PID 42 → Process C reçoit la réponse destinée à Process A

**Source** : ChatGPT5 Hard Stress §1.1

### Analyse de criticité
Ce problème est **réel** mais limité en impact :
- La réponse est dans le contexte de la requête initiale (msg_type correspondant)
- Process C recevant un message inattendu doit le rejeter (vérification msg_type)
- Les capabilities vérifient déjà l'identité (CAP-01)

Pour Phase 8, une mitigation légère suffit sans refactoring majeur du système IPC.

### Correction — `libs/exo-types/src/ipc_msg.rs`

```rust
// libs/exo-types/src/ipc_msg.rs — CORR-17
// Ajout d'un reply_nonce pour invalider les réponses stales.

/// Message IPC — 64 bytes total (1 cache line).
///
/// CORR-17 : Ajout de reply_nonce pour prévenir les réponses stales
/// après réutilisation de PID.
///
/// Layout :
///   sender_pid:  u32  — [0]   renseigné par le kernel (inaltérable Ring 3)
///   msg_type:    u32  — [4]   type du message
///   reply_nonce: u32  — [8]   NOUVEAU : nonce unique par requête
///   _pad:        u32  — [12]  alignement
///   payload:     [u8;48] — [16..63]  données
///
/// NOTE : payload passe de 56 à 48 bytes pour faire place à reply_nonce.
/// Si les protocoles existants ont besoin de 56B, utiliser SHM IPC pour
/// les transferts larges (chemin de données chaud).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IpcMessage {
    /// PID de l'expéditeur — renseigné par le kernel, non falsifiable.
    pub sender_pid:  u32,

    /// Type du message — discriminant du protocole.
    pub msg_type:    u32,

    /// Nonce de réponse — CORR-17.
    /// Pour les requêtes : le client génère un nonce aléatoire.
    /// Pour les réponses : le serveur copie le nonce de la requête.
    /// Le client vérifie que le nonce correspond → détecte les réponses stales.
    /// Valeur 0 = pas de nonce (messages non-requêtes, notifications).
    pub reply_nonce: u32,

    /// Padding alignement.
    pub _pad:        u32,

    /// Payload de données.
    pub payload:     [u8; 48], // ÉTAIT 56 — RÉDUIT à 48 pour reply_nonce
}

const _: () = assert!(core::mem::size_of::<IpcMessage>() == 64);

/// Génère un nonce aléatoire pour une nouvelle requête IPC.
/// Utilisé côté client avant d'envoyer une requête.
pub fn generate_reply_nonce() -> u32 {
    // Pour Phase 8 : utiliser RDRAND directement (pas besoin de crypto_server)
    // Le nonce n'est pas cryptographiquement signé — il protège uniquement
    // contre les réponses stales accidentelles, pas contre un attaquant actif.
    arch::rdrand_u32()
        .unwrap_or_else(|| unsafe { core::arch::x86_64::_rdtsc() } as u32)
}
```

**Migration des protocoles existants** : `payload[0..4]` était souvent inutilisé (zéro) en début de payload. Le remplacement par `reply_nonce` est rétrocompatible si les handlers vérifient via `msg_type` en priorité.

---

## CORR-21 ⚠️ — SRV-03 : documenter comme supprimé

### Problème
Les règles server sont numérotées SRV-01, SRV-02, SRV-04.  
SRV-03 est absent sans explication.

### Correction — Architecture v7 §1.3 + Arborescence V4 §8

```markdown
<!-- Ajouter dans le tableau des règles -->

| **SRV-03** | **(SUPPRIMÉ)** | Règle retirée lors de la révision v4. Était : contrainte de validation |
|            |                | des capabilities au niveau de l'IPC broker. Remplacée par CAP-01 |
|            |                | (verify_cap_token en main.rs) qui est plus générale et décentralisée. |
```

---

## CORR-25 ⚠️ — device_server : fichiers pci/ et gdi/ manquants dans arborescence

### Problème
L'Arborescence V4 liste `servers/device_server/src/` avec :
```
pci_registry.rs / lifecycle.rs / probe.rs / irq_router.rs / claim_validator.rs / protocol.rs
```
Mais omet les fichiers référencés dans Kernel Types v10 §10 et Driver Framework v10 :
- `pci/scanner.rs` (scan_bus_recursive)
- `pci/link_retraining.rs` (wait_link_retraining)
- `gdi/pci_handle.rs` (bar_phys)

**Source** : MiniMax ES-06

### Correction — Arborescence V4 §6.7 (device_server)

```
servers/device_server/src/
├── main.rs              // init_sequence : scan→topology→claim→irq_register→spawn
├── pci_registry.rs      // Registre PCI : bus/device/func → driver assigné
├── lifecycle.rs         // Start/Stop/Reset driver Ring 1. FLR PCI sur reset.
├── probe.rs             // Découverte device↔driver
├── irq_router.rs        // Routage IRQs hardware → drivers via IPC
├── claim_validator.rs   // Valide : CapToken + PciId autorisé + libre (CAP-02)
├── protocol.rs          // Probe, Claim{device_id, driver_cap, nonce}, Release, IrqNotify
├── isolation.rs         // PrepareIsolation → bus master disable → drain IOMMU → ACK (CORR-14)
│
├── pci/                 // ← AJOUT (CORR-25 — était absent de Arborescence V4)
│   ├── scanner.rs       // scan_bus_recursive — découverte PCI complète
│   └── link_retraining.rs  // wait_link_retraining (FIX-94) + fallback 250ms
│
└── gdi/                 // ← AJOUT (CORR-25 — était absent de Arborescence V4)
    └── pci_handle.rs    // bar_phys — adresse physique d'un BAR PCI
```

---

## CORR-26 🔵 — CI script : harmoniser `virtio_block` vs `virtio-block`

### Problème
Architecture v7 §7.3 CI script `PHX-03` :
```bash
for binary in ... virtio_block virtio_net virtio_console; do
    HASH=$(b3sum --no-names target/release/$binary)
```

Mais dans le workspace Cargo.toml, les crates s'appellent `virtio-block`, `virtio-net`, `virtio-console` (avec tirets).  
Cargo normalise les tirets en underscore pour les binaires **uniquement** dans certaines conditions.

**Source** : MiniMax ES-07

### Correction — `build/register_binaries.sh`

```bash
#!/usr/bin/env bash
# build/register_binaries.sh — PHX-03
# CORR-26 : Noms harmonisés avec workspace Cargo.toml (tirets → underscores)
# Cargo normalise : le binaire de virtio-block s'appelle virtio_block dans target/release/

set -euo pipefail

BINARIES=(
    "ipc_broker"
    "init_server"
    "vfs_server"
    "memory_server"
    "crypto_server"
    "device_server"
    "scheduler_server"
    "network_server"
    "virtio_block"    # ← CORR-26 : underscore (binaire Cargo)
    "virtio_net"      # ← CORR-26 : underscore
    "virtio_console"  # ← CORR-26 : underscore
)

for binary in "${BINARIES[@]}"; do
    BINARY_PATH="target/release/${binary}"
    if [[ ! -f "${BINARY_PATH}" ]]; then
        echo "ERREUR PHX-03 : Binaire manquant : ${BINARY_PATH}" >&2
        exit 1
    fi
    HASH=$(b3sum --no-names "${BINARY_PATH}")
    exofs-deploy register --binary "${binary}" --hash "${HASH}"
    echo "PHX-03 : ${binary} → ${HASH}"
done

echo "PHX-03 : Tous les binaires enregistrés dans ExoFS."
```

---

## CORR-28 🔵 — Arborescence V3 : archiver, ne plus référencer

### Action
Tous les documents et scripts qui référencent `ExoOS_Arborescence_V3.docx` doivent le remplacer par `ExoOS_Arborescence_V4.docx`.

**Liste des changements V3→V4 à conserver** :
- V4 ajoute `scheduler_server` (étape 12 → 13 avec exo_shield décalé)
- V4 ajoute `build/register_binaries.sh` (PHX-03)
- V4 spécifie `sealed blob` format dans `crypto_server/isolation.rs`

```bash
# Vérification CI : aucune référence à V3 dans les sources actives
grep -r "Arborescence_V3\|arborescence_v3\|arborescence-v3" \
    --include="*.md" --include="*.rs" --include="*.toml" \
    . && echo "ERREUR : Référence à Arborescence V3 trouvée" && exit 1
echo "OK : Aucune référence à Arborescence V3"
```

---

## Résumé des règles absolues — Version canonique unifiée

Les règles suivantes sont confirmées correctes et identiques dans tous les documents :

| Règle | Description | Statut |
|-------|-------------|--------|
| **SRV-01** | init_server reçoit ChildDied IPC → sigchld_handler.rs → supervisor.rs | ✅ Inchangé |
| **SRV-02** | Aucun crate sauf crypto_server n'importe blake3/chacha20poly1305 | ✅ Inchangé |
| **SRV-03** | **(Supprimé)** voir CORR-21 | ✅ Documenté |
| **SRV-04** | Toute opération crypto passe par crypto_server IPC | ✅ Inchangé |
| **IPC-01** | SpscRing : `#[repr(C, align(64))]` sur head et tail | ✅ Inchangé |
| **IPC-02** | Types protocol.rs Sized, FixedString<N>, pas de Vec/String/Box | ✅ Inchangé |
| **IPC-03** | sender_pid:u32 renseigné par kernel + reply_nonce (CORR-17) | ✅ Mis à jour |
| **CAP-01** | verify_cap_token() en première instruction de main.rs | ✅ Inchangé |
| **CAP-02** | Claim{device_id, driver_cap, nonce} — nonce kernel non forgeable | ✅ Inchangé |
| **PHX-01** | Chaque server critique → PrepareIsolationAck avant gel | ✅ Inchangé |
| **PHX-02** | `#![no_std]` + `panic='abort'` dans chaque crate | ✅ Inchangé |
| **PHX-03** | Blake3(ELF) dans ExoFS via register_binaries.sh | ✅ Inchangé |

---

## Ordre de démarrage canonique final (13 étapes)

| Étape | Crate | PID | Dépendance | Note |
|-------|-------|-----|-----------|------|
| 1 | `libs/exo-types` | — | — | Types, CapToken (CORR-05) |
| 2 | `libs/exo-ipc + exo-syscall` | — | exo-types | phoenix.rs syscalls 520-529 |
| 3 | `servers/ipc_broker` | 2 | — | Kernel assigne PID 2 |
| 4 | `servers/memory_server` | dyn | ipc_broker | Bloque userspace |
| 5 | `servers/init_server` | 1 | ipc+memory | boot_info_virt virtuel (CORR-09) |
| 6 | `servers/vfs_server` | 3 | init+ExoFS | POSIX bridge |
| 7 | `servers/crypto_server` | 4 | vfs | SEUL avec RustCrypto |
| 8 | `servers/device_server` | dyn | ipc+memory | AVANT tout driver |
| 9 | `drivers/virtio-block` | dyn | device_server | ExoFS backend |
| 10 | `drivers/virtio-net + virtio-console` | dyn | device_server | Réseau + console |
| 11 | `servers/network_server` | dyn | virtio-net | smoltcp |
| 12 | `servers/scheduler_server` | dyn | init | Politique Ring 1/3 |
| 13 | `servers/exo_shield` | dyn | Phase 3 Phoenix | phoenix_notify(521) |

---

*ExoOS — Corrections Servers & Arborescence — Mars 2026*
