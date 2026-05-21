# ExoOS v0.2.0 — Audit Architecturale (P1/P2)
## Incohérences Structurelles : Boot, SSR, Bibliothèques, Démarrage Ring1

**Auteur :** claude-beta  
**Date :** 2026-05-20  
**Sévérité :** P1 (boot/SSR), P2 (libs absentes)  
**Checklist :** BLOC 1 (P-07), BLOC 5, BLOC 6, CORR-81

---

## ARCH-01 — Ring1 : démarrage séquentiel au lieu de parallèle (CORR-81 ERR-11 / P-07)

**Fichier :** `servers/init_server/src/boot_sequence.rs`  
**Checklist :** P-07

### Situation actuelle

La fonction `boot_services()` itère sur les services dans une boucle
`while progress` qui démarre **un service à la fois** : elle attend la
confirmation IPC (`wait_for_ipc_ready`) de chaque service avant de
lancer le suivant.

```rust
// servers/init_server/src/boot_sequence.rs — boot_services()

while progress {
    // ...
    while idx < services.len() {
        // Spawn un service si ses dépendances sont satisfaites
        let pid = spawn_service(service.name, service.bin_path);
        service.set_pid(pid);

        // ← BLOQUANT : attend que ce service soit prêt avant de continuer
        if wait_for_ipc_ready(service.name, pid, timeout_ms) {
            progress = true;
        }
        idx += 1;
    }
}
```

### Ce que CORR-81 requiert (P-07)

Après une bascule ExoPhoenix A→B, les serveurs Ring1 doivent redémarrer
**en parallèle** (spawn tous les services dont les dépendances immédiates
sont satisfaites simultanément), puis attendre leur stabilisation en groupe.

Le démarrage séquentiel actuel entraîne une latence de boot proportionnelle
à la somme des `ready_timeout_ms` de la chaîne critique, potentiellement
`5+5+8+30+5+5+5+3+3+5+30+5 = ~109 secondes` dans le pire cas vs la cible
`< 500 ms` (P-04, P-05, P-06).

### Impact chiffré (estimation)

```
Chaîne séquentielle critique (si chaque service prend 2s à démarrer) :
  ipc_router → 2s
  memory_server → 2s (attend ipc_router)
  vfs_server → 2s (attend memory_server)
  crypto_server → 2s
  device_server → 2s
  ...
Total estimé : 12 services × 2s = 24 secondes au minimum

Chaîne parallèle (même scénario) :
  Wave 1 : ipc_router seul → 2s
  Wave 2 : memory_server + device_server en parallèle → 2s
  Wave 3 : vfs_server + scheduler_server + input_server → 2s
  Wave 4 : crypto_server + virtio_drivers → 2s
  Wave 5 : network_server → 2s
  Wave 6 : tty_server → 2s
  Wave 7 : exo_shield → 2s (long init mais non-blocking pour exosh)
  Wave 8 : exosh → 2s
Total estimé : 8 vagues × 2s = 16 secondes (même scénario, -33%)
```

### Correction requise

```rust
// servers/init_server/src/boot_sequence.rs — version parallèle

pub unsafe fn boot_services(services: &[Service]) -> usize {
    // PHASE 1 : spawn tous les services dont les deps sont satisfaites
    // (sans attendre leur readiness)
    let mut launched = [false; SERVICE_COUNT];
    loop {
        let mut any_launched = false;
        for (idx, svc) in services.iter().enumerate() {
            if launched[idx] || svc.current_pid() != 0 { continue; }
            if !supervisor::can_start(services, svc.name) { continue; }

            let pid = spawn_service(svc.name, svc.bin_path);
            if pid > 0 {
                svc.set_pid(pid);
                launched[idx] = true;
                any_launched = true;
            }
        }
        if !any_launched { break; }
        // bref yield pour laisser les deps se déclarer prêtes
        syscall::syscall0(syscall::SYS_SCHED_YIELD);
    }

    // PHASE 2 : attendre en groupe la readiness de tous les services lancés
    for (idx, svc) in services.iter().enumerate() {
        if !launched[idx] { continue; }
        let pid = svc.current_pid();
        if pid == 0 { continue; }
        if !wait_for_ipc_ready(svc.name, pid, dependency::ready_timeout_ms(svc.name)) {
            log::service_status(b"init: timeout ", svc.name, b"\n");
            syscall::syscall2(syscall::SYS_KILL, pid as u64, 15);
            svc.mark_dead();
        }
    }

    service_manager::running_count(services)
}
```

---

## ARCH-02 — SSR : const_assert! de taille manquant (O-02 / P-01)

**Fichier :** `kernel/src/exophoenix/ssr.rs`  
**Checklist :** O-02, P-01

### Situation actuelle

```rust
// kernel/src/exophoenix/ssr.rs

pub use exo_phoenix_ssr::{
    SSR_SIZE,
    // ...
};

// debug_assert! sur les offsets individuels SEULEMENT — pas de const_assert! sur SSR_SIZE
pub unsafe fn ssr_atomic(offset: usize) -> &'static AtomicU64 {
    debug_assert!(offset + core::mem::size_of::<AtomicU64>() <= SSR_SIZE);
    // ...
}
```

La taille de la SSR (`SSR_SIZE`) est définie dans la crate externe
`exo-phoenix-ssr`. Rien dans le kernel ne vérifie statiquement que
`SSR_SIZE <= 4096` octets.

### Pourquoi c'est critique

La SSR doit tenir dans une seule page physique (4 096 octets) pour être
mappée atomiquement entre Kernel A et Kernel B via une seule entrée de
table de pages. Si `SSR_SIZE > 4096`, le mapping span deux pages physiques
et la cohérence n'est plus garantie lors d'une bascule.

### Correction requise

```rust
// kernel/src/exophoenix/ssr.rs — ajouter après les use

// CONTRAT STRUCTUREL P-01 : la SSR doit tenir dans une page physique.
// SSR_SIZE est définie par la crate exo-phoenix-ssr. Ce const_assert!
// garantit que toute modification de la crate externe qui dépasserait
// 4096 octets échoue à la compilation du kernel.
const _: () = assert!(
    SSR_SIZE <= 4096,
    "SSR_SIZE > 4096 : la SSR ne peut plus être mappée en une seule page. \
     Réduire la structure ou augmenter SSR_MAX_PROCESSES."
);
```

---

## ARCH-03 — Bibliothèques ExoOS absentes (BLOC 5 et BLOC 6)

**Checklist :** L-01 à L-12 (BLOC 5), M-01 à M-08 (BLOC 6)

### Inventaire des crates requises non créées

Selon `VISION-V0.2.0.md` et `SPEC-EXO-CRATES.md`, les bibliothèques
userland suivantes sont requises pour v0.2.0 :

| Crate | Priorité (VISION) | État |
|---|---|---|
| `exo-alloc` | #1 — sans allocateur, rien ne compile | **ABSENT** |
| `musl-exo` core | #2 — fork/exec POSIX | **ABSENT** |
| `exo-crypto` | #3 — crypto IPC client | **ABSENT** |
| `exo-net` | #4 — smoltcp dans network_server | **ABSENT** |
| `exo-fs` | #5 — primitives ExoFS natives | **ABSENT** |
| `exo-pkg` | #6 — gestionnaire de paquets | **ABSENT** |
| `exo-runtime` | #7 — async executor | **ABSENT** |
| `exo-observability` | #8 — logging/tracing | **ABSENT** |
| `exo-libc` étendu | #9 — POSIX ~80% | **ABSENT** |
| `fat_server` | #10 — FAT/ext4 compat | **ABSENT** |

**La seule infrastructure userland existante :**
```
userspace/libexo/        → syscall wrappers basiques (sys.rs, vfs.rs)
userspace/apps/coreutils/ → cat, echo, ls, mkdir, rm, rmdir, touch (stubs)
userspace/apps/exosh/    → shell basique
```

### Impact sur la checklist

Sans `exo-alloc`, aucune application Ring3 ne peut s'exécuter en dehors
des crates `no_std` avec allocateur embarqué. Sans `musl-exo`, `fork()`
et `exec()` ne sont pas accessibles depuis le userspace POSIX standard.

Ces 10 crates bloquent **158 - (BLOC 5 + BLOC 6) = 138** critères en chaîne.

### Chemin critique de démarrage

```
exo-alloc (snmalloc no_std)
    ↓
musl-exo core (fork, exec, wait, pipe)
    ↓
exo-crypto client IPC + exo-net client IPC
    ↓
exo-fs natif (blobs, capabilities FS)
    ↓
exo-pkg (installe les premiers programmes)
    ↓
exo compat install calendar  ← critère final Pilier 4
```

---

## ARCH-04 — Démarrage exosh sans network_server (CORR-79 / B-10)

**Fichiers :**
- `servers/init_server/src/service_table.rs`
- `servers/init_server/src/supervisor.rs`
**Checklist :** B-10

### Situation actuelle — analyse

La chaîne de dépendances de `exosh` :

```
exosh → [ipc_router, tty_server, vfs_server, exo_shield]
exo_shield → [ipc_router, memory_server, vfs_server, crypto_server,
               device_server, input_server, tty_server]
           + optionnel [virtio_drivers, network_server, scheduler_server]
```

`network_server` est dans `requires_optional` d'`exo_shield`, donc théoriquement
exosh peut démarrer sans réseau. La logique de `supervisor::dependency_ready()`
le confirme : un service `critical=false` mort est considéré comme
"satisfait" pour ses dépendants.

**Cependant** : `network_server` a `ready_timeout_ms: 3_000` et `critical: false`.
Si `network_server` tente de démarrer (car ses propres dépendances sont satisfaites)
et échoue ou timeout, il est marqué `dead`. Ensuite, `exo_shield` peut démarrer.

### Risque résiduel

Le timeout de 3 secondes pour `network_server` est **trop court** pour
l'initialisation d'un stack réseau (smoltcp + virtio-net + DHCP discovery).
En pratique, `network_server` peut être marqué `dead` prématurément alors
qu'il est encore en cours d'initialisation, entraînant une absence de
réseau même quand le hardware est disponible.

### Recommandation

```rust
// servers/init_server/src/service_table.rs

ServiceMetadata {
    name: "network_server",
    ready_timeout_ms: 15_000,   // ← 3s → 15s (init smoltcp + DHCP)
    critical: false,
    // ...
},
```

Et ajouter un test explicite (B-10) :

```bash
# Test B-10 : boot QEMU sans réseau
qemu-system-x86_64 -kernel exoos.iso -net none
# Vérifier : exosh démarre et affiche le prompt "$"
# sans attendre ni crasher sur l'absence de network_server
```

---

## Récapitulatif Architecture

| ID | Fichier | Problème | Priorité | Checklist |
|---|---|---|---|---|
| ARCH-01 | `servers/init_server/src/boot_sequence.rs` | Ring1 séquentiel au lieu de parallèle | P1 | P-07 |
| ARCH-02 | `kernel/src/exophoenix/ssr.rs` | `const_assert!(SSR_SIZE <= 4096)` absent | P1 | O-02, P-01 |
| ARCH-03 | (workspace entier) | 10 crates userland absentes | P1 | BLOC 5, BLOC 6 |
| ARCH-04 | `servers/init_server/src/service_table.rs` | timeout network_server trop court | P2 | B-10 |

---

*claude-beta — ExoOS v0.2.0 Audit — AUDIT-ARCHITECTURE.md*
