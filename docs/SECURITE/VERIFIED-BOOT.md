# Verified Boot — chaîne de confiance bootloader → kernel

> Date : 2026-06-20. Comble le trou « la sécurité du kernel ne s'applique pas au
> bootloader » : la **chaîne de confiance** (vérification de signature du kernel)
> était une *fausse promesse*. Elle est désormais **réelle, fail-closed, toujours
> active**, et **structurellement** protégée contre la ré-introduction des erreurs.

## Le problème (état antérieur)

exo-boot est un binaire **séparé** du kernel (`BOOT-01`). La sécurité *runtime* du
kernel (capabilities, zero-trust, exo_shield, ExoLedger) ne s'exécute pas avant le
kernel — c'est correct. Le **seul** lien sécurité est la signature du kernel, et
elle était cassée de quatre façons :

1. **Stub fail-open** : sans la feature `secure-boot`, `verify_full()` renvoyait
   `Ok(())` / `true` pour *n'importe quelle* image → « signature valide » mentait.
2. **`secure_boot_required = false`** par défaut.
3. **Clé de test** : la clé publique embarquée était le vecteur RFC 8032.
4. **Aucune signature** : rien ne signait le kernel (`EXOSIG01` jamais produit).

Résultat : un kernel altéré ou malveillant démarrait sans objection.

## La correction — par construction, pas par rustine

### Crate partagée `exo-verity` (`drivers/security/verity`)
Source **UNIQUE** du format de signature et de la logique de vérification,
utilisée à l'identique par le bootloader (no_std) et l'outil de signature (host,
std). Le signataire et le vérificateur **ne peuvent plus diverger**.

Propriétés structurelles qui empêchent la « fausse sécurité » de revenir :

| Erreur passée | Prévention structurelle |
|---------------|--------------------------|
| Stub fail-open (`Ok` sans vérifier) | Verdict **enum** [`KernelVerdict`] (`Verified`/`Unsigned`/`Tampered`/`NoVerifierKey`) — pas de `bool` qui confond « non signé » et « vérifié ». |
| Crypto éteinte par défaut | Ed25519+SHA-512 **toujours compilés**. Seule la *politique* (refuser vs avertir) est configurable. |
| Clé de test expédiée comme réelle | `key_is_usable()` refuse les clés nulles/vecteurs de test → `NoVerifierKey`. **Garde de compilation** côté kernel ET bootloader (`const _: () = assert!(...)`) : le binaire **ne compile pas** avec une clé de test. |
| `verify` (malléable, clés faibles) | **`verify_strict`** partout (anti-clé-faible + anti-malléabilité, cofacteur 8). |
| Altéré toléré comme « non signé » | `Tampered` (signature présente mais invalide) est **toujours fatal**, même en dev. |

### Politique fail-closed (`kernel_loader::verify::decide`)
Partagée UEFI **et** BIOS, pilotée par un verdict **honnête** :

| Verdict | dev (permissif) | strict (`secure_boot_required` ou UEFI SB enforcing) |
|---------|-----------------|------------------------------------------------------|
| `Verified` | démarre | démarre |
| `Tampered` | **REFUS** | **REFUS** |
| `Unsigned` | avertit + démarre | **REFUS** |
| `NoVerifierKey` | avertit + démarre | **REFUS** |

Défense en profondeur : `load_kernel` re-refuse toute image `Tampered` juste avant
de charger les segments, même si une couche supérieure se trompe.

### Format de signature (footer 256 o, fin de l'ELF)
`EXOSIG01`(8) ‖ Ed25519(64) ‖ SHA-512 du corps(64) ‖ padding(120). Message signé =
`SHA-512(corps)`. La vérification **recalcule** le hash du corps (pas de confiance
au hash stocké). UEFI lit le fichier exact (footer en fin) ; BIOS lit la fenêtre
shadow et localise le footer via la **taille réelle de l'ELF** (en-têtes programme
+ sections), donc la même image signée fonctionne sur les deux chemins.

### Vraies clés + pipeline de signature
- `tools/kernel_signer` (host, std, via `exo-verity`) :
  - `keygen` : génère une paire Ed25519 (getrandom). Privée → `.secrets/kernel_signing.seed` (0600, **gitignored**). Publique → `exo-boot/src/kernel_loader/signing_key.rs` (embarquée).
  - `sign <elf>` : append `EXOSIG01` (idempotent — retire un footer existant).
  - `verify <elf>` : retour ≠ 0 si pas `Verified` (CI).
- **Makefile** : `make build`/`release` signe le kernel automatiquement si la clé
  privée existe (sinon avertit, kernel non signé en dev). Cibles :
  `make keygen-kernel`, `make sign-kernel`, `make verify-kernel`.

### Durcissement crypto kernel (même classe de bug)
- `security/crypto/ed25519.rs` : `ed25519_verify` passe à **`verify_strict`** →
  corrige la signature des **modules kernel** (`code_signing.rs`).
- `code_signing.rs` : **garde de compilation** refusant les clés `MASTER`/`UPDATE`
  nulles ou vecteurs de test.

## Déploiement (décision opérateur)
La clé privée de dev est dans `.secrets/` (jamais committée). **Production** :
régénérer hors-ligne (`make keygen-kernel`), stocker la privée en HSM, committer
uniquement `signing_key.rs` (publique), activer `secure_boot_required = true` (ou
build production strict). Le seul choix laissé à l'opérateur est *où vit la clé
privée* — tout le reste est fail-closed par défaut.

## Vérification
- `exo-verity` : **6 tests** (sign→Verified, tamper→Tampered, wrong-key→Tampered,
  unsigned→Unsigned, clés test/nulle→NoVerifierKey).
- `kernel_signer` : pipeline bout-en-bout (sign → verified ; tamper → TAMPERED).
- kernel `ed25519` : **4 tests** verts avec `verify_strict`.
- Builds **0 warning** : exo-boot UEFI + BIOS (crypto toujours liée), kernel
  bare-metal.
- ⚠️ Boot QEMU réel via exo-boot (chargement + refus d'un kernel altéré en vrai) :
  après réparation du boot-to-shell (#25). Le flux actuel reste GRUB/multiboot2 ;
  le footer de signature est ignoré par GRUB (octets en fin d'ELF) → aucune
  régression du flux existant.
