# 🖥️ x86_64 - Architecture Support

## Vue d'ensemble

Support complet de l'architecture x86_64 avec optimisations spécifiques.

## Architecture

```
kernel/src/arch/x86_64/
├── boot/              # Séquence de boot
├── cpu/               # Features CPU, MSRs, CPUID
├── interrupts/        # APIC, IOAPIC, IDT
├── memory/            # Paging, PAT, TLB
├── gdt.rs             # Global Descriptor Table
├── idt.rs             # Interrupt Descriptor Table
├── tss.rs             # Task State Segment
├── syscall.rs         # SYSCALL/SYSRET
├── simd.rs            # SSE/AVX support
└── serial.rs          # Debug serial output
```

## Modules

- [Boot Sequence](./boot.md)
- [CPU Features](./cpu.md)
- [Interrupts & APIC](./interrupts.md)
- [Memory Management](./memory.md)
- [System Calls](./syscall.md)
