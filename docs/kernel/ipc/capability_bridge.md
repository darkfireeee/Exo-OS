# Capability Bridge IPC

Le sous-module `ipc/capability_bridge/` est un shim de délégation vers `security::capability/`. Il ne contient aucune logique de vérification de droits.

## Principe (IPC-04)

```
ipc/capability_bridge/         security/capability/
─────────────────────────────────────────────────────
verify_ipc_access(token, rights)
  └── security::capability::verify(
            token.object_id,
            token.rights as u32
      )
```

**Toute** décision de droit est prise par `security/capability/`. Le bridge ne fait que :
1. Adapter les types IPC vers les types capability
2. Transcrire les codes d'erreur

## Vue d'ensemble

```
capability_bridge/
├── mod.rs    — Re-exports uniquement, zéro logique
└── check.rs  — Shim : CapToken mirror, Rights, verify_ipc_access()
```

---

## Types — `capability_bridge/check.rs`

### CapToken (miroir)

```rust
/// Miroir local de security::capability::CapToken.
/// Permet à ipc/ de manipuler des tokens sans importer directement
/// l'implémentation de security/.
#[repr(C)]
pub struct CapToken {
    pub object_id:  u64,   // ID de l'objet protégé (ex. : EndpointId.to_object_id())
    pub generation: u32,   // génération du token (anti-UAF)
    pub rights:     u16,   // bitmask Rights
    pub _pad:       u16,
}
```

**Note** : Ce type est un miroir de lecture. La création de tokens est réservée à `security/capability/`. IPC ne crée jamais de tokens, il les vérifie seulement.

### Rights

```rust
pub struct Rights(pub u16);
impl Rights {
    pub const READ:      Self = Self(1 << 0);
    pub const WRITE:     Self = Self(1 << 1);
    pub const EXECUTE:   Self = Self(1 << 2);
    pub const SEND:      Self = Self(1 << 3);
    pub const RECEIVE:   Self = Self(1 << 4);
    pub const DELEGATE:  Self = Self(1 << 5);
    pub const CONNECT:   Self = Self(1 << 6);
    pub const LISTEN:    Self = Self(1 << 7);
}
```

### CapTable (stub)

```rust
/// Stub opaque vers la CapTable du PCB courant.
/// En production, pointe vers la table de capabilities du processus.
pub struct CapTable {
    _opaque: u64,   // adresse de la CapTable réelle
}

impl CapTable {
    /// Bootstrap uniquement — crée une CapTable "de confiance" pour le kernel.
    /// Valeur sentinelle : 0xDEAD_BEEF_0000_0000.
    /// Ne jamais utiliser en userland ou dans du code non-boot.
    pub fn trusted() -> Self {
        Self { _opaque: 0xDEAD_BEEF_0000_0000 }
    }
}
```

---

## Fonctions de vérification — `capability_bridge/check.rs`

### `verify_ipc_access`

```rust
/// Vérifie qu'un token confère les droits requis sur un objet IPC.
///
/// Délègue ENTIÈREMENT à security::capability::verify().
/// Aucune logique locale.
pub fn verify_ipc_access(
    token:    &CapToken,
    required: Rights,
) -> Result<(), IpcCapError>
```

Algorithme :
1. Appelle `security::capability::verify(token.object_id, token.rights as u32)`
2. Mappe les erreurs `CapError` en `IpcCapError`
3. Vérifie `(token.rights & required.0) == required.0`

### `verify_endpoint_access`

```rust
/// Vérifie les droits d'accès à un endpoint spécifique.
///
/// Convertit EndpointId en object_id via EndpointId::to_object_id()
/// avant délégation à security/.
pub fn verify_endpoint_access(
    ep:       EndpointId,
    token:    &CapToken,
    required: Rights,
) -> Result<(), IpcCapError>
```

L'`object_id` est calculé par `EndpointId::to_object_id()` :
```rust
// Les 32 bits hauts = 0x01 (type endpoint IPC)
// Les 32 bits bas   = id & 0xFFFF_FFFF
(0x01u64 << 32) | (ep.0.get() & 0xFFFF_FFFF)
```

---

## Ce qui est interdit dans `capability_bridge/`

| Interdit | Raison |
|---|---|
| Listes d'autorisations locales | Crée une surface d'attaque indépendante |
| Cache de droits local | Peut se désynchroniser de `security/capability/` |
| Court-circuit pour les chemins rapides | Contourne la politique de sécurité |
| Import de logique de révocation | Appartient à `security/capability/` |
| Création de CapToken | Réservé à `security/capability/` |

---

## Conversion d'erreurs

```rust
impl From<IpcCapError> for IpcError {
    fn from(e: IpcCapError) -> Self {
        match e {
            IpcCapError::Revoked            => IpcError::PermissionDenied,
            IpcCapError::ObjectNotFound     => IpcError::EndpointNotFound,
            IpcCapError::InsufficientRights => IpcError::PermissionDenied,
            IpcCapError::DelegationDenied   => IpcError::PermissionDenied,
        }
    }
}
```

---

## Intégration avec les endpoints

```rust
// Avant toute opération IPC sensible :
let token = process_get_cap_token(pid, ep_id)?;
verify_endpoint_access(ep_id, &token, Rights::CONNECT)
    .map_err(IpcError::from)?;
// continuation de l'opération...
```

Le `CapToken` est récupéré depuis le PCB du processus appelant, jamais fourni par l'appelant lui-même.

---

## Re-exports publics (`capability_bridge/mod.rs`)

```rust
// mod.rs — aucune logique, uniquement des re-exports
pub use self::check::IpcCapBridge;
pub use self::check::verify_ipc_access;
pub use self::check::verify_endpoint_access;
```

Le trait `IpcCapBridge` résume le contrat du bridge :
```rust
pub trait IpcCapBridge {
    fn check_send(token: &CapToken) -> Result<(), IpcCapError>;
    fn check_receive(token: &CapToken) -> Result<(), IpcCapError>;
    fn check_connect(token: &CapToken, ep: EndpointId) -> Result<(), IpcCapError>;
}
```
