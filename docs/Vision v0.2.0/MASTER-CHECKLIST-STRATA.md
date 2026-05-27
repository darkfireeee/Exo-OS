# MASTER-CHECKLIST-STRATA — Critères de Validation Complets
## ExoOS v0.2.0 — Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE VIVANTE — remplace MASTER-CHECKLIST-V0.2-REV2.md

---

## Légende

```
[ ] → À faire
[x] → Validé
[-] → Non applicable ou hors périmètre Strata
[!] → Bloquant non résolu
```

---

## BLOC -1 — Bugs Kernel Bloquants (10/10 requis avant Phase 0)

```
[ ] B-01 : VirtIO BAR lu depuis PCI config space (CORR-86)
[ ] B-02 : ExoFS persistant après reboot
[ ] B-03 : Boot avec -m 2G sans panic (CORR-76)
[ ] B-04 : phys_to_virt() valide pour adresses > 1 GiB
[ ] B-05 : cgroup::init() avant runqueue_init() (CORR-77)
[ ] B-06 : Ring1 servers attachés au root cgroup sans crash
[ ] B-07 : ELF base 0x400000 accepté par ELF loader (CORR-80)
[ ] B-08 : const_assert!(USER_ELF_BASE_MIN <= 0x400000)
[ ] B-09 : MSG len==128 sans cap → PolicyDenied (CORR-78)
[ ] B-10 : exosh démarre sans network_server (CORR-79)
```

## BLOC 0 — Outillage Audit (13/13 requis avant Phase 0)

```
[ ] O-01 : arch/constants.rs — constantes canoniques
[ ] O-02 : const_assert! ssr.rs (SSR size ≤ 4096)
[ ] O-03 : const_assert! exokairos.rs
[ ] O-04 : const_assert! physmap.rs (≥ 1 GiB coverage)
[ ] O-05 : const_assert! CORE_MASK_WORDS × 64 == MAX_CORES_LAYOUT
[ ] O-06 : tools/audit_constants.py créé
[ ] O-07 : audit_constants.py → 0 erreurs sur kernel/
[ ] O-08 : semgrep-rules/exoos.yaml créé
[ ] O-09 : semgrep → 0 violations kernel/
[ ] O-10 : deny.toml configuré
[ ] O-11 : cargo deny check → 0 violations
[ ] O-12 : pre-commit hook actif
[ ] O-13 : .github/workflows/audit.yml créé
```

---

## PHASE 0 — Fondations Runtime

```
[ ] 0.1 : SSR bitmask → [u64; CORE_MASK_WORDS] (256-core)
[ ] 0.2 : phoenix_test::ssr_bitmask_256_cores PASS
[ ] 0.3 : exo-alloc : DlmallocAllocator<Exo> compile
[ ] 0.4 : #[global_allocator] → exo-alloc (zéro libc)
[ ] 0.5 : exo_alloc_test :: 5 tests PASS
[ ] 0.6 : generic-rt : __tls_get_addr implémenté
[ ] 0.7 : gs:[0x20] current TCB accessible Ring3
[ ] 0.8 : TLS initialisées avant main()
[ ] 0.9 : TLS survit à clone()
```

## PHASE 1 — Chaîne de Sécurité Active

```
[ ] 1.1  : ExoSeal vérifie boot chain au démarrage
[ ] 1.2  : Hash kernel et ring1 vérifiés (ou DEV_BYPASS loggé)
[ ] 1.3  : SMEP actif sur BSP + tous APs
[ ] 1.4  : SMAP actif
[ ] 1.5  : KPTI actif
[ ] 1.6  : CET Shadow Stack actif
[ ] 1.7  : CET IBT actif
[ ] 1.8  : NX/XD actif
[ ] 1.9  : IBRS + SSBD actifs
[ ] 1.10 : exocage_verify_active() PASS
[ ] 1.11 : ZeroTrustLabel sur chaque message IPC
[ ] 1.12 : zero_trust::check_ipc() sur chaque ipc_send
[ ] 1.13 : Ring3→Ring3 direct IPC bloqué
[ ] 1.14 : CapToken vérifie chaque accès FS
[ ] 1.15 : Révocation cap → propagation immédiate
[ ] 1.16 : ExoKairos budget init à chaque new Ring3 process
[ ] 1.17 : Throttle à 100% budget
[ ] 1.18 : Kill à 200% cumulé
[ ] 1.19 : ExoLedger persiste dans ExoFS (sealed)
[ ] 1.20 : ExoLedger chaîne BLAKE3 vérifiée au boot
[ ] 1.21 : exo audit → 0 rupture de chaîne
[ ] 1.22 : IOMMU domaines NET/BLOCK/BLACKHOLE actifs
[ ] 1.23 : IommuFaultQueue CAS-strong actif
[ ] 1.24 : ExoNMI watchdog armé 200ms
[ ] 1.25 : security_test :: 12/12 PASS (pré-ExoShield Ring1)
```

## PHASE 2 — Crypto & Réseau

```
[ ] 2.1 : AES-GCM-256 round-trip PASS
[ ] 2.2 : ChaCha20-Poly1305 round-trip PASS
[ ] 2.3 : BLAKE3 vecteurs NIST PASS
[ ] 2.4 : Ed25519 sign/verify PASS
[ ] 2.5 : Argon2id hash PASS
[ ] 2.6 : Clés privées non exportables hors crypto_server
[ ] 2.7 : smoltcp IPv4 + IPv6 actif
[ ] 2.8 : DHCP automatique (dhcp4r) PASS
[ ] 2.9 : DNS résolution (hickory-dns) PASS
[ ] 2.10 : TCP connect PASS
[ ] 2.11 : UDP send/recv PASS
[ ] 2.12 : Zéro panic! réseau (tous chemins d'erreur propagés)
[ ] 2.13 : rustls TLS 1.3 — connexion établie
[ ] 2.14 : TLS 1.2 désactivé
```

## PHASE 3 — Filesystem

```
[ ] 3.1 : fsck phase 1 PASS (inode scan)
[ ] 3.2 : fsck phase 2 PASS (directory check)
[ ] 3.3 : fsck phase 3 PASS (connectivity)
[ ] 3.4 : fsck phase 4 PASS (bad block)
[ ] 3.5 : Recovery auto après crash simulé
[ ] 3.6 : open/close/read_at/write_at PASS
[ ] 3.7 : mkdir/rmdir/unlink/rename PASS
[ ] 3.8 : SYS_GETDENTS64 = 217 actif
[ ] 3.9 : SYS_GETCWD = 79 actif
[ ] 3.10 : Epochs : commit atomique PASS
[ ] 3.11 : snapshot_create() PASS
[ ] 3.12 : relation_create() PASS
[ ] 3.13 : fat_server : monter FAT32 QEMU PASS
[ ] 3.14 : fat_server : lire/écrire fichier FAT32 PASS
```

## PHASE 4 — Processus & POSIX

```
[ ] 4.1 : TLB shootdown deadlock résolu (skip single-CPU)
[ ] 4.2 : VMA tree cloné dans fork()
[ ] 4.3 : KERNEL_FAULT_ALLOC : vérifier CR3 avant CoW
[ ] 4.4 : fork_exec_wait test PASS
[ ] 4.5 : musl-exo : 127 syscalls priorité 1+2 compilent
[ ] 4.6 : socket_tcp_connect PASS
[ ] 4.7 : getdents64 PASS
[ ] 4.8 : SYS_CLONE avec CLONE_THREAD PASS
[ ] 4.9 : SYS_FUTEX FUTEX_WAIT + FUTEX_WAKE PASS
```

## PHASE 5 — Drivers Bare Metal

```
# AHCI
[ ] 5.1 : ahci_test::detect_sata_controller PASS
[ ] 5.2 : ahci_test::read_first_sector PASS
[ ] 5.3 : ahci_test::write_read_roundtrip PASS

# NVMe
[ ] 5.4 : nvme_test::detect_nvme_controller PASS
[ ] 5.5 : nvme_test::identify_controller PASS
[ ] 5.6 : nvme_test::identify_namespace PASS
[ ] 5.7 : nvme_test::read_first_block PASS
[ ] 5.8 : nvme_test::write_read_roundtrip PASS

# USB
[ ] 5.9  : usb_test::xhci_init PASS
[ ] 5.10 : usb_test::enumerate_keyboard PASS
[ ] 5.11 : usb_test::enumerate_mass_storage PASS
[ ] 5.12 : usb_test::mass_storage_read_first_sector PASS
[ ] 5.13 : usb_test::hid_keyboard_event_received PASS

# Audio
[ ] 5.14 : audio_test::hda_init PASS
[ ] 5.15 : audio_test::virtio_sound_init PASS
[ ] 5.16 : audio_test::play_boot_chime PASS
[ ] 5.17 : audio_test::synthesize_beep_800hz PASS

# Clock
[ ] 5.18 : clock_test::rtc_read_valid_datetime PASS
[ ] 5.19 : clock_test::hpet_counter_running PASS
[ ] 5.20 : clock_test::hpet_1ms_interrupt PASS
```

## PHASE 6 — ExoShield Ring1

```
[ ] 6.1 : exo_shield démarre en Vague 5 (après tous Ring1)
[ ] 6.2 : hooks syscall/exec/memory/net enregistrés
[ ] 6.3 : Signatures YARA chargées au boot
[ ] 6.4 : IPC gate policy persistée ExoFS
[ ] 6.5 : Audit ring buffer 4096 entrées actif
[ ] 6.6 : Sandbox appliquée aux processus exo compat
[ ] 6.7 : Syscall filter par manifest PASS
[ ] 6.8 : FS restriction par manifest PASS
[ ] 6.9 : Net isolation par manifest PASS
[ ] 6.10 : Firewall default-deny actif
[ ] 6.11 : DNS guard actif
[ ] 6.12 : ML inference modèle v0 actif
[ ] 6.13 : PhoenixSafe on_pre_switch() PASS
[ ] 6.14 : PhoenixSafe on_post_switch() PASS
[ ] 6.15 : exo_shield scan initial Ring1 = 0 threats
[ ] 6.16 : security_test::exoshield_sandbox_escape_blocked PASS
[ ] 6.17 : Alerte audio HIGH (3 bips) déclenchée correctement
[ ] 6.18 : Alerte audio CRITICAL (ton long) déclenchée correctement
```

## PHASE 7 — Installeur PKG

```
[ ] 7.1 : exo install <pkg> → exécuté sans crash
[ ] 7.2 : exo compat install calendar PASS
[ ] 7.3 : exo compat run calendar → calendrier affiché
[ ] 7.4 : exo compat install curl PASS
[ ] 7.5 : curl https://example.com → 200 OK
[ ] 7.6 : exo remove <pkg> → caps révoquées + ExoFS nettoyé
[ ] 7.7 : Manifest capabilities affiché avant confirmation
[ ] 7.8 : Signature pkg vérifiée avant installation
```

## PHASE 8 — Bootloader UEFI GPT

```
[ ] 8.1 : bootloader_test::gpt_header_valid_crc PASS
[ ] 8.2 : bootloader_test::gpt_find_esp_partition PASS
[ ] 8.3 : bootloader_test::gpt_find_exofs_root PASS
[ ] 8.4 : bootloader_test::gpt_find_exofs_data PASS
[ ] 8.5 : bootloader_test::gpt_backup_recovery PASS
[ ] 8.6 : BootInfo v2 magic correct
[ ] 8.7 : BootInfo v2 exofs_root_phys non zéro
[ ] 8.8 : BootInfo v2 exofs_data_phys non zéro
[ ] 8.9 : Signature kernel vérifiée PASS
[ ] 8.10 : KASLR offset non zéro
[ ] 8.11 : Boot depuis QEMU OVMF PASS
[ ] 8.12 : NVRAM UEFI : entrée ExoOS créée
```

## PHASE 9 — USB Transfer + Audio Système

```
[ ] 9.1 : Clé FAT32 insérée → /mnt/usb monté automatiquement
[ ] 9.2 : Clé ExoFS insérée → /mnt/usb monté automatiquement
[ ] 9.3 : Scan automatique USB au mount
[ ] 9.4 : exo ls /mnt/usb → format capability natif (pas rwx)
[ ] 9.5 : exo cp /mnt/usb/file /apps/ → transfert réussi
[ ] 9.6 : Hash source == hash destination après copie
[ ] 9.7 : ExoLedger : USB_TRANSFER event chainé
[ ] 9.8 : Fichier avec menace → transfert bloqué
[ ] 9.9 : exo umount /mnt/usb → éjection propre
[ ] 9.10 : Retrait physique → log USB_UNEXPECTED_REMOVAL
[ ] 9.11 : Boot chime joué après exosh prêt
[ ] 9.12 : BEL(0x07) → beep tty PASS
[ ] 9.13 : audio_test::silent_fallback_on_no_hardware PASS
```

## PHASE 10 — Shell & Framebuffer

```
[ ] 10.1 : fb_server : blit SHM Ring3 PASS
[ ] 10.2 : fb_server : double buffering (pas de tearing)
[ ] 10.3 : exosh : prompt visible sans dépendance réseau
[ ] 10.4 : exosh : exo ls fonctionne
[ ] 10.5 : exosh : exo cp fonctionne
[ ] 10.6 : exosh : exo mount/umount fonctionne
[ ] 10.7 : exosh : format affichage capability natif (jamais rwx)
[ ] 10.8 : exosh : pas de crash sur entrée invalide
[ ] 10.9 : exosh : bell sur erreur (audio_server)
```

## PHASE 11 — Observabilité & Qualité

```
[ ] 11.1 : monitor_server : logs persistés ExoFS
[ ] 11.2 : exo log --filter shield PASS
[ ] 11.3 : exo metrics → CPU/mémoire/IPC/réseau affichés
[ ] 11.4 : security_test :: 13/13 PASS (+ sandbox_escape_blocked)
[ ] 11.5 : ExoPhoenix : bascule A→B < 500ms
[ ] 11.6 : ExoPhoenix : bascule B→A < 500ms
[ ] 11.7 : phoenix_stress_test 1000 bascules → 0 échec
[ ] 11.8 : Mémoire : zéro leak sur stress 2h
[ ] 11.9 : IPC : zéro drop sur stress 1h
[ ] 11.10 : ExoLedger : exo audit --verify-chain → 0 rupture
```

## PHASE 12 — Validation Release Strata

```
[ ] 12.1 : cargo test --all → 100% PASS
[ ] 12.2 : cargo test --test integration → 100% PASS
[ ] 12.3 : cargo deny check → 0 violations
[ ] 12.4 : semgrep → 0 violations
[ ] 12.5 : audit_constants.py → 0 erreurs
[ ] 12.6 : exo doctor → 0 erreur critique
[ ] 12.7 : Tous les P0 résolus
[ ] 12.8 : Tous les items de ce checklist [x]
[ ] 12.9 : CORR series à jour (CORR-87+ appliqués)
[ ] 12.10 : Git tag v0.2.0-rc1 créé
[ ] 12.11 : Boot test sur matériel physique (bare metal) PASS
[ ] 12.12 : USB transfer test sur matériel physique PASS
[ ] 12.13 : Audio chime entendu sur matériel physique
[ ] 12.14 : Git tag v0.2.0-strata créé

TOTAL : 0 / ~150 items validés
```

---

## Résumé des Métriques Cibles

| Métrique | Cible | Actuel |
|---|---|---|
| Kernel stability | ≥ 98% | ~82% |
| ExoPhoenix recovery | < 500ms | Validé release |
| Tests sécurité | 13/13 | 12/13 (sandbox manquant) |
| Syscalls POSIX | ≥ 127 | À implémenter |
| calendar POSIX | OK | Bloqué exo-pkg |
| curl POSIX | OK | Bloqué network |
| USB transfer | OK | Bloqué USB driver |
| Boot chime | Joué | Bloqué audio driver |
| UEFI GPT natif | OK | Bloqué Phase 8 |
| AHCI ou NVMe | ≥ 1 | À implémenter |
| Checklist items | 100% | 0% (release candidate) |

---

*claude-alpha — ExoOS v0.2.0 — Strata — MASTER-CHECKLIST-STRATA.md*
