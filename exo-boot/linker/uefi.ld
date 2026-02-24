/* linker/uefi.ld — Linker script UEFI PE32+ pour exo-boot
 *
 * Cible : x86_64-unknown-uefi
 * Linker : lld-link (LLD en mode PE/COFF)
 *
 * Ce script est appliqué via `build.rs` qui passe :
 *   cargo:rustc-link-arg=-T<path>/linker/uefi.ld
 *
 * Pour la cible UEFI, lld-link gère automatiquement :
 *   - Le header PE32+ (MZ + PE signature)
 *   - Les sections .text, .data, .rdata, .bss dans le format COFF
 *   - Le sous-système EFI_APPLICATION (subsystem = 10)
 *
 * NOTE : Contrairement à BIOS, la cible x86_64-unknown-uefi produit directement
 * un binaire PE32+ exécutable par le firmware UEFI. Pas besoin d'un linker
 * script GNU ld complet — les defaults de lld-link sont corrects.
 *
 * RÈGLES IMPORTANTES (DOC10/BOOT-02..07) :
 *   - Adresse de chargement : décidée par UEFI (AllocatePages) → pas fixée ici
 *   - KASLR : adresse du kernel randomisée par exo-boot, PAS le bootloader lui-même
 *   - ExitBootServices DOIT être appelé avant le handoff
 *
 * Configuration d'entrée passée via build.rs (ou .cargo/config.toml) :
 *   /entry:efi_main             — point d'entrée UEFI standard
 *   /subsystem:efi_application  — type d'image EFI
 *
 * Ordonnancement des sections optimisé pour minimiser TLB pressure au boot :
 *   .text    → code exécutable (R-X)
 *   .rdata   → données en lecture seule, tables GUID (R--)
 *   .data    → données initialisées mutables    (RW-)
 *   .bss     → BSS zéro-initialisé              (RW-)
 *   .reloc   → table de relocations PE (requise pour Secure Boot)
 */

/* Pour lld-link en mode EFI, ce fichier sert principalement de documentation.
 * Les vrais arguments sont dans build.rs :
 *   cargo_link_arg("/entry:efi_main");
 *   cargo_link_arg("/subsystem:efi_application,1.0");
 *
 * IMPORTANT : La section .reloc DOIT être présente pour que Secure Boot
 * vérifie correctement la signature. lld la génère automatiquement pour PIE.
 */
