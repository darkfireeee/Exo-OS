# 📋 Changelog - Exo-OS v0.5.0

**Date:** 3 décembre 2025  
**Version:** 0.5.0 "Shell Complete"

## 🎯 Highlights

Cette version apporte un shell interactif complet et une réorganisation majeure du kernel pour améliorer la maintenabilité.

## ✨ Nouvelles fonctionnalités

### Shell interactif (Exo-Shell v0.5.0)

- **Shell complet no_std** : Implémentation pure Rust sans dépendances
- **Interface utilisateur ANSI** : Support couleurs et prompt coloré
- **Édition de ligne** : Backspace, Ctrl+C (interrupt), Ctrl+D (EOF)
- **Syscalls directs** : Communication directe avec le kernel via SYSCALL/SYSRET

### Commandes built-in
- `help` - Affiche l'aide
- `exit [code]` - Quitte le shell
- `clear` - Efface l'écran
- `echo [args...]` - Affiche les arguments
- `pwd` - Répertoire courant
- `cd <dir>` - Change de répertoire
- `version` - Info version

### Commandes fichiers (VFS)
- `ls [dir]` - Liste les fichiers
- `cat <file>` - Affiche un fichier
- `mkdir <dir>` - Crée un répertoire
- `rm <file>` - Supprime un fichier
- `rmdir <dir>` - Supprime un répertoire vide
- `touch <file>` - Crée un fichier vide
- `write <file> <text>` - Écrit dans un fichier

## 🔧 Améliorations

### Nettoyage du kernel

- **Fusion PIC** : `pic.rs` fusionné dans `pic_wrapper.rs` (redondance éliminée)
- **Suppression keyboard FFI** : Bindings obsolètes retirés de `ffi/bindings.rs`
- **Fichiers backup** : `main.rs.bak` supprimé
- **Organisation modules** : Structure arch/x86_64 nettoyée

### VFS (Virtual File System)

- VFS déjà complet avec tmpfs
- Support full POSIX : open, close, read, write, mkdir, unlink, rmdir
- Handles de fichiers globaux
- Cache de paths
- Symlinks

## 🐛 Corrections

- Doublons de modules PIC éliminés
- Références FFI keyboard obsolètes retirées
- Imports inutilisés nettoyés

## 📊 Statistiques

- **Lignes de code shell** : ~800 lignes
- **Commandes implémentées** : 14 commandes (7 built-in + 7 VFS)
- **Warnings réduits** : 194 → À optimiser dans v0.6.0
- **Fichiers supprimés** : 3 (pic.rs, main.rs.bak, keyboard.c)

## 🚀 Prochaines étapes (v0.6.0)

1. **Tests QEMU** : Valider le shell en environnement réel
2. **Programmes userspace** : Créer /bin/hello et autres exécutables
3. **Fork/Exec** : Implémenter pour lancer des processus externes
4. **SMP réactivation** : Multi-core après tests shell
5. **Corrections warnings** : Nettoyer les 194 warnings restants

## 📝 Notes de migration

### Pour les développeurs

- Utiliser `pic_wrapper` au lieu de `pic` pour les interruptions
- Les bindings keyboard FFI n'existent plus (driver Rust pur)
- Le shell utilise des syscalls Linux x86_64 standards

### Pour les utilisateurs

- Le shell démarre automatiquement après boot
- Utiliser `help` pour découvrir les commandes
- Ctrl+D pour quitter proprement

## 🔗 Liens

- Commit principal : refactor: Clean kernel structure and implement complete shell v0.5.0
- Documentation shell : `/userland/shell/src/`
- VFS API : `/kernel/src/fs/vfs/mod.rs`

---

**Version précédente** : [v0.4.0](CHANGELOG_v0.4.0.md)  
**Prochaine version** : v0.6.0 (prévue après tests QEMU)
