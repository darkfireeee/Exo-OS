# ExoOS v0.1.0 "Elder and Bobby" - Rapport de cloture

Date de cloture: 2026-05-12

## Statut

ExoOS v0.1.0 marque le premier jalon ou le systeme atteint une session userspace interactive exploitable sous QEMU. Le noyau boote, les payloads Ring1 sont injectes, `init_server` lance le graphe de services, puis `exosh` devient utilisable comme terminal de travail.

## Resultat valide

| Domaine | Resultat v0.1.0 |
|---|---|
| Boot QEMU | ISO GRUB Multiboot2 bootable sous `qemu-system-x86_64 -machine q35` |
| Services Ring1 | `init_server`, `ipc_router`, `memory_server`, `vfs_server`, `crypto_server`, `device_server`, `virtio_drivers`, `network_server`, `scheduler_server`, `input_server`, `tty_server`, `exo_shield`, `exosh` |
| Shell | Prompt `exosh:/$`, saisie clavier, curseur visible, historique, edition gauche/droite |
| ExoFS | Creation, lecture, ecriture, copie, deplacement, suppression et listage via shell |
| Timekeeping | Calibration PIT acceptee: plus de fallback force `[CAL:FB3G]` quand le PIT repond |
| Diagnostics | Traces kernel bruyantes des chemins `kstack`, `fork`, `execve`, `pf` desactivees par defaut |

Log QEMU valide:

```text
[CAL:PIT-DRV hz=2614777097][TIME-INIT hz=2614800000]
```

Artifact de verification:

- `docs/special/1/qemu_verify/e9.log`

## Commandes shell fonctionnelles

| Commande | Fonction |
|---|---|
| `help` | Affiche les commandes disponibles |
| `clear`, `Ctrl+L` | Nettoie l'ecran |
| `pwd`, `cd` | Navigation |
| `ls`, `ls -l`, `ls -a`, `ls -la`, `ls -lah` | Listing, metadonnees, fichiers caches, tailles humaines |
| `mkdir`, `rmdir` | Creation et suppression de dossiers |
| `touch`, `cat`, `echo`, `echo ... > file` | Fichiers simples |
| `rm`, `rm -f`, `rm -rf`, `rm *` | Suppression simple, forcee, recursive, glob simple |
| `cp`, `mv` | Copie et deplacement de fichiers |
| `tree` | Vue recursive |
| `top`, `ps`, `kill`, `kill -9` | Inspection et controle minimal des processus |
| `history` | Historique des commandes |
| `time` | Mesure d'une commande shell |
| `dd` | Benchmark I/O minimal |
| `exit` | Quitte le shell courant |

## Exemple de session

```text
exosh:/$ mkdir /tmp
exosh:/$ touch /tmp/a
exosh:/$ echo hello > /tmp/a
exosh:/$ cat /tmp/a
hello
exosh:/$ cp /tmp/a /tmp/b
exosh:/$ mv /tmp/b /tmp/c
exosh:/$ ls -lah /tmp
exosh:/$ time echo ok
exosh:/$ dd if=/dev/zero of=/tmp/bench bs=1M count=4
```

## Commandes hote

Build complet:

```bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS
make iso
make qemu
```

Execution sans rebuild quand `exo-os.iso` est deja pret:

```bash
qemu-system-x86_64 \
  -machine q35 \
  -m 256M \
  -boot d \
  -vga std \
  -serial stdio \
  -no-reboot \
  -no-shutdown \
  -d int,cpu_reset -D /tmp/qemu-exoos.log \
  -debugcon file:/tmp/e9k.txt \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0 \
  -cdrom exo-os.iso
```

Lecture du log:

```bash
cat /tmp/e9k.txt
```

## Limites connues apres v0.1.0

- `top` utilise encore une table PID/nom connue cote shell; la prochaine etape est un vrai syscall de liste de processus.
- `cp`, `mv`, `rm` et les globs couvrent le workflow minimal, pas encore tout POSIX.
- Le smoke QMP peut etre sensible au timing clavier hote; les logs kernel restent l'autorite de diagnostic.

