# Boot Validation 2026-04-24

## Etat actuel

Le noyau Exo-OS atteint maintenant la fin de `kernel_main()` sous QEMU et émet `OK` sur le port debug `0xE9`.

Les étapes visibles réellement franchies sont :

- `ARCH`
- `MEMORY`
- `TIME`
- `DRIVERS`
- `SCHEDULER`
- `PROCESS`
- `SECURITY`
- `IPC`
- `FS`

Le chemin de validation ne repose plus uniquement sur des traces internes temporaires. Les sondes profondes ajoutées pendant le débogage du bootstrap ont été retirées des chemins TCB / heap / SLUB / création de thread. Il ne reste que les marqueurs de phase de haut niveau dans [lib.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/lib.rs), [main.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/main.rs) et [security/mod.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/security/mod.rs).

## Blocages déjà corrigés

Les arrêts réels observés pendant l’audit bootstrap étaient :

1. Allocation du premier TCB via SLUB.
2. Double initialisation sécurité à cause d’un `SECURITY_READY` jamais atteint quand aucun NIC n’était présent.
3. Explosion mémoire dans le backend mock `virtio-blk` à cause d’un faux disque contigu de 512 MiB.

Ces points sont désormais corrigés respectivement dans :

- [slab.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/memory/physical/allocator/slab.rs)
- [slub.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/memory/physical/allocator/slub.rs)
- [memory_map.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/arch/x86_64/boot/memory_map.rs)
- [exoseal.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/security/exoseal.rs)
- [virtio_blk/lib.rs](/C:/Users/xavie/Desktop/Exo-OS/drivers/storage/virtio_blk/src/lib.rs)
- [virtio_adapter.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/fs/exofs/storage/virtio_adapter.rs)

## Cablage noyau confirmé

Les points suivants sont confirmés comme réellement câblés et appelés au boot :

- Initialisation IPC, hooks scheduler/VMM : [lib.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/lib.rs)
- Bridge FS syscall : [fs_bridge.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/syscall/fs_bridge.rs)
- IPC 300-305 sur backend raw/RPC réel : [table.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/syscall/table.rs), [raw.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/ipc/channel/raw.rs), [rpc/raw.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/ipc/rpc/raw.rs)
- Boot display VGA + framebuffer : [boot_display.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/arch/x86_64/boot_display.rs), [framebuffer_early.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/arch/x86_64/framebuffer_early.rs)

## Pourquoi VirtualBox montre parfois seulement GRUB

La cause la plus probable est la combinaison suivante :

- GRUB démarre en mode graphique via `gfxterm`.
- `gfxpayload=keep` demande de conserver ce mode.
- Sous QEMU `-vga std`, le noyau reçoit bien un framebuffer exploitable et prend le relais.
- Sous VirtualBox, il est plausible que le framebuffer Multiboot2 attendu par le noyau ne soit pas fourni ou pas conservé de manière compatible.

Dans ce cas, le noyau continue sur son fallback VGA texte, mais ce fallback n’est pas forcément visible si l’affichage reste figé sur le shell graphique GRUB.

Pour cela, une entrée dédiée a été ajoutée dans [grub.cfg](/C:/Users/xavie/Desktop/Exo-OS/bootloader/grub.cfg) :

- `Exo-OS v0.1.0 (VirtualBox / texte)`

Cette entrée force `gfxpayload=text` et `terminal_output console`, ce qui rend le fallback VGA visible même si VirtualBox ne préserve pas le framebuffer graphique utilisé sous QEMU.

## Scripts et captures

Les scripts QEMU ont été rangés dans :

- [scripts/qemu/capture_e9_boot.sh](/C:/Users/xavie/Desktop/Exo-OS/scripts/qemu/capture_e9_boot.sh)
- [scripts/qemu/capture_boot_detached.sh](/C:/Users/xavie/Desktop/Exo-OS/scripts/qemu/capture_boot_detached.sh)
- [scripts/qemu/capture_boot_framebuffer.sh](/C:/Users/xavie/Desktop/Exo-OS/scripts/qemu/capture_boot_framebuffer.sh)

Les captures générées sont maintenant rangées dans :

- [docs/avancement/qemu_boot](/C:/Users/xavie/Desktop/Exo-OS/docs/avancement/qemu_boot)

## Captures

Capture framebuffer QEMU la plus récente :

![QEMU boot framebuffer](/C:/Users/xavie/Desktop/Exo-OS/docs/avancement/qemu_boot/exoos-qemu-latest.png)

## Validation effectuée

Sous WSL :

- `cargo check -p exo-os-kernel --message-format short`
- `cargo test -p exo-virtio-blk sparse_backend_roundtrip_persists_written_block -- --nocapture`
- `cargo test -p exo-virtio-blk sparse_backend_stress_multiple_blocks_roundtrip -- --nocapture`
- `cargo test -p exo-os-kernel --lib security_ready_store_load_contract -- --nocapture`
- `cargo test -p exo-os-kernel --lib test_01_iommu_queue_hft_smp_stress -- --nocapture`
- `scripts/qemu/capture_boot_framebuffer.sh`

Le dernier boot QEMU validé aboutit toujours à `OK`.

## Ecarts encore ouverts

Les écarts réels encore visibles après cette passe sont :

- `sys_vfork()` reste en `ENOSYS` dans [table.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/syscall/table.rs) ; ce n’est pas encore une implémentation complète.
- `fork` et `execve` restent `ENOSYS` dans la table brute mais passent bien par le dispatch spécial dans [dispatch.rs](/C:/Users/xavie/Desktop/Exo-OS/kernel/src/syscall/dispatch.rs).
- Le FS n’est pas encore POSIX complet pour les vrais liens symboliques.
- Les warnings build-script ExoPhoenix sur `KERNEL_A_IMAGE_HASH` et `KERNEL_A_MERKLE_ROOT` restent présents tant que ces variables ne sont pas fournies.
