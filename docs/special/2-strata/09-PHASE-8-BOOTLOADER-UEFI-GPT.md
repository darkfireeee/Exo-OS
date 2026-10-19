# 09 — PHASE 8 : Bootloader UEFI GPT Complet (`exo-boot`)

## 1. État Réel dans le Dépôt
* Le bootloader `exo-boot` (`exo-boot/src/main.rs`) et ses crates partagées sont déjà **très avancés** :
  * **Parseur GPT/MBR réel (`drivers/storage/partition` / `exo-partition`)** : Valide la signature `"EFI PART"`, le CRC-32 du header primaire (LBA 1) et de la table des partitions, bascule automatiquement sur le GPT de backup (dernier LBA) si le primaire est corrompu (`scan_corrupt_primary_header_falls_back_to_backup`), et localise `GUID_ESP`, `GUID_EXOOS_ROOT` et `GUID_EXOOS_DATA` (`8.1` à `8.5` déjà validés par tests unitaires)[cite: 10, 17, 20].
  * **Vérification Ed25519 stricte (`drivers/security/verity` / `exo-verity`)** : `verify_kernel()` vérifie le footer `EXOSIG01` (256 octets) avec `verify_strict` et SHA-512, et une assertion compile-time dans `exo-boot/src/kernel_loader/verify.rs` bloque toute clé nulle ou vecteur de test (`8.9`)[cite: 10, 17, 20].
  * **KASLR & Handoff 64-bit** : `compute_kaslr_base()` calcule une base aléatoire alignée sur 2 Mio à partir des 64 octets d'entropie (`8.10`), applique les relocations PIE (`apply_pie_relocations`), et `handoff_to_kernel()` saute sur le symbole 64-bit `_start_uefi` du noyau (`8.11`)[cite: 7, 10, 20].
* **Écarts restants (`8.6`, `8.7`, `8.8`, `8.12`)** :
  1. Dans `exo-boot/src/kernel_loader/handoff.rs`, `BOOT_INFO_VERSION` vaut encore `1` et `BootInfo` ne contient pas les champs Strata v2 (`exofs_root_phys`/`lba`/`sectors`, `exofs_data_phys`/`lba`/`sectors`, `disk_guid`, `boot_partition_guid`, `boot_from_usb`, `nvme_controller_phys`, `ahci_controller_phys`)[cite: 10, 13, 20]. Dans `exo-boot/src/main.rs` (Étape 3b), le résultat de `disk::gpt::scan_boot_disk()` est affiché à l'écran mais n'est pas copié dans `BootInfo`[cite: 20].
  2. Le module `exo-boot/src/uefi/nvram.rs` (`register_boot_entry` créant l'entrée `BootXXXX` dans la NVRAM UEFI, `8.12`) n'existe pas encore[cite: 8, 10, 13, 20].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Passage de `BootInfo` à `BootInfoV2` (`exo-boot/src/kernel_loader/handoff.rs` & `main.rs`) (`8.6`–`8.8`)
1. Dans `exo-boot/src/disk/gpt.rs`, ajouter `pub disk_guid: [u8; 16]` dans `DiscoveredLayout` et le remplir depuis `table.disk_guid` dans `DiscoveredLayout::from_table()`[cite: 13, 17, 20].
2. Dans `exo-boot/src/kernel_loader/handoff.rs` :
   * Passer `pub const BOOT_INFO_VERSION: u32 = 2;`[cite: 13, 20].
   * Ajouter à la fin de `BootInfo` (avant `_reserved`) les champs exacts de `SPEC-BOOTLOADER-GPT-STRATA.md` §4 :
     ```rust
     pub exofs_root_phys:      u64,
     pub exofs_root_lba:       u64,
     pub exofs_root_sectors:   u64,
     pub exofs_data_phys:      u64,
     pub exofs_data_lba:       u64,
     pub exofs_data_sectors:   u64,
     pub disk_guid:            [u8; 16],
     pub boot_partition_guid:  [u8; 16],
     pub boot_from_usb:        bool,
     pub _pad_v2:              [u8; 7],
     pub nvme_controller_phys: u64,
     pub ahci_controller_phys: u64,
     ```
   * Vérifier que `core::mem::size_of::<BootInfo>() % 8 == 0` et `<= 65536`[cite: 13, 20].
3. Dans `exo-boot/src/main.rs`, conserver le `Option<DiscoveredLayout>` retourné à l'Étape 3b par `disk::gpt::scan_boot_disk(boot_services, image_handle)` et renseigner `boot_info_ref.exofs_root_lba`, `exofs_root_sectors`, `exofs_data_lba`, `exofs_data_sectors` et `disk_guid` à l'Étape 9[cite: 8, 13, 20].
4. Synchroniser la structure mirroir `BootInfo` côté noyau dans `kernel/src/arch/x86_64/boot/` afin que le kernel lise la version `2` (tout en acceptant la version `1` en fallback QEMU)[cite: 7, 13, 20].

### B. Enregistrement NVRAM UEFI (`exo-boot/src/uefi/nvram.rs`) (`8.12`)
1. Créer `exo-boot/src/uefi/nvram.rs` et l'exposer dans `exo-boot/src/uefi/mod.rs`[cite: 13, 20].
2. Implémenter `pub fn register_boot_entry(st: &SystemTable<Boot>, image_handle: Handle)` :
   * Utiliser `st.runtime_services().get_variable()` et `set_variable()` avec `VariableVendor::GLOBAL_VARIABLE`[cite: 13, 20].
   * Vérifier si une variable `Boot0000`..`Boot000F` contient déjà la description UCS-2 `"ExoOS v0.2.0 — Strata"`[cite: 13].
   * Si absente, écrire l'`EFI_LOAD_OPTION` pointant vers `\EFI\EXOOS\BOOTX64.EFI` et, si `!query_secure_boot_status(st).is_enforcing()`, insérer l'index en tête de `BootOrder`[cite: 8, 13, 20].
3. Appeler `uefi::nvram::register_boot_entry(&system_table, image_handle)` dans `exo-boot/src/main.rs` **avant** l'Étape 10 (`exit_boot_services`)[cite: 8, 13, 20].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Synchronisation binaire du layout `BootInfo` entre `exo-boot` et `kernel`** : `exo-boot` est un workspace Cargo indépendant de `kernel` (`exo-boot/Cargo.toml`)[cite: 20]. Les nouveaux champs v2 doivent être ajoutés **après** les champs v1 existants (ou les deux définitions `repr(C)` dans `exo-boot/src/kernel_loader/handoff.rs` et `kernel/src/arch/x86_64/boot/` doivent être mises à jour dans le même commit) pour éviter tout décalage d'offset sur `framebuffer`, `acpi_rsdp`, `entropy` ou `kernel_physical_base`[cite: 7, 13, 20].
* **Règle `BOOT-06` (Point de non-retour `ExitBootServices`)** : Tout accès à `EFI_BLOCK_IO_PROTOCOL` (`scan_boot_disk`), `EFI_FILE_PROTOCOL` (`load_file`) ou `AllocatePages` doit impérativement avoir lieu **avant** `system_table.exit_boot_services()` et `mark_boot_services_exited()` dans `exo-boot/src/main.rs`[cite: 8, 20].