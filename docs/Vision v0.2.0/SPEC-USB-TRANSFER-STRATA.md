# SPEC-USB-TRANSFER-STRATA — Pipeline USB → ExoFS
## Transferts de Fichiers via Clé USB — ExoOS v0.2.0 Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** NOUVEAU

---

## 1. Objectif

ExoOS Strata est un ordinateur complet. Une clé USB s'insère et fonctionne.
Les fichiers se transfèrent. L'audit ExoLedger trace chaque opération.
ExoShield scanne automatiquement tout ce qui arrive depuis l'extérieur.

Aucune commande supplémentaire n'est nécessaire si le comportement auto-mount est activé. L'utilisateur peut aussi piloter manuellement.

---

## 2. Pipeline Complet : Insertion à Transfert

```
┌─────────────────────────────────────────────────────────────────┐
│  COUCHE HARDWARE                                                │
│                                                                 │
│  Clé USB insérée                                                │
│    → XHCI détecte CCS (Connected) sur le port                  │
│    → Enumération : SET_ADDRESS + GET_DESCRIPTOR + SET_CONFIG    │
│    → Interface classe 0x08 (Mass Storage) détectée             │
│    → BBB protocol identifié                                     │
│    → SCSI INQUIRY → vendor/product string                      │
│    → SCSI READ_CAPACITY → total_sectors, sector_size           │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼ IPC : UsbEvent::MassStorageAttached
┌─────────────────────────────────────────────────────────────────┐
│  device_server (Ring1)                                          │
│                                                                 │
│  Reçoit MassStorageAttached { address, total_sectors, ... }     │
│  Génère device_id unique                                        │
│  Log ExoLedger : USB_DEVICE_ATTACHED                           │
│  → IPC vers vfs_server : MOUNT_REQUEST (device_id, auto=true)  │
│  → IPC vers exo_shield : SCAN_REQUEST (device_id)              │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼ IPC : MOUNT_REQUEST
┌─────────────────────────────────────────────────────────────────┐
│  vfs_server (Ring1)                                             │
│                                                                 │
│  probe_filesystem(device_id) :                                  │
│    1. Lire secteur 0 + secteur 1 (GPT ou MBR)                   │
│    2. Détecter type FS :                                        │
│       ├─ GPT + GUID_EXOOS_ROOT  → ExoFS natif (vfs_server)     │
│       ├─ FAT32 (signature 0xAA55 + BPB) → fat_server           │
│       └─ Inconnu → MOUNT_FAILED + log + message exosh          │
│    3. Monter sous /mnt/usb (ExoFS relation typée "usb_mount")  │
│    4. Générer CapToken pour /mnt/usb                            │
│    5. Notifier exosh : mount point disponible                   │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼ Mount OK
┌─────────────────────────────────────────────────────────────────┐
│  exo_shield (Ring1) — scan en parallèle                        │
│                                                                 │
│  Sur MOUNT_OK :                                                 │
│    1. Scan racine : engine::scanner::scan_directory("/mnt/usb") │
│    2. Chaque fichier exécutable : YARA + heuristiques           │
│    3. Si menace détectée : QUARANTINE + alerte sonore           │
│    4. Si clean : marquer objet ExoFS avec tag "usb_scanned"     │
│    5. Log ExoLedger : USB_SCAN_COMPLETE + summary               │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼ exosh disponible
┌─────────────────────────────────────────────────────────────────┐
│  exosh — Interface Utilisateur                                  │
│                                                                 │
│  Notification : "USB monté : /mnt/usb (FAT32, 15.9 GiB)"      │
│  Commandes disponibles :                                        │
│    exo ls /mnt/usb                                              │
│    exo cp /mnt/usb/app.elf /apps/                               │
│    exo hash /mnt/usb/app.elf                                    │
│    exo verify /mnt/usb/app.elf                                  │
│    exo umount /mnt/usb                                          │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Commandes exosh — Spécification

### `exo ls /mnt/usb`

Format d'affichage capability natif (jamais rwx) :

```
/mnt/usb [FAT32] [15.9 GiB used: 2.3 GiB]
────────────────────────────────────────────────────────
d  ----l--  [usb]    @0000  ep:----  4 entries   --------  docs/
x  r-x----  [usb]    @0000  ep:----  2.1 MiB     9e4a72f1  app.elf  [scanned ✓]
f  rw-----  [usb]    @0000  ep:----  450 KiB     bc38f091  data.db
f  rw-----  [usb]    @0000  ep:----  12 KiB      --------  notes.txt
```

Légende du tag de scan :
- `[scanned ✓]` : scanné par ExoShield, aucune menace
- `[scanned ⚠]` : menace détectée (transfert bloqué)
- `[unscanned]` : scan en cours ou non encore scanné

### `exo cp /mnt/usb/app.elf /apps/app.elf`

```
Séquence interne :
1. Vérifier CapToken source (/mnt/usb accessible)
2. Vérifier CapToken destination (/apps/ accessible)
3. Vérifier tag scan : si "unscanned" → attendre scan ou forcer avec --force
4. Si tag "threat" → REFUS sauf --override-shield (log ExoLedger + confirmation)
5. read_at(src) → write_at(dst) avec hash continu
6. Vérifier hash final source == hash final destination
7. ExoLedger : USB_TRANSFER event
8. Créer CapToken pour /apps/app.elf
9. Afficher : "Copié : app.elf (2.1 MiB) → /apps/app.elf [hash: 9e4a72f1]"
```

### `exo hash /mnt/usb/app.elf`

```
Calcule BLAKE3 sur le fichier source.
Affiche : "BLAKE3: 9e4a72f1c3d8b4a2e7f9012345678901234567890123456789012345678901234"
```

### `exo verify /mnt/usb/app.elf`

```
Vérifie la signature Ed25519 embarquée dans l'ELF.
Clé publique de vérification : /etc/exoos/trusted_signing.pub
Affiche :
  "✓ Signature valide — signée par ExoOS Dev Key"
  "✗ Signature invalide — exécution refusée"
  "⚠ Pas de signature — non-signé (nécessite --allow-unsigned pour installer)"
```

### `exo umount /mnt/usb`

```
Séquence :
1. Vérifier qu'aucune opération de copie n'est en cours
2. flush ExoFS (sync toutes les écritures en attente sur la clé)
3. Fermer tous les file handles ouverts sur /mnt/usb
4. vfs_server : détacher le mount point
5. device_server : DEVICE_DETACH signal vers USB driver
6. ExoLedger : USB_UNMOUNTED event
7. Afficher : "USB éjecté proprement. Vous pouvez retirer la clé."
```

### Éjection physique sans `exo umount`

```
USB driver détecte déconnexion (PORTSC.CCS = 0)
  → device_server : DEVICE_DETACHED event
  → vfs_server : force_umount("/mnt/usb")
      → Flush buffers si possible
      → Si écriture en cours : marquer ExoFS "dirty" + log WARNING
  → ExoLedger : USB_UNEXPECTED_REMOVAL
  → exosh : "⚠ USB retiré sans éjection propre. Données possiblement perdues."
```

---

## 4. Audit ExoLedger — Entrées USB

Chaque opération USB génère une entrée ExoLedger chainée BLAKE3.

```
[timestamp] USB_DEVICE_ATTACHED
    vendor=SanDisk product=Ultra product_id=0x5571
    total_sectors=31457280 sector_size=512 (15.9 GiB)
    device_id=usb:4:1

[timestamp] USB_SCAN_STARTED device_id=usb:4:1

[timestamp] USB_SCAN_COMPLETE device_id=usb:4:1
    files_scanned=47 threats=0 suspicious=0
    duration_ms=1240

[timestamp] USB_MOUNT_OK device_id=usb:4:1
    mount_point=/mnt/usb fs=FAT32

[timestamp] USB_TRANSFER
    src=/mnt/usb/app.elf
    dst=/apps/app.elf
    hash_src=9e4a72f1... hash_dst=9e4a72f1...
    size=2203648 pid=42 cap=@3d8f
    duration_ms=380

[timestamp] USB_UNMOUNTED device_id=usb:4:1
    mount_point=/mnt/usb reason=user_request
```

---

## 5. Politique de Sécurité USB — ExoShield

### Politique par défaut (configurable dans `/etc/exoshield/usb_policy.toml`)

```toml
[usb]
# Scan automatique à l'insertion
auto_scan = true

# Bloquer les exécutables non-scannés
block_unsigned_executables = false    # warning uniquement en v0.2.0
# block_unsigned_executables = true   # activer en production

# Bloquer les transferts si menace détectée
block_on_threat = true

# Scan en arrière-plan si clé volumineuse (> 1 GiB)
background_scan_threshold_gib = 1

# Éjection automatique si CRITICAL threat détectée
auto_eject_on_critical = false        # off par défaut (choix utilisateur)
```

### Niveaux de réponse

| Résultat scan | Comportement |
|---|---|
| CLEAN | Transfert autorisé, tag `scanned ✓` |
| SUSPICIOUS | Warning affiché, transfert autorisé avec confirmation |
| THREAT_LOW | Warning + log ExoLedger, transfert avec `--force` seulement |
| THREAT_HIGH | Blocage + beep × 3 + log ExoLedger + alerte exosh |
| THREAT_CRITICAL | Blocage + bip long + log ExoLedger + force umount |

---

## 6. Tests Requis

```
usb_transfer_test::insert_fat32_usb_mounts         PASS
usb_transfer_test::insert_exofs_usb_mounts         PASS
usb_transfer_test::auto_scan_on_mount              PASS
usb_transfer_test::copy_clean_file                 PASS
usb_transfer_test::copy_hash_verified              PASS
usb_transfer_test::copy_threat_blocked             PASS
usb_transfer_test::umount_flushes_writes           PASS
usb_transfer_test::unexpected_removal_logged       PASS
usb_transfer_test::exoledger_entries_chained       PASS
usb_transfer_test::format_display_no_rwx           PASS
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-USB-TRANSFER-STRATA.md*
