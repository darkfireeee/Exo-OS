# INSTRUCTIONS DE TEST MANUEL - Phase 8 Boot
## Date : 12 Novembre 2024

### État Actuel
✅ Toutes les corrections appliquées :
- linker.ld corrigé (plus de segments négatifs)
- boot.asm avec sauvegarde EBX sur pile
- Marqueurs VGA debug ajoutés
- grub.cfg mis à jour avec v0.2.0-PHASE8-BOOT

### TEST À EFFECTUER MANUELLEMENT

**Fichier ISO à tester** : `C:\Users\Eric\Documents\Exo-OS\build\exo-os-v2.iso`

#### Option 1 : Test avec QEMU (si vous avez un serveur X11)

1. Installer VcXsrv ou X410 sur Windows
2. Lancer le serveur X11
3. Dans PowerShell :
   ```powershell
   wsl bash -c "export DISPLAY=:0 && qemu-system-x86_64 -cdrom /mnt/c/Users/Eric/Documents/Exo-OS/build/exo-os-v2.iso -m 512M"
   ```

#### Option 2 : Test avec VirtualBox

1. Ouvrir VirtualBox
2. Créer une nouvelle VM :
   - Type : Linux
   - Version : Other Linux (64-bit)
   - RAM : 512 MB
3. Settings → Storage → Controller IDE → Ajouter un lecteur optique
4. Sélectionner `C:\Users\Eric\Documents\Exo-OS\build\exo-os-v2.iso`
5. Démarrer la VM

#### Option 3 : Test avec Hyper-V

1. Ouvrir Hyper-V Manager
2. New → Virtual Machine
3. Generation 1
4. 512 MB RAM
5. No network
6. Dans Settings → DVD Drive → Image file : sélectionner l'ISO
7. Start

### CE QUE VOUS DEVRIEZ VOIR

#### 1. Menu GRUB (après ~2 secondes)
```
GNU GRUB version 2.12

*Exo-OS Kernel v0.2.0-PHASE8-BOOT
 Exo-OS Kernel v0.2.0 (Safe Mode)
 Reboot
 Shutdown
```

**✅ SUCCÈS SI** : Le menu affiche **v0.2.0-PHASE8-BOOT** (pas v0.1.0)

#### 2. Après avoir sélectionné la première entrée (ou timeout 5s)

**SCÉNARIO A** : Si l'erreur "address is out of range" apparaît ENCORE
→ ❌ Le linker script n'est pas correctement appliqué
→ Vérifier que l'ISO a été rebuild APRÈS les corrections

**SCÉNARIO B** : Écran noir, rien ne se passe
→ 🔍 Le kernel boot mais ne produit pas de sortie
→ Chercher des caractères colorés en haut à gauche :

**Marqueurs attendus** (en haut à gauche de l'écran) :
- `AA` (blanc sur fond rouge) = _start appelé en mode 32-bit ✅
- `BB` (vert) = Pile configurée ✅
- `PP` (bleu) = check_long_mode OK ✅
- `64` (blanc/rouge puis vert) = Mode 64-bit atteint ✅
- `SC` (bleu, jaune) = Arguments OK avant appel Rust ✅
- `XXXXXXX...` (ligne de X verts) = rust_main s'exécute ! ✅

**SCÉNARIO C** : Caractères présents mais pas tous
→ 🔍 Le kernel s'arrête à une étape spécifique
→ Noter quels marqueurs sont visibles et lesquels manquent

**SCÉNARIO D** : Tous les marqueurs présents
→ ✅ Le kernel démarre correctement !
→ Le problème est juste l'initialisation du port série

### RAPPORTER LES RÉSULTATS

Prenez une **capture d'écran** de ce que vous voyez et partagez-la.

Notez :
1. ✅/❌ Le menu GRUB affiche-t-il v0.2.0-PHASE8-BOOT ?
2. ✅/❌ L'erreur "address is out of range" apparaît-elle encore ?
3. 🔍 Quels marqueurs VGA sont visibles ? (AA, BB, PP, 64, SC, XXXX)
4. ✅/❌ Y a-t-il une sortie série/texte quelconque ?

### FICHIERS DE TEST ALTERNATIFS

Si vous voulez tester avec le kernel minimal (qui devrait juste afficher `!!ETST`) :
- Fichier : `C:\Users\Eric\Documents\Exo-OS\build\test-minimal.iso`
- Devrait afficher : `!!ETST` en couleurs en haut à gauche
- Si même celui-là ne fonctionne pas, il y a un problème avec GRUB/QEMU lui-même

### PROCHAINES ÉTAPES SELON LES RÉSULTATS

- Si **aucun marqueur** : Problème avec GRUB ou adresses de chargement
- Si **AA BB PP seulement** : Problème dans la transition 32→64 bit
- Si **tous marqueurs sauf X** : `rust_main` n'est pas appelé ou crash
- Si **tous marqueurs présents** : Serial driver ne fonctionne pas, mais kernel OK !

---

**Note** : Cette phase de test nécessite un affichage visuel. WSL ne peut pas afficher l'interface graphique QEMU facilement, donc un test avec VirtualBox/Hyper-V ou un serveur X11 est recommandé.
