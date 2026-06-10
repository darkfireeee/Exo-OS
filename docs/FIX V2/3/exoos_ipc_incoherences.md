# ExoOS — Rapport d'analyse des incohérences IPC
## Audit inter-modules : `ipc_router` ↔ `fb_server`, `tty_server`, `exo_shield`, `exofs/vfs_server`, `exosh`
**Date :** 05 juin 2026 | **Révision dépôt :** `darkfireeee/Exo-OS` (HEAD)

---

## Synthèse exécutive

L'analyse croisée du code source révèle **7 incohérences structurelles** entre l'`ipc_router` (PID 2) et les modules adjacents. La plus critique est une désynchronisation totale entre le DAG `ExoCordon` de l'`ipc_router` (5 arêtes) et la politique IPC du kernel `ipc_policy.rs` (51 paires autorisées) : **46 des 51 chemins légitimes sont bloqués silencieusement** par le routeur Ring 1. En pratique, tout le pipeline d'affichage (`tty_server → fb_server`), la chaîne d'entrée, et la communication `exosh ↔ services` sont inaccessibles via l'`ipc_router`.

---

## Incohérence #1 — **[CRITIQUE]** DAG ExoCordon fragmentaire vs politique kernel complète

### Localisation
- `servers/ipc_router/src/exocordon.rs` — `static AUTHORIZED_GRAPH`
- `kernel/src/security/ipc_policy.rs` — `static POLICY`

### Description
Le DAG ExoCordon de l'`ipc_router` ne contient que **5 arêtes** :

```
Init     → Memory
Init     → Vfs
Vfs      → Crypto
Network  → Vfs
Device   → VirtioDrivers
```

Or la politique du kernel autorise **51 paires** dont notamment :
- `Memory ↔ Init`, `Init ↔ Crypto`, `Init ↔ Device`, `Init ↔ Scheduler`, `Init ↔ ExoShield`
- `InputServer ↔ TtyServer`, `TtyServer ↔ FbServer`, `TtyServer ↔ Vfs`
- `ExoShield ↔ Crypto`, `ExoShield ↔ InputServer`, `ExoShield ↔ TtyServer`
- `Exosh ↔ TtyServer`, `Exosh ↔ ExoShield`, `Exosh ↔ Crypto`
- `Network ↔ Device`, `Network ↔ VirtioDriver`

**Résultat :** `check_ipc()` renvoie `IpcError::UnauthorizedPath` (ou `UnknownService`) pour **46/51 chemins** autorisés côté kernel. Ces messages sont silencieusement droppés par le `security_gate` sans notification d'erreur vers l'expéditeur.

### Impact
- Le pipeline d'affichage complet (`tty → fb`) est bloqué si les messages transitent par le router.
- `exosh` ne peut contacter ni `tty_server`, ni `crypto_server`, ni `exo_shield` via le routeur.
- Les réponses de type `Memory → Init` sont bloquées (deadlock potentiel).
- Les heartbeats bidirectionnels `Init ↔ *` sont mutilés.

### Correction
Synchroniser `AUTHORIZED_GRAPH` avec `POLICY` du kernel, ou implémenter un mécanisme de délégation (le DAG devient une sur-couche vérifiée après la politique kernel, non un remplacement).

---

## Incohérence #2 — **[CRITIQUE]** Numéro de syscall hardcodé erroné dans `router.rs`

### Localisation
- `servers/ipc_router/src/router.rs` — fonctions `forward_message()` et `batch_forward()` / `Broadcast`

### Description
La fonction `forward_message()` utilise le numéro **302** en dur pour envoyer les messages :

```rust
// Dans router.rs, forward_message() et broadcast loop
let result = unsafe {
    crate::syscall::syscall6(
        302, // SYS_IPC_SEND ← FAUX
        ...
    )
};
```

Or dans `syscall_abi/src/lib.rs` :
```
SYS_EXO_IPC_SEND     = 300   ← correct
SYS_EXO_IPC_RECV     = 301
SYS_EXO_IPC_RECV_NB  = 302   ← c'est ce qui est appelé !
SYS_IPC_SEND         = SYS_EXO_IPC_SEND = 300
```

**Le routeur appelle `SYS_EXO_IPC_RECV_NB` (réception non-bloquante) au lieu de `SYS_EXO_IPC_SEND`.** Tous les messages routés via `forward_message()` échouent silencieusement.

En revanche, `main.rs` utilise correctement `syscall::SYS_IPC_SEND` (la constante), ce qui crée une asymétrie entre la boucle principale et le module de routage avancé.

### Correction
```rust
// router.rs — remplacer 302 par la constante
use exo_syscall_abi as syscall;
let result = unsafe {
    syscall::syscall6(
        syscall::SYS_IPC_SEND,  // = 300
        ...
    )
};
```

---

## Incohérence #3 — **[HAUTE]** ExoShield : conflit PID/endpoint entre modules

### Localisation
- `servers/exo_shield/src/main.rs` — `const EXO_SHIELD_ENDPOINT: u64 = 10`
- `servers/exo_shield/src/ipc_gate/policy.rs` — `const EXO_SHIELD_PID: u32 = 12`
- `servers/ipc_router/src/exocordon.rs` — `ServiceId::ExoShield = 10`
- `servers/ipc_router/src/router.rs` — route default `(10, ...) // exo_shield`
- `servers/syscall_abi/src/lib.rs` — `TTY_SERVER_ENDPOINT: u64 = 12`

### Description
Trois valeurs contradictoires coexistent pour identifier ExoShield :

| Source | Valeur | Signification |
|--------|--------|---------------|
| `exo_shield/main.rs` | 10 | Endpoint d'enregistrement IPC |
| `exocordon.rs` | 10 | `ServiceId::ExoShield` |
| `router.rs` (route) | 10 | PID destination dans la table |
| `ipc_gate/policy.rs` | **12** | `EXO_SHIELD_PID` ← diverge ! |
| `syscall_abi` | **12** | `TTY_SERVER_ENDPOINT` ← collision ! |

La `policy.rs` interne d'ExoShield applique ses règles sur le PID 12 — qui correspond en réalité à `TTY_SERVER_ENDPOINT`. Les règles de filtrage IPC d'ExoShield sont donc **appliquées sur le mauvais service**.

### Correction
Aligner `EXO_SHIELD_PID` dans `policy.rs` sur la valeur d'endpoint réelle (10) :
```rust
// servers/exo_shield/src/ipc_gate/policy.rs
const EXO_SHIELD_PID: u32 = 10;  // était 12
```

---

## Incohérence #4 — **[HAUTE]** `MAX_INLINE_PAYLOAD` irréaliste bloque `FbRequest` et `TtyRequest`

### Localisation
- `servers/ipc_router/src/security_gate.rs` — `const MAX_INLINE_PAYLOAD: usize = 48`
- `servers/syscall_abi/src/lib.rs` — `FbRequest`, `TtyRequest`, `IPC_INLINE_PAYLOAD_SIZE`

### Description
Le `security_gate` rejette tout payload supérieur à **48 octets** (règle IPC-04). Or les structures de message des serveurs d'affichage dépassent largement cette limite :

```
FbRequest  = 4 + 8 + 8 + 8 + 208 = 236 octets  (FB_TEXT_MAX = 208)
TtyRequest = 4 + 8 + 8 + 8 + 184 = 212 octets  (TTY_LINE_MAX = 184)
IPC_INLINE_PAYLOAD_SIZE (ABI officielle) = 192 octets
```

Si `tty_server` ou `exosh` envoyaient leurs requêtes via l'`ipc_router`, elles seraient **systématiquement rejetées** avec verdict `DenyPayloadTooLarge`. La règle de 48 octets est une ancienne contrainte conservatoire jamais mise à jour après l'extension de l'enveloppe ABI à 192 octets.

**Note :** En pratique, `tty_server → fb_server` bypass l'`ipc_router` (SYS_IPC_SEND direct à `FB_SERVER_ENDPOINT`), ce qui masque ce problème mais contourne le contrôle de sécurité.

### Correction
```rust
// security_gate.rs
const MAX_INLINE_PAYLOAD: usize = IPC_INLINE_PAYLOAD_SIZE; // = 192, depuis syscall_abi
```

---

## Incohérence #5 — **[MOYENNE]** Double registre de noms désynchronisé

### Localisation
- `servers/ipc_router/src/main.rs` — `Registry` (hash FNV-32, max 64 entrées, local)
- `kernel/src/syscall/table.rs` — `sys_exo_ipc_lookup()` (endpoint registry kernel)

### Description
Il existe deux registres de noms d'endpoints indépendants et **non synchronisés** :

1. **Registre kernel** (`sys_exo_ipc_lookup`) : utilisé par `tty_server` (`fb_endpoint_ready()`), `fb_server`, `exosh`. C'est ce registre qui est consulté via `SYS_IPC_LOOKUP (306)`.

2. **Registre local ipc_router** (`Registry`) : table hash FNV-32 en mémoire du PID 2, alimentée via messages `IPC_MSG_REGISTER`. L'`ipc_router` ne consulte jamais le registre kernel pour résoudre les destinations.

**Conséquence :** Un service enregistré dans le kernel via `SYS_IPC_CREATE` n'est pas automatiquement connu de l'`ipc_router`. Le routeur peut décider qu'une destination est "inconnue" (`DenyUnknownService`) alors que le kernel la connaît parfaitement. Et inversement, un service qui s'enregistre auprès du routeur mais pas du kernel est invisible à `SYS_IPC_LOOKUP`.

De plus, la résolution par hash FNV-32 sans vérification de collision peut causer des **aliasing silencieux** (deux noms de services avec le même hash FNV-32 seraient routés vers le même endpoint).

### Correction
L'`ipc_router` devrait utiliser `SYS_IPC_LOOKUP` pour résoudre les noms, ou propager ses enregistrements vers le kernel via `SYS_IPC_CREATE`.

---

## Incohérence #6 — **[MOYENNE]** `exosh` et services d'affichage absents du DAG ExoCordon

### Localisation
- `servers/ipc_router/src/exocordon.rs` — `service_id_of()`

### Description
`service_id_of()` ne mappe que 10 PIDs (1–10). Les services suivants n'ont **aucun `ServiceId`** :

- `exosh` (PID dynamique > 10)
- `tty_server` (endpoint 12, PID dynamique)
- `fb_server` (endpoint 20, PID dynamique)
- `input_server` (endpoint 11, PID dynamique)
- `ps2_driver`, `scheduler_server`, `device_server` (PIDs dynamiques)

Tout IPC depuis ces services via le routeur retourne `IpcError::UnknownService` → `SecurityVerdict::DenyUnknownService`. En particulier :
- `exosh` ne peut pas s'authentifier auprès du routeur
- Les PIDs assignés dynamiquement par l'`init_server` ne correspondent pas aux IDs statiques d'ExoCordon

### Correction
Étendre `service_id_of()` pour couvrir les endpoints fixes (11, 12, 20) ou implémenter une registration dynamique côté ExoCordon quand un service s'enregistre via `IPC_MSG_REGISTER`.

---

## Incohérence #7 — **[FAIBLE]** `audit_log_violation` : stub sans émission IPC vers ExoShield

### Localisation
- `servers/ipc_router/src/security_gate.rs` — `audit_log_violation()`

### Description
La fonction `audit_log_violation()` est documentée comme un stub Phase 3 censé envoyer un message IPC vers `exo_shield` en Phase 4. Dans l'état actuel, elle appelle uniquement `record_violation()` (compteur local) sans aucun IPC :

```rust
pub fn audit_log_violation(...) {
    record_violation(src_pid, reason as u8);  // compteur local seulement
    // Aucun IPC vers exo_shield
}
```

De plus, `exo_shield` définit son propre système de règles IPC dans `ipc_gate/policy.rs` qui est **parallèle et indépendant** du `security_gate` de l'`ipc_router`. Il n'existe aucun canal établi pour que l'`ipc_router` notifie `exo_shield` des violations détectées.

Compte tenu de l'incohérence #1 et #3, même si le stub était implémenté, le message IPC vers ExoShield serait bloqué par ExoCordon (chemin absent) et envoyé au mauvais PID (12 = TTY).

### Correction
- Implémenter l'émission IPC réelle vers `exo_shield` avec le bon endpoint (10)
- Ajouter l'arête `IpcBroker → ExoShield` dans le DAG ExoCordon (le routeur est IpcBroker)
- Coordonner les politiques `security_gate` et `ipc_gate/policy.rs`

---

## Tableau récapitulatif

| # | Sévérité | Module source | Module cible | Nature | Impact runtime |
|---|----------|--------------|--------------|--------|----------------|
| 1 | 🔴 CRITIQUE | `ipc_router/exocordon.rs` | Tous serveurs | DAG 5/51 arêtes | 46 chemins bloqués |
| 2 | 🔴 CRITIQUE | `ipc_router/router.rs` | Kernel syscall | Syscall 302≠300 | Forward impossible |
| 3 | 🟠 HAUTE | `exo_shield/policy.rs` | `tty_server` | PID 12 confondu | Règles sécurité erronées |
| 4 | 🟠 HAUTE | `ipc_router/security_gate.rs` | `fb_server`, `tty_server` | Payload 48 < 192 | FbRequest/TtyRequest bloqués |
| 5 | 🟡 MOYENNE | `ipc_router/main.rs` | Kernel endpoint registry | Double registre désync | Résolution incohérente |
| 6 | 🟡 MOYENNE | `ipc_router/exocordon.rs` | `exosh`, `tty`, `fb`, `input` | Services non mappés | UnknownService systématique |
| 7 | 🔵 FAIBLE | `ipc_router/security_gate.rs` | `exo_shield` | Stub audit non implémenté | Traçabilité absente |

---

## Plan de correction recommandé

**Phase 1 — Urgences (bloquant le boot fonctionnel) :**
1. Corriger le syscall 302→300 dans `router.rs` (correctif trivial, une ligne)
2. Aligner `AUTHORIZED_GRAPH` d'ExoCordon avec les 51 paires de `ipc_policy.rs`
3. Corriger `EXO_SHIELD_PID = 10` dans `ipc_gate/policy.rs`

**Phase 2 — Cohérence architecturale :**
4. Aligner `MAX_INLINE_PAYLOAD` sur `IPC_INLINE_PAYLOAD_SIZE` (192)
5. Étendre `service_id_of()` aux endpoints fixes (11, 12, 20) ou implémenter un registre dynamique

**Phase 3 — Consolidation :**
6. Unifier les registres de noms (router local ↔ kernel registry)
7. Implémenter l'émission d'audit IPC vers ExoShield

---

*Rapport généré par analyse statique du code source — ExoOS v0.1.0 → v0.2.0 pre-stabilization*
