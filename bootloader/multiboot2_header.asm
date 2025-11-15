; multiboot2_header.asm
; Header Multiboot2 pour le bootloader

section .multiboot_header
header_start:
    ; Magic number
    dd 0xe85250d6                ; multiboot2 magic
    ; Architecture (i386 protected mode)
    dd 0                         ; architecture 0 (i386)
    ; Header length
    dd header_end - header_start
    ; Checksum
    dd 0x100000000 - (0xe85250d6 + 0 + (header_end - header_start))

    ; Information request tag (mémoire et mmap uniquement)
    align 8
info_request_tag_start:
    dw 1                         ; type = info request
    dw 0                         ; flags
    dd info_request_tag_end - info_request_tag_start
    dd 3                         ; request memory map
    dd 6                         ; request memory info
info_request_tag_end:

    ; Module alignment tag
    align 8
module_align_tag_start:
    dw 6                         ; type = module alignment
    dw 0                         ; flags
    dd module_align_tag_end - module_align_tag_start
module_align_tag_end:

    ; End tag
    align 8
    dw 0                         ; type = end
    dw 0                         ; flags
    dd 8                         ; size

header_end:
