# Sécurité dans exo-boot

---

## Vue d'ensemble de la chaîne de confiance

```
Firmware UEFI (Secure Boot DB)
         │
         │  Vérifie signature PE32+ de exo-boot.efi
         │  via certificat dans Secure Boot DB
         ▼
    exo-boot.efi  ◄── Niveau 1 : authentifié par firmware
         │
         │  Vérifie Ed25519 de kernel.elf
         │  via clé publique intégrée dans exo-boot
         ▼
     kernel.elf   ◄── Niveau 2 : authentifié par bootloader
         │
         │  Vérifie que BootInfo est intègre (magic + version)
         ▼
    Kernel Init   ◄── Niveau 3 : contrat de démarrage validé
```

**Règle BOOT-02** : Si la signature du kernel est invalide,
exo-boot s'arrête immédiatement (panic) sans jamais sauter au kernel.

---

## Signature kernel Ed25519

### Structure KernelSignature (256 octets)

```
Offset  Taille  Champ         Description
──────  ──────  ─────         ──────────────────────────────────────
  0       8     marker        Signature ASCII "EXOSIG01"
  8       4     version       Version du format de signature (1)
 12       4     flags         Drapeaux (réservé, 0)
 16       4     sig_type      Type algorithme : 1 = Ed25519
 20       4     _reserved     Padding
 24      64     signature     Signature Ed25519 du hash SHA-512
 88      32     pubkey        Clé publique Ed25519 (32 octets)
120      64     hash          SHA-512 du binaire kernel (sans la struct)
184      72     _pad          Rembourrage (total = 256)
```

### Placement dans l'ELF kernel

La structure `KernelSignature` est placée dans la section ELF nommée
`.kernel_sig`, à la fin du fichier. Elle n'est pas incluse dans le hash.

```
ELF kernel.elf
├── PT_LOAD [0] : .text .rodata ...
├── PT_LOAD [1] : .data .bss ...
└── Section .kernel_sig (non loadable)
    └── KernelSignature (256 bytes)
```

### Algorithme de vérification (exo-boot)

```
1. Localiser section .kernel_sig :
   - Parser les en-têtes de section ELF (SHT) 
   - Chercher le nom ".kernel_sig" dans la string table
   - Lire 256 octets à sh_offset

2. Vérifier le marqueur :
   - sig.marker == b"EXOSIG01"  →  sinon PANIC

3. Calculer SHA-512 du kernel (sans la struct sig) :
   - sha2::Sha512::digest(&kernel_data[..offset_of_sig])
   - Comparer avec sig.hash  →  sinon PANIC

4. Vérifier la signature Ed25519 :
   - ed25519_dalek::VerifyingKey::from_bytes(&sig.pubkey)
   - ed25519_dalek::Signature::from_bytes(&sig.signature)
   - verifying_key.verify_strict(&sha512_hash, &signature)
   - Si Err(_)  →  PANIC immédiat
```

### Feature `dev-skip-sig`

En développement, compiler avec `--features dev-skip-sig` :
- Ignore la vérification Ed25519
- Affiche `[WARN] kernel signature verification SKIPPED (dev mode)` en rouge
- `boot_flags` ne met **pas** `SECURE_BOOT_ACTIVE`

⚠ Ne jamais activer `dev-skip-sig` en production.

---

## Secure Boot UEFI

### Détection de l'état Secure Boot

```rust
// Via variables EFI GlobalVariable
let sb = read_variable("SecureBoot")  // u8: 1 = activé
let sm = read_variable("SetupMode")   // u8: 1 = setup (non activé)

let secure_boot_active = sb == Some(1) && sm == Some(0);
```

### Signature PE32+ du bootloader

exo-boot.efi doit être signé avec un certificat dont la chaîne remonte
à une autorité présente dans la Secure Boot DB (Base de données de clés
du firmware, variable `db`).

Outils recommandés :
```
sbsign --key your.key --cert your.crt exo-boot.efi --output exo-boot.efi
sbverify --cert your.crt exo-boot.efi
```

Le bit `SECURE_BOOT_ACTIVE` dans `boot_flags` est positionné uniquement si :
1. La variable UEFI `SecureBoot` = 1 ET `SetupMode` = 0
2. La signature Ed25519 du kernel a été vérifiée avec succès

---

## KASLR — Randomisation de l'adresse du kernel

### Algorithme

```
1. Collecter entropie = [u8; 64] (EFI_RNG si dispo, sinon RDRAND/TSC)

2. Dériver un u64 depuis les 8 premiers octets de l'entropie :
   random_bits = u64::from_le_bytes(entropy[0..8])

3. Calculer offset :
   KASLR_MIN  = 0x4000_0000       (1 GiB)
   KASLR_MAX  = 0x4000_0000_0000  (256 GiB / 64 TiB)
   KASLR_ALIGN = 0x200000         (2 MiB = taille grande page)

   range = (KASLR_MAX - KASLR_MIN) / KASLR_ALIGN
   base  = KASLR_MIN + (random_bits % range) * KASLR_ALIGN

4. Le kernel est chargé à cette adresse physique.
   kernel_virtual = KERNEL_HIGHER_HALF_BASE + (base - KASLR_MIN)
```

### Relocations PIE

Le kernel est compilé en `-C relocation-model=pie` (Position Independent Executable).
Les relocations sont dans la section `.rela.dyn` :

| Type | Valeur | Traitement |
|------|--------|------------|
| `R_X86_64_RELATIVE` | 8 | `*addr = kaslr_base + addend` |
| `R_X86_64_64` | 1 | `*addr = sym_value + addend` |

```rust
// Application des relocations après chargement
for rela in elf.rela_dyn_entries() {
    let addr = phys_base + rela.r_offset;
    match rela.r_type() {
        8 => unsafe { *(addr as *mut u64) = phys_base + rela.r_addend as u64 },
        1 => /* résolution symbole + addend */,
        _ => { /* warn: type inconnu */ }
    }
}
```

### Impact sur BootInfo

```rust
boot_info.kernel_physical_base = kaslr_computed_base;
boot_info.kernel_entry_offset  = elf.e_entry - elf.load_segments[0].p_vaddr;
// Adresse d'entrée physique = kernel_physical_base + kernel_entry_offset
```

Le bit `KASLR_ENABLED` est mis dans `boot_flags` si la feature `kaslr` est active.

---

## Invariants de sécurité

| # | Invariant | Conséquence si violé |
|---|-----------|---------------------|
| SEC-01 | Signature validée avant tout mapping mémoire kernel | Exécution de code non authentifié |
| SEC-02 | ExitBootServices appelé une seule fois | Double-free UEFI pool, corruption état |
| SEC-03 | Entropie collectée avant dernière allocation | Entropie biaisée, KASLR prévisible |
| SEC-04 | BootInfo non modifiable après construction | Données corrompues au kernel |
| SEC-05 | Clé publique Ed25519 intégrée dans exo-boot signé PE32+ | Substitution de clé possible |
