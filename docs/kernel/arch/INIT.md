# Initialisation `arch` — `arch_boot_init()` et séquence de boot

## Signature

```rust
pub unsafe fn arch_boot_init(mb2_magic: u32, mb2_info: u64, rsdp_phys: u64) -> BootInfo
```

## Rôle

`arch_boot_init()` est l’orchestrateur BSP de la couche architecture. Il
prépare le noyau avant le retour vers le chemin normal d’initialisation.

La fonction suppose :

- CPU déjà en mode long 64 bits
- interruptions désactivées
- pile BSP valide
- identité mémoire boot disponible pour le trampoline SMP

---

## Séquence de boot

```
bootloader
 │
 ├─ 1. Détection CPU + features
 ├─ 2. GDT BSP + TSS
 ├─ 3. IDT
 ├─ 4. per-CPU BSP / GS
 ├─ 5. TSC
 ├─ 6. FPU / SSE / AVX
 ├─ 7. Détection hyperviseur
 ├─ 8. ACPI (RSDP → MADT → HPET → PM timer)
 ├─ 9. APIC + IOAPIC + calibrage LAPIC timer
 ├─ 10. Pont mémoire (`memory_iface`)
 ├─ 11. MSR SYSCALL
 ├─ 12. Parsing Multiboot2 / exo-boot
 ├─ 13. Protections mémoire
 ├─ 14. Mitigations Spectre/Meltdown
 ├─ 15. Sécurité globale
 ├─ 16. Installation trampoline SMP
 └─ 17. Boot APs puis retour `BootInfo`
```

---

## Étapes détaillées

### Étape 1 — Détection CPU

- initialise les `CpuFeatures`
- vérifie les prérequis de base (`SSE2`, `SYSCALL`)

### Étape 2 — GDT / TSS

- charge la GDT BSP
- configure la pile noyau et les IST nécessaires

### Étape 3 — IDT

- initialise la table des interruptions
- charge l’IDT avant toute activation d’interruptions

### Étape 4 — per-CPU BSP / GS

- initialise la structure CPU-local du BSP
- prépare le GS base pour les fast paths

### Étape 5 — TSC

- fixe l’horloge de base du BSP
- sert de référence pour la suite du boot

### Étape 6 — FPU / SSE / AVX

- prépare l’état vectoriel et les extensions requises

### Étape 7 — Hypervisor

- détecte un environnement virtualisé
- prépare les chemins paravirt si disponibles

### Étape 8 — ACPI

- localise RSDP, MADT, HPET et PM timer
- extrait la topologie et les timers matériels

### Étape 9 — APIC

- initialise le système APIC
- configure IOAPIC et calibres du timer LAPIC

### Étape 10 — mémoire

- branche les interfaces `arch ↔ memory`
- enregistre l’espace d’adressage noyau courant

### Étape 11 — SYSCALL

- configure les MSR nécessaires aux entrées SYSCALL/SYSRET

### Étape 12 — protocole de boot

- parse Multiboot2 ou exo-boot
- remplit `BootInfo`

### Étape 13 — protections mémoire

- active les protections matérielles (`NX`, `SMEP`, `SMAP`, `PKU`)

### Étape 14 — mitigations

- applique les protections Spectre/Meltdown côté BSP

### Étape 15 — sécurité

- publie `SECURITY_READY` lorsque le boot sécurité est prêt

### Étape 16 — SMP

- copie le trampoline AP
- le couple à la PML4 du BSP
- démarre les AP via `INIT`/`SIPI`

### Étape 17 — fin de séquence

- remplit `BootInfo`
- retourne vers le noyau principal

---

## Dépendances de boot

`arch_boot_init()` doit être appelée après :

1. `memory::init()`
2. la table futex globale
3. les bases d’allocation mémoire nécessaires
4. l’initialisation minimale de sécurité

Elle doit précéder :

- l’ordonnanceur complet
- les chemins IPC de niveau supérieur
- le retour en mode pleinement opérationnel

---

## Invariants importants

- le BSP garde les interruptions désactivées jusqu’à ce que l’IDT, TSS et APIC soient prêts
- `SYSCALL` est initialisé avant toute sortie vers userspace
- le trampoline AP n’est installé qu’après la configuration mémoire requise
- `security::is_security_ready()` doit pouvoir libérer les AP en attente

---

## Référence de lecture

- [OVERVIEW.md](OVERVIEW.md) — vue d’ensemble du module
- [API.md](API.md) — surface publique
- [BOOT_TRAMPOLINE.md](BOOT_TRAMPOLINE.md) — trampoline SMP détaillé