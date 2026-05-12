# Rapport kernel - timekeeping, traces et warning percpu

## Probleme initial

Le boot post-shell montrait encore:

```text
[CAL:PIT-DRV-FAIL][CAL:FB3G hz=3000000000][TIME-INIT hz=3000000000]
```

Le shell etait utilisable, mais le noyau retombait sur une frequence TSC fixe. Cette valeur suffisait pour booter, mais elle rendait les timeouts et benchmarks moins fiables.

## Cause racine

`kernel/src/arch/x86_64/cpu/tsc.rs` programmait le PIT canal 2 pour environ 10 ms:

```text
duration = PIT_CALIBRATE_COUNT / PIT_BASE_HZ
```

La formule de frequence correcte est donc:

```text
tsc_hz = tsc_delta * PIT_BASE_HZ / PIT_CALIBRATE_COUNT
```

L'ancienne implementation multipliait encore par `100`, comme si `tsc_delta` etait mesure sur une fenetre artificielle de 10 ms apres coup. Le resultat devenait environ 100 fois trop grand, et la validation rejetait la mesure comme impossible. La chaine de calibration passait alors au fallback `FB3G`.

## Correction

La formule PIT a ete corrigee dans:

```text
kernel/src/arch/x86_64/cpu/tsc.rs
```

Resultat valide:

```text
[CAL:PIT-DRV hz=2614777097][TIME-INIT hz=2614800000]
```

## Nettoyage des traces kernel

Les traces suivantes saturaient le log de boot debug:

- `kstack:`
- `fork:`
- `fork_dbg:`
- `execve:`
- `pf: user`
- `boot_payload:`
- `elf:`
- `init_create:`

Elles restent disponibles, mais uniquement en mode explicite:

```bash
EXO_KERNEL_TRACE=1 make iso
```

Le build normal garde un log E9 lisible pour l'utilisateur et pour les tests QEMU.

## Warning percpu

Le warning d'import inutilise dans:

```text
kernel/src/arch/x86_64/smp/percpu.rs
```

a ete corrige en utilisant les chemins qualifies uniquement dans le bloc `target_os = "none"`. Cela evite d'importer `PhysAddr` et `phys_to_virt` dans les builds ou ils ne sont pas consommes.

## Validation

Commandes executees:

```bash
cargo fmt -p exo-os-kernel
cargo check -p exo-os-kernel --message-format short
```

Verification QEMU:

```text
docs/special/1/qemu_verify/e9.log
```

Le log ne contient plus `CAL:FB3G`, `kstack:`, `fork_dbg:`, `execve:`, `pf: user`, `panic` ou `fault` sur le chemin valide.

