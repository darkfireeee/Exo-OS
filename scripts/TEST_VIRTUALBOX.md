# Test Exo-OS avec VirtualBox

## Installation VirtualBox (si nécessaire)

1. Télécharger : https://www.virtualbox.org/wiki/Downloads
2. Installer la version Windows
3. Redémarrer si demandé

## Procédure de test (5 minutes)

### Étape 1 : Créer la VM

1. Ouvrir **VirtualBox**
2. Cliquer **Nouvelle**
3. Configurer :
   - **Nom** : `Exo-OS-Test`
   - **Type** : Linux
   - **Version** : Other Linux (64-bit)
   - **Mémoire** : 512 MB
   - **Disque dur** : Ne pas ajouter de disque (sélectionner "Ne pas ajouter de disque dur virtuel")
4. Cliquer **Créer**

### Étape 2 : Attacher l'ISO

1. Sélectionner la VM `Exo-OS-Test`
2. Cliquer **Configuration** (ou Settings)
3. Aller dans **Stockage** (Storage)
4. Sous "Contrôleur IDE", cliquer sur le petit disque vide (Empty)
5. À droite, cliquer sur l'icône de disque 💿
6. Choisir **"Choose a disk file..."**
7. Naviguer vers :
   ```
   C:\Users\Eric\Documents\Exo-OS\build\exo-os-v2.iso
   ```
8. Cliquer **OK**

### Étape 3 : Démarrer et observer

1. Sélectionner la VM `Exo-OS-Test`
2. Cliquer **Démarrer** (ou Start)
3. Une fenêtre s'ouvre avec l'écran de la VM

### Étape 4 : Vérifier le boot

#### 🔍 **ÉCRAN 1 - Menu GRUB** (devrait apparaître en premier)

```
┌────────────────────────────────────────┐
│ Exo-OS Kernel v0.2.0-PHASE8-BOOT      │  ← Doit dire v0.2.0 !
└────────────────────────────────────────┘
```

**✅ SUCCÈS si** : Vous voyez "v0.2.0-PHASE8-BOOT"  
**❌ PROBLÈME si** : Vous voyez "v0.1.0" → ISO non à jour

#### 🔍 **ÉCRAN 2 - Marqueurs de boot** (après sélection du menu)

Regardez le **coin supérieur gauche** de l'écran :

```
AA BB PP 64 4 S C XXXXXXXXXXXXXXXXXXXXXXX...
```

**Signification des marqueurs** :

| Marqueur | Couleur | Signification |
|----------|---------|---------------|
| `AA` | Blanc/Rouge | ✅ Point d'entrée 32-bit atteint |
| `BB` | Vert | ✅ Stack configuré correctement |
| `PP` | Bleu | ✅ CPU supporte le mode 64-bit |
| `64` | Blanc/Rouge | ✅ Passage en mode 64-bit réussi |
| `4` | Vert | ✅ Segments 64-bit chargés |
| `S` | Bleu | ✅ Stack 64-bit configuré |
| `C` | Jaune | ✅ Appel à rust_main imminent |
| `XXX...` | Vert (ligne) | ✅ **rust_main s'exécute !** |

#### 🔍 **RÉSULTATS POSSIBLES**

##### ✅ **SUCCÈS TOTAL**
- Menu montre "v0.2.0-PHASE8-BOOT"
- Tous les marqueurs apparaissent : `AA BB PP 64 4 S C XXX...`
- Pas d'erreur "address is out of range"
- **→ Le kernel boot correctement ! Phase 8 réussie !**

##### ⚠️ **SUCCÈS PARTIEL** (diagnostic)
- Marqueurs `AA BB` seulement → Problème dans `check_long_mode`
- Marqueurs `AA BB PP` seulement → Problème dans `setup_page_tables`
- Marqueurs jusqu'à `C` mais pas de `XXX` → Problème dans `rust_main`

##### ❌ **ÉCHEC**
- Erreur "address is out of range" → Linker script non appliqué
- Aucun marqueur → Problème Multiboot ou GRUB
- Écran noir complet → ISO corrompue

### Étape 5 : Test diagnostic (si échec)

Si vous ne voyez **AUCUN marqueur** avec `exo-os-v2.iso`, testez le kernel minimal :

1. Éteindre la VM
2. Configuration → Stockage → Changer l'ISO pour :
   ```
   C:\Users\Eric\Documents\Exo-OS\build\test-minimal.iso
   ```
3. Redémarrer la VM
4. Vous devriez voir : **`!!ETST`** en couleurs

**Interprétation** :
- `test-minimal.iso` fonctionne → Problème dans le kernel principal
- `test-minimal.iso` ne fonctionne pas → Problème GRUB/VirtualBox

### Étape 6 : Capturer et reporter

1. **Prendre des screenshots** (Périphériques → Prendre une capture d'écran)
   - Menu GRUB
   - Écran avec marqueurs (ou erreurs)

2. **Reporter les résultats** :
   - Quels marqueurs vous voyez exactement
   - Messages d'erreur éventuels
   - Comportement (freeze, reboot, etc.)

## Commandes de nettoyage

Pour supprimer la VM de test après :
1. VirtualBox → Sélectionner `Exo-OS-Test`
2. Clic droit → Supprimer
3. Choisir "Supprimer tous les fichiers"

## Alternative : Ligne de commande VirtualBox

```powershell
# Créer VM
VBoxManage createvm --name "Exo-OS-Test" --ostype "Linux_64" --register
VBoxManage modifyvm "Exo-OS-Test" --memory 512 --boot1 dvd --boot2 none --boot3 none --boot4 none
VBoxManage storagectl "Exo-OS-Test" --name "IDE" --add ide
VBoxManage storageattach "Exo-OS-Test" --storagectl "IDE" --port 0 --device 0 --type dvddrive --medium "C:\Users\Eric\Documents\Exo-OS\build\exo-os-v2.iso"

# Démarrer
VBoxManage startvm "Exo-OS-Test"

# Supprimer (après test)
VBoxManage unregistervm "Exo-OS-Test" --delete
```

## Aide supplémentaire

- Guide complet : `Docs/MANUAL_TEST_INSTRUCTIONS.md`
- Rapport technique : `Docs/TEST_REPORT.md`
- Session debug : `Docs/DEBUG_SESSION_2024-11-12.md`
