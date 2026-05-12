# Runbook boot payloads et QEMU

## Objectif

Garantir qu'une machine Linux/WSL puisse reconstruire ou lancer ExoOS v0.1.0 avec les memes conventions que l'environnement de validation.

## Prerequis hote

```bash
sudo apt update
sudo apt install -y build-essential qemu-system-x86 xorriso grub-pc-bin grub-common mtools llvm
rustup toolchain install nightly
rustup component add rust-src rustfmt clippy llvm-tools-preview --toolchain nightly
```

## Pipeline de build

La cible `make iso` declenche:

```text
build-boot-payloads
  -> cargo build des serveurs Ring1 pour x86_64-exo-userspace
  -> copie dans target/boot-payloads-stripped
  -> strip des symboles debug
  -> build kernel A
  -> build kernel B avec EXO_BOOT_PAYLOAD_DIR
  -> grub-mkrescue vers exo-os.iso
```

Le stripping des payloads evite d'injecter des binaires Rust avec symboles debug dans l'image de boot. Les payloads valides sont:

```text
exo-init-server
exo-ipc-router
exo-memory-server
exo-vfs-server
exo-crypto-server
exo-device-server
exo-virtio-drivers
exo-network-server
exo-scheduler-server
exo-input-server
exo-tty-server
exosh
exo-shield
```

## Build complet

```bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS
make iso
```

## Lancement avec rebuild

```bash
make qemu
```

## Lancement sans rebuild

Utiliser cette commande quand `exo-os.iso` et `target/qemu/exofs-root.img` existent deja:

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

## Logs

| Fichier | Contenu |
|---|---|
| `/tmp/e9k.txt` | Debugcon port E9: boot, services, shell |
| `/tmp/qemu-exoos.log` | Trace QEMU `-d int,cpu_reset` |
| serial stdio | Console QEMU visible |

Lire le log principal:

```bash
cat /tmp/e9k.txt
```

## Smoke automatise

```bash
make qemu-shell-smoke
```

Ce test pilote QEMU via QMP, envoie des commandes shell, puis verifie le log E9. Les sockets QMP doivent rester dans `/tmp`; WSL ne supporte pas les sockets Unix sous `/mnt/c`.

## Debug avance

Activer les traces kernel bruyantes uniquement pour enquete:

```bash
EXO_KERNEL_TRACE=1 make iso
```

Le mode normal doit rester lisible et ne pas imprimer les traces `kstack`, `fork_dbg`, `execve` ou `pf`.

