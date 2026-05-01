--- docs/recast/ANALYSE_APPROFONDIE_DERIVE_DOC_CODE.md (原始)


+++ docs/recast/ANALYSE_APPROFONDIE_DERIVE_DOC_CODE.md (修改后)
# Analyse Approfondie — Dérive Documentation/Code ExoPhoenix & ExoShield

**Date** : 30 avril 2026
**Auteur** : Audit IA
**Périmètre** : `docs/recast/ExoPhoenix_Spec_v6.md`, `docs/recast/ExoShield_v1_Production.md`, `libs/exo-phoenix-ssr`, `kernel/src/exophoenix/*`, `servers/exo_shield/*`, `servers/crypto_server/*`

---

## Résumé Exécutif

Le rapport d'audit initial a correctement identifié **7 écarts majeurs** entre la documentation (specs v6) et le code actif. Cette analyse approfondie confirme ces écarts, en précise les causes racines, et propose un **plan de correction optimal en 4 phases** pour réaligner la documentation et le code.

### État des lieux confirmé

| Module | Statut doc | Statut code | Écart | Gravité |
|--------|------------|-------------|-------|---------|
| **ExoPhoenix SSR Layout** | v6 : MAX_CORES=64, offsets figés | Lib : MAX_CORES=256, offsets décalés | **Majeur** | 🔴 Bloquant SMP >64 |
| **ExoPhoenix Runtime** | Spec v6 unifiée | Kernel : hard-cap `slot >= 64` dans 3 fichiers | **Majeur** | 🔴 Incohérence runtime |
| **ExoPhoenix Reseed** | GI-05 exige `PhoenixWakeEntropy` post-restore | Aucun symbole ni appel visible | **Majeur** | 🔴 Sécurité crypto compromise |
| **ExoShield Boot** | Spec = architecture boot/invariants | Serveur PID 10 = confinement process | **Majeur** | 🟠 Confusion nomenclature |
| **ExoShield Crypto** | SRV-02/04 : tout crypto via `crypto_server` | `exo_shield/signatures/update.rs` implémente Ed25519/sha local | **Majeur** | 🔴 Violation architecture |
| **Crypto CAP-01** | Commenté dans `crypto_server/main.rs` | Aucune vérification effective visible | **Moyen** | 🟠 À vérifier |
| **Noyau Sécurité** | Aligné TLA + docs | Cohérent avec invariants boot | **OK** | ✅ |

---

## 1. ExoPhoenix — Dérive SSR Layout (MAX_CORES 64 → 256)

### Constat

**Documentation (`ExoPhoenix_Spec_v6.md`)** :
```rust
pub const MAX_CORES: usize = 64;
pub const SSR_FREEZE_ACK:    usize = 0x0080;  // 64 bytes × MAX_CORES
pub const SSR_PMC_SNAPSHOT:  usize = 0x1080;  // 64 bytes × MAX_CORES
pub const SSR_LOG_AUDIT:     usize = 0x8000;
pub const SSR_METRICS_PUSH:  usize = 0xC000;

pub fn freeze_ack_offset(apic_id: usize) -> usize {
    SSR_FREEZE_ACK + apic_id * 64  // 1 AtomicU64 + 56 padding
}
```

**Code (`libs/exo-phoenix-ssr/src/lib.rs`)** :
```rust
pub const SSR_MAX_CORES_LAYOUT: usize = 256;  // CORR-02
pub const SSR_FREEZE_ACK_OFFSET: usize = 0x0080;
pub const SSR_PMC_OFFSET:        usize = 0x4080;  // ≠ 0x1080
pub const SSR_LOG_AUDIT_OFFSET:  usize = 0xC000;  // ≠ 0x8000
pub const SSR_METRICS_OFFSET:    usize = 0xE000;  // ≠ 0xC000

pub const fn freeze_ack_offset(apic_id: u32) -> usize {
    SSR_FREEZE_ACK_OFFSET + apic_id as usize * 4  // u32, pas 64 bytes
}
```

### Causes racines

1. **CORR-02 non répercutée dans la doc** : La lib SSR a été mise à jour pour supporter 256 cœurs (décision architecturale valide), mais `ExoPhoenix_Spec_v6.md` est restée à 64.
2. **Changement de format ACK** : La spec v6 impose 64 bytes par ACK (1 cache line, anti-false-sharing), le code utilise 4 bytes (u32 atomique). Les deux sont valides, mais incompatibles.
3. **Offsets décalés** : Le passage de 64→256 cœurs a nécessité un recalcul des offsets PMC (0x1080→0x4080) et log/metrics (0x8000/0xC000→0xC000/0xE000).

### Impact

- **Silent corruption potentielle** : Si un développeur suit la spec v6 pour écrire du code kernel, les accès SSR seront à des offsets incorrects.
- **SMP >64 partiellement cassé** : La lib supporte 256 cœurs, mais le kernel limite encore à 64 dans plusieurs modules.

### Solution optimale

**Option A (recommandée)** : Mettre à jour la documentation pour refléter le code actuel.
- **Pourquoi** : Le code SSR à 256 cœurs est plus robuste, mieux testé, et les offsets actuels sont cohérents avec la lib partagée.
- **Action** : Créer `ExoPhoenix_Spec_v7.md` qui acte MAX_CORES=256, freeze_ack=u32, et les nouveaux offsets.

**Option B** : Revenir à la spec v6.
- **Pourquoi** : Si l'objectif est de limiter volontairement à 64 cœurs pour des raisons de validation formelle TLA+.
- **Coût** : Nécessite de modifier la lib partagée et tous les kernels qui l'utilisent → risque élevé.

**Recommandation** : **Option A**. La décision 256 cœurs est raisonnable, et le code est déjà aligné dessus.

---

## 2. ExoPhoenix — Hard-cap Runtime `slot >= 64`

### Constat

Fichiers concernés :
- `kernel/src/exophoenix/isolate.rs:54` : `if Some(slot) == self_slot || slot >= 64 { continue; }`
- `kernel/src/exophoenix/handoff.rs:112,138` : `if slot >= 64 { return/false; }`
- `kernel/src/exophoenix/forge.rs:314` : `if Some(slot) == self_slot || slot >= 64 { continue; }`

### Impact

Même si la lib SSR supporte 256 cœurs, **tout cœur avec un slot ≥64 est ignoré** :
- Pas de freeze ACK attendu
- Pas de TLB shootdown
- Pas de reconstruction Forge

Sur un système à 128 cœurs, les cœurs 64-127 ne seraient pas isolés correctement.

### Solution optimale

**Remplacer `slot >= 64` par `slot >= exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT`** dans les 3 fichiers.

```rust
// Avant
if Some(slot) == self_slot || slot >= 64 {
    continue;
}

// Après
if Some(slot) == self_slot || slot >= exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT {
    continue;
}
```

**Note** : `stage0.rs` appelle déjà `init_core_count(n_cores.min(SSR_MAX_CORES_LAYOUT))`, donc la borne runtime est correcte. Il faut juste harmoniser les gardes dans `isolate`, `handoff`, `forge`.

---

## 3. ExoPhoenix/Crypto — Reseed Post-Restore Non Câblé

### Constat

**Documentation (`GI-05_ExoPhoenix.md`)** :
> "Après un restore ExoPhoenix, Kernel A DOIT envoyer un message `PhoenixWakeEntropy` au `crypto_server` avant tout autre IPC. Ce message déclenche un reseed du nonce compteur XChaCha20."

**Code** :
- Aucun symbole `PhoenixWakeEntropy` trouvé dans `kernel/src/exophoenix/*` ni `servers/crypto_server/*`.
- `crypto_server/src/xchacha20.rs` gère un nonce compteur + sel, mais aucun mécanisme de reseed externe n'est exposé.
- `kernel/src/security/crypto/rng.rs` a un `reseed()` interne, mais non déclenché par Phoenix.

### Impact

**Violation de l'invariant de non-réutilisation de nonce** :
- Si Kernel A est restauré depuis un snapshot, son état interne (compteurs, RNG) est identique à l'avant-freeze.
- Sans reseed explicite, les nonces XChaCha20 pourraient être réutilisés → compromission AEAD.

### Solution optimale

**Phase 1 — Ajouter un message IPC `PhoenixWakeEntropy`** :
```rust
// servers/crypto_server/src/main.rs
const CRYPTO_PHOENIX_RESEED: u32 = 12;  // Nouveau type

// Dans handle_request()
CRYPTO_PHOENIX_RESEED => {
    // Vérifier que l'appelant est le kernel (PID 0 ou capability spéciale)
    // Reseed le nonce compteur xchacha20 avec une nouvelle entropy
    xchacha20::force_reseed_from_entropy(&req.payload[..32]);
    reply.status = CRYPTO_OK;
}
```

**Phase 2 — Appeler ce message depuis `forge.rs` après reconstruction** :
```rust
// kernel/src/exophoenix/forge.rs, après reconstruct_kernel_a()
pub fn reconstruct_kernel_a() -> Result<(), ForgeError> {
    // ... reconstruction ...

    // NEW : Reseed crypto_server avant tout autre IPC
    phoenix_crypto_reseed();

    Ok(())
}

fn phoenix_crypto_reseed() {
    // Générer 32 bytes d'entropy (RDRAND + timestamp + state hash)
    let mut entropy = [0u8; 32];
    // ... remplir entropy ...

    // Envoyer CRYPTO_PHOENIX_RESEED au crypto_server
    let msg = PhoenixWakeEntropy { entropy };
    ipc_send_to_pid(CRYPTO_SERVER_PID, &msg);
}
```

---

## 4. ExoShield — Confusion Nomenclature (Boot vs Serveur)

### Constat

**Deux "ExoShield" distincts coexistent** :

1. **ExoShield v1.0 (doc)** : Architecture de boot/invariants matériels
   - Modules : ExoSeal, ExoCage, ExoVeil, ExoKairos, ExoCordon, ExoLedger, ExoArgos, ExoNmi
   - Implémenté dans : `kernel/src/security/*`
   - Rôle : Boot sûr, CET, PKS, IOMMU, audit, watchdog

2. **ExoShield serveur (code)** : PID 10 AI/Process Containment
   - Implémenté dans : `servers/exo_shield/*`
   - Rôle : Scan processus, détection anomalies, quarantaine, forensics

### Impact

- **Confusion pour les développeurs** : Quel "ExoShield" est référencé dans un ticket ?
- **TLA+ ambigu** : `ExoShield.tla` modélise l'architecture boot, pas le serveur PID 10.

### Solution optimale

**Renommer le serveur PID 10** :
- `servers/exo_shield` → `servers/process_guard` ou `servers/threat_monitor`
- `exo_shield` dans les IPC → `process_guard`
- Documentation : Clarifier que "ExoShield v1.0" = modules kernel uniquement.

**Alternative** : Garder le nom `exo_shield` pour le serveur, mais ajouter un préambule explicite dans `ExoShield_v1_Production.md` :
> "ExoShield v1.0 désigne l'architecture de sécurité kernel (ExoSeal..ExoNmi). Le serveur `exo_shield` (PID 10) est un composant distinct de confinement process, nommé ainsi par héritage historique."

---

## 5. ExoShield/Crypto — Ed25519 Local dans `signatures/update.rs`

### Constat

**Fichier** : `servers/exo_shield/src/signatures/update.rs`
- ~800 lignes d'arithmétique de champ Ed25519 from-scratch
- Hash simplifié (non SHA-512/Blake3)
- Commentaire ligne 562 : *"En production, cela serait remplacé par le vrai SHA-512 via le crypto_server."*

**Violation** :
- **SRV-02** : "Pas d'imports RustCrypto ailleurs que dans `crypto_server`"
- **SRV-04** : "Toutes les opérations crypto Ring 1 passent par `crypto_server`"

### Impact

- **Code non audité** : L'implémentation Ed25519 maison n'a pas été revue cryptographiquement.
- **Double emploi** : `crypto_server` a déjà Ed25519 (via crate `ed25519-dalek` ou équivalent).
- **Risque de faille** : Une erreur d'implémentation (ex: timing attack, réduction mod p incorrecte) compromettrait la vérification de signatures.

### Solution optimale

**Supprimer l'implémentation locale et déléguer à `crypto_server`** :

```rust
// servers/exo_shield/src/signatures/update.rs

// AVANT (supprimer ~800 lignes)
// struct Fe([u64; 4]); impl Fe { ... }
// struct Point { ... }
// fn ed25519_verify(...) { ... }

// APRÈS
use exo_ipc_client::{ipc_send_recv, Endpoint};

static CRYPTO_ENDPOINT: Endpoint = Endpoint::new("crypto_server");

pub fn verify_signature(pubkey: &[u8; 32], sig: &[u8; 64], msg: &[u8]) -> bool {
    let req = CryptoVerifyRequest {
        pubkey: *pubkey,
        signature: *sig,
        message_hash: blake3::hash(msg),  // Ou envoyer msg complet si petit
    };

    let rep = ipc_send_recv(&CRYPTO_ENDPOINT, &req, MSG_CRYPTO_VERIFY);
    rep.status == CRYPTO_OK
}
```

**Note** : Si la performance est un enjeu (vérifications fréquentes), envisager un cache de résultats de vérification dans `exo_shield`, mais **pas** d'implémentation crypto locale.

---

## 6. Crypto CAP-01 — Vérification de Capability

### Constat

**Commenté dans `crypto_server/src/main.rs:31`** :
> "CAP-01 : vérification de capability token en première instruction"

**Mais** :
- `_start()` → `ipc_register()` → boucle `ipc_recv()` → `handle_request()`
- Aucune appel à `verify_cap_token()` avant `handle_request()`
- `keystore.rs` mentionne CAP-01, mais c'est une vérification _interne_ (handle owner_pid), pas une capability IPC.

### Hypothèses

1. **La vérification est dans l'IPC broker** : `ipc_router` ou `ipc_broker` pourrait filtrer les appels avant qu'ils n'atteignent `crypto_server`.
2. **La vérification est manquante** : Le commentaire est un rappel de conception non implémenté.

### Investigation requise

```bash
grep -rn "verify_cap\|cap_token\|capability_check" servers/ipc_router/
grep -rn "CAP-01\|capability" servers/init_server/
```

### Solution optimale

**Si la vérification n'existe pas** :
```rust
// crypto_server/src/main.rs, dans _start() avant la boucle
// OU dans handle_request(), première ligne

fn verify_crypto_capability(sender_pid: u32) -> bool {
    // Option A : Vérifier via IPC vers capability_server
    // Option B : Table statique des PIDs autorisés (init, vfs, network, exo_shield)
    const ALLOWED_PIDS: &[u32] = &[1, 2, 3, 5, 10];
    ALLOWED_PIDS.contains(&sender_pid)
}

fn handle_request(req: &CryptoRequest) -> CryptoReply {
    if !verify_crypto_capability(req.sender_pid) {
        return CryptoReply {
            status: CRYPTO_ERR_AUTH,
            key_handle: 0,
            data: [0u8; 56],
        };
    }
    // ... suite normale
}
```

---

## Plan de Correction Optimal

### Phase 1 — Harmonisation ExoPhoenix (1-2 jours)

| Action | Fichier(s) | Effort | Priorité |
|--------|------------|--------|----------|
| Remplacer `slot >= 64` par `SSR_MAX_CORES_LAYOUT` | `isolate.rs`, `handoff.rs`, `forge.rs` | 30 min | 🔴 Haute |
| Créer `ExoPhoenix_Spec_v7.md` actant MAX_CORES=256, offsets actuels | `docs/recast/` | 2h | 🔴 Haute |
| Marquer `ExoPhoenix_Spec_v6.md` comme **OBSOLÈTE** | `docs/recast/ExoPhoenix_Spec_v6.md` (header) | 10 min | 🔴 Haute |

### Phase 2 — Reseed Post-Restore (2-3 jours)

| Action | Fichier(s) | Effort | Priorité |
|--------|------------|--------|----------|
| Ajouter `CRYPTO_PHOENIX_RESEED` dans `crypto_server` | `servers/crypto_server/src/main.rs`, `xchacha20.rs` | 4h | 🔴 Haute |
| Implémenter `phoenix_crypto_reseed()` dans `forge.rs` | `kernel/src/exophoenix/forge.rs` | 3h | 🔴 Haute |
| Tests QEMU : cycle freeze/restore + vérif nonces uniques | `tests/phoenix_restore.rs` | 4h | 🔴 Haute |

### Phase 3 — Nettoyage ExoShield Crypto (1-2 jours)

| Action | Fichier(s) | Effort | Priorité |
|--------|------------|--------|----------|
| Supprimer arithmétique Ed25519 locale | `servers/exo_shield/src/signatures/update.rs` | 1h | 🔴 Haute |
| Délégation vérif signatures → `crypto_server` | `servers/exo_shield/src/signatures/update.rs` | 3h | 🔴 Haute |
| Renommer ou clarifier nomenclature ExoShield | `docs/recast/ExoShield_v1_Production.md` + `servers/exo_shield/` | 2h | 🟠 Moyenne |

### Phase 4 — CAP-01 et Documentation (1 jour)

| Action | Fichier(s) | Effort | Priorité |
|--------|------------|--------|----------|
| Investiguer vérif capability dans `ipc_router` | `servers/ipc_router/*` | 2h | 🟠 Moyenne |
| Implémenter CAP-01 si manquant | `servers/crypto_server/src/main.rs` | 2h | 🟠 Moyenne |
| Mettre à jour index `ExoOS_Corrections_00_Master_Index.md` | `docs/recast/` | 1h | 🟢 Basse |

---

## Conclusion

Les écarts identifiés sont **réels et corrigibles**. La bonne nouvelle est que :

1. **Le code est globalement plus avancé que la doc** : SSR 256 cœurs, offsets optimisés, etc.
2. **Aucune refonte majeure n'est nécessaire** : Juste des ajustements ciblés et de la documentation à jour.
3. **Le noyau sécurité est sain** : `kernel/src/security/*` est aligné avec les TLA et les intentions architecturales.

**Recommandation immédiate** : Commencer par la **Phase 1** (harmonisation ExoPhoenix) car c'est la plus simple et elle débloque la cohérence globale. Ensuite, enchaîner sur la **Phase 2** (reseed) qui est critique pour la sécurité crypto.

---

**Annexe A — Fichiers à Modifier (Récapitulatif)**

```
kernel/src/exophoenix/isolate.rs       : ligne 54 (slot >= 64)
kernel/src/exophoenix/handoff.rs       : lignes 112, 138 (slot >= 64)
kernel/src/exophoenix/forge.rs         : ligne 314 (slot >= 64)
kernel/src/exophoenix/forge.rs         : ajouter phoenix_crypto_reseed()
servers/crypto_server/src/main.rs      : ajouter CRYPTO_PHOENIX_RESEED
servers/crypto_server/src/xchacha20.rs : ajouter force_reseed_from_entropy()
servers/exo_shield/src/signatures/update.rs : supprimer Ed25519 local, délégation crypto_server
docs/recast/ExoPhoenix_Spec_v6.md      : header "OBSOLÈTE → voir v7"
docs/recast/ExoPhoenix_Spec_v7.md      : NOUVEAU (MAX_CORES=256, offsets actuels)
docs/recast/ExoShield_v1_Production.md : clarification nomenclature
```

**Annexe B — Commandes de Vérification Post-Correction**

```bash
# Vérifier qu'il n'y a plus de hard-cap 64
grep -rn "slot >= 64" kernel/src/exophoenix/  # Doit être vide

# Vérifier que PhoenixWakeEntropy existe
grep -rn "PhoenixWakeEntropy\|phoenix_crypto_reseed" kernel/src/exophoenix/ servers/crypto_server/

# Vérifier que exo_shield ne fait plus de crypto locale
grep -rn "Fe\|Point\|ed25519_d()" servers/exo_shield/src/signatures/  # Doit être vide
```