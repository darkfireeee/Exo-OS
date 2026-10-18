# AUDIT SÉCURITÉ & CRYPTO — Objectif 100% RÉEL

> Date : 2026-06-16
> Périmètre : kernel `security/`, `fs/exofs/crypto`, `syscall/` (table + dispatch),
> `servers/crypto_server`, `servers/exo_shield` (ML NGAV).
> Méthode : audit manuel ligne à ligne + traçage des chemins d'appel réels (pas
> seulement l'existence du code). Principe directeur : **« la promesse d'une
> fausse sécurité est pire que l'absence de sécurité »** — on distingue
> *fonctionnalité réelle* de *fonctionnalité partielle / théâtre*.

## Légende sévérité

| Niveau | Sens |
|--------|------|
| **CRITICAL** | Fausse sécurité : une garantie annoncée n'est PAS appliquée sur le chemin réel. |
| **HIGH** | Faille exploitable ou modèle de sécurité significativement affaibli. |
| **MEDIUM** | Durcissement requis ; exploitable sous conditions. |
| **LOW** | Non-idiomatique / dette ; sûr aujourd'hui mais fragile. |
| **INFO** | Limitation connue, comportement par conception. |
| **RESOLVED** | Corrigé pendant cette passe d'audit. |

---

## TABLEAU DE SYNTHÈSE

| ID | Sévérité | Sujet | Fichier | État |
|----|----------|-------|---------|------|
| F1 | CRITICAL | Chiffrement-at-rest ExoFS NON câblé sur le chemin réel | `fs/exofs/syscall/object_store.rs:543` | **RESOLVED (mécanisme + outillage)** — kernel lit/déchiffre, `mkfs --encrypt` crée des volumes chiffrés, `unlock_encrypted_volume()` ; reste auto-unlock boot (source passphrase) + QEMU #25 |
| F2 | HIGH | Clé de chiffrement blob dérivée du BlobId **public** | `fs/exofs/storage/blob_writer.rs:502` | **RESOLVED** (requiert secret de volume) |
| F3 | HIGH | Loader de poids du MLP profond **non authentifié** | `servers/exo_shield/src/ml/mlp.rs:215` | **RESOLVED** (checksum+version) |
| F4 | HIGH | Modèles ML **non entraînés** (poids LCG seedés) | `servers/exo_shield/src/ml/{mlp,iforest}.rs` | **RESOLVED** (poids entraînés) |
| F10 | **CRITICAL** | MLP **inerte** : features brutes [0,99] interprétées comme ~0 en Q16.16 | `servers/exo_shield/src/ml/{mlp,ensemble}.rs` | **RESOLVED** (normalisation) |
| F5 | HIGH | Clés Ed25519 = test vectors RFC 8032 (privées publiques) | `security/integrity_check/code_signing.rs:32` | **RESOLVED** (paire dev réelle) |
| F6 | MEDIUM | Secure Boot désactivable + exec non-bloquant en dev | `security/integrity_check/secure_boot.rs:214` | **RESOLVED** (gate + report testable) |
| F7 | LOW | Wrapping clé maître/volume = XOR+HMAC (pas AES-KW) | `fs/exofs/crypto/master_key.rs:202` | **RESOLVED** (XChaCha20-Poly1305 AEAD) |
| F8 | INFO | PKI Root CA éphémère (clé régénérée chaque boot) | `servers/crypto_server/src/pki.rs:658` | DIFFÉRÉ (persistance) |
| F9 | INFO | KERNEL_SECRET per-boot (OK pour MAC, INTERDIT pour at-rest) | `security/exokairos.rs:695` | NOTE D'ARCHITECTURE |

### Déjà corrigés dans cette passe (RESOLVED)

| ID | Sujet | Fichier |
|----|-------|---------|
| R1 | `sys_reboot()` sans vérif PID → DoS | `syscall/table.rs:1555` |
| R2 | Page fault Segfault non remonté au NGAV | `arch/x86_64/exceptions.rs:980` |
| R3 | `do_execve()` sans trace ExoLedger | `process/lifecycle/exec.rs` |
| R4 | `do_fork()` audit ring-buffer seul (pas ExoLedger) | `process/lifecycle/fork.rs:658` |
| R5 | ExoFS read/write sans Zero-Trust MLS | `fs/exofs/syscall/object_{read,write}.rs` |
| R6 | shield_feed non câblé depuis verify_access (P1 audit précédent) | `security/zero_trust/verify.rs` |
| R7 | F5 — clés Ed25519 RFC TV remplacées par paire dev réelle (privée gitignored) | `security/integrity_check/code_signing.rs:32` |
| R8 | F4/F10 — MLP entraîné (recall malicious 1.0, FP 0.0) + normalisation features | `servers/exo_shield/src/ml/{ensemble,mlp,trained_weights}.rs` |
| R9 | F3 — loader MLP authentifié (checksum FNV-1a + version monotone) | `servers/exo_shield/src/ml/mlp.rs` |
| R10 | F4 — Isolation Forest entraîné (seuils fittés) + loader | `servers/exo_shield/src/ml/iforest.rs` |
| R11 | F2 — clé blob dérivée d'un secret de volume (refus si absent) + module volume_secret | `fs/exofs/crypto/volume_secret.rs`, `fs/exofs/storage/blob_writer.rs` |
| R12 | F1 — chiffrement-at-rest : provider KEK + wrap/unwrap VK + cipher de bloc + câblage persist/load gated + stockage superblock + déverrouillage montage (testé e2e) | `fs/exofs/crypto/at_rest.rs`, `fs/exofs/syscall/object_store.rs`, `fs/exofs/storage/superblock.rs`, `security/crypto/xchacha20_poly1305.rs` |
| R13 | F7 — wrap clé maître/volume migré XOR+HMAC → XChaCha20-Poly1305 AEAD ; **SHA-256/HMAC bespoke supprimés** (~115 lignes) | `fs/exofs/crypto/master_key.rs`, `fs/exofs/crypto/volume_key.rs` |
| R14 | F6 — `production_security_status()` + `warn_if_not_production_hardened()` + `secure_boot::enforcement_enabled()` (testable) | `security/mod.rs`, `security/integrity_check/secure_boot.rs` |
| R15 | F1-activation — crate partagé **exo-fscrypt** (source unique kernel↔mkfs, XChaCha20 u32 + AEAD BLAKE3 + Argon2id, testé RFC+roundtrip) ; kernel `at_rest` y délègue ; **`exofs-mkroot --encrypt --passphrase`** crée des volumes chiffrés (vérifié : flag ENCRYPTION + clé wrappée + plaintext absent du disque) ; `exofs::unlock_encrypted_volume(passphrase)` (hook montage) | `drivers/storage/fscrypt/`, `fs/exofs/crypto/at_rest.rs`, `tools/exofs_mkroot/`, `fs/exofs/mod.rs` |

---

## F1 — CRITICAL — Chiffrement-at-rest ExoFS NON appliqué sur le chemin réel

### Constat (vérifié par traçage des appels)

Le chemin d'écriture **réel** d'un fichier utilisateur est :

```
sys_exofs_object_write()                    [syscall/object_write.rs:220]
  └─ write_fd()                             [object_write.rs:191]
      └─ write_blob() (fn locale)           [object_write.rs:150]
          ├─ BLOB_CACHE.write_at()          (cache mémoire)
          └─ persist_cached_blob_if_disk()  [object_write.rs:91]
              └─ object_store::persist_blob_data_if_disk()  [object_store.rs:543]
                  └─ device.write_block(lba, &block)   ← ÉCRIT EN CLAIR
```

`persist_blob_data_if_disk` (object_store.rs:570-577) copie les octets bruts du
cache directement dans les blocs disque **sans aucun chiffrement**. La lecture
(`load_blob_data_if_available`, object_store.rs:591) relit les blocs bruts.

Le **pipeline de chiffrement existe** (`BlobWriter::write_blob` → `encrypt_payload`
→ XChaCha20-Poly1305, blob_writer.rs:489-513 ; et `ObjectWriter::write_object`,
object_writer.rs:595) **mais n'est appelé QUE depuis du code de test.**

> Vérification : `grep ObjectWriter::` et `grep BlobWriter::write_blob` sur tout
> `kernel/src` hors `tests/` ⇒ aucun appelant runtime. Les seuls appelants non-test
> de `BlobWriter::write_blob` sont dans `ObjectWriter`, lui-même appelé uniquement
> par les modules de test (`storage/mod.rs`, `object_reader.rs`, `tier_5_*`).

### Pourquoi c'est CRITICAL

La tâche #16 « EXOFS-CORE-2 : router écritures via BlobWriter (crypto/checksum) »
était marquée *complétée*, mais le routage **n'atteint pas le syscall réel**. Un
disque ExoFS volé est lisible en clair. La garantie de chiffrement-at-rest est
annoncée mais non tenue → fausse sécurité.

### Remédiation

Router `persist_blob_data_if_disk` (ou la couche writeback `commit_epoch`) à
travers le writer chiffrant, avec :
1. Une **clé de volume persistante** (voir F2/F9) — surtout PAS le `KERNEL_SECRET`
   per-boot, qui rendrait les blobs d'hier indéchiffrables.
2. Le chemin de **lecture** doit déchiffrer symétriquement.
3. Décision d'architecture requise : chiffrer au niveau bloc (transparent) vs au
   niveau blob (via BlobWriter). Le niveau bloc est plus simple à câbler sur le
   chemin existant.

> Si l'implémentation complète + vérifiée n'est pas atteignable immédiatement, la
> position honnête est de **gater** la fonctionnalité derrière un flag explicite
> (`exofs_encryption_at_rest`) **désactivé par défaut** et documenté comme tel,
> plutôt que de laisser croire qu'elle est active.

### Chemin d'implémentation concret (vérifié dans le code)

Le superblock (`storage/superblock.rs`) possède DÉJÀ l'infrastructure :
- un flag `encryption activé sur le volume` (ligne 67),
- un `uuid: [u8; 16]` persistant (public — sert de tweak/sel, pas de secret),
- **`_pad1: [u8; 272]`** réservé → place pour une **clé de volume wrappée**.

Conception honnête :
1. À `mkfs` : générer une clé de volume aléatoire (CSPRNG), la **wrapper** avec une
   KEK dérivée d'une **passphrase** (Argon2id, déjà dispo) ou d'une clé scellée
   TPM/Secure Boot, et la stocker dans `_pad1`.
2. Au **mount** : déwrapper → clé de volume en RAM → `derive_object_key(vk, blob_id)`.
3. Router `persist_blob_data_if_disk` à travers le chiffrement (niveau bloc).

⚠️ **Décision d'architecture requise** : la source de la KEK (passphrase au boot /
TPM / Secure Boot sealed key). **Sans** cette source, le chiffrement-at-rest ne peut
qu'**obfusquer** (clé dérivable du disque) — ce qui serait F1/F2 déguisés. Tant que
la décision n'est pas prise, **gater + documenter** est la seule posture honnête.

### État actuel — MÉCANISME IMPLÉMENTÉ ET TESTÉ (R12)

Décision d'architecture retenue (« le mieux pour le projet et le futur ») : un
**provider de KEK abstrait** plutôt qu'un TPM figé (impossible à vérifier sans
pilote TIS, et le PCR bank actuel est simulé). Backend `Passphrase` implémenté ;
`TpmSealed`/`SecureBootSealed` = **points d'extension** prêts (renvoient
`NotSupported` — échec honnête). C'est le modèle LUKS2/clevis : on branche un TPM
plus tard sans rien casser.

**Implémenté + unit-testé :**
- `fs/exofs/crypto/at_rest.rs` — provider KEK (Argon2id), wrap/unwrap clé de volume
  (XChaCha20-Poly1305 **authentifié**), chiffrement de blob **longueur-préservant**
  (`xchacha20_xor` exposé depuis le site crypto unique). 10 tests.
- **Sûreté du cipher de flux** : justifiée car les blobs ExoFS sont **immuables et
  adressés par contenu** → une paire (clé, nonce) ne chiffre qu'un seul plaintext.
  Intégrité = checksum BLAKE3 de blob existant.
- **Câblage persist/load** (`object_store.rs`) — chiffre/déchiffre chaque bloc à son
  offset logique, **gated** sur `blob_at_rest_key()` (None si pas de volume chiffré
  → chemin en clair inchangé, **zéro régression**). Test `persist_then_reload_roundtrip`
  passe AVEC chiffrement actif (clé de volume présente en `cfg(test)`).
- **Stockage superblock** (`superblock.rs`) — clé de volume wrappée dans `_pad1`
  (110 B), flag `ENCRYPTION`, checksum recalculé. Test de round-trip persistance +
  déwrap.
- **Déverrouillage au montage** — `at_rest::install_volume_key_from_wrapped(wrapped,
  passphrase)` déwrappe et installe la clé.

**Reste pour ACTIVER sur un vrai volume (déploiement, hors chemin par défaut) :**
1. `mkfs` (exofs-mkroot) : générer VK aléatoire, `wrap_volume_key`, `set_wrapped_volume_key`.
2. Boot : fournir la passphrase (cmdline `exofs.key=` / scellé) à `install_volume_key_from_wrapped`
   au montage si `superblock.is_encrypted()`.
3. Vérification end-to-end sous QEMU une fois le boot réparé (#25).

> Posture honnête : le chiffrement est **réel et vérifié** au niveau mécanisme ; il
> est **inactif par défaut** (aucun volume chiffré monté) — et c'est documenté comme
> tel, pas présenté comme actif. Aucun risque pour les volumes existants.

---

## F2 — HIGH — Clé de chiffrement blob dérivée du BlobId public

### Constat

```rust
// fs/exofs/storage/blob_writer.rs:502
fn derive_blob_payload_key(blob_id: &BlobId) -> ExofsResult<[u8; 32]> {
    let dk = KeyDerivation::derive_key(
        &blob_id.0,                          // ← BlobId = Blake3(contenu), PUBLIC
        b"exofs-blob-payload-salt-v1",       // sel constant
        b"exofs-blob-payload-key-v1",        // contexte constant
    )?;
    Ok(*dk.as_bytes())
}
```

`blob_id.0` est le hash Blake3 du **contenu** (HASH-02). Il est déductible par
quiconque connaît le contenu OU le chemin (→ BlobId via PathIndex). Le sel et le
contexte sont constants. **Donc la clé de déchiffrement est calculable sans aucun
secret.** Là où le chiffrement EST utilisé (export, snapshot, tests), il est
cosmétique.

### Remédiation — APPLIQUÉE (R11)

`derive_blob_payload_key` (blob_writer.rs) incorpore désormais un **secret de
volume** :

```rust
let vk = crate::fs::exofs::crypto::volume_secret::volume_key()
    .ok_or(ExofsError::PermissionDenied)?;          // pas de clé → REFUS (pas de fausse sécu)
let dk = KeyDerivation::derive_key(&vk, &blob_id.0, b"exofs-blob-payload-key-v2")?;
```

Nouveau module `fs/exofs/crypto/volume_secret.rs` : détient la clé de volume
(`set_volume_key` au montage, `volume_key()` à l'usage). Sans clé installée, le
chiffrement **refuse** d'opérer au lieu de fabriquer une clé dérivable du BlobId.
En `cfg(test)`, une clé déterministe permet les round-trips. **L'attaquant
connaissant le BlobId ne peut plus reconstruire la clé sans `vk`.**

---

## F3 — HIGH — Loader de poids du MLP profond non authentifié

### Constat

Il existe **deux** modèles dans exo_shield :

| Module | Modèle | Update protégé ? |
|--------|--------|------------------|
| `ml/mlp.rs` (`MlpWeights`, 32→128→64→4) | **Celui utilisé par l'ensemble** (`mlp_infer`) | ❌ `mlp_update_weights()` : setter nu, aucune vérif |
| `ml/model.rs` + `ml/update.rs` (`ModelWeights`, 32×4) | Modèle superficiel | ✅ checksum + version + rollback (`ModelUpdateManager`) |

`mlp_update_weights(w: &MlpWeights)` (mlp.rs:215) écrase les poids du modèle
**réellement utilisé** sans checksum, sans version monotone, sans signature. Un
acteur capable d'invoquer ce chemin (ou de corrompre le payload de mise à jour)
peut **neutraliser le NGAV** (mettre tous les poids à 0 → tout devient bénin).

### Remédiation

Faire passer toute mise à jour du MLP profond par un payload authentifié,
calqué sur `update.rs` : version monotone + checksum + (idéalement) signature
Ed25519 vérifiée via `crypto_server`. Snapshot/rollback comme `ModelUpdateManager`.

---

## F4 — HIGH (fonctionnel) — Modèles ML non entraînés

### Constat

- **MLP** (`mlp.rs:90` `init_domain_seeded`) : poids générés par **LCG**
  (pseudo-aléatoire) + biais domaine codés à la main (neurones 0..16 poussés vers
  Malicious). Le *moteur* (forward leaky_relu/sigmoid) est réel et correct, mais
  les poids ne sont **pas appris sur des données**.
- **Isolation Forest** (`iforest.rs:93` `initialize`) : nœuds de split aléatoires
  (LCG) avec biais sur 8 features dangereuses. Calibration EMA en ligne, mais
  arbres non entraînés. **Aucun loader** de modèle entraîné (contrairement au MLP).
- **Markov** (`markov.rs`) : ordre-2, prior de Laplace, apprend **en ligne** — OK
  par conception (pas de pré-entraînement nécessaire).

### Pourquoi c'est HIGH (fonctionnel, pas exploit)

Le pipeline *s'exécute* et a un *signal domaine*, mais ce n'est pas un détecteur
entraîné. Annoncer « NGAV ML opérationnel à 100% » est trompeur tant que les poids
sont des heuristiques seedées. C'est de la fonctionnalité **partielle**.

### Remédiation

1. Script Python (`tools/ml_training/train_ngav.py`) :
   - Génère des données synthétiques bénin/malveillant alignées sur le schéma
     32-features (features.rs).
   - Entraîne un MLP **identique** à l'architecture noyau (32→128→64→4,
     leaky_relu + sigmoid) → export direct vers `MlpWeights` (Q16.16).
   - Entraîne une `IsolationForest` (sklearn) → export au format 8×63 SplitNode.
   - Construit la table Markov ordre-2 de référence (baseline « normal »).
2. Exporte un fichier Rust `trained_weights.rs` (constantes Q16.16) chargé via
   `mlp_update_weights` / nouveau `iforest_load` au boot.
3. **Le modèle final DEVRA être ré-entraîné** sur traces réelles Exo-OS
   (`profiler.rs` sous QEMU : bénin vs malveillant simulé). Le script synthétique
   ne produit qu'un **premier jeu fonctionnel** — documenté comme tel.

> Note : la proposition initiale mentionnait « GBDT + Isolation Forest ». Le noyau
> exécute un **MLP** (pas un GBDT), donc on entraîne le MLP pour chargement direct.
> Un GBDT ne mapperait sur aucun composant noyau → produirait des poids
> inutilisables = fausse fonctionnalité. On reste honnête : MLP + IF + Markov.

---

## F10 — CRITICAL — MLP inerte par incohérence d'échelle des features

### Constat (le plus profond de l'audit)

Au runtime (`main.rs:281` `behaviour_data_for_event`), **toutes** les features sont
clampées à `.min(99)` → vecteur d'entiers bruts dans `[0, ~180]`. Or le forward du
MLP (`mlp.rs:131`) fait `(input[i] * w_q16) >> 16`, ce qui **interprète l'entrée
comme du Q16.16**. Une feature brute de `50` est donc vue comme `50/65536 ≈ 0.0008`
— quasi nulle.

Conséquence : avec les poids seedés (±0.088), `h1 ≈ 0`, `h2 ≈ 0`, et la sortie
vaut `sigmoid(b3) ≈ 0.5` **quelle que soit l'entrée**. Le MLP — qui pèse **45%**
de l'ensemble — ne contribue presque rien. Vérifié arithmétiquement :
`(50 × 5800) >> 16 = 4` (Q16.16) ≈ `0.00006`.

C'est la pire « fausse fonctionnalité » de l'audit : le modèle phare *tourne*, ne
*plante* pas, passe ses tests de bornes — mais **ne classe rien**.

### Remédiation (FIX-F10, appliqué avec F4)

1. **Normaliser** les features brutes en `[0,65536]` (Q16.16 [0,1]) AVANT le MLP,
   via `FeatureVector::normalise_minmax(&[0;32], &FEATURE_MAX)` où `FEATURE_MAX` est
   exporté par l'entraînement Python (max runtime par feature).
2. Conserver les features **brutes** pour l'Isolation Forest (ses seuils u16 sont
   définis sur l'échelle brute 0..99) et Markov.
3. Entraîner le MLP sur la **même** distribution normalisée → cohérence parfaite
   entre entraînement (float) et inférence (Q16.16 quantifié).

> Sans F10, entraîner le MLP (F4) ne servirait à rien : il recevrait toujours des
> entrées ≈0. F10 + F4 doivent être corrigés ensemble.

---

## F5 — HIGH — Clés Ed25519 = test vectors RFC 8032

### Constat

```rust
// security/integrity_check/code_signing.rs:32
static MASTER_PUBLIC_KEY: [u8; 32] = [ /* RFC 8032 TV2 */ ];
static UPDATE_PUBLIC_KEY: [u8; 32] = [ /* RFC 8032 TV1 */ ];
```

Les deux clés publiques sont les Test Vectors 1 et 2 de la RFC 8032 — leurs clés
**privées sont publiées dans la RFC**. N'importe qui peut **forger** une signature
de module noyau valide. Un commentaire d'avertissement a été ajouté (passe
précédente) mais les clés ne sont pas remplacées.

### Remédiation

Générer une vraie paire offline, embarquer la clé **publique** réelle, conserver
la privée **hors-dépôt** (HSM / fichier gitignored pour le dev) :

```
openssl genpkey -algorithm ed25519 -out master.pem
openssl pkey -in master.pem -pubout -outform DER | tail -c 32 | xxd -i
```

Même une clé de dev (privée gitignored) est **strictement supérieure** aux TV RFC.

---

## F6 — MEDIUM — Secure Boot désactivable + exec non-bloquant en dev

### Constat

- `secure_boot::disable_enforcement()` (secure_boot.rs:214) existe.
- `check_chain_of_trust()` ne renvoie Err que si `SECBOOT_ENFORCE` (secure_boot.rs:204).
- `do_execve` (exec.rs:272) : binaire non signé = simple *warning* sauf si
  `feature = "strict_exec_signatures"`.

C'est un comportement de **dev** intentionnel, mais le risque est qu'un build de
prod parte sans `strict_exec_signatures` ni enforcement.

### Remédiation

Checklist de build prod (à documenter et idéalement asserter au boot) :
`--features strict_exec_signatures` + `SECBOOT_ENFORCE=1` + `EXOPHOENIX_REQUIRE_HASHES=1`.
Émettre un **avertissement bruyant** au boot si l'un manque.

---

## F7 — LOW — Wrapping clé maître/volume via XOR+HMAC

### Constat

`master_key.rs:202` et `volume_key.rs:195` : la clé est chiffrée par **XOR** avec
une KEK dérivée (Argon2id) + HMAC-SHA256. Sûr **tant que** la KEK est pleine
longueur, unique par wrap (sel aléatoire — OK) et authentifiée (HMAC — OK). Mais
un futur bug de réutilisation de KEK serait catastrophique (réutilisation de
one-time-pad).

### Remédiation

Remplacer par AES-256-KW (RFC 3394) ou XChaCha20-Poly1305 sur la clé. Faible
priorité car actuellement correct, mais fragile.

---

## F8 — INFO — PKI Root CA éphémère

`crypto_server/src/pki.rs:658` : `ROOT_PRIVATE_KEY` régénérée par CSPRNG à chaque
boot (`spin::Once`). Correct (jamais all-zeros — FIX-SEC-2C-PKI), mais les certs
émis ne survivent pas au reboot. Différé : nécessite stockage persistant scellé.

## F9 — INFO/ARCHITECTURE — KERNEL_SECRET per-boot

`security/exokairos.rs:695` : `KERNEL_SECRET` = CSPRNG à chaque boot. **Correct**
pour les MAC de capabilities temporelles (éphémères par nature). **INTERDIT** comme
racine du chiffrement-at-rest (F1/F2) car casserait la persistance disque. Cette
note existe pour éviter une « correction » naïve de F1/F2 qui mélangerait le
KERNEL_SECRET et rendrait ExoFS non-persistant.

---

## CE QUI EST RÉELLEMENT SOLIDE (vérifié)

Pour être juste — beaucoup est réel et bien câblé :

- **Dispatch syscall Zero-Trust** (`dispatch.rs:185-219`) : `verify_syscall(&ctx, nr)`
  avec contexte RÉEL par processus + forward des refus vers shield_feed. Pas un stub.
- **Audit syscall pré-exécution** (`dispatch.rs:163`) : `audit_syscall_entry` peut
  bloquer (DenyEperm/Kill) avant le handler.
- **CapTable** (`security/capability/table.rs`) : enforcement réel droits+type+génération ;
  `inherit_from_masked` met bien à jour `count` (vérifié, pas le bug supposé).
- **crypto_server keystore** : `KEY_TABLE` = slots vides (pas de clés codées en dur).
- **crypto_server PKI root key** : CSPRNG au boot (plus all-zeros).
- **Ed25519/Blake3/XChaCha20** : via crates RustCrypto validées, pas d'impl maison fragile.
- **HKDF-BLAKE3 + Argon2id** : dérivation de clés conforme.
- **ExoLedger** : chaîne Blake3 append-only + zone P0 immuable, réelle.

---

## PLAN DE CORRECTION (ordre)

1. **F2** (prérequis F1) : clé de volume persistante + `derive_object_key`.
2. **F1** : câbler chiffrement-at-rest sur le chemin réel (ou gater honnêtement).
3. **F3** : authentifier le loader du MLP profond.
4. **F4** : script Python d'entraînement + génération + câblage des poids.
5. **F5** : générer une vraie paire Ed25519 de dev (privée gitignored).
6. **F6** : checklist prod + warning boot.
7. **F7/F8** : durcissement différé, documenté.

> Mise à jour de ce fichier au fil des corrections (section ÉTAT du tableau de synthèse).

---

## ÉTAT FINAL DE LA PASSE (2026-06-16)

### Corrigés ET vérifiés (build + tests verts)

| Item | Preuve |
|------|--------|
| 6 correctifs kernel (reboot, segfault→shield, execve/fork ledger, FS zero-trust) | `cargo check -p exo-os-kernel` exit 0 |
| F5 — paire Ed25519 dev réelle | kernel build exit 0 |
| **F10 — MLP rendu opérationnel** (normalisation) | test e2e `ensemble_trained_weights_loaded_and_mlp_discriminates` PASS |
| **F4 — MLP+IF entraînés** (recall malicious 1.0, FP 0.0) | 52 tests ML PASS |
| F3 — loader MLP authentifié (checksum+version) | test e2e PASS (checksum Python↔Rust concordant) |
| F2 — clé blob = secret de volume requis | 15 tests blob storage PASS (round-trip OK) |
| **F1 — chiffrement-at-rest** (mécanisme complet) | 10 tests at_rest + `persist_then_reload_roundtrip` AVEC chiffrement + `encrypted_volume_key_storage_roundtrip` PASS |
| **F7 — wrap clé → AEAD** (SHA-256/HMAC bespoke supprimés) | 212 tests crypto PASS (serialise_roundtrip nouveau format, tampered, wrong-passphrase) |
| **F6 — gate sécurité production** | 81 tests security PASS (2 nouveaux : feature-flag + is_hardened) |
| Dispatch syscall zero-trust | revue de code (dispatch.rs:185-219) |

### F1 — activation déploiement restante (hors chemin par défaut)

Le **mécanisme** de chiffrement-at-rest est implémenté + testé. Pour l'**activer**
sur un volume (n'affecte AUCUN volume existant) :
1. `mkfs` (exofs-mkroot, outil hôte) : `wrap_volume_key` + `set_wrapped_volume_key`.
2. Boot : passer la passphrase (cmdline `exofs.key=` / scellé TPM futur) à
   `install_volume_key_from_wrapped` au montage si `superblock.is_encrypted()`.
3. Vérif end-to-end sous QEMU une fois le boot réparé (#25).

### Reportés — pour raisons d'ingénierie HONNÊTES (pas par négligence)

| Item | Pourquoi reporté |
|------|------------------|
| **F1 — auto-unlock au boot** | Tout est codé+testé : crate partagé exo-fscrypt (R15), kernel lit/déchiffre, `exofs-mkroot --encrypt` crée des volumes chiffrés, `unlock_encrypted_volume()` déverrouille. Restent UNIQUEMENT : (a) la **source de la passphrase** (param boot `exofs.key=`/TPM — décision déploiement, pas câblée pour éviter une passphrase en dur) ; (b) l'appel auto dans `exofs_init` ; (c) la **vérif QEMU** bloquée par #25. |
| **F8** — PKI Root CA persistante | Nécessite un **stockage scellé persistant** (le crypto_server devrait écrire sa clé racine wrappée via ExoFS et la relire au boot). Brique de wrap dispo (exo-fscrypt R15) ; reste l'intégration serveur↔FS + #25. |

### Principe directeur respecté

Aucune « fausse sécurité » n'a été ajoutée. Là où un correctif complet exigeait
une décision d'architecture (KEK) ou une vérification impossible (boot #25), on a
**corrigé ce qui est sûr et vérifiable**, **construit l'infrastructure**, et
**documenté honnêtement** le reste — plutôt que de livrer du chiffrement-théâtre ou
du code de persistance non vérifié susceptible de corrompre le FS.

> **Décision KEK — TRANCHÉE** : provider abstrait (`KekSource`) plutôt qu'un TPM
> figé. Backend `Passphrase` (Argon2id) implémenté ; `TpmSealed`/`SecureBootSealed`
> sont des points d'extension prêts (modèle LUKS2/clevis) — on branchera un vrai TPM
> quand un pilote TIS existera, sans rien casser. Le mécanisme complet est codé et
> testé ; il ne reste que l'activation déploiement (mkfs + passphrase au boot) et la
> vérif QEMU une fois #25 résolu.
