# 🚀 Fichiers de Test - Exo-OS Phase 8

## Fichiers Disponibles

### 1. `exo-os-v2.iso` (5.0 MB) ⭐ PRINCIPAL
**Description**: Kernel Exo-OS complet v0.2.0-PHASE8-BOOT
**Contenu**:
- Kernel Rust 64-bit avec marqueurs debug VGA
- Driver série COM1 (C)
- Boot.asm avec transition 32→64 bit
- GDT, IDT, pagination configurés

**Attendu au boot**:
- Menu GRUB: "Exo-OS Kernel v0.2.0-PHASE8-BOOT"
- Marqueurs VGA: `AA BB PP 64 4 S C XXXXXXX...`
- Boucle idle stable

### 2. `test-minimal.iso` (5.0 MB) 🔧 DIAGNOSTIC
**Description**: Kernel minimal 32-bit pour test GRUB
**Contenu**:
- Code assembleur ultra-simple
- Affiche juste `!!ETST` en couleurs
- Loop infini

**But**: Valider que GRUB fonctionne correctement

**Attendu au boot**:
- Menu GRUB standard
- Caractères `!!ETST` en haut à gauche (colorés)

---

## 🧪 Comment Tester

### Option A: VirtualBox (Recommandé)

```
1. Ouvrir VirtualBox
2. New VM:
   - Name: Exo-OS-Test
   - Type: Linux
   - Version: Other Linux (64-bit)
   - RAM: 512 MB
   - No disk needed
3. Settings → Storage → Controller IDE
   - Add optical drive
   - Select: exo-os-v2.iso
4. Start VM
5. Observer l'écran
6. Prendre une capture d'écran
```

### Option B: Hyper-V (Windows Pro/Enterprise)

```
1. Hyper-V Manager → New Virtual Machine
2. Generation 1
3. 512 MB RAM
4. No network
5. Settings → DVD Drive
   - Image file: exo-os-v2.iso
6. Start
7. Observer l'écran
8. Prendre une capture d'écran
```

### Option C: QEMU avec Serveur X11 (WSL)

```powershell
# 1. Installer VcXsrv ou X410 sur Windows
# 2. Lancer le serveur X11
# 3. Dans PowerShell:
wsl bash -c "export DISPLAY=:0 && qemu-system-x86_64 -cdrom /mnt/c/Users/Eric/Documents/Exo-OS/build/exo-os-v2.iso -m 512M"
```

---

## 📊 Que Chercher

### Menu GRUB
✅ Doit afficher: **"Exo-OS Kernel v0.2.0-PHASE8-BOOT"**
❌ Si affiche: "v0.1.0" → ISO obsolète

### Après Sélection du Menu

#### Scénario 1: Erreur GRUB
```
error: address is out of range
error: you need to load the kernel first
```
→ ❌ Linker script pas appliqué correctement

#### Scénario 2: Écran Noir (aucun caractère)
→ 🔍 Kernel ne démarre pas ou crash immédiatement

#### Scénario 3: Marqueurs Partiels
```
AA BB       → Problème dans check_long_mode
AA BB PP    → Problème dans setup_page_tables
AA BB PP 64 → Problème avant appel rust_main
```

#### Scénario 4: Tous Marqueurs Présents ✅
```
AA BB PP 64 4 S C XXXXXXXXXXXXXXX...
```
→ 🎉 **SUCCÈS !** Le kernel boot correctement

---

## 🎯 Marqueurs Debug VGA

| Position | Marqueur | Couleur | Signification |
|----------|----------|---------|---------------|
| 0xB8000 | `AA` | Blanc/Rouge | _start appelé (32-bit) |
| 0xB8004 | `BB` | Vert | Pile configurée |
| 0xB8008 | `PP` | Bleu | CPU supporte 64-bit |
| 0xB8000 | `64` | Blanc/Rouge | Mode 64-bit actif |
| 0xB8002 | `4` | Vert | Segments chargés |
| 0xB8004 | `S` | Bleu | Pile 64-bit OK |
| 0xB8006 | `C` | Jaune | Avant call Rust |
| 0xB8000+ | `XXX...` | Vert | rust_main exécute |

---

## 📸 Rapporter les Résultats

**Veuillez capturer**:
1. ✅/❌ Menu GRUB affiche v0.2.0-PHASE8-BOOT ?
2. ✅/❌ Erreur "address is out of range" ?
3. 🔍 Quels marqueurs VGA sont visibles ?
4. 📷 Capture d'écran complète

---

## 📁 Fichiers Source

- **Kernel ELF**: `../target/x86_64-unknown-none/release/exo-kernel`
- **Linker Script**: `../linker.ld`
- **Boot Code**: `../kernel/src/arch/x86_64/boot.asm`
- **Main Entry**: `../kernel/src/main.rs`
- **GRUB Config**: `./isofiles/boot/grub/grub.cfg`

---

## 🔄 Rebuilder l'ISO

Si besoin de recompiler après modifications:

```bash
cd /mnt/c/Users/Eric/Documents/Exo-OS
source ~/.cargo/env
./scripts/build-iso.sh
```

L'ISO sera recréée dans `build/exo-os.iso` et `build/exo-os-v2.iso`.

---

**Dernière build**: 12 novembre 2025 18:30  
**Version**: 0.2.0-PHASE8-BOOT  
**Build Tool**: scripts/build-iso.sh  
**Validation**: grub-file --is-x86-multiboot2 ✅
