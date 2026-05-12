# Rapport userspace - console, clavier et shell

## Objectif

Rendre le terminal ExoOS réellement utilisable sous QEMU, depuis le premier prompt `exosh:/$`, sans dependre d'un serveur graphique ou d'un outil externe.

## Chemin fonctionnel

```text
kernel terminal
  -> stdio process
  -> init_server
  -> service graph Ring1
  -> exosh
  -> ExoFS syscalls
```

Le noyau fournit le chemin console minimal: ecriture visible, miroir debugcon E9, lecture clavier PS/2, sequences ANSI pour les fleches. `exosh` consomme ce flux comme un terminal interactif.

## Edition de ligne

Fonctionnalites validees:

| Entree | Effet |
|---|---|
| Texte ASCII | Insertion a la position du curseur |
| Backspace | Supprime le caractere avant le curseur |
| Fleche gauche/droite | Deplace le curseur dans la ligne |
| Fleche haut/bas | Rappel de l'historique |
| Ctrl+L | Efface l'ecran |
| Ctrl+C / Ctrl+D | Annule la ligne |
| Curseur reverse-video | Rend la position d'ecriture visible |

Le curseur est rendu par sequence ANSI reverse-video. La ligne est redessinee apres chaque modification pour garder le prompt, le texte et les espaces de nettoyage coherents.

## Builtins disponibles

```text
help clear pwd cd ls mkdir touch cat echo rm cp mv rmdir tree top ps kill history time dd exit
```

Options et sous-commandes importantes:

- `ls -l`, `ls -a`, `ls -la`, `ls -lah`
- coloration des dossiers en bleu
- `rm -f`, `rm -rf`, `rm *`
- `echo texte > fichier`
- `dd if=/dev/zero of=/tmp/bench bs=1M count=4`
- `dd if=/tmp/bench of=/dev/null bs=1M`
- `kill <pid>`, `kill -9 <pid>`

## Correction des commandes fichier

`cp` et `mv` gerent maintenant le cas courant ou la destination est un dossier:

```text
cp /tmp/a /tmp/d
mv /tmp/a /tmp/d
```

Dans ce cas, le nom de base de la source est ajoute au dossier destination. Cela evite les erreurs de type `-21` observees avant la correction.

## Interaction avec ExoFS

Les commandes shell valident les appels minimaux suivants:

- `openat`
- `read`
- `write`
- `getdents64`
- `mkdir`
- `rmdir`
- `unlink`
- `rename`
- `chdir`
- `stat`
- `clock_gettime`

Ce chemin confirme que le shell n'est plus seulement un affichage: il exerce vraiment le VFS/ExoFS, les descripteurs de fichiers et le routage syscall.

## Fichiers principaux

| Fichier | Role |
|---|---|
| `kernel/src/arch/x86_64/terminal.rs` | Console visible, debugcon E9, clavier PS/2, fleches ANSI |
| `kernel/src/arch/x86_64/framebuffer_early.rs` | Rendu ANSI, dont reverse-video pour le curseur |
| `servers/exosh/src/main.rs` | Shell, builtins, edition de ligne, historique, I/O |
| `scripts/qemu/shell_smoke_qmp.sh` | Smoke QEMU automatise |

