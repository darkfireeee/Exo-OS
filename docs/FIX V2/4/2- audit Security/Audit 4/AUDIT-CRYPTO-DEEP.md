# Audit crypto/sécurité profond — kernel + serveurs (2026-06-20)

> Double vérification de **toute** la crypto (kernel + serveurs) et des modules
> de sécurité, guidée par recherche web sur les classes de bugs/CVE. Objectif :
> combler les failles **oubliées**. Trois corrections, le reste validé solide.

## Méthode
Recherche des classes de vulnérabilités connues, puis audit de chaque primitive
contre ces classes :
- Ed25519 `verify` vs `verify_strict` (clés faibles low-order, malléabilité).
- X25519 secret partagé tout-à-zéro / sous-groupes d'ordre faible (RFC 7748 §6).
- AES-GCM réutilisation de nonce (catastrophique), GHASH temps-constant.
- AEAD : encrypt-then-MAC, comparaison de tag temps-constant, dépassement de
  compteur ChaCha20 (CVE-2019-25005), séparation des clés MAC/chiffrement.
- RNG/entropie, zéroïsation des clés, comparaisons temps-constant.

## Corrections appliquées

### 1. [HAUT] `verify_strict` manquant dans le crypto_server (faille **oubliée**)
La correction `verify` → `verify_strict` de la session précédente avait été
appliquée au **kernel** (`security/crypto/ed25519.rs`) et à **exo-verity**, mais
**oubliée** dans le **crypto_server** — qui est la vérification Ed25519 *centrale*
(exo_shield et tous les serveurs y délèguent par IPC, cf. SRV-02). Deux sites :
- `servers/crypto_server/src/main.rs:340` (handler de vérification central).
- `servers/crypto_server/src/pki.rs:690` (vérification de chaîne de certificats).

Risque : `verify` accepte les clés publiques faibles (forge universelle) et les
signatures malléables (deux signatures valides pour un message). Pour une chaîne
PKI inter-serveurs et la mise à jour signée de la base NGAV, c'est exploitable.
→ **Corrigé** : `verify_strict` aux deux endroits (trait `Verifier` retiré).

### 2. [MOYEN] GHASH `gf128_mul` non temps-constant (AES-GCM)
`kernel/src/security/crypto/aes_gcm.rs` : la multiplication GF(2^128) **branchait**
sur les bits de son opérande (`if bit_set`, `if lsb`). Pendant le calcul du tag,
cet opérande contient la **sous-clé de hachage** `H = AES_K(0)` ; le branchement
fuite `H` par canal temporel → forge de tags GCM. → **Corrigé** : branches
remplacées par des **masques** (`0 - bit`), multiplication entièrement temps-constant.

### 3. [MINEUR] Repli getrandom dégradé dans le crypto_server
`servers/crypto_server/src/xchacha20.rs` : `getrandom_u64` faisait UN essai
SYS_GETRANDOM puis repliait sur `TSC ^ adresse_pile` (faible, présenté comme
aléatoire = fausse sécurité). → **Corrigé** : 16 essais getrandom avant repli ;
repli dégradé **explicite** (multi-TSC espacé). Impact réel faible : l'unicité des
nonces intra-session est de toute façon garantie par le compteur monotone ; ce sel
n'ajoute que de l'unicité inter-sessions et ne dérive aucune clé.

## Audité — solide, aucune correction nécessaire

| Primitive / module | Vérifié | Verdict |
|--------------------|---------|---------|
| **X25519** (`crypto/x25519.rs`) | Rejet du secret partagé tout-à-zéro (RFC 7748 §6) + test point d'ordre faible | ✅ |
| **Ed25519 kernel** (`crypto/ed25519.rs`) | `verify_strict` + garde de compilation anti-clé-test | ✅ |
| **AES-GCM kernel** | Tag comparé **temps-constant** (`subtle::ct_eq`), **vérif-avant-déchiffrement** | ✅ (+GHASH corrigé) |
| **XChaCha20-BLAKE3 AEAD** (`crypto/xchacha20_poly1305.rs`) | encrypt-then-MAC, vérif-avant-déchiffrement, tag temps-constant, **clé MAC séparée** (`derive_key`), AAD préfixé en longueur, vecteur RFC 8439 | ✅ |
| **exo-fscrypt** (at-rest partagé) | Même AEAD ; **Argon2id** (m=64 MiB, t=3, p=4) ; nonces déterministes par (blob, offset) sûrs via blobs **immuables adressés par contenu** + clés par-blob | ✅ |
| **BLAKE3** (`crypto/blake3.rs`) | `constant_time_eq` via `subtle` ; `derive_key` à séparation de domaine | ✅ |
| **RNG** (`crypto/rng.rs`) | RDSEED×4 + RDRAND×6 + jitter TSC + SP, **conditionné BLAKE3**, reseed/4096, flag `hw_seeded` (mode dégradé détectable), zéroïsation | ✅ |
| **Gestion des nonces** | `secret_writer` : HKDF(sel aléatoire, **compteur monotone**) ; crypto_server : compteur + sel → uniques par session | ✅ |
| **exo_shield maj base NGAV** (`signatures/update.rs`) | Mise à jour **signée Ed25519** + liste de clés éditeur de confiance (CRC32 = intégrité seule, pas la frontière de sécurité) ; crypto déléguée au crypto_server (SRV-02) | ✅ |
| **crypto_server keystore** | Zéroïsation des clés multi-passes (`write_volatile`) | ✅ |
| **Comparaisons temps-constant** | Tous les tags/MAC AEAD en CT ; les autres `==` portent sur des ID d'objet / magic, pas des secrets porteurs | ✅ |

## Vérification
- Kernel `security::crypto::` : **31 tests** verts (AES-GCM roundtrip/tamper + gf128 identité/zéro confirment le GHASH temps-constant correct ; ed25519/x25519/blake3/xchacha20/rng).
- crypto_server : build propre (verify_strict + getrandom).
- Kernel bare-metal `x86_64-unknown-none` : build propre.

## Références (recherche web)
- [ed25519-dalek `verify_strict` — clés faibles + malléabilité](https://docs.rs/ed25519-dalek/latest/ed25519_dalek/struct.VerifyingKey.html)
- [X25519 sortie tout-à-zéro / RFC 7748 §6](https://datatracker.ietf.org/doc/html/rfc7748)
- [AES-GCM réutilisation de nonce (Nonce-Disrespecting Adversaries)](https://eprint.iacr.org/2016/475.pdf)
- [Comparaison temps-constant / MAC (BearSSL)](https://www.bearssl.org/constanttime.html)
- [ChaCha20 dépassement de compteur — CVE-2019-25005](https://vulert.com/vuln-db/crates-io-chacha20-595)
