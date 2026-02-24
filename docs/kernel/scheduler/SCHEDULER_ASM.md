# Scheduler ASM — switch_asm.s & fast_path.s

> **Sources** : `kernel/src/scheduler/asm/`  
> **Règles** : SCHED-06, SCHED-07, SCHED-08  
> **Assembleur** : AT&T syntax, LLVM integrated assembler

---

## Table des matières

1. [switch_asm.s — Commutation de contexte](#1-switch_asms--commutation-de-contexte)
2. [Ordre de sauvegarde des registres](#2-ordre-de-sauvegarde-des-registres)
3. [Sauvegarde MXCSR et x87 FCW](#3-sauvegarde-mxcsr-et-x87-fcw)
4. [Commutation CR3 et KPTI](#4-commutation-cr3-et-kpti)
5. [fast_path.s — Retour rapide interrupt](#5-fast_paths--retour-rapide-interrupt)
6. [Contraintes LLVM vs GNU as](#6-contraintes-llvm-vs-gnu-as)

---

## 1. switch_asm.s — Commutation de contexte

### Prototype C ABI

```c
// Appelé depuis switch.rs::context_switch()
void context_switch_asm(
    uint64_t *save_rsp,    // %rdi : &prev.kernel_rsp
    uint64_t  load_rsp,    // %rsi : next.kernel_rsp
    uint64_t  next_cr3     // %rdx : adresse physique PML4 de next
);
```

### Code annoté complet

```asm
.section .text
.global context_switch_asm
.type context_switch_asm, @function

context_switch_asm:
    # ─── PHASE 1 : Sauvegarde (SCHED-06) ───────────────────────────────────
    # r15 EN PREMIER — obligatoire (règle SCHED-06)
    push %r15
    push %r14
    push %r13
    push %r12
    push %rbp
    push %rbx

    # ─── PHASE 2 : Sauvegarde MXCSR + FCW (SCHED-07) ───────────────────────
    # Alloue 16 octets sur la pile pour MXCSR (32 bits) + FCW (16 bits)
    sub $16, %rsp
    stmxcsr 0(%rsp)    # MXCSR → [RSP+0]  (état SSE : arrondi, masques, flags)
    fstcw   8(%rsp)    # FCW   → [RSP+8]  (état x87 : précision, arrondi)

    # ─── PHASE 3 : Sauvegarde RSP de prev ───────────────────────────────────
    mov %rsp, (%rdi)   # prev.kernel_rsp = RSP actuel

    # ─── PHASE 4 : Commutation CR3 (SCHED-08) ───────────────────────────────
    # AVANT de charger RSP de next → respect KPTI
    mov %rdx, %rax
    mov %rax, %cr3     # Switch espace d'adressage + flush TLB (ou PCID)

    # ─── PHASE 5 : Chargement RSP de next ───────────────────────────────────
    mov %rsi, %rsp     # RSP = next.kernel_rsp

    # ─── PHASE 6 : Restauration MXCSR + FCW ────────────────────────────────
    fldcw   8(%rsp)    # Restaure FCW
    ldmxcsr 0(%rsp)    # Restaure MXCSR
    add $16, %rsp      # Libère l'espace MXCSR/FCW

    # ─── PHASE 7 : Restauration registres (ordre inverse) ───────────────────
    pop %rbx
    pop %rbp
    pop %r12
    pop %r13
    pop %r14
    pop %r15

    ret                # Retourne dans le contexte de next
```

---

## 2. Ordre de sauvegarde des registres

**Convention appelant x86-64 System V** : rbx, rbp, r12, r13, r14, r15 sont callee-saved.

| Étape | Registre | Raison |
|-------|----------|--------|
| 1 | `r15` | Premier — obligatoire SCHED-06 (détection corruption) |
| 2 | `r14` | Callee-saved |
| 3 | `r13` | Callee-saved |
| 4 | `r12` | Callee-saved |
| 5 | `rbp` | Frame pointer (callee-saved) |
| 6 | `rbx` | Callee-saved |
| 7 | `[MXCSR, FCW]` | État FPU de base (indépendant de XSAVE) |

**Registres non sauvés ici** (gérés ailleurs) :
- `rax, rcx, rdx, rsi, rdi, r8..r11` → caller-saved, le compilateur les gère
- Registres XMM/YMM/ZMM → gérés par `fpu/save_restore.rs` via XSAVE

### Pile avant `context_switch_asm` (frame prev)

```
RSP → [ rbx     ]  +0
      [ rbp     ]  +8
      [ r12     ]  +16
      [ r13     ]  +24
      [ r14     ]  +32
      [ r15     ]  +40
      [ MXCSR   ]  +48  (32 bits + 32 bits padding)
      [ FCW     ]  +56  (16 bits + 48 bits padding)
      ← prev.kernel_rsp pointe ici après mov %rsp, (%rdi)
```

---

## 3. Sauvegarde MXCSR et x87 FCW

### MXCSR (Media Control and Status Register)

Registre 32 bits contrôlant l'unité SSE/SSE2/AVX :

| Bits | Champ | Description |
|------|-------|-------------|
| 0 | IE | Invalid Operation Exception |
| 1 | DE | Denormal |
| 2 | ZE | Divide by Zero |
| 3 | OE | Overflow |
| 4 | UE | Underflow |
| 5 | PE | Precision |
| 6 | DAZ | Denormals Are Zeros |
| 7-12 | IM,DM,ZM,OM,UM,PM | Exception Masks |
| 13-14 | RC | Rounding Control |
| 15 | FZ | Flush to Zero |

**Valeur par défaut** : `0x1F80` (toutes exceptions masquées, arrondi au plus proche).

### x87 FCW (Floating-Point Control Word)

Registre 16 bits contrôlant l'unité x87 :

| Bits | Champ | Description |
|------|-------|-------------|
| 0-5 | Exception Masks | IM, DM, ZM, OM, UM, PM |
| 8-9 | PC | Precision Control (00=24b, 10=53b, 11=64b) |
| 10-11 | RC | Rounding Control |

**Valeur par défaut** : `0x037F` (double extended precision, arrondi au plus proche).

**Pourquoi sauvegarder MXCSR/FCW séparément de XSAVE** : XSAVE est optionnel et coûteux. MXCSR et FCW sont toujours présents depuis Pentium III / SSE. La sauvegarde dans la pile garantit la conformité même sans XSAVE.

---

## 4. Commutation CR3 et KPTI

### Pourquoi CR3 AVANT RSP (SCHED-08)

```
Incorrect (vulnérabilité KPTI) :
    mov %rsi, %rsp  ← RSP pointe maintenant dans espace next
    mov %rdx, %cr3  ← TLB flush APRÈS avoir utilisé RSP → accès avec ancien mapping

Correct (SCHED-08) :
    mov %rdx, %cr3  ← Switch espace d'adressage FIRST
    mov %rsi, %rsp  ← RSP chargé dans le nouvel espace d'adressage
```

### PCID (Process Context ID)

Si le CPU supporte PCID (bit 17 de CR4), le flush TLB est sélectif :
- Les bits [11:0] de CR3 encodent le PCID.
- Bit 63 de CR3 = 1 → pas de flush (réutilise les entrées TLB du PCID).
- Géré par `arch/x86_64/mm/` — transparent pour `switch_asm.s`.

### KPTI (Kernel Page Table Isolation)

Avec KPTI (Meltdown mitigation) :
- Chaque processus possède deux tables de pages : user (espace réduit) et kernel.
- `next.cr3` pointe vers la page table kernel de next.
- Le switch CR3 dans `switch_asm.s` bascule atomiquement l'espace d'adressage.

---

## 5. fast_path.s — Retour rapide interrupt

```asm
.section .text
.global sched_fast_return
.type sched_fast_return, @function

sched_fast_return:
    # Vérifie NEED_RESCHED sans acquérir de lock
    # Utilisé en sortie d'IRQ pour éviter un appel complet à schedule()
    testl $0x10, 64(%rdi)   # flags[NEED_RESCHED] dans TCB
    jnz   .do_schedule
    iretq

.do_schedule:
    # Sauvegarde minimale + appel schedule_yield
    call schedule_from_irq
    iretq
```

`fast_path.s` est appelé depuis le stub de retour interrupt (`arch/x86_64/idt.rs`) pour éviter un tour complet du scheduler quand `NEED_RESCHED` n'est pas positionné.

---

## 6. Contraintes LLVM vs GNU as

### Différences importantes (apprises en compilant Exo-OS)

| Instruction | GNU as | LLVM (Exo-OS) | Solution |
|-------------|--------|----------------|----------|
| `ljmp $seg, $off` | ✅ Supporté | ❌ Non supporté | Encodage `.byte` manuel |
| `sysretl` | ✅ (suffix l = 32b) | ❌ Inconnu | `sysret` |
| `.long -(expr)` | ✅ Valeur négative | ❌ Rejeté | Précalculer constante |
| `stmxcsr`, `ldmxcsr` | ✅ | ✅ | Direct |
| `fstcw`, `fldcw` | ✅ | ✅ | Direct |
| `mov %cr3` | ✅ | ✅ avec `unsafe` | Direct |

### Encodage manuel `ljmp` (trampoline_asm.rs)

```asm
# GNU as (non supporté par LLVM) :
ljmp $0x08, $0x6080

# LLVM (encodage opcode FF /5 = far jmp, puis EA = far jmp imm) :
.byte 0xEA                    # opcode far jmp absolu
.byte 0x80, 0x60, 0x00, 0x00  # offset 32 bits little-endian = 0x6080
.byte 0x08, 0x00              # segment selector = 0x0008
```
