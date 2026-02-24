; stage2.asm — Stage 2 — Active A20, passe en mode protégé puis mode long.
;
; Chargé par mbr.asm à 0x1000:0x0000 (adresse physique 0x10000 = 64 KB).
; Taille maximale : 64 secteurs × 512 = 32 KB.
;
; Chemin d'exécution :
;   [16-bit Real Mode @ 0x10000]
;     → Activer ligne A20
;     → Détecter RAM via INT 15h E820
;     → Charger le kernel.elf en mémoire via INT 13h EDD
;     → Configurer GDT + IDT minimale
;     → Passer en mode protégé 32-bit
;     [32-bit Protected Mode]
;       → Configurer PAE (Physical Address Extension)
;       → Configurer page tables initiales (identité 0–8 MB)
;       → Activer Long Mode via EFER MSR
;       → Passer en mode long 64-bit
;     [64-bit Long Mode]
;       → Initialiser registres de segment 64-bit
;       → Zéroïser BSS de exo-boot Rust
;       → Appeler exoboot_main_bios(e820_buffer_addr, e820_count)
;
; CONTRAT AVEC exoboot_main_bios() (voir main.rs) :
;   - Argument 1 (RDI) : adresse physique du buffer E820 (format E820Entry[])
;   - Argument 2 (RSI) : nombre d'entrées E820
;   - CPU en mode long 64-bit
;   - A20 activé
;   - GDT chargée (code=0x08, data=0x10)
;   - Interruptions désactivées (CLI)
;   - RSP pointe vers __stack_top (bios.ld)
;
; ADRESSES DU KERNEL SHADOW BUFFER (contrat avec disk.rs) :
;   0x0200_0000 (2 MB) : début du shadow buffer pour les secteurs kernel

BITS 16
ORG  0x0000           ; Chargé à 0x1000:0x0000

; ─── Constantes ────────────────────────────────────────────────────────────────

; Adresses physiques clés
E820_BUFFER_PHYS        equ 0x00050000  ; Buffer E820 (320 KB, hors zone critique)
E820_MAX_ENTRIES        equ 128         ; Maximum d'entrées E820 supportées
KERNEL_SHADOW_BASE      equ 0x00200000  ; Shadow buffer kernel (2 MB physique)
KERNEL_SHADOW_LBA_START equ 2048        ; LBA début partition kernel
KERNEL_SECTOR_COUNT     equ 131072      ; 64 MB max en secteurs (131072 × 512)

; GDT Segments (mode protégé 32-bit)
GDT_CODE32_SEL  equ 0x08
GDT_DATA32_SEL  equ 0x10

; GDT Segments (mode long 64-bit)
GDT_CODE64_SEL  equ 0x18
GDT_DATA64_SEL  equ 0x20

; ─── Entrée Stage 2 (16-bit) ───────────────────────────────────────────────────
stage2_entry:
    cli
    xor  ax, ax
    mov  ds, ax
    mov  es, ax
    mov  ss, ax
    mov  sp, 0x7C00             ; Stack temporaire en mode réel

    ; Sauvegarde le numéro de disque
    mov  [boot_drive], dl

    ; ── Étape 1 : Activation A20 ─────────────────────────────────────────────
    call enable_a20
    call test_a20
    jz   .a20_failed

    ; ── Étape 2 : Détection mémoire E820 ────────────────────────────────────
    call detect_e820

    ; ── Étape 3 : Chargement kernel shadow ──────────────────────────────────
    call load_kernel_shadow

    ; ── Étape 4 : Passage en mode protégé ───────────────────────────────────
    lgdt [gdt_descriptor_32]
    mov  eax, cr0
    or   eax, 0x01              ; PE bit
    mov  cr0, eax
    jmp  GDT_CODE32_SEL:.protected_mode_entry

.a20_failed:
    mov  si, msg_a20_fail
    call print16
    jmp  .halt16

.halt16:
    cli
    hlt
    jmp  .halt16

; ─── Activation A20 ───────────────────────────────────────────────────────────
enable_a20:
    ; Méthode 1 : BIOS INT 15h AX=2401h (Fast A20 Enable)
    mov  ax, 0x2401
    int  0x15
    jnc  .a20_ok

    ; Méthode 2 : Keyboard Controller (Port 0x64/0x60)
    call a20_kbc
    call test_a20
    jnz  .a20_ok

    ; Méthode 3 : Port 0x92 Fast A20 (portables/chipsets modernes)
    in   al, 0x92
    or   al, 0x02
    and  al, 0xFE               ; Ne pas déclencher de reset (bit 0)
    out  0x92, al

.a20_ok:
    ret

a20_kbc:
    ; Attend que le contrôleur clavier soit prêt (status bit 1 = input buffer)
    call kbc_wait_input
    mov  al, 0xAD               ; Désactive clavier
    out  0x64, al
    call kbc_wait_input
    mov  al, 0xD0               ; Lit output port
    out  0x64, al
    call kbc_wait_output
    in   al, 0x60
    push ax
    call kbc_wait_input
    mov  al, 0xD1               ; Écrit output port
    out  0x64, al
    call kbc_wait_input
    pop  ax
    or   al, 0x02               ; Active A20
    out  0x60, al
    call kbc_wait_input
    mov  al, 0xAE               ; Réactive clavier
    out  0x64, al
    ret

kbc_wait_input:
    in   al, 0x64
    test al, 0x02
    jnz  kbc_wait_input
    ret

kbc_wait_output:
    in   al, 0x64
    test al, 0x01
    jz   kbc_wait_output
    ret

test_a20:
    ; Test A20 : écrit à 0x112345, lit à 0x012345 (adresses qui diffèrent par le bit A20)
    ; Si A20 inactif → masquage du bit 20 → mêmes adresses physiques
    push es
    push di
    push si
    mov  ax, 0xFFFF
    mov  es, ax
    mov  di, 0x0010             ; ES:DI = 0xFFFF:0x0010 = physique 0x100000
    mov  ax, 0x0000
    mov  ds, ax
    mov  si, 0x0500             ; DS:SI = 0:0x0500 = physique 0x000500
    mov  al, [ds:si]
    push ax
    mov  al, [es:di]
    push ax
    mov  byte [ds:si], 0x00
    mov  byte [es:di], 0xFF
    mov  al, [ds:si]
    cmp  al, 0xFF               ; Si A20 inactif, on lirait 0xFF ici aussi
    pop  ax
    mov  [es:di], al
    pop  ax
    mov  [ds:si], al
    pop  si
    pop  di
    pop  es
    ; ZF=0 si A20 actif (les deux adresses sont distinctes)
    ; ZF=1 si A20 inactif (les deux adresses se recouvrent)
    ret

; ─── Détection mémoire E820 ────────────────────────────────────────────────────
detect_e820:
    ; INT 15h AX=E820h — remplit E820_BUFFER_PHYS avec les entrées de la carte mémoire
    ; Format E820Entry : Base(8) + Length(8) + Type(4) + ACPI3(4) = 24 bytes
    xor  ebx, ebx               ; Continuation = 0 (première entrée)
    mov  edi, E820_BUFFER_PHYS  ; Destination buffer
    xor  bp, bp                 ; Compteur d'entrées

.e820_loop:
    mov  eax, 0xE820
    mov  edx, 0x534D4150        ; Signature 'SMAP'
    mov  ecx, 24                ; Taille buffer entrée (24 bytes inclut ACPI3)
    int  0x15
    jc   .e820_done             ; Carry = fin ou erreur
    cmp  eax, 0x534D4150        ; Vérifie la signature SMAP en retour
    jne  .e820_done

    ; Une entrée Length=0 est invalide — l'ignorer
    mov  eax, [edi + 8]
    or   eax, [edi + 12]
    jz   .e820_next

    inc  bp
    add  edi, 24                ; Avance au prochain slot du buffer

    cmp  bp, E820_MAX_ENTRIES   ; Vérifie overflow buffer
    jge  .e820_done

.e820_next:
    test ebx, ebx               ; EBX=0 = dernière entrée
    jnz  .e820_loop

.e820_done:
    mov  [e820_count], bp       ; Sauvegarde le nombre d'entrées
    ret

; ─── Chargement kernel shadow via INT 13h EDD ─────────────────────────────────
load_kernel_shadow:
    ; Charge les secteurs kernel en mémoire à KERNEL_SHADOW_BASE.
    ; Cette zone sera lue par Rust (disk.rs) en mode long.
    ; On charge par blocs de 63 secteurs (limite sûre pour certains firmware).
    mov  cx, KERNEL_SECTOR_COUNT / 63   ; Nombre de blocs complets
    mov  edx, KERNEL_SHADOW_LBA_START
    mov  edi, KERNEL_SHADOW_BASE

.shadow_loop:
    push cx
    push edx
    push edi

    ; Construit le DAP en pile
    sub  sp, 16
    mov  si, sp
    mov  byte  [si + 0], 0x10
    mov  byte  [si + 1], 0x00
    mov  word  [si + 2], 63             ; 63 secteurs par lot
    mov  word  [si + 4], di             ; Offset destination (segment:offset)
    mov  ax, edi
    shr  ax, 4
    mov  word  [si + 6], ax             ; Segment = adresse >> 4
    mov  dword [si + 8], edx            ; LBA low
    mov  dword [si + 12], 0             ; LBA high
    mov  ah, 0x42
    mov  dl, [boot_drive]
    int  0x13
    add  sp, 16

    pop  edi
    pop  edx
    pop  cx

    add  edx, 63
    add  edi, 63 * 512
    loop .shadow_loop
    ret

; ─── print16 : Affiche chaîne ASCIIZ en mode réel ────────────────────────────
print16:
    lodsb
    test al, al
    jz   .done
    mov  ah, 0x0E
    mov  bx, 7
    int  0x10
    jmp  print16
.done:
    ret

; ─── Messages ──────────────────────────────────────────────────────────────────
msg_a20_fail:
    db  "EXO-BOOT: A20 activation failed -- halted", 0x0D, 0x0A, 0

; ─── Variables ─────────────────────────────────────────────────────────────────
boot_drive  db  0x80
e820_count  dw  0

; ─── GDT 32-bit (utilisée pour la transition) ────────────────────────────────
ALIGN 8
gdt_table_32:
    dq  0x0000000000000000   ; Null descriptor
    dq  0x00CF9A000000FFFF   ; Code32 : base=0, lim=4GB, P=1, DPL=0, S=1, Type=0xA(code rx)
    dq  0x00CF92000000FFFF   ; Data32 : base=0, lim=4GB, P=1, DPL=0, S=1, Type=0x2(data rw)
    dq  0x00AF9A000000FFFF   ; Code64 : L=1 (long mode), P=1, DPL=0
    dq  0x00AF92000000FFFF   ; Data64 : idem, pour les accès de données

gdt_descriptor_32:
    dw  ($ - gdt_table_32 - 1)
    dd  gdt_table_32

; ═══════ Passage en mode protégé 32-bit ═══════════════════════════════════════
BITS 32
.protected_mode_entry:
    ; Configure les registres de segment 32-bit
    mov  ax, GDT_DATA32_SEL
    mov  ds, ax
    mov  es, ax
    mov  fs, ax
    mov  gs, ax
    mov  ss, ax
    mov  esp, 0x7C000           ; Stack 32-bit temporaire à 496 KB

    ; ── Activation PAE + préparation mode long ─────────────────────────────────
    ; Configure les page tables minimales pour le passage en mode long.
    ; Identité-mappe les 8 premiers MB (suffisant pour le bootloader).
    call setup_identity_page_tables

    ; Active PAE dans CR4
    mov  eax, cr4
    or   eax, (1 << 5)          ; CR4.PAE = 1
    mov  cr4, eax

    ; Charge PML4 dans CR3
    mov  eax, PML4_PHYS_BASE
    mov  cr3, eax

    ; Active Long Mode dans EFER MSR (MSR 0xC0000080)
    mov  ecx, 0xC0000080
    rdmsr
    or   eax, (1 << 8)          ; EFER.LME = 1
    wrmsr

    ; Active la pagination + Long Mode (cr0.PG + cr0.PE déjà mis)
    mov  eax, cr0
    or   eax, (1 << 31)         ; CR0.PG = 1 → active la pagination + Long Mode effectif
    mov  cr0, eax

    ; Far jump 64-bit pour vider le pipeline et passer en Long Mode
    jmp  GDT_CODE64_SEL:long_mode_entry

; ─── Construction des page tables identité (0–8 MB) ──────────────────────────
; Utilise des large pages 2 MB (PD entries avec PS bit = 1).
; Place les tables à des adresses fixes dans la mémoire basse.
PML4_PHYS_BASE     equ 0x00070000   ; PML4 @ 448 KB
PDPT_PHYS_BASE     equ 0x00071000   ; PDPT @ 449 KB
PD_PHYS_BASE       equ 0x00072000   ; PD   @ 450 KB

setup_identity_page_tables:
    ; Zéroïse les 3 tables (3 × 4 KB)
    mov  edi, PML4_PHYS_BASE
    xor  eax, eax
    mov  ecx, 3 * 1024          ; 3 pages × 1024 DWORDs
    rep  stosd

    ; PML4[0] → PDPT (présente, writable, user)
    mov  edi, PML4_PHYS_BASE
    mov  eax, PDPT_PHYS_BASE | 0x3   ; P=1, RW=1
    mov  [edi], eax

    ; PDPT[0] → PD (présente, writable)
    mov  edi, PDPT_PHYS_BASE
    mov  eax, PD_PHYS_BASE | 0x3
    mov  [edi], eax

    ; PD[0..3] → 4 × 2 MB large pages (0–8 MB), identité
    ; Chaque entrée PD : adresse + PS(bit7) + RW(bit1) + P(bit0)
    mov  edi, PD_PHYS_BASE
    mov  eax, 0x0000083         ; Base=0, PS=1, RW=1, P=1
    mov  ecx, 4                 ; 4 entrées = 4 × 2 MB = 8 MB
.pd_loop:
    mov  [edi], eax
    add  eax, 0x200000          ; +2 MB
    add  edi, 8                 ; Entrée PD = 8 bytes
    loop .pd_loop
    ret

; ═══════ Mode long 64-bit ═════════════════════════════════════════════════════
BITS 64
long_mode_entry:
    ; Configure les segments 64-bit
    mov  ax, GDT_DATA64_SEL
    mov  ds, ax
    mov  es, ax
    mov  fs, ax
    mov  gs, ax
    mov  ss, ax

    ; Stack 64-bit : utilise __stack_top depuis bios.ld (défini extern)
    extern __stack_top
    mov  rsp, __stack_top

    ; ── Zéroïse la BSS de exo-boot Rust ──────────────────────────────────────
    extern __bss_start
    extern __bss_end
    mov  rdi, __bss_start
    xor  rax, rax
.bss_zero:
    cmp  rdi, __bss_end
    jge  .bss_done
    mov  [rdi], rax
    add  rdi, 8
    jmp  .bss_zero
.bss_done:

    ; ── Appel de exoboot_main_bios(e820_buffer, count) ───────────────────────
    ; Convention d'appel System V AMD64 :
    ;   RDI = premier argument (e820_buffer_addr)
    ;   RSI = deuxième argument (e820_count)
    mov  rdi, E820_BUFFER_PHYS
    movzx rsi, word [e820_count]    ; ZX pour convertir u16 → u64

    ; Appel du point d'entrée Rust
    extern exoboot_main_bios
    call exoboot_main_bios

    ; Ne doit jamais retourner
    cli
.hang:
    hlt
    jmp  .hang
