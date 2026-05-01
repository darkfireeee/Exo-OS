# `boot/trampoline_asm.rs` — trampoline SMP 16 → 32 → 64 bits

> Fichier source : `kernel/src/arch/x86_64/boot/trampoline_asm.rs`

Ce trampoline est exécuté par chaque **Application Processor** après les
`INIT` / `SIPI` envoyés par le BSP. Sa mission est unique : amener l’AP dans
un état 64 bits minimal, charger la pile dédiée, puis appeler `ap_entry()`.

---

## 1. Rôle fonctionnel

Le trampoline ne fait pas de politique système.
Il sert uniquement à :

- passer du mode réel au mode protégé
- activer `PAE`, `LME` et le paging nécessaire au long mode
- charger la GDT temporaire du trampoline
- restaurer les paramètres CPU localement utiles
- transférer le contrôle à Rust via `ap_entry(cpu_id, lapic_id, kernel_stack_top)`

---

## 2. Position mémoire et contrat d’exécution

Le trampoline est copié à l’adresse physique `0x6000`.

### Pourquoi `0x6000`

- c’est l’emplacement classique de la page SIPI (`TRAMPOLINE_PAGE = 6`)
- l’adresse est sous 1 MiB
- elle est accessible très tôt avec l’identity mapping de boot

### Contrat d’entrée

Au moment du `SIPI` :

| Élément | Valeur attendue |
|---|---|
| mode CPU | réel 16 bits |
| `CS:IP` | `0x0000:0x6000` |
| `IF` | 0 |
| trampoline copié | oui |
| PML4 du BSP écrit dans le trampoline | oui |

---

## 3. Layout interne du trampoline

Le code et les données partagées vivent dans la même image.

```text
0x6000 ─────────────────────────────────────────────
0x0000 : code 16 bits
0x0010 : handshake u32     (AP_ALIVE_MAGIC)
0x0020 : pml4_phys u64     (CR3 du BSP)
0x0028 : cpu_count u32     (réservé / diagnostic)
0x0030 : code 32 bits
0x0050 : GDT/GDTR temporaires
0x0080 : code 64 bits
0x00C0 : point d’entrée final 64 bits
────────────────────────────────────────────────────
```

### Vue ASCII du flux

```text
[16 bits] → [32 bits] → [64 bits] → ap_entry()
   │            │           │
   │            │           └─ pile AP + paramètres Rust
   │            └─ PAE / CR3 / EFER.LME / CR0.PG
   └─ segments + saut lointain
```

---

## 4. Séquence détaillée

### 4.1 Phase 16 bits

Le trampoline démarre avec :

- désactivation des interruptions (`cli`)
- remise à zéro des segments (`ds/es/ss = 0`)
- chargement de la GDT temporaire
- bascule vers le mode protégé avec `CR0.PE = 1`

Ensuite il saute manuellement en 32 bits.

### 4.2 Phase 32 bits

Dans cette phase, le trampoline prépare le passage en long mode :

- charge les segments noyau 32 bits
- active `CR4.PAE`
- charge `CR3` avec le PML4 du BSP
- active `EFER.LME`
- active `CR0.PG`

#### Diagramme de transition

```text
CR0.PE=1
   ↓
mode protégé 32 bits
   ↓
CR4.PAE=1
CR3 = PML4 BSP
EFER.LME=1
CR0.PG=1
   ↓
long mode armé
```

### 4.3 Phase 64 bits

Une fois la transition validée, le trampoline :

- charge la pile AP en `rsp`
- aligne la pile sur 16 octets
- lit `cpu_id` et `lapic_id` dans la zone partagée
- appelle `ap_entry()`

---

## 5. GDT temporaire

Le trampoline embarque sa propre GDT minimale.

```text
null descriptor
descriptor code 64 bits
descriptor data 32 bits
```

### Pourquoi une GDT temporaire

Parce que l’AP doit survivre au saut entre les modes avant d’utiliser la
GDT/IST complète configurée par `gdt::init_gdt_for_cpu()`.

---

## 6. Installation par le BSP

La fonction Rust `install_trampoline()` fait trois choses :

1. copie le trampoline en mémoire physique à `0x6000`
2. récupère le `CR3` courant du BSP
3. écrit ce `CR3` à l’offset `0x20`

### Schéma

```text
trampoline binaire linker
        │
        ├─ copy_nonoverlapping()
        │
        ▼
   phys 0x6000
        │
        └─ offset 0x20 = PML4 BSP
```

---

## 7. Handshake BSP ↔ AP

Le BSP surveille une valeur magique écrite par l’AP.

```text
BSP:  écrire 0 dans [0x6010]
AP :  démarrer trampoline
AP :  écrire AP_ALIVE_MAGIC dans [0x6010]
BSP:  boucle d’attente jusqu’au signal
```

### Pourquoi ce handshake

- il permet de distinguer un AP vivant d’un AP silencieux
- il protège contre les chipsets qui nécessitent deux `SIPI`
- il fournit un point de diagnostic simple pour le boot SMP

---

## 8. Paramètres transmis à `ap_entry()`

```text
cpu_id            → argument 1 (`edi`)
lapic_id          → argument 2 (`esi`)
kernel_stack_top  → argument 3 (`rdx`)
```

### Rôle de `ap_entry()`

Une fois appelée, la partie assembleur du trampoline a fini son travail.
Le Rust prend alors en charge :

- `percpu` pour l’AP
- GDT/TSS locaux
- IDT partagée
- `SYSCALL`
- LAPIC
- TSC
- FPU
- mitigations
- publication du contexte d’attente avant `sti`

---

## 9. Mapping logique des zones

```text
0x6000  trampoline_start
0x6010  AP handshake magic
0x6020  PML4 physique BSP
0x6028  cpu_count / réserves
0x6050  GDT/GDTR temporaire
0x6080  code 32 bits
0x60C0  code 64 bits
```

---

## 10. Contraintes et invariants

- le trampoline doit rester sous 1 MiB
- le BSP doit avoir publié un PML4 valide avant `smp_boot_aps()`
- `TRAMPOLINE_PHYS` doit être identité-mappé pendant l’installation
- la pile AP doit être accessible avant l’entrée 64 bits
- le saut final ne doit jamais revenir ; un `hlt` de secours est prévu

---

## 11. Sécurité et robustesse

### Ce que le trampoline ne doit pas faire

- aucune allocation
- aucun appel à l’allocateur dynamique
- aucune dépendance à un scheduler déjà vivant
- aucun accès mémoire hors contrat d’identity mapping

### Risques couverts

- AP absent ou désactivé dans la MADT
- chipset nécessitant deux `SIPI`
- AP qui n’atteint pas le long mode
- AP qui n’exécute jamais `ap_entry()`

Dans tous ces cas, le BSP retombe sur un timeout d’attente plutôt que de
se bloquer définitivement.
