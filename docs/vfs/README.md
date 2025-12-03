# 📁 VFS - Virtual File System

## Vue d'ensemble

Le VFS d'Exo-OS fournit une interface unifiée pour tous les systèmes de fichiers.

## Architecture

```
kernel/src/fs/
├── vfs/              # Virtual File System core
│   ├── mod.rs        # API principale
│   ├── inode.rs      # Structure inode
│   ├── dentry.rs     # Directory entries
│   ├── mount.rs      # Points de montage
│   ├── cache.rs      # Cache d'inodes/dentries
│   └── tmpfs.rs      # TmpFS intégré
├── fat32/            # FAT32
├── ext4/             # ext4
├── tmpfs/            # TmpFS standalone
├── devfs/            # Device filesystem
├── procfs/           # /proc
├── sysfs/            # /sys
└── descriptor.rs     # File descriptors
```

## Systèmes de Fichiers Supportés

| FS | Status | Description |
|----|--------|-------------|
| tmpfs | ✅ | RAM filesystem |
| devfs | ✅ | Devices (/dev) |
| procfs | ✅ | Process info (/proc) |
| sysfs | ✅ | System info (/sys) |
| FAT32 | 🔄 | Disques USB, SD |
| ext4 | 🔄 | Disques Linux |

## Modules

- [Inodes](./inodes.md)
- [Dentries](./dentries.md)
- [Mount Points](./mount.md)
- [File Descriptors](./descriptors.md)
