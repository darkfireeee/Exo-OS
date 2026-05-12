# claude-iota-bug-P2-ipc-ready.md

**Sévérité** : P2 — Moyen  
**Fichier** : `servers/init_server/src/boot_sequence.rs`  
**Symptôme** : Race condition sur démarrage des services, timeouts aléatoires

---

## Description

`wait_for_ipc_ready()` utilise `kill(pid, 0)` comme proxy de readiness IPC :

```rust
// boot_sequence.rs
pub unsafe fn wait_for_ipc_ready(pid: u32, timeout_ms: u64) -> bool {
    let mut waited_ms = 0u64;
    while waited_ms <= timeout_ms {
        if pid_alive(pid) {   // ← kill(pid, 0) == 0
            sleep_ms(IPC_READY_SETTLE_MS);  // attente 10ms
            return true;
        }
        sleep_ms(POLL_INTERVAL_MS);
        waited_ms = waited_ms.saturating_add(POLL_INTERVAL_MS);
    }
    false
}
```

### Problème

`kill(pid, 0)` retourne 0 dès que le processus existe dans la table, **avant** que `_start()` ait terminé l'enregistrement de son endpoint IPC. La fenêtre de stabilisation de 10ms (`IPC_READY_SETTLE_MS`) est un hack fragile qui échoue sous charge ou sur matériel rapide.

### Cas de failure observés

1. init lance ipc_router → `kill(pid, 0)` OK → 10ms → ipc_router pas encore prêt → memory_server démarre → tente de se connecter à ipc_router → timeout → memory_server crashe → boucle
2. Sur QEMU avec `-m 256M` et CPU rapide, 10ms est insuffisant pour que le serveur ELF soit chargé et exécute `register_endpoint()`

---

## Correction recommandée

Remplacer le polling `kill(pid, 0)` par un mécanisme de handshake explicite :

### Option A — Pipe anonyme

```rust
// init ouvre un pipe avant fork
// le fils, dans _start(), ferme le côté write quand prêt
// init lit le côté read → EOF = prêt
```

### Option B — Semaphore partagé via SYS_FUTEX

```rust
// init alloue un futex dans la mémoire partagée parent/fils (avant fork)
// le fils fait futex_wake(1) après register_endpoint()
// init fait futex_wait() avec timeout
```

### Option C — Protocole IPC de heartbeat (le plus propre)

Le code de `handle_control_plane()` dans init_server implémente déjà `INIT_MSG_HEARTBEAT`. Les serveurs Ring1 pourraient envoyer un `INIT_MSG_HEARTBEAT` dès que prêts. init attend ce message au lieu de poll `kill()`.

---

## Note sur le timeout actuel

La constante `dependency::ready_timeout_ms()` retourne probablement une valeur fixe. Avec les bugs P0 bloquants (VMA tree, mark_vma_cow), tous les services crashent avant d'être ready → tous les timeouts expirent → init_server boucle en backoff exponentiel.

Ce bug P2 ne sera visible qu'après correction des P0.
