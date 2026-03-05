# ExoOS — Premier Boot Réussi sur QEMU

> **Date** : 5 mars 2026  
> **Statut** : ✅ Kernel démarre et s'exécute complètement sur QEMU (machine q35, BIOS Multiboot2)

---

## 1. Résumé

ExoOS démarre de bout en bout sur QEMU avec la séquence d'init complète.
Le noyau traverse toutes les phases d'initialisation architecture (CPU, GDT, IDT, TSS,
TSC, FPU, ACPI, APIC, mémoire, syscall, mitigations, SMP) et se termine proprement
dans la boucle `halt_cpu()`.

### Séquence de boot observée (port 0xE9)

```
XK12356ps789abcdefgZAI
OK
```

| Marqueur | Signification |
|----------|--------------|
| `X` | Trampoline `_start64` atteint (transition 32→64 bits réussie) |
| `K` | `kernel_main()` atteint |
| `1` | `init_cpu_features` |
| `2` | `init_gdt_for_cpu` |
| `3` | `init_idt` + `load_idt` |
| `5` | `init_percpu_for_bsp` |
| `6` | `init_tsc` |
| `p` | TSC : sonde PIT désactivée |
| `s` | TSC : fréquence fixée (CPUID 0x15 ou 1 GHz fallback) |
| `7` | `init_fpu_for_cpu` |
| `8` | `detect_hypervisor` |
| `9` | ACPI init (RSDP → RSDT → MADT/HPET/FADT) |
| `a` | `init_apic_system` |
| `b` | `calibrate_lapic_timer` |
| `c` | `init_memory_integration` |
| `d` | `init_syscall` |
| `e` | `apply_mitigations_bsp` |
| `f` | Multiboot2 / UEFI path + init sous-système mémoire |
| `g` | SMP boot |
| `Z` | `arch_boot_init` terminé |
| `A` | `arch_boot_init` retour dans `kernel_main` |
| `I` | `kernel_init` terminé |
| `OK` | Kernel complet, en boucle `halt_cpu()` |

### Résultats des tests de stabilité

```
Run 1: exit=124  output=XK12356ps789abcdefgZAIOK
Run 2: exit=124  output=XK12356ps789abcdefgZAIOK
Run 3: exit=124  output=XK12356ps789abcdefgZAIOK
```

`exit=124` = QEMU atteint le timeout (kernel en HLT loop stable).

---

## 2. Environnement

| Composant | Version |
|-----------|---------|
| Rust | nightly 1.96.0-nightly (2026-03-04) |
| Cible | `x86_64-unknown-none` |
| GRUB | 2.12 (grub-bios + grub-mkrescue) |
| QEMU | 10.1.3 |
| Machine QEMU | `q35 -m 256M` |
| Mode boot | BIOS Legacy + Multiboot2 |
| ISO | `exo-os.iso` (36 MB, debug build) |

---

## 3. Corrections critiques réalisées

### 3.1 Trampoline 32→64 bits (correction fondamentale)

**Problème** : Le point d'entrée `_start` était compilé en mode 64 bits mais
invoqué par GRUB en mode protégé 32 bits. Les préfixes REX 64 bits étaient
décodés comme `DEC EAX`, corrompant les registres et le magic Multiboot2.

**Solution** : Implémentation d'un trampoline complet en `.code32` :

1. Sauvegarde de `EAX`/`EBX` (magic + info Multiboot2) dans `.bss`
2. Construction d'un PD avec 512 huge pages 2 MiB (identity 0..1 GiB)
3. PDPT[0] → PD, PML4[0] → PDPT
4. **PDPT[3] → `_boot_pd_high`** (MMIO APIC : LAPIC 0xFEE00000, IOAPIC 0xFEC00000)
5. Chargement GDT 64 bits, activation PAE + LME + PG
6. `retf` vers CS=0x08 → mode 64 bits pur (`_start64`)

Voir [BOOT_TRAMPOLINE.md](kernel/arch/BOOT_TRAMPOLINE.md) pour les détails techniques.

### 3.2 Calibration TSC sans PIT

**Problème** : `calibrate_tsc_with_pit()` boucle sur le bit 5 du port 0x61
(PIT canal 2). En mode QEMU TCG, les callbacks du timer PIT ne s'exécutent
pas pendant une boucle d'attente active → boucle infinie.

**Solution** (`kernel/src/arch/x86_64/cpu/tsc.rs`) :
- Désactivation de la calibration PIT au boot
- Utilisation de CPUID 0x15 (crystal clock) en priorité
- Fallback : fréquence fixée à 1 GHz

### 3.3 Parseur ACPI — alignement mémoire

**Problème** : Rust ≥ 1.82 (mode debug) vérifie l'alignement du pointeur
dans `ptr::read_volatile::<u32/u64>`. Les tables ACPI (RSDT, MADT, HPET, FADT)
peuvent être placées à des adresses non-alignées par SeaBIOS → panic au premier
accès à un champ > 1 octet.

**Solution** : Remplacement systématique de `read_volatile` par
`ptr::read_unaligned` pour tous les champs > u8 dans les parseurs ACPI.

Fichiers corrigés :
- `kernel/src/arch/x86_64/acpi/parser.rs`
- `kernel/src/arch/x86_64/acpi/madt.rs`
- `kernel/src/arch/x86_64/acpi/hpet.rs`
- `kernel/src/arch/x86_64/acpi/pm_timer.rs`

### 3.4 Mapping MMIO APIC dans les page tables de boot

**Problème** : Le LAPIC (0xFEE00000) et l'IOAPIC (0xFEC00000) sont situés
dans le 4ème GiB. Notre identity map initiale ne couvrait que 0..1 GiB.
L'accès MMIO dans `init_apic_system` provoquait un triple fault immédiat.

**Solution** : Ajout d'une page directory `_boot_pd_high` dans le trampoline :

```
PML4[0] → _boot_pdpt
  PDPT[0] → _boot_pd       (identity 0..1 GiB, 512 × huge pages 2 MiB)
  PDPT[3] → _boot_pd_high  (MMIO APIC region 3..4 GiB)
    PD[502] = 0xFEC00083     (IOAPIC : phys 0xFEC00000, R/W, Present, PageSize)
    PD[503] = 0xFEE00083     (LAPIC  : phys 0xFEE00000, R/W, Present, PageSize)
```

### 3.5 Initialisation HPET différée

**Problème** : Le MMIO HPET est à `0xFED00000` (> 1 GiB). L'accès aux
registres HPET dans `init_hpet()` provoquait un page fault.

**Solution** : `init_hpet()` enregistre uniquement l'adresse MMIO lors du
boot précoce. L'accès aux registres est différé après l'initialisation
complète du sous-système mémoire.

---

## 4. Architecture des page tables de boot

```
Virtual address space au boot :

0x0000_0000_0000_0000 ─┐
                        │  PML4[0] → PDPT
0x0000_0000_3FFF_FFFF ─┘    PDPT[0] → PD (512 × 2MiB huge pages)
                              Identity map 0..1 GiB (kernel + stack + ACPI)

0x0000_00C0_0000_0000 ─┐  (virtuel 3 GiB, correspond à PDPT[3])
  0xFEC00000 (IOAPIC)  │    PDPT[3] → PD_high
  0xFEE00000 (LAPIC)   │      PD_high[502] = phys 0xFEC00000 | 0x83
0x0000_00FF_FFFF_FFFF ─┘      PD_high[503] = phys 0xFEE00000 | 0x83
```

> **Note** : Les adresses virtuelles APIC MMIO sont identiques aux adresses
> physiques pendant le boot (l'espace d'adressage virtuel n'est pas encore
> séparé). PDPT[3] couvre l'espace virtuel `3*512GiB..4*512GiB`... attendez,
> recalculons : PDPT[3] couvre `3 GiB..4 GiB` (chaque entrée PDPT = 1 GiB).
> 0xFEE00000 est bien dans [3 GiB, 4 GiB[. ✓

---

## 5. Commandes de build et test

### Build de l'ISO

```bash
source "$HOME/.cargo/env"
cd /workspaces/Exo-OS
cargo build -p exo-os-kernel          # debug build
make iso                               # → exo-os.iso
```

### Test headless (CI / terminal)

```bash
rm -f /tmp/t.log
timeout 25 qemu-system-x86_64 \
  -machine q35 -m 256M \
  -no-reboot -display none \
  -device isa-debugcon,iobase=0xe9,chardev=dc \
  -chardev file,id=dc,path=/tmp/t.log \
  -cdrom exo-os.iso 2>/dev/null
echo "exit=$?"
cat /tmp/t.log
# Attendu : exit=124  output=XK12356ps789abcdefgZAIOK
```

### Test graphique (VNC)

```bash
qemu-system-x86_64 -machine q35 -m 256M \
  -vga std -display vnc=:1 \
  -cdrom exo-os.iso
# Connexion VNC : localhost:5901
```

### Capture d'écran

```bash
qemu-system-x86_64 ... -monitor stdio <<'EOF'
screendump /tmp/screenshot.ppm
quit
EOF
```

---

## 6. Fichiers modifiés

| Fichier | Nature de la modification |
|---------|--------------------------|
| `kernel/src/main.rs` | Trampoline 32→64 bits complet, `_boot_pd_high` pour APIC MMIO |
| `kernel/src/arch/x86_64/boot/early_init.rs` | Sondes debug port 0xE9 par étape |
| `kernel/src/arch/x86_64/cpu/tsc.rs` | Calibration TSC sans PIT (QEMU safe) |
| `kernel/src/arch/x86_64/acpi/parser.rs` | `read_unaligned`, gardes adresse, sondes debug |
| `kernel/src/arch/x86_64/acpi/madt.rs` | `read_unaligned` systématique, gardes longueur |
| `kernel/src/arch/x86_64/acpi/hpet.rs` | Init différée (MMIO > 1 GiB) |
| `kernel/src/arch/x86_64/acpi/pm_timer.rs` | `read_unaligned`, garde adresse |
| `Makefile` | Remplacé (obsolète `cargo bootimage` → `grub-mkrescue`) |
| `bootloader/grub.cfg` | Corrigé (était vide) |

---

## 7. Prochaines étapes

- [ ] **Affichage graphique** : implémenter le framebuffer VGA/VESA pour un logo ExoOS au boot
- [ ] **Mémoire virtuelle** : activer le pagination kernel complet (PML4 kernel haute mémoire)
- [ ] **HPET post-init** : terminer l'init HPET après mapping mémoire complet
- [ ] **Userspace** : amorcer le processus `init_server`
- [ ] **Tests automatisés** : intégrer le test headless dans la CI GitHub Actions
- [ ] **Build release** : optimisation `-C opt-level=3` + strip symbols
