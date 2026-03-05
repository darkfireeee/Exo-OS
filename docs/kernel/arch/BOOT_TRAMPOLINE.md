# Trampoline de Boot 32→64 bits — Documentation Technique

> Fichier source : `kernel/src/main.rs`  
> Section assembleur : `.text.boot` (`.code32`)

---

## 1. Contexte

GRUB Multiboot2 entre dans le kernel en **mode protégé 32 bits** :

| Registre | Valeur | Description |
|----------|--------|-------------|
| `EAX` | `0x36d76289` | Magic Multiboot2 |
| `EBX` | `< 4 GiB` | Adresse physique de la structure Multiboot2 Info |
| `PE` | 1 | Bit Protection Enable dans CR0 |
| `PG` | 0 | Pagination désactivée |
| `IF` | 0 | Interruptions désactivées |
| `CS` | flat 32-bit | Base=0, Limit=4 GiB |

Le noyau Rust est compilé en 64 bits (`x86_64-unknown-none`).
Il est donc nécessaire d'activer le **Long Mode** avant d'appeler
le premier code Rust.

---

## 2. Sections mémoire utilisées

### `.bss` — Tables de pages de boot (zéro-initialisées)

```
_boot_pml4    : 4096 octets (aligné 4 KiB) — PML4 root
_boot_pdpt    : 4096 octets — Page-Directory Pointer Table
_boot_pd      : 4096 octets — Page Directory (0..1 GiB)
_boot_pd_high : 4096 octets — Page Directory (3..4 GiB, MMIO APIC)
_mb2_saved_magic : 4 octets — sauvegarde EAX
_mb2_saved_info  : 8 octets — sauvegarde EBX (zero-extended)
```

### `.rodata` — GDT 64 bits minimale

```
_boot_gdt:
  [0x00] 0x0000000000000000  — null descriptor
  [0x08] 0x00af9a000000ffff  — 64-bit code, DPL=0, L=1, P=1
  [0x10] 0x00cf92000000ffff  — data,        DPL=0, D=1, P=1
_boot_gdtr:
  .short 23      — limit = 3*8 - 1
  .long _boot_gdt — base (physique 32 bits)
```

### `.text.boot` — Code assembleur 32/64 bits

Point d'entrée ELF : `_start` (mode `.code32`)

### `.boot_stack` — Pile de boot BSP

```
_exo_boot_stack_top : sommet (64 KiB, type NOLOAD)
```

---

## 3. Séquence d'exécution du trampoline

### Étape 1 : Sauvegarde des arguments Multiboot2

```asm
cli
cld
mov dword ptr [_mb2_saved_magic], eax   ; EAX = 0x36d76289
mov dword ptr [_mb2_saved_info],  ebx   ; EBX = adresse Multiboot2 Info
mov dword ptr [_mb2_saved_info+4], 0    ; zero-extend à 64 bits
lea esp, [_exo_boot_stack_top]
and esp, -16
```

EAX et EBX seraient écrasés par la construction des page tables.

### Étape 2 : Construction du PD — identity map 0..1 GiB

```asm
xor ecx, ecx
_pd_loop:
  mov eax, ecx
  shl eax, 21           ; adresse physique = index × 2 MiB
  or  eax, 0x83         ; flags : Present | R/W | PageSize (huge 2 MiB)
  lea edi, [_boot_pd]
  mov ebx, ecx
  shl ebx, 3            ; offset = index × 8 octets
  add edi, ebx
  mov dword ptr [edi],     eax
  mov dword ptr [edi + 4], 0
  inc ecx
  cmp ecx, 512
  jl  _pd_loop
```

Résultat : 512 entrées × 2 MiB = **1 GiB identity-mapped**.

### Étape 3 : Construction de `_boot_pd_high` — MMIO APIC

Les registres MMIO du Local APIC et de l'I/O APIC sont situés dans
le 4ème GiB (> 1 GiB), hors de l'identity map standard.

```
IOAPIC : physique 0xFEC00000
LAPIC  : physique 0xFEE00000
```

Calcul des indices dans `_boot_pd_high` (PD couvrant 3..4 GiB) :
```
index = (phys - 3*GiB) / 2MiB
  IOAPIC : (0xFEC00000 - 0xC0000000) / 0x200000 = 0x3EC00000 / 0x200000 = 502
  LAPIC  : (0xFEE00000 - 0xC0000000) / 0x200000 = 0x3EE00000 / 0x200000 = 503
```

```asm
; PD_high[502] = IOAPIC (offset 502×8 = 4016)
lea edi, [_boot_pd_high]
add edi, 4016
mov dword ptr [edi],     0xFEC00083   ; phys 0xFEC00000 | P | R/W | PS
mov dword ptr [edi + 4], 0

; PD_high[503] = LAPIC (offset 503×8 = 4024)
lea edi, [_boot_pd_high]
add edi, 4024
mov dword ptr [edi],     0xFEE00083   ; phys 0xFEE00000 | P | R/W | PS
mov dword ptr [edi + 4], 0
```

### Étape 4 : Câblage de la hiérarchie de pages

```asm
; PDPT[0] → PD (identity 0..1 GiB)
lea eax, [_boot_pd]
or  eax, 0x03                          ; P | R/W
mov dword ptr [_boot_pdpt],     eax
mov dword ptr [_boot_pdpt + 4], 0

; PDPT[3] → PD_high (MMIO APIC 3..4 GiB) ; offset = 3×8 = 24
lea eax, [_boot_pd_high]
or  eax, 0x03
mov dword ptr [_boot_pdpt + 24], eax
mov dword ptr [_boot_pdpt + 28], 0

; PML4[0] → PDPT
lea eax, [_boot_pdpt]
or  eax, 0x03
mov dword ptr [_boot_pml4],     eax
mov dword ptr [_boot_pml4 + 4], 0
```

### Étape 5 : Activation du Long Mode

```asm
lgdt [_boot_gdtr]                  ; charger la GDT 64 bits

lea eax, [_boot_pml4]
mov cr3, eax                       ; CR3 = adresse physique PML4

mov eax, cr4
or  eax, 0x20                      ; CR4.PAE = 1 (Physical Address Extension)
mov cr4, eax

mov ecx, 0xC0000080                ; MSR EFER
rdmsr
or  eax, 0x100                     ; EFER.LME = 1 (Long Mode Enable)
wrmsr

mov eax, cr0
or  eax, 0x80000000                ; CR0.PG = 1 → active Long Mode
mov cr0, eax
```

### Étape 6 : Saut lointain vers CS=0x08 (mode 64 bits pur)

```asm
lea eax, [_start64]
push dword ptr 0x08    ; CS = 0x08 (64-bit code descriptor)
push eax               ; EIP = adresse de _start64
retf                   ; far return : charge CS:EIP → mode 64 bits
```

Après le `retf`, le CPU est en mode 64 bits. Le premier code 64 bits est `_start64`.

---

## 4. Code 64 bits : `_start64`

```asm
.code64
_start64:
  mov al, 'X'
  out 0xe9, al             ; marqueur debug : trampoline réussi

  mov ax, 0x10             ; DS = 0x10 (data descriptor)
  mov ds, ax
  mov es, ax
  mov fs, ax
  mov gs, ax
  mov ss, ax

  lea rsp, [_exo_boot_stack_top]  ; charge RSP 64 bits
  and rsp, -16

  ; Restaurer les arguments Multiboot2 pour kernel_main
  mov edi, dword ptr [_mb2_saved_magic]   ; arg1 : mb2_magic (u32)
  mov rsi, qword ptr [_mb2_saved_info]    ; arg2 : mb2_info  (u64)
  xor rdx, rdx                            ; arg3 : rsdp_phys = 0 (auto-scan)

  call kernel_main
```

---

## 5. Mapping virtuel résultant

```
Adresse virtuelle            Adresse physique     Contenu
─────────────────────────────────────────────────────────────
0x0000_0000_0000_0000        0x0000_0000_0000_0000  kernel ELF, BIOS area
  ...                        ...                    (512 × huge pages 2MiB)
0x0000_0000_3FFF_FFFF        0x0000_0000_3FFF_FFFF  fin identity map 1 GiB

0x0000_00FE_C0_0000          0x00000000_FEC00000    IOAPIC MMIO (2 MiB page)
0x0000_00FE_E0_0000          0x00000000_FEE00000    LAPIC MMIO  (2 MiB page)
```

> Adresses virtuelles = adresses physiques (identity mapping) → le code
> d'init APIC peut utiliser directement des adresses physiques comme pointeurs.

---

## 6. Contraintes et invariants

- Toutes les adresses de symboles doivent être < 1 GiB (kernel chargé à 1 MiB)
- La stack de boot (64 KiB) doit être alignée sur 16 octets pour les appels Rust
- `CR3` doit contenir une adresse physique 4 KiB-alignée
- `EFER.LME` doit être mis **avant** `CR0.PG`
- Le `retf` doit avoir CS correspondant à un descripteur 64 bits dans la GDT

---

## 7. Remarques de sécurité

Les pages APIC MMIO sont mappées avec le flag `P | R/W | PS`.
Elles **ne disposent pas** du flag `NX` (no-execute) car la page directory
de boot ne supporte pas les attributs étendus (PA high bits non utilisés ici).
Après l'init complète de la mémoire virtuelle, ces pages seront remappées
avec des attributs corrects (NX + UC = Uncacheable pour MMIO).
