# ExoOS — Corrections Sécurité & Crypto

**Version** : 1.0  
**Date** : Avril 2026  
**Périmètre** : `kernel/src/security/*`, `servers/crypto_server/*`, `servers/exo_shield/src/signatures/`  
**Méthode** : lecture complète des fichiers sources + croisement avec `ExoShield_v1_Production.md`, `GI-04-05-06`, les règles `SRV-02/SRV-04/CAP-01`  

---

## 0. Résumé exécutif

Quatre bugs P0 indépendants, dont deux liés par une causalité directe.

| Réf. | Titre | Gravité | Fichiers |
|---|---|---|---|
| SEC-BUG-01 | `crypto_server` : CAP-01 déclaré, non implémenté | P0 | `crypto_server/src/main.rs` |
| SEC-BUG-02 | `crypto_server` : keystore propre = code mort, keystore simplifié = actif | P0 | `crypto_server/src/main.rs`, `src/keystore.rs` |
| SEC-BUG-03 | `crypto_server` : messages 5-11 documentés, non câblés → cause racine de SEC-BUG-04 | P0 | `crypto_server/src/main.rs` |
| SEC-BUG-04 | `exo_shield` : Ed25519 local (SRV-02) — **conséquence de SEC-BUG-03** | P0 | `exo_shield/src/signatures/update.rs` |
| SEC-BUG-05 | `xchacha20` : aucune API de reseed post-Phoenix | P0 | `crypto_server/src/xchacha20.rs` |
| SEC-NOTE-01 | `security_init()` : séquence 3 phases non documentée | P1 | `kernel/src/security/mod.rs` |
| SEC-NOTE-02 | ExoShield : collision de nomenclature kernel vs serveur | P1 | `ExoShield_v1_Production.md`, `servers/exo_shield/` |

---

## 1. Réflexion sur la chaîne causale

Avant de lister les corrections mécaniques, il est important de comprendre **pourquoi** ces bugs coexistent.

La règle `SRV-02` dit : aucune implémentation crypto dans les serveurs Ring 1 — tout doit passer par `crypto_server`. Le rapport d'audit signale `exo_shield` comme violateur de cette règle. C'est exact, mais c'est un symptôme, pas la cause.

La vraie chaîne est :

```
crypto_server ne câble pas CRYPTO_SIGN / CRYPTO_VERIFY (SEC-BUG-03)
  ↓
exo_shield a besoin de vérifier des signatures Ed25519 pour les updates
  ↓
exo_shield implémente Ed25519 localement (SEC-BUG-04)
  ↓
Le commentaire dans update.rs l'admet : "En production, cela serait remplacé par le crypto_server"
```

Autrement dit, **corriger SEC-BUG-04 sans corriger SEC-BUG-03 est inutile**. Si on retire l'Ed25519 local d'`exo_shield` sans que `crypto_server` expose `CRYPTO_SIGN`/`CRYPTO_VERIFY`, `exo_shield` n'a plus aucun moyen de valider les signatures de mises à jour.

La même logique s'applique pour `CAP-01` (SEC-BUG-01) : le protocole IPC du `crypto_server` transmet actuellement `sender_pid` comme seul identifiant de l'appelant. Un vrai `CapToken` n'est jamais transmis dans le `CryptoRequest`. Implémenter `verify_cap_token()` sans modifier le protocole IPC serait du code mort supplémentaire.

---

## 2. SEC-BUG-01 — CAP-01 déclaré, non implémenté

### Observation dans le code

Le header de `servers/crypto_server/src/main.rs` déclare :
```
//! - CAP-01 : vérification de capability token en première instruction
```

`keystore.rs` l'affirme aussi :
```
//! - CAP-01 : toute opération vérifie le handle via constant-time compare
```

Analyse du code réel de `handle_request()` : l'unique information d'identité de l'appelant dans `CryptoRequest` est `sender_pid: u32`. Les fonctions `ks_get()` et `ks_insert()` vérifient `owner_pid == sender_pid`. Ce n'est **pas CAP-01**.

**CAP-01 dans la spec** (`capability/mod.rs`) désigne la règle :
```
// RÈGLE CAP-01 : security/capability/ est l'UNIQUE source de vérité pour les capabilities.
```

La vérification de capability au sens du noyau implique un `CapToken` (24 bytes : generation + object_id + rights), pas un PID. Un PID peut être usurpé par tout processus capable d'injecter un message IPC avec le bon `sender_pid` — ce qui est exactement le vecteur que CAP-01 doit bloquer.

### Cause racine

Le `CryptoRequest` ne transporte pas de `CapToken`. Le protocole IPC de `crypto_server` n'a pas été conçu pour en transporter un. La validation actuelle par `owner_pid` est un contrôle de paternité des clés (empêcher le vol de handle), non une vérification de capability sur le droit d'appeler le service crypto.

Ces deux mécanismes sont distincts :
- **Paternité de handle** : qui a créé cette clé → implémenté via `owner_pid`
- **Droit d'appel du service** : ce processus a-t-il le droit d'utiliser `crypto_server` → **non implémenté**

### Correction

**Étape 1 — Étendre le protocole IPC**

```rust
/// Message IPC entrant v2 (136 bytes).
#[repr(C)]
struct CryptoRequest {
    sender_pid: u32,
    msg_type: u32,
    // Nouveau : CapToken serialisé (24 bytes = gen[4] + oid[8] + rights[4] + _pad[8])
    // Le kernel garantit que sender_pid correspond à l'émetteur réel.
    // Le CapToken est vérifié par le kernel avant livraison (ExoCordon DAG).
    cap_token_raw: [u8; 24],
    payload: [u8; 96],  // réduit de 120 à 96 pour compenser
}
```

**Étape 2 — Vérification en entrée de `handle_request()`**

```rust
fn handle_request(req: &CryptoRequest) -> CryptoReply {
    // CAP-01 — première instruction, avant tout accès au msg_type
    // Le token doit avoir le droit IPC_CALL sur l'ObjectId du crypto_server
    let cap = CapToken::from_bytes(&req.cap_token_raw);
    if !ipc_cap_verify(cap, Rights::IPC_CALL) {
        return CryptoReply { status: CRYPTO_ERR_CAP, ..Default::default() };
    }
    // Suite normale...
}
```

`ipc_cap_verify()` est un appel syscall vers le noyau (`SYS_CAP_VERIFY`) — le serveur délègue la vérification au kernel qui a accès à la CapTable du processus appelant.

**Étape 3 — Ajouter `CRYPTO_ERR_CAP`**

```rust
const CRYPTO_ERR_CAP: u32 = 5;  // Capability insuffisante ou invalide
```

**Note** : cette correction implique que tous les appelants de `crypto_server` (`exo_shield`, `network_server`, `vfs_server`) doivent transmettre leur `CapToken` dans chaque requête. C'est le coût normal d'une vérification de capability correcte.

---

## 3. SEC-BUG-02 — Deux keystores : le bon est mort, le mauvais est actif

### Observation

`servers/crypto_server/` contient deux implémentations de keystore :

**`keystore.rs`** (531 lignes, 13 fonctions publiques) :
- `insert_key()` avec TTL (`KEY_MAX_LIFETIME_TSC`), type de clé, owner_pid, compteur d'utilisation
- `get_key()` avec vérification propriétaire et expiration automatique
- `revoke_key()` avec shredding DoD 5220.22-M 3 passes
- `rotate_key()` avec réallocation de handle
- `expire_check()` pour maintenance périodique
- `revoke_all_for_owner()` pour nettoyage sur terminaison de processus
- `get_stats()` avec compteurs d'activité

**`main.rs`** — keystore inline `KS_SLOTS` (45 lignes, 2 fonctions) :
- `ks_insert()` : insertion basique, pas de TTL, pas de type
- `ks_get()` : récupération par handle + owner_pid

Le module `keystore` est importé (`mod keystore;`) dans `Cargo.toml`/`lib.rs` mais **jamais utilisé dans `main.rs`**. La vérification :

```rust
// main.rs ne contient aucune des lignes suivantes :
use keystore::*;
keystore::insert_key(...)
keystore::get_key(...)
```

`ks_insert()` ne fait pas de shredding. `ks_get()` ne vérifie pas l'expiration. `KS_SLOTS` a 32 slots contre 64 dans `keystore.rs`. Les handles alloués par `KS_HANDLE_CTR` dans `main.rs` et par `keystore.rs` divergeraient si les deux étaient actifs.

### Cause racine

La refactorisation de `main.rs` (ajout du keystore inline simplifié) n'a pas supprimé les fonctions correspondantes de `keystore.rs`. Le fichier `keystore.rs` est devenu code mort progressivement.

### Correction

Remplacer le keystore inline de `main.rs` par le vrai `keystore.rs`. Procédure :

```rust
// main.rs — supprimer :
// - struct KeySlot, struct KeySlots, KS_SLOTS, KS_HANDLE_CTR
// - fn ks_insert(), fn ks_get()

// main.rs — ajouter en tête :
use crate::keystore::{self, KeyType};

// Dans handle_request(), CRYPTO_DERIVE_KEY :
let handle = keystore::insert_key(&derived_key, KeyType::Derived, req.sender_pid);

// Dans handle_request(), CRYPTO_ENCRYPT / CRYPTO_DECRYPT :
let (key, _key_type) = match keystore::get_key(key_handle, req.sender_pid) {
    Some(kv) => kv,
    None => { reply.status = CRYPTO_ERR_KEY_INVALID; return reply; }
};

// Dans la boucle principale, sur timeout :
if r == ETIMEDOUT {
    IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    keystore::expire_check();  // maintenance TTL
    continue;
}
```

**Bénéfice immédiat** : le shredding DoD 3 passes s'applique à toutes les clés expirées ou révoquées, pas seulement à la clé locale dans `CRYPTO_DERIVE_KEY`. La rotation de clé devient disponible. Les stats de keystore sont exploitables.

---

## 4. SEC-BUG-03 — Messages 5-11 documentés, non implémentés

### Observation

Le header de `main.rs` documente 12 types de messages (0-11). Seuls les types 0-4 ont des constantes définies et un handler dans `match req.msg_type`. Les types 5-11 tombent dans le bras `_ => CRYPTO_ERR_ARGS`.

```
Implémenté  : CRYPTO_DERIVE_KEY(0), CRYPTO_RANDOM(1), CRYPTO_ENCRYPT(2),
              CRYPTO_DECRYPT(3), CRYPTO_HASH(4)
Non câblé   : CRYPTO_SIGN(5), CRYPTO_VERIFY(6), CRYPTO_TLS_INIT(7),
              CRYPTO_TLS_RESPOND(8), CRYPTO_TLS_CLOSE(9),
              CRYPTO_KEY_REVOKE(10), CRYPTO_KEY_ROTATE(11)
```

Le serveur se présente comme "seul service autorisé aux primitives cryptographiques" (`SRV-04`) mais ne peut ni signer, ni vérifier, ni gérer des sessions TLS, ni révoquer une clé à la demande.

### Impact de l'absence de CRYPTO_SIGN / CRYPTO_VERIFY

Le kernel contient déjà `kernel/src/security/crypto/ed25519.rs` avec `ed25519_sign()` et `ed25519_verify()`. Ce module Ring 0 est conforme à l'architecture (SRV-02 ne s'applique qu'au Ring 1). `crypto_server` doit être le relais Ring 1 de ces primitives — il ne l'est pas.

C'est la raison directe pour laquelle `exo_shield` a réimplémenté Ed25519 localement (voir SEC-BUG-04) : aucun autre chemin Ring 1 n'était disponible.

### Correction — CRYPTO_SIGN et CRYPTO_VERIFY

Ces deux types sont prioritaires car ils débloquent SEC-BUG-04.

```rust
// Ajouter les constantes
const CRYPTO_SIGN: u32   = 5;
const CRYPTO_VERIFY: u32 = 6;
const CRYPTO_KEY_REVOKE: u32 = 10;
const CRYPTO_KEY_ROTATE: u32 = 11;

// Dans handle_request(), ajouter ces bras au match :

CRYPTO_SIGN => {
    // payload[0..4] = key_handle (LE) — clé Ed25519 privée
    // payload[4..36] = message (32 bytes) — à signer
    let key_handle = u32::from_le_bytes(req.payload[0..4].try_into().unwrap_or_default());
    let (privkey, _) = match keystore::get_key(key_handle, req.sender_pid) {
        Some(kv) => kv,
        None => { reply.status = CRYPTO_ERR_KEY_INVALID; return reply; }
    };
    // Appel syscall vers kernel Ring 0 (SYS_CRYPTO_SIGN)
    let sig_ret = unsafe {
        syscall::syscall4(
            syscall::SYS_CRYPTO_SIGN,
            privkey.as_ptr() as u64,
            32u64,
            req.payload[4..].as_ptr() as u64,
            32u64,
        )
    };
    if sig_ret >= 0 {
        // La signature (64 bytes) est retournée dans un buffer kernel → reply.data
        // Protocole exact à définir avec le kernel ABI
        reply.status = CRYPTO_OK;
    } else {
        reply.status = CRYPTO_ERR_ARGS;
    }
}

CRYPTO_VERIFY => {
    // payload[0..32] = clé publique Ed25519
    // payload[32..64] = signature (64 bytes)
    // payload[64..96] = message (32 bytes)
    let pubkey  = &req.payload[0..32];
    let sig     = &req.payload[32..96];
    let message = &req.payload[96..];  // taille variable
    let verify_ret = unsafe {
        syscall::syscall6(
            syscall::SYS_CRYPTO_VERIFY,
            pubkey.as_ptr() as u64,  32u64,
            sig.as_ptr()   as u64,   64u64,
            message.as_ptr() as u64, message.len() as u64,
        )
    };
    reply.status = if verify_ret == 0 { CRYPTO_OK } else { CRYPTO_ERR_AUTH };
}

CRYPTO_KEY_REVOKE => {
    let key_handle = u32::from_le_bytes(req.payload[0..4].try_into().unwrap_or_default());
    let ok = keystore::revoke_key(key_handle); // inclut shredding 3 passes
    reply.status = if ok { CRYPTO_OK } else { CRYPTO_ERR_KEY_INVALID };
}

CRYPTO_KEY_ROTATE => {
    let key_handle = u32::from_le_bytes(req.payload[0..4].try_into().unwrap_or_default());
    let new_handle = keystore::rotate_key(key_handle, req.sender_pid);
    if new_handle != 0 {
        reply.status = CRYPTO_OK;
        reply.key_handle = new_handle;
    } else {
        reply.status = CRYPTO_ERR_KEY_INVALID;
    }
}
```

**Note sur TLS (7, 8, 9)** : les messages TLS sont plus complexes (état de session multi-messages). Ils ne peuvent pas être résolus dans cette correction sans définir d'abord un protocole de session IPC. Ils doivent être marqués `CRYPTO_ERR_NOT_IMPLEMENTED` (nouveau code d'erreur = 6) plutôt que `CRYPTO_ERR_ARGS`, pour distinguer "non disponible" de "mauvais arguments".

---

## 5. SEC-BUG-04 — Ed25519 local dans `exo_shield` (violation SRV-02)

### Observation

`servers/exo_shield/src/signatures/update.rs` contient une implémentation complète d'Ed25519 from-scratch : arithmétique de champ Curve25519, scalaire (L = 2^252 + ...), compression/décompression de points, hash SHA-512 simplifié. Le commentaire à la ligne 562 l'admet explicitement :

```
// En production, cela serait remplacé par le vrai SHA-512 via le crypto_server.
```

Cette implémentation from-scratch est un risque en soi : les erreurs dans l'arithmétique de champ Ed25519 sont connues pour être subtiles et exploitables (signature malléabilité, forgeabilité via edge cases des points).

### Cause racine (voir SEC-BUG-03)

`exo_shield` a besoin de vérifier les signatures Ed25519 sur les mises à jour de bases de signatures. `crypto_server` ne propose pas `CRYPTO_VERIFY`. La seule option disponible était l'implémentation locale.

### Correction — dépendante de SEC-BUG-03

Une fois `CRYPTO_VERIFY` câblé dans `crypto_server`, remplacer dans `update.rs` :

```rust
// Avant (from-scratch) :
fn verify_signature(
    public_key: &[u8; ED25519_PUBLIC_KEY_SIZE],
    message: &[u8],
    signature: &[u8; ED25519_SIGNATURE_SIZE],
) -> bool {
    let msg_hash = hash_message(message);  // SHA-512 simplifié local
    ed25519_verify_internal(public_key, &msg_hash, signature)  // arithmétique locale
}

// Après (délégation crypto_server) :
fn verify_signature_via_crypto_server(
    public_key: &[u8; 32],
    message: &[u8; 32],   // ou hash du message
    signature: &[u8; 64],
) -> bool {
    let mut req = CryptoRequest {
        sender_pid: self_pid(),
        msg_type: CRYPTO_VERIFY,
        cap_token_raw: current_cap_token().to_bytes(),
        payload: [0u8; 96],
    };
    req.payload[0..32].copy_from_slice(public_key);
    req.payload[32..96].copy_from_slice(signature);
    // message dans payload[96..] — nécessite d'agrandir le payload ou pré-hasher

    let reply = ipc_send_recv(PID_CRYPTO_SERVER, &req);
    reply.status == CRYPTO_OK
}
```

**Suppression** : une fois la délégation en place, supprimer de `update.rs` :
- L'intégralité de la section `ARITHMÉTIQUE DE CHAMP ED25519`
- `fn hash_message()` (SHA-512 simplifié)
- `fn ed25519_verify_internal()`
- Les imports associés

---

## 6. SEC-BUG-05 — xchacha20 : aucune API de reseed post-Phoenix

### Observation complète du code

`servers/crypto_server/src/xchacha20.rs` construit les nonces comme suit :

```
nonce[0..8]  = NONCE_COUNTER  (AtomicU64, incrémenté par fetch_add)
nonce[8..16] = NONCE_SALT_LO  (AtomicU64, initialisé une fois par xchacha20_init())
nonce[16..24] = NONCE_SALT_HI (AtomicU64, initialisé une fois par xchacha20_init())
```

Il n'existe **aucune fonction** `reseed_rng()`, `reset_salt()`, ou équivalent. `xchacha20_init()` est la seule fonction qui écrit dans `NONCE_SALT_*`, et elle n'est appelée qu'une fois dans `_start()`.

### Analyse du risque post-Phoenix

Après un handoff ExoPhoenix (Kernel B prend la main, forge une image propre, restaure Kernel A) :

1. `NONCE_COUNTER` reprend là où il s'était arrêté — **correct** : les nonces futurs sont plus grands que les nonces passés.
2. `NONCE_SALT_*` reste identique à avant le handoff — **problème** : le sel est le même que celui utilisé pour toutes les sessions établies avant la forge.

Le risque concret dépend du comportement de la forge :
- Si la forge invalide toutes les sessions TLS actives, les nonces pre-forge ne seront plus utilisés avec les mêmes clés. Le risque est faible.
- Si certaines sessions persistent à travers la forge (cas non clairement documenté), le sel identique + des clés potentiellement restaurées = espace de nonces partiellement partagé avec la période pre-forge.

La spec (`GI-05`) demande un reseed **explicite** avant tout IPC post-restore précisément pour éliminer ce doute sans avoir à raisonner sur l'état des sessions.

### Correction — en deux parties

**Partie 1 — `xchacha20.rs` : exposer une API de reseed**

```rust
/// Reseed post-Phoenix : nouveau sel + bump compteur.
///
/// Doit être appelé après chaque restore ExoPhoenix, avant tout chiffrement.
/// `entropy` est fourni par Kernel B (double RDRAND).
///
/// Le compteur est BUMPED (jamais remis à zéro) pour garantir
/// la non-réutilisation avec les nonces pre-forge.
pub fn xchacha20_reseed(entropy: u64) {
    // Nouveau sel = entropy XOR sel actuel (mixing, pas remplacement pur)
    // Cela garantit qu'un entropy prévisible ne peut pas annuler le sel
    let old_lo = NONCE_SALT_LO.load(Ordering::Relaxed);
    let old_hi = NONCE_SALT_HI.load(Ordering::Relaxed);
    let new_lo = old_lo ^ entropy ^ NONCE_COUNTER.load(Ordering::Relaxed);
    let new_hi = old_hi ^ entropy.rotate_left(17) ^ NONCE_COUNTER.load(Ordering::Relaxed).rotate_left(31);

    // Bump le compteur d'abord — rend les nonces avec l'ancien sel non répétables
    NONCE_COUNTER.fetch_add(1_000_000, Ordering::Release); // espace de réservation

    // Mise à jour du sel
    NONCE_SALT_LO.store(new_lo, Ordering::Release);
    NONCE_SALT_HI.store(new_hi, Ordering::Release);

    // Fence : tout chiffrement ultérieur verra le nouveau sel
    core::sync::atomic::fence(Ordering::SeqCst);
}
```

**Partie 2 — `main.rs` : ajouter le type de message `PHOENIX_WAKE_ENTROPY`**

```rust
const PHOENIX_WAKE_ENTROPY: u32 = 255; // Type réservé kernel B → crypto_server

// Dans handle_request() :
PHOENIX_WAKE_ENTROPY => {
    // Vérifier que l'émetteur est bien Kernel B (PID 0 ou PID réservé)
    if req.sender_pid != PID_KERNEL_B {
        reply.status = CRYPTO_ERR_CAP;
        return reply;
    }
    let entropy = u64::from_le_bytes(req.payload[0..8].try_into().unwrap_or_default());
    xchacha20::xchacha20_reseed(entropy);
    keystore::revoke_all_pre_phoenix(); // invalider sessions pre-forge
    reply.status = CRYPTO_OK;
}
```

**Partie 3 — `kernel/src/exophoenix/handoff.rs` : émettre l'événement**

```rust
// Après restore confirmé (HANDOFF_FLAG retourne à NORMAL), avant de relâcher A :
let entropy: u64 = unsafe {
    let mut v: u64;
    core::arch::asm!("rdrand {}", out(reg) v, options(nostack, nomem));
    let mut v2: u64;
    core::arch::asm!("rdrand {}", out(reg) v2, options(nostack, nomem));
    v ^ v2
};
// IPC vers crypto_server (PID 4) avec msg_type = PHOENIX_WAKE_ENTROPY
ipc_send_kernel_b(PID_CRYPTO_SERVER, PHOENIX_WAKE_ENTROPY, &entropy.to_le_bytes())?;
```

---

## 7. SEC-NOTE-01 — `security_init()` : séquence 3 phases non documentée

### Observation

Le commentaire de `mod.rs` (ligne 22) décrit `security_init()` comme la séquence d'initialisation v7. Ce qu'il ne dit pas : `exoseal_boot_phase0()` est appelé **avant** `security_init()` depuis le code de boot, et `exoseal_boot_complete()` est appelé **à l'intérieur** de `security_init()` comme dernière étape.

La vraie séquence d'appel depuis le point de vue du code de boot est :

```
arch::boot::kernel_main()
  │
  ├─ exoseal::exoseal_boot_phase0()    ← avant security_init()
  │    CET global, PKS default-deny, watchdog 500ms, IOMMU NIC
  │
  ├─ security::security_init(entropy, phys_base)
  │    integrity → capability → crypto → isolation →
  │    mitigations → audit → access_control →
  │    exoledger → exokairos → exoargos → exonmi → exocage →
  │    └─ exoseal::exoseal_boot_complete()
  │         PKS restore, SECURITY_READY ← true, watchdog 50ms
  │
  └─ (APs débloqués par SECURITY_READY)
```

Ce n'est pas un bug — la logique est correcte. Mais quelqu'un qui lit `mod.rs` sans suivre le code de boot ne voit pas `exoseal_boot_phase0()`. Cela peut amener à croire que CET et PKS ne sont activés qu'à l'étape 0 de `security_init()`, alors qu'ils le sont avant.

### Correction — documentation uniquement

Mettre à jour le commentaire d'entête de `mod.rs` :

```rust
// Ordre réel de boot sécurité (depuis arch/boot/main.rs) :
//
//   [AVANT security_init()] exoseal_boot_phase0()
//     → IOMMU NIC lock, CET global, PKS default-deny, watchdog 500ms
//
//   [security_init(kaslr_entropy, phys_base)]
//     1. integrity_check::integrity_init()
//     2. capability::init_capability_subsystem()
//     3. zero_trust (lazy)
//     4. crypto_init()
//     5. isolation (lazy)
//     6. mitigations_init(kaslr_entropy, phys_base)
//     7. audit_init()
//     8. access_control::init()
//     9. exoledger::exo_ledger_init()
//    10. exokairos::init_kernel_secret()
//    11. exoargos::exoargos_init()
//    12. exonmi::exonmi_init()
//    12b. exocage::enable_cet_for_thread(BSP)
//    13. exoseal_boot_complete()
//        → PKS restore, SECURITY_READY ← true (Release), watchdog 50ms
//
// RÈGLE SEC-INIT-01 : aucun sous-système ne doit être utilisé avant security_init().
// RÈGLE SEC-INIT-02 : integrity_init() est le premier sous-système (avant IRQs).
```

---

## 8. SEC-NOTE-02 — Collision de nomenclature : ExoShield kernel vs ExoShield serveur

### Le problème

Le terme "ExoShield" désigne deux composants distincts dans la base de code :

**ExoShield v1.0 (couche kernel)** — `kernel/src/security/` :
- Modules : `exoseal`, `exocage`, `exoveil`, `exoledger`, `exokairos`, `exoargos`, `exonmi`
- Rôle : invariants de boot, hardware enforcement (CET, PKS, IOMMU), dual-kernel observer
- Cycle de vie : actif de `exoseal_boot_phase0()` à la fin du runtime
- Document de référence : `ExoShield_v1_Production.md`
- Modèles TLA+ : `ExoShield.tla`, `ExoShield_v1.tla`

**ExoShield serveur (Ring 1)** — `servers/exo_shield/` :
- Modules : `engine`, `behavioral`, `network`, `sandbox`, `forensics`, `signatures`, `ipc_gate`
- Rôle : containment applicatif, détection d'anomalies processus, forensics, sandboxing
- Cycle de vie : démarre après `SECURITY_READY`, PID 10
- Document de référence : **aucun** (c'est le problème)
- Modèles TLA+ : aucun correspondant direct

Les modèles TLA+ existants (`ExoShield.tla`, `ExoShield_v1.tla`) modélisent la **couche kernel**, pas le serveur. Les propriétés `BootSafety`, `IommuEnforced`, `BudgetMonotonicity` sont des propriétés de la couche kernel. Elles ne couvrent pas le scan de processus, le forensics, ou le sandboxing applicatif du serveur.

### Correction — documentation + portée

**Action 1 : Créer `ExoShield_Server_v1.md`** — document de spécification du serveur PID 10, distinct de `ExoShield_v1_Production.md`. Ce document doit décrire :
- Le rôle exact du serveur (containment runtime, pas boot hardware)
- Son protocole IPC (les 6 types de messages : SCAN, EVENT, QUARANTINE, THREAT_QUERY, POLICY_UPDATE, HEARTBEAT)
- Sa dépendance à `crypto_server` pour toute vérification de signature
- Sa relation avec la couche kernel ExoShield (le serveur est un observateur de Ring 1, pas un remplaçant de la couche kernel)

**Action 2 : Clarifier la nomenclature dans `mod.rs`**

Ajouter dans le commentaire d'entête de `kernel/src/security/mod.rs` :

```rust
// NOTE NOMENCLATURE :
//   Ce module = "ExoShield v1.0 kernel layer" — invariants hardware + boot.
//   Ne pas confondre avec servers/exo_shield/ = serveur Ring 1 PID 10
//   de containment applicatif, qui est un composant DISTINCT avec un rôle DISTINCT.
//   Voir ExoShield_Server_v1.md pour la spec du serveur Ring 1.
```

**Action 3 (recommander) : renommer le serveur**

Pour éliminer définitivement la confusion, le serveur PID 10 pourrait être renommé `exo_sentinel` ou `exo_guard` pour le distinguer de la couche kernel ExoShield. Ce renommage est invasif (tous les appelants IPC, les logs, les configs) — à décider en fonction du budget.

---

## 9. Tableau de priorité et plan d'action

### Ordre d'exécution recommandé

Les bugs forment un graphe de dépendances. Respecter cet ordre évite les régressions :

```
SEC-BUG-03 (CRYPTO_SIGN/VERIFY câblés)
    ↓
SEC-BUG-04 (retirer Ed25519 local exo_shield)
    ↓
SEC-BUG-02 (wirer keystore.rs dans main.rs)
    ↓
SEC-BUG-01 (CAP-01 — nécessite keystore.rs pour le shredding correct)
    ↓
SEC-BUG-05 (reseed — nécessite la boucle IPC complète pour PHOENIX_WAKE_ENTROPY)
```

`SEC-NOTE-01` et `SEC-NOTE-02` sont indépendants et peuvent être traités à tout moment.

### Tableau récapitulatif

| Réf. | Gravité | Effort | Dépend de | Effet |
|---|---|---|---|---|
| SEC-BUG-03 | P0 | 3-4h | — | Débloque SEC-BUG-04, complète l'API IPC |
| SEC-BUG-04 | P0 | 1-2h | SEC-BUG-03 | Élimine Ed25519 from-scratch dans exo_shield |
| SEC-BUG-02 | P0 | 2h | — | Active TTL, rotation, shredding DoD |
| SEC-BUG-01 | P0 | 4-6h | SEC-BUG-02 | Vraie vérification CAP-01, protocole IPC v2 |
| SEC-BUG-05 | P0 | 3h | — | Reseed post-Phoenix, correctness nonces |
| SEC-NOTE-01 | P1 | 30min | — | Documentation uniquement |
| SEC-NOTE-02 | P1 | 2h | — | Création ExoShield_Server_v1.md |

**Effort total estimé** : 15-20 heures d'implémentation + tests.

---

## 10. Ce qui est correct et ne doit pas être touché

Pour éviter la sur-correction :

- `kernel/src/security/crypto/*` — conforme à l'architecture. Les primitives Ring 0 (blake3, xchacha20_poly1305, ed25519, rng, kdf) sont légitimes à ce niveau. SRV-02 ne s'applique pas au Ring 0.
- `SECURITY_READY` — implémentation `AtomicBool` Release/Acquire correcte, TLA conforme.
- `exoseal_boot_complete()` — logique de boot inversé correcte, `verify_p0_fixes()` bien câblé.
- `exoveil.rs` / `exocage.rs` / `exoledger.rs` / `exokairos.rs` — alignés avec la spec.
- Le nonce counter monotone de `xchacha20.rs` — la construction counter+sel est correcte pour la non-réutilisation ordinaire. Seul le reseed post-Phoenix manque.
- Le filtrage `owner_pid` dans `ks_get()` — c'est un contrôle de paternité de handle valide, complémentaire à CAP-01 (pas un remplacement).
- `ipc_gate/` dans `exo_shield` — la couche de politique IPC est architecturalement juste.

---

*Corrections ExoOS — Sécurité & Crypto — Avril 2026*  
*Analyse directe des sources — `kernel/src/security/*`, `servers/crypto_server/*`, `servers/exo_shield/src/signatures/`*