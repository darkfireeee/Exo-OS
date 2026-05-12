# ExoOS — Plan de corrections post-shell
> Référence : log `codex-qemu3-e9.log` — shell validé, boot 15-20s, TSC FB3G, top PID swap

---

## Vue d'ensemble

```
[FIX-1] Strip binaires
      ↓
[FIX-2] TSC calibration CPUID 0x15
      ↓
[FIX-3] top PID name swap
      ↓
[FIX-4] time builtin exosh
      ↓
[FIX-5] dd builtin exosh
      ↓
[BENCH] Benchmark ExoFS
```

---

## FIX-1 — Strip des binaires au build

### Problème
Les 13 binaires Ring1 embarquent leurs debug symbols. Tailles actuelles :

| Binaire | Taille actuelle |
|---|---|
| exo-crypto-server | 6.3 MB |
| exo-shield | 3.5 MB |
| exosh | 2.6 MB |
| autres ×10 | ~2.5-2.9 MB chacun |
| **Total payload** | **~37 MB** |

Conséquences :
- ELF loading lent au boot (~3-5s estimé)
- Seulement ~475 MB libres dans l'image pour les benchmarks (sur 512 MB)
- fsck proportionnel au nombre de blocs alloués → boot lent

### Fix
Dans le `Makefile` ou le script de build, ajouter une passe `strip` sur chaque binaire avant injection dans l'image ExoFS :

```makefile
# Après cargo build --release, avant mkexofs
strip target/x86_64-unknown-none/release/exo-init-server
strip target/x86_64-unknown-none/release/exo-ipc-router
strip target/x86_64-unknown-none/release/exo-memory-server
strip target/x86_64-unknown-none/release/exo-vfs-server
strip target/x86_64-unknown-none/release/exo-crypto-server
strip target/x86_64-unknown-none/release/exo-device-server
strip target/x86_64-unknown-none/release/exo-virtio-drivers
strip target/x86_64-unknown-none/release/exo-network-server
strip target/x86_64-unknown-none/release/exo-scheduler-server
strip target/x86_64-unknown-none/release/exo-input-server
strip target/x86_64-unknown-none/release/exo-tty-server
strip target/x86_64-unknown-none/release/exosh
strip target/x86_64-unknown-none/release/exo-shield
```

### Résultat attendu
- Payload ~37 MB → **~5-6 MB** (binaires Rust strippés typiquement 200-500 KB chacun)
- ~506 MB libres dans l'image pour les benchmarks
- Boot estimé : **-3 à -5s**

### Validation
```sh
exosh:/$ ls -lah /sbin/
# Vérifier que chaque binaire est < 1 MB
```

---

## FIX-2 — Calibration TSC via CPUID 0x15

### Problème
Log ligne 1 : `[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]`

La chaîne de calibration échoue jusqu'au fallback nominal 3 GHz hardcodé. Si la fréquence réelle de l'hôte est différente de 3 GHz (ex: 3.2 GHz), `ktime_get_ns()` dérive de ~6%, ce qui affecte :
- Précision des quanta scheduler
- Timeouts IPC
- Tout benchmark basé sur le temps

La chaîne actuelle :
```
HPET → PM Timer → CPUID 0x15 → PIT → Nominal (3 GHz)  ← on atterrit ici
```

### Fix
Remonter CPUID 0x15 en **première position** de la chaîne. QEMU expose la fréquence TSC dans cette leaf si KVM est actif. La leaf retourne :

```
EAX = ratio dénominateur (Core Crystal Clock / TSC)
EBX = ratio numérateur
ECX = fréquence Core Crystal Clock en Hz
TSC_hz = ECX * EBX / EAX
```

Localiser dans le code la fonction de calibration TSC (probablement dans `arch/x86_64/time.rs` ou `drivers/tsc.rs`) et ajouter en tête :

```rust
fn calibrate_tsc_cpuid0x15() -> Option<u64> {
    let cpuid = unsafe { core::arch::x86_64::__cpuid(0x15) };
    // EAX=0 ou EBX=0 → leaf non supportée
    if cpuid.eax == 0 || cpuid.ebx == 0 {
        return None;
    }
    if cpuid.ecx == 0 {
        // Crystal freq inconnue — QEMU KVM la remplit toujours, si ecx=0 → not KVM
        return None;
    }
    let tsc_hz = (cpuid.ecx as u64) * (cpuid.ebx as u64) / (cpuid.eax as u64);
    if tsc_hz < 100_000_000 || tsc_hz > 10_000_000_000 {
        // Valeur aberrante
        return None;
    }
    Some(tsc_hz)
}
```

Nouvelle chaîne :
```
CPUID 0x15 → HPET → PM Timer → PIT → Nominal (3 GHz)
```

### Résultat attendu
Log ligne 1 devient : `[CAL:CPUID15 hz=XXXX][TIME-INIT hz=XXXX]`

TSC précis → `ktime_get_ns()` fiable → benchmarks valides.

### Validation
Vérifier que `hz` affiché correspond à la fréquence réelle de la VM QEMU (configurable dans le `-cpu` flag QEMU, typiquement 2-4 GHz).

---

## FIX-3 — top : PID/nom swappés

### Problème
Le log init_server indique :
```
init: spawned exo_shield pid=12
init: spawned exosh pid=13
```

Mais `top` affiche :
```
12   exosh            running   ← FAUX, c'est exo_shield
13   exo_shield       running   ← FAUX, c'est exosh
```

Les noms sont swappés dans la lookup de noms du builtin `top`.

### Cause probable
La table de noms est itérée dans l'ordre d'insertion mais les PIDs sont assignés dans un ordre différent (exosh fork avant exo_shield dans le code, mais exo_shield est spawné en premier par init). Le `top` builtin fait probablement une association `index → nom` au lieu de `pid → nom`.

### Fix
Dans le code du builtin `top` d'exosh, la lookup doit faire `pcb.pid → pcb.name` directement via le syscall approprié (probablement `SYS_PROCESS_LIST` ou équivalent), sans hypothèse sur l'ordre d'insertion.

### Validation
```sh
exosh:/$ top
# PID 12 → exo_shield
# PID 13 → exosh
```

---

## FIX-4 — Builtin `time` dans exosh

### Prérequis
FIX-2 (TSC précis) doit être appliqué, sinon `time` mesure des nanosecondes incorrectes.

### Implémentation
`time` est un builtin shell qui :
1. Lit le TSC via `rdtsc` avant l'exécution de la commande
2. Fork/exec la commande normalement
3. Attend la fin via `waitpid`
4. Relit le TSC après
5. Calcule la durée via `BOOT_TSC_KHZ` (déjà disponible dans le kernel)

```rust
// Dans exosh builtins
fn builtin_time(args: &[&str]) -> i32 {
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    let status = exec_command(args);
    let end = unsafe { core::arch::x86_64::_rdtsc() };
    let tsc_khz = get_tsc_khz(); // syscall ou constante exportée
    let ms = (end - start) / tsc_khz;
    println!("real {}ms", ms);
    status
}
```

### Résultat attendu
```sh
exosh:/$ time cat /tmp/t/a
hi
real 12ms
```

### Validation
```sh
exosh:/$ time echo test
test
real Xms   # doit être < 5ms pour une commande triviale
```

---

## FIX-5 — Builtin `dd` dans exosh

### Rôle
Outil de benchmark I/O brut. Lit/écrit des blocs de taille fixe, mesure le débit.

### Syntaxe cible
```sh
dd if=/dev/zero of=/tmp/bench bs=1M count=256
dd if=/tmp/bench of=/dev/null bs=1M
```

### Implémentation minimale
`dd` dans exosh n'a pas besoin d'être complet POSIX. Version minimale :
- `if=` : source (`/dev/zero` → génère des zéros, ou fichier existant)
- `of=` : destination (`/dev/null` → discard, ou fichier)
- `bs=` : block size (ex: `4K`, `1M`)
- `count=` : nombre de blocs

Pour le débit, utiliser `time` combiné ou intégrer la mesure TSC directement dans `dd`.

### Résultat attendu
```sh
exosh:/$ dd if=/dev/zero of=/tmp/bench bs=1M count=256
256 MB écrits en Xms → Y MB/s
```

---

## BENCH — Benchmark ExoFS

### Prérequis
FIX-1 + FIX-2 + FIX-4 + FIX-5 tous appliqués.

### Procédure

#### Écriture séquentielle
```sh
exosh:/$ dd if=/dev/zero of=/tmp/seq_write bs=1M count=256
# Mesure : débit écriture séquentielle ExoFS
```

#### Lecture séquentielle
```sh
exosh:/$ dd if=/tmp/seq_write of=/dev/null bs=1M
# Mesure : débit lecture séquentielle (avec cache VFS)
```

#### Petits fichiers (coût lookup capabilities)
```sh
exosh:/$ time for i in $(seq 1 1000); do touch /tmp/f$i; done
exosh:/$ time for i in $(seq 1 1000); do rm /tmp/f$i; done
# Mesure : coût par opération sur petits fichiers
```

#### fsck seul
```sh
# Mesurer au boot via time depuis le host :
time make qemu-release-phoenix-resurrection
# Comparer avant/après FIX-1 (strip binaires)
```

### Interprétation des résultats

| Mesure | Attendu sous QEMU virtio-blk | Bon signe |
|---|---|---|
| Écriture séquentielle 256 MB | 150-350 MB/s | > 200 MB/s |
| Lecture séquentielle 256 MB | 300-600 MB/s | > 400 MB/s |
| 1000 touch | < 2s total | < 1ms/op |
| Boot total post-fix | < 5s | < 3s |

### Nuance importante
Ces chiffres mesurent la **pile complète** : ExoFS → vfs_server → virtio-blk → QEMU → host filesystem. Ils ne reflètent pas la vitesse intrinsèque d'ExoFS seul. Pour isoler ExoFS, il faudrait un ramdisk (image en RAM pure sans virtio), ce qui est une étape ultérieure.

---

## Récapitulatif des délais estimés

| Fix | Complexité | Impact boot | Impact bench |
|---|---|---|---|
| FIX-1 Strip binaires | Faible (Makefile) | **-3 à -5s** | +506 MB libres |
| FIX-2 CPUID 0x15 | Faible (10 lignes Rust) | **-1 à -3s** | TSC précis |
| FIX-3 top PID swap | Faible (cosmétique) | aucun | aucun |
| FIX-4 time builtin | Moyen | aucun | chronométrage |
| FIX-5 dd builtin | Moyen | aucun | mesures I/O |

**Boot estimé post FIX-1+2 : 6-11s → objectif < 5s avec strip complet**
