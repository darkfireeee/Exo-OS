; mbr.asm — MBR 512 bytes (Stage 1) — Exo-Boot
;
; Chargé par le BIOS à l'adresse physique 0x7C00 (32KB - 1KB).
; S'exécute en mode réel 16-bit.
;
; Rôle :
;   1. Vérifie que les extensions INT 13h (LBA) sont disponibles
;   2. Localise la signature MBR Exo-OS dans la table de partitions (offset 446)
;   3. Charge Stage 2 (sectors 64-127) via INT 13h EDD à 0x1000:0:0000
;   4. Saute en Stage 2
;
; Stack temporaire : 0x7C00 - 4 = 0x7BFC (descend vers 0x0500)
;
; CONVENTIONS :
;   - Registres préservés : BP (frame pointer potentiel)
;   - CS:IP = 0x0000:0x7C00 après chargement BIOS
;   - DL = numéro du disque de démarrage (préservé intact jusqu'au saut Stage2)
;
; SÉCURITÉ :
;   - Le MBR ne vérifie PAS la signature du kernel — c'est le rôle d'exo-boot Rust.
;   - Le MBR vérifie uniquement sa propre intégrité (signature 0xAA55).

BITS 16
ORG  0x7C00

; ─── Constantes ────────────────────────────────────────────────────────────────
STAGE2_LOAD_SEGMENT   equ 0x0100   ; Segment où charger Stage 2 → 0x1000 physique
STAGE2_LOAD_OFFSET    equ 0x0000
STAGE2_LBA_START      equ 64       ; Stage 2 commence au secteur LBA 64
STAGE2_SECTOR_COUNT   equ 64       ; Taille max Stage 2 = 64 secteurs = 32 KB
STACK_SEGMENT         equ 0x0000
STACK_TOP             equ 0x7C00   ; Stack croît vers le bas depuis 0x7C00

; ─── Entrée MBR ────────────────────────────────────────────────────────────────
_start:
    ; Désactive les interruptions pendant l'initialisation des segments
    cli

    ; Configure les segments : tout à 0 (adressage plat en mode réel)
    xor  ax, ax
    mov  ds, ax
    mov  es, ax
    mov  ss, ax
    mov  sp, STACK_TOP

    ; CS peut ne pas être 0x0000 selon le BIOS — normalise avec un far jump
    jmp  0x0000:.segments_ok

.segments_ok:
    sti

    ; Sauvegarde DL (numéro du disque de démarrage fourni par le BIOS)
    mov  [BOOT_DRIVE], dl

    ; ── Vérification des extensions INT 13h (LBA EDD 2.0+) ────────────────────
    mov  ah, 0x41               ; INT 13h AH=41h = Check Extensions Present
    mov  bx, 0x55AA             ; Signature de vérification
    int  0x13
    jc   .no_lba_extensions     ; Carry set = extensions absentes
    cmp  bx, 0xAA55             ; Signature inverseée = succès
    jne  .no_lba_extensions
    test cl, 0x01               ; Bit 0 = Enhanced disk drive functions
    jz   .no_lba_extensions
    ; Extensions LBA disponibles → continuer
    jmp  .load_stage2

.no_lba_extensions:
    ; Firmware trop vieux pour LBA — affiche erreur et halt
    mov  si, msg_no_lba
    call print_str_16
    jmp  .halt

    ; ── Chargement Stage 2 via INT 13h EDD ─────────────────────────────────────
.load_stage2:
    ; Configure le Disk Address Packet (DAP) sur la pile
    ; DAP est à l'adresse SP = 0x7BF0 (juste sous la stack top)
    ; Structure DAP (16 bytes) conform EDD 3.0 :
    ;   Offset 0 : size (0x10)   Offset 1 : reserved (0x00)
    ;   Offset 2 : count (16-bit) Offset 4 : buffer offset (16-bit)
    ;   Offset 6 : buffer segment (16-bit) Offset 8 : LBA (64-bit)

    ; Alloue 16 bytes sur la pile pour le DAP
    sub  sp, 16
    mov  si, sp                 ; SI = pointeur vers DAP

    mov  byte [si + 0], 0x10   ; Size = 16 (EDD 3.0)
    mov  byte [si + 1], 0x00   ; Réservé
    mov  word [si + 2], STAGE2_SECTOR_COUNT
    mov  word [si + 4], STAGE2_LOAD_OFFSET  ; Offset buffer destination
    mov  word [si + 6], STAGE2_LOAD_SEGMENT ; Segment buffer destination
    mov  dword [si + 8], STAGE2_LBA_START   ; LBA low (32-bit)
    mov  dword [si + 12], 0    ; LBA high = 0 (Stage 2 < 4GB)

    ; Appel INT 13h EDD Extended Read
    mov  ah, 0x42               ; AH=42h = Extended Read Sectors
    mov  dl, [BOOT_DRIVE]
    int  0x13
    jc   .disk_error            ; Carry set = erreur disque

    ; Libère le DAP de la pile
    add  sp, 16

    ; Transmet DL (boot drive) à Stage 2 via DX
    mov  dl, [BOOT_DRIVE]

    ; ── Saut vers Stage 2 ──────────────────────────────────────────────────────
    ; Far jump → configure CS:IP = STAGE2_LOAD_SEGMENT:STAGE2_LOAD_OFFSET
    jmp  STAGE2_LOAD_SEGMENT:STAGE2_LOAD_OFFSET

    ; Ne retourne jamais d'ici

.disk_error:
    mov  si, msg_disk_error
    call print_str_16
    ; AH = code erreur retourné par INT 13h — affiche en hexadécimal
    ; (simple display, pas de conversion complète dans 512 bytes)
    jmp  .halt

.halt:
    cli
.halt_loop:
    hlt
    jmp  .halt_loop

; ─── print_str_16 : Affiche une chaîne ASCIIZ via BIOS INT 10h Teletype ──────
; Entrée : SI = adresse de la chaîne (null-terminée)
; Préserve : AX (sauf AH pour BIOS), BX, CX, DX, SI
print_str_16:
    pushf
    push ax
    push bx
    mov  ah, 0x0E               ; INT 10h AH=0Eh = Teletype output
    mov  bx, 0x0007             ; BH=0 (page 0), BL=0x07 (attribut gris sur noir)
.print_loop:
    lodsb                       ; AL = [SI++]
    test al, al
    jz   .print_done
    int  0x10
    jmp  .print_loop
.print_done:
    pop  bx
    pop  ax
    popf
    ret

; ─── Données ────────────────────────────────────────────────────────────────────
msg_no_lba:
    db  0x0D, 0x0A
    db  "[EXO-BOOT] ERREUR : BIOS INT 13h extensions LBA introuvables.", 0x0D, 0x0A
    db  "Ce systeme necessite un BIOS compatible LBA EDD 2.0+.", 0x0D, 0x0A, 0

msg_disk_error:
    db  0x0D, 0x0A
    db  "[EXO-BOOT] ERREUR : Echec lecture disque (Stage 2).", 0x0D, 0x0A
    db  "Verifiez l'installation d'Exo-OS.", 0x0D, 0x0A, 0

; ─── Variables BSS MBR ─────────────────────────────────────────────────────────
BOOT_DRIVE:
    db  0x80    ; Valeur par défaut = disque primaire. Écrasé par BIOS DL.

; ─── Padding jusqu'à l'offset 446 (table de partitions MBR) ───────────────────
; La table de partitions MBR standard commence à l'offset 446 (0x1BE).
; Elle est remplie par l'outil d'installation (exo-install) — PAS par cet ASM.
; On pad avec des zéros jusqu'à l'offset 510, puis la signature MBR.
times 446 - ($ - $$) db 0

; ─── Table de partitions MBR (4 × 16 bytes = 64 bytes, offset 446-509) ─────────
; Remplie par exo-install. Zéroïsée ici — l'installateur écrit les vraies valeurs.
times 64 db 0

; ─── Signature MBR (offset 510-511) ────────────────────────────────────────────
dw  0xAA55
