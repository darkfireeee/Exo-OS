# Guide de compilation et déploiement exo-boot

---

## Prérequis

### Toolchain Rust

```powershell
# Installer/mettre à jour rustup
rustup update nightly

# Composants requis
rustup target add x86_64-unknown-uefi
rustup component add rust-src --toolchain nightly
rustup component add llvm-tools-preview --toolchain nightly
```

### rust-toolchain.toml

exo-boot utilise un fichier `rust-toolchain.toml` dans son répertoire :

```toml
[toolchain]
channel = "nightly"
components = ["rust-src", "llvm-tools-preview"]
targets = ["x86_64-unknown-uefi"]
```

### Outils système

| Outil | Usage | Installation Windows |
|-------|-------|---------------------|
| `cargo` | Build Rust | Inclus avec rustup |
| `qemu-system-x86_64` | Tests VM | `winget install QEMU.QEMU` |
| `OVMF` | Firmware UEFI pour QEMU | Fourni dans `bootloader/` ou tiretéléchargé |
| `sbsign` (optionnel) | Signer PE32+ pour Secure Boot | WSL / sous-système Linux |

---

## Commandes de compilation

### Build UEFI (mode courant)

```powershell
cd c:\Users\GALAXY BOOK FLEX\Documents\Exo-OS

# Debug — dossier exo-boot uniquement
cargo build `
  --manifest-path exo-boot/Cargo.toml `
  --target x86_64-unknown-uefi `
  --features "uefi-boot,dev-skip-sig" `
  -Z build-std=core,alloc,compiler_builtins

# Release
cargo build `
  --manifest-path exo-boot/Cargo.toml `
  --target x86_64-unknown-uefi `
  --release `
  --features "uefi-boot,kaslr,secure-boot" `
  -Z build-std=core,alloc,compiler_builtins
```

**Artefact** : `target/x86_64-unknown-uefi/debug/exo-boot.efi`

### Build workspace complet (kernel + bootloader + serveurs)

```powershell
cargo build -Z build-std=core,alloc,compiler_builtins
```

### Build BIOS (expérimental)

```powershell
cargo build `
  --manifest-path exo-boot/Cargo.toml `
  --target x86_64-unknown-none `
  --features "bios-boot,dev-skip-sig" `
  -Z build-std=core,compiler_builtins
```

---

## Tableau des features

| Feature | Par défaut | Description |
|---------|-----------|-------------|
| `uefi-boot` | oui | Active le chemin UEFI (efi_main, GOP, RNG) |
| `bios-boot` | non | Active le chemin BIOS legacy (MBR, VGA, INT 13h) |
| `kaslr` | non | Active la randomisation d'adresse base du kernel |
| `secure-boot` | non | Active la vérification Ed25519 du kernel (exige `ed25519-dalek`) |
| `dev-skip-sig` | non | Bypasse la vérification de signature (WARNING visible) |

> `uefi-boot` et `bios-boot` sont mutuellement exclusifs.
> `secure-boot` et `dev-skip-sig` sont mutuellement exclusifs.

---

## Dépendances Cargo

```toml
[dependencies]
# Framework UEFI
uefi          = { version = "0.26", features = ["alloc"] }
uefi-services = "0.23"

# CPU x86_64 (registres, MSR, CPUID)
x86_64        = "0.15"

# Spinlock no_std
spinning_top  = "0.3"

# Vecteurs sans heap
arrayvec      = { version = "0.7", default-features = false }

# Cryptographie (feature secure-boot uniquement)
ed25519-dalek = { version = "2", optional = true, default-features = false,
                  features = ["digest"] }
sha2          = { version = "0.10", optional = true, default-features = false }
```

---

## Artefacts produits

| Cible | Chemin debug | Chemin release | Format |
|-------|-------------|---------------|--------|
| UEFI | `target/x86_64-unknown-uefi/debug/exo-boot.efi` | `.../release/exo-boot.efi` | PE32+ |
| BIOS | `target/x86_64-unknown-none/debug/exo-boot` | `.../release/exo-boot` | ELF64 |

---

## Tests avec QEMU

### Structure de l'image ESP

```
esp/
└── EFI/
    └── EXOOS/
        ├── exo-boot.efi       ← bootloader UEFI
        ├── kernel.elf          ← kernel
        └── exo-boot.cfg        ← configuration (optionnel)
```

### Créer l'image FAT32

```powershell
# Taille 64 MiB
$null > esp.img
fsutil file seteof esp.img 67108864
# Formater (nécessite diskpart ou des outils Linux via WSL)
```

### Lancer QEMU UEFI

```powershell
qemu-system-x86_64 `
  -enable-kvm `
  -m 512M `
  -drive if=pflash,format=raw,readonly=on,file=bootloader/OVMF_CODE.fd `
  -drive if=pflash,format=raw,file=bootloader/OVMF_VARS.fd `
  -drive format=raw,file=fat:rw:esp/ `
  -serial stdio `
  -display gtk
```

### Lancer QEMU BIOS

```powershell
qemu-system-x86_64 `
  -m 512M `
  -drive format=raw,file=disk.img `
  -serial stdio
```

---

## Variables d'environnement utiles

| Variable | Valeur | Description |
|----------|--------|-------------|
| `RUST_LOG` | `debug` | Active les messages de debug (uefi-services) |
| `CARGO_PROFILE_DEV_OPT_LEVEL` | `0` à `3` | Niveau d'optimisation debug |
| `OVMF_PATH` | chemin OVMF | Utilisé par les scripts Makefile |

---

## Makefile

Le projet fournit un `Makefile` racine :

```makefile
make build          # Build debug complet (workspace)
make build-release  # Build release
make run            # Build + lancer QEMU UEFI
make run-bios       # Build + lancer QEMU BIOS
make clean          # cargo clean
```

---

## Vérification de la compilation

```powershell
# Vérifier sans compiler (rapide)
cargo check `
  --manifest-path exo-boot/Cargo.toml `
  --target x86_64-unknown-uefi `
  --features "uefi-boot,dev-skip-sig" `
  -Z build-std=core,alloc,compiler_builtins

# Vérifier tous les avertissements
$env:RUSTFLAGS = "-D warnings"
cargo build --manifest-path exo-boot/Cargo.toml ...
```
