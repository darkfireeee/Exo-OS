# Session de Débogage - 12 Novembre 2024
## Phase 8 : Boot Kernel v0.2.0 dans QEMU

### Problèmes Identifiés et Résolus

#### 1. ❌ Erreur GRUB : "address is out of range"
**Symptôme** : GRUB affiche "error: address is out of range" et "you need to load the kernel first"

**Cause** : Chevauchement des sections dans le linker script. La section `.bss` de boot.asm était en conflit avec `.boot`, créant un segment LOAD avec une taille mémoire invalide (`0xffffffffffffc000` = nombre négatif)

**Solution** :
- Modifié `linker.ld` pour séparer clairement les sections
- Nouvelle structure mémoire :
  ```
  0x100000 : .boot (header Multiboot2 + code _start)
  0x101000 : .bss.boot (pile 16KB + tables de pages 12KB = 28KB total)
  0x108000 : .text (code Rust)
  0x10A000 : .rodata (données lecture seule)
  ```

**Validation** :
```bash
readelf -l target/x86_64-unknown-none/release/exo-kernel
# Tous les segments LOAD ont des tailles valides
# Plus d'erreur "address is out of range"
```

#### 2. ❌ Version affichée : "v0.1.0" au lieu de "v0.2.0"
**Symptôme** : Menu GRUB affiche "Exo-OS Kernel v0.1.0"

**Cause** : Fichier `bootloader/grub.cfg` non mis à jour

**Solution** : Modifié `bootloader/grub.cfg` :
```
menuentry "Exo-OS Kernel v0.2.0-PHASE8-BOOT" {
    multiboot2 /boot/kernel.bin
    boot
}
```

**Validation** :
```bash
cat build/isofiles/boot/grub/grub.cfg
# Affiche bien "v0.2.0-PHASE8-BOOT"
```

#### 3. ❌ Perte de l'adresse Multiboot lors du passage en mode 64-bit
**Symptôme** : Code `rust_main` potentiellement appelé avec mauvais argument

**Cause** : Registre `edi` (contenant l'adresse Multiboot) était écrasé par les appels de fonction en mode 32-bit

**Solution** : Modifié `boot.asm` :
```asm
_start:
    push ebx          ; Sauvegarder adresse Multiboot sur la pile
    call check_long_mode
    call setup_page_tables
    ; ...

long_mode_start:
    pop rdi           ; Récupérer adresse Multiboot dans RDI (1er arg x86_64)
    call rust_main
```

#### 4. 🔍 Problème actuel : Aucune sortie (VGA ni série)
**Symptôme** : Le kernel ne produit aucune sortie visible

**Actions de diagnostic** :
1. ✅ Ajout de marqueurs VGA dans `boot.asm` à chaque étape :
   - `AA` (blanc/rouge) : `_start` appelé (mode 32-bit)
   - `BB` (vert) : Pile configurée
   - `PP` (bleu) : `check_long_mode` OK
   - `64` (blanc/rouge puis vert) : Mode 64-bit atteint
   - `SC` (bleu, jaune) : Pile 64-bit et arguments OK
   
2. ✅ Ajout de marqueurs VGA dans `main.rs` :
   - Ligne de `X` verts : `rust_main` s'exécute
   
3. ✅ Créé script `run-qemu-debug.sh` pour affichage VGA sans redirection série

**Test en cours** : Attente capture d'écran QEMU pour identifier où l'exécution s'arrête

### Fichiers Modifiés

1. **linker.ld**
   - Séparation des sections `.bss.boot` et `.boot`
   - Alignement 4KB pour toutes les sections
   - Suppression de `(NOLOAD)` sur `.bss`

2. **kernel/src/arch/x86_64/boot.asm**
   - Sauvegarde EBX sur pile au lieu de EDI
   - Récupération via POP RDI en mode 64-bit
   - Ajout de 7 marqueurs VGA de debug (AA, BB, PP, 64, S, C)

3. **bootloader/grub.cfg**
   - Version mise à jour : "v0.2.0-PHASE8-BOOT"

4. **kernel/src/main.rs**
   - Ajout de marqueurs VGA (ligne de X verts)
   - Code d'initialisation série conservé

### Validations Effectuées

✅ Compilation réussie sans erreurs
✅ Segments ELF corrects (plus de taille négative)
✅ Symboles aux bonnes adresses :
```
_start      @ 0x100018
stack_top   @ 0x105000
p4_table    @ 0x105000
rust_main   @ 0x108000
```
✅ grub-file valide le binaire comme Multiboot2
✅ ISO créée avec succès (5.0 MB)
✅ Menu GRUB affiche v0.2.0-PHASE8-BOOT

### Prochaines Étapes

1. ⏳ **Analyser capture d'écran QEMU** pour voir quels marqueurs apparaissent
2. 🔍 Selon les marqueurs visibles, identifier le point de blocage :
   - Si aucun marqueur : GRUB ne charge pas le kernel
   - Si AA/BB seulement : Problème dans check_long_mode ou setup_page_tables
   - Si AA/BB/PP/64 seulement : Problème lors de l'appel à rust_main
   - Si tous marqueurs sauf X : `rust_main` ne s'exécute pas ou crash
   - Si tous marqueurs présents : Port série ne fonctionne pas

3. 🛠️ Corriger le problème identifié

### Outils et Commandes Utiles

```bash
# Rebuilder l'ISO
./scripts/build-iso.sh

# Lancer QEMU avec affichage VGA debug
./scripts/run-qemu-debug.sh

# Vérifier les segments ELF
readelf -l target/x86_64-unknown-none/release/exo-kernel

# Vérifier les symboles
nm target/x86_64-unknown-none/release/exo-kernel | grep -E '(_start|rust_main|stack_top)'

# Vérifier le header Multiboot2
xxd -s 0x1000 -l 64 build/isofiles/boot/kernel.bin

# Valider Multiboot2
grub-file --is-x86-multiboot2 target/x86_64-unknown-none/release/exo-kernel
```

### Notes Techniques

- **Multiboot2 Magic** : `0xE85250D6` présent à l'offset 0x1000 du fichier ELF ✅
- **Entry Point** : `0x100018` (_start dans .boot) ✅
- **Taille kernel** : 20-24 KB (très compact) ✅
- **Architecture cible** : x86_64-unknown-none (bare-metal) ✅
- **Outils build** : NASM 2.16.01, GCC 11.4.0, Rust 1.93.0-nightly ✅

---

**Statut** : 🔄 En cours de diagnostic avec marqueurs VGA
**Prochain test** : Analyse capture d'écran QEMU pour localiser le point de blocage
