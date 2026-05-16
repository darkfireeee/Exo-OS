# Audit des incohérences — Kernel ExoOS v0.2.0
**Rédigé par : claude iota**
**Date : 15 mai 2026**
**Scope : stabilisation complète v0.2.0 avant passage à Wayland + installation + manipulation visuelle**

---

## Résumé exécutif

L'analyse du codebase `kernel.zip` révèle **3 incohérences critiques**, **5 incohérences majeures** et **5 incohérences mineures**. Aucune ne bloque le boot GRUB/BIOS ni le chemin principal ExoFS. En revanche, le chemin UEFI, le contrat IPC entre kernel et Ring1, et la logique de criticité des services représentent des risques réels pour la stabilisation v0.2.0.

---

## CRITIQUES

### IC-01 — Trampoline `_start` incompatible avec le boot UEFI

**Localisation :** `kernel/src/main.rs` (`.code32` `_start`) × `exo-boot/src/kernel_loader/handoff.rs` (`handoff_to_kernel`)

**Symptôme :**
Le commentaire de `main.rs` affirme que les deux chemins de boot (GRUB et UEFI) empruntent "le même trampoline". C'est faux.

- Chemin GRUB : GRUB laisse le CPU en **mode protégé 32 bits** → `_start` (`.code32`) s'exécute correctement.
- Chemin UEFI : `handoff_to_kernel()` saute à `_start` depuis **long mode 64 bits** (`CS = 0x08`, `L=1`). Le CPU décode alors l'assembleur `.code32` comme du 64 bits.

**Conséquence directe :** l'instruction `retf` à la fin de `_start` effectue en mode 64 bits un dépilage de 8 + 8 = 16 octets, alors que `_start` ne pousse que deux valeurs 32 bits (4 + 4 = 8 octets). Le retour lointain atterrit à une adresse incorrecte → triple fault immédiat.

```
// exo-boot/src/kernel_loader/handoff.rs — préconditions déclarées
// GDT 64-bit chargée (CS = 0x08, DS/SS = 0x10)  ← long mode actif
// ...
"jmp {entry}",  // saute à _start… qui est du code32
```

**Correction requise :** avant le `jmp`, `handoff_to_kernel` doit switcher CS vers un descripteur de compatibilité 32 bits (L=0) via un far jump ou un far call factice, ou bien le kernel doit exposer un `_start_uefi` en `.code64` qui s'applique à `_start64` directement.

---

### IC-02 — Mismatch de taille d'enveloppe IPC entre kernel et ABI Ring1

**Localisation :** `kernel/src/ipc/core/constants.rs` × `servers/syscall_abi/src/lib.rs`

**Divergence :**

| Constante | Kernel (ring interne) | ABI Ring1 |
|---|---|---|
| Header message | `MSG_HEADER_SIZE = 16` | `IPC_HEADER_SIZE = 8` |
| Payload inline | `MAX_MSG_SIZE = 240` | `IPC_INLINE_PAYLOAD_SIZE = 120` |
| Slot total | `RING_SLOT_SIZE = 256` | `IPC_ENVELOPE_SIZE = 128` |

Le kernel interne travaille avec des slots de **256 octets** ; les serveurs Ring1 (`virtio_drivers`, `scheduler_server`, `network_server`…) déclarent des structs de **128 octets** et passent `buf_len = 128` au syscall `SYS_IPC_RECV`.

Le code `recv_ipc_message` calcule `recv_cap = buf_len.min(MAX_MSG_SIZE) = 128`. En cas de sender qui envoie > 128 octets (possible puisque `MAX_MSG_SIZE = 240`), le message est **silencieusement tronqué à 128 octets** — aucun `EMSGSIZE` n'est retourné au receiver. Ce comportement masque les débordements de protocole.

**Risque v0.2.0 :** tout futur serveur exploitant la capacité réelle de 240 octets enverra des messages que les serveurs actuels liront partiellement sans en être informés.

---

### IC-03 — `exo_shield` (critique=true) bloqué par des dépendances non critiques

**Localisation :** `servers/init_server/src/service_table.rs`

**Problème :** `exo_shield` est marqué `critical: true` mais ses dépendances directes incluent trois services marqués `critical: false` :

```
DEPS_EXO_SHIELD = [
    "ipc_router",       // critical: true   ✓
    "memory_server",    // critical: true   ✓
    "vfs_server",       // critical: true   ✓
    "crypto_server",    // critical: true   ✓
    "device_server",    // critical: true   ✓
    "virtio_drivers",   // critical: false  ✗
    "network_server",   // critical: false  ✗
    "scheduler_server", // critical: false  ✗
    "input_server",     // critical: true   ✓
    "tty_server",       // critical: true   ✓
]
```

`exosh` dépend à son tour de `exo_shield`. Si `virtio_drivers`, `network_server` ou `scheduler_server` crashent définitivement au démarrage, `exo_shield` ne s'initialise jamais et `exosh` non plus. Le système reste **silencieusement sans shell** alors qu'aucune panique n'est remontée — les services non critiques peuvent échouer sans déclencher d'alerte.

**Correction requise :** soit monter `virtio_drivers`, `network_server`, `scheduler_server` en `critical: true`, soit implémenter une logique de dégradation dans `exo_shield` (démarrer en mode dégradé sans réseau/scheduling avancé).

---

## MAJEURES

### IM-01 — 54 fichiers de drivers entièrement vides (stubs 0 octet)

**Localisation :** `drivers/` (sous-crates suivants)

Les crates suivants sont déclarés dans `drivers/Cargo.toml` mais tous leurs fichiers source ont une taille de **0 octet** :

| Crate | Fichiers vides | Impact |
|---|---|---|
| `audio/hda` | `codec.rs`, `controller.rs`, `main.rs`, `stream.rs` | Aucun audio HDA physique |
| `audio/virtio_sound` | `main.rs` | Aucun audio VirtIO |
| `display/virtio_gpu` | `gpu.rs`, `main.rs`, `virtqueue.rs` | Aucun GPU VirtIO (display en Ring1) |
| `network/e1000` | tous (5 fichiers) | Aucune carte réseau physique e1000 |
| `input/evdev` | `events.rs`, `main.rs` | Aucun input evdev |
| `input/usb_hid` | tous (4 fichiers) | Aucun clavier/souris USB |
| `clock/` | tous (5 fichiers) | Driver horloge userspace absent |
| `storage/ahci` | tous (6 fichiers) | Aucun stockage SATA/AHCI |
| `storage/nvme` | tous (6 fichiers) | Aucun stockage NVMe |
| `drivers/framework` | tous (10 fichiers) | Framework driver Ring1 absent |
| `drivers/manager` | tous (6 fichiers) | Manager de drivers absent |

**Implication v0.2.0 :** ExoOS ne peut actuellement booter que sur QEMU avec VirtioBlk (kernel-side) et VirtIO-net. Toute machine physique sans QEMU est hors de portée. Cela doit être documenté explicitement dans les release notes v0.2.0 pour éviter toute confusion lors des tests d'installation (étape post-v0.2.0).

---

### IM-02 — `CryptoRequest` : convention de header non standard

**Localisation :** `servers/crypto_server/src/main.rs` × `kernel/src/syscall/table.rs`

La convention générale IPC pour les structs de requête est :

```rust
// Convention standard (tous les autres serveurs)
struct XxxRequest {
    sender_pid: u32,    // [0..4]  — overwritten by kernel
    msg_type:   u32,    // [4..8]
    payload:    [u8; IPC_INLINE_PAYLOAD_SIZE],  // [8..128]
}
```

`CryptoRequest` utilise une convention différente :

```rust
struct CryptoRequest {
    sender_endpoint: u64,  // [0..8]  — seuls [0..4] overwritten par kernel
    msg_type:        u32,  // [8..12]
    payload_len:     u16,
    version:         u8,
    flags:           u8,
    cap_token:       ExoCapTokenWire,
    payload:         [u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
}
```

Conséquence : le kernel n'écrase que `[0..4]` avec `caller_pid` (u32) uniquement si `len == 128`. Les octets `[4..7]` de `sender_endpoint` restent à la valeur envoyée par le client. La vérification `is_reserved_kernel_ipc` lit `msg_type` à `[8..12]`, ce qui est cohérent avec ce struct — mais incompatible avec la convention standard où `msg_type` est à `[4..8]`.

**Risque :** tout futur serveur qui lirait un message forwarded par crypto_server en supposant la convention standard lirait le mauvais champ `msg_type`.

---

### IM-03 — Nommage `PhoenixState` incohérent entre kernel et network_server

**Localisation :** `kernel/src/exophoenix/mod.rs` × `servers/network_server/src/isolation.rs`

Le kernel définit :
```rust
pub enum PhoenixState {
    NetworkDraining   = 9,
    NetworkSerialized = 10,
}
```

Le `network_server` définit un enum local non partagé :
```rust
enum PhoenixPhase {
    Normal,
    Draining,    // ≠ NetworkDraining
    Serialized,  // ≠ NetworkSerialized
}
```

La correspondance est faite manuellement via des constantes syscall :
```rust
PhoenixPhase::Draining   => syscall::EXO_PHOENIX_STATE_NETWORK_DRAINING,
PhoenixPhase::Serialized => syscall::EXO_PHOENIX_STATE_NETWORK_SERIALIZED,
```

Le fichier `EXONET_V4_AUDIT.md` décrit la transition comme `Normal -> Draining -> Serialized -> Normal`, ce qui correspond au `PhoenixPhase` local — mais pas directement aux noms du kernel. Tout nouveau composant qui intégrerait ExoPhoenix doit connaître les deux nomenclatures. Un type partagé dans `syscall_abi` éviterait cette dérive.

---

### IM-04 — Chemin I/O disque inaccessible depuis Ring3

**Localisation :** `drivers/storage/virtio_blk/` × `servers/virtio_drivers/src/main.rs`

`drivers/storage/virtio_blk/` est le driver kernel-side opérationnel pour les accès blocs. Le serveur Ring1 `servers/virtio_drivers/` ne répond qu'aux messages `VIRTIO_MSG_HEARTBEAT` et `VIRTIO_MSG_STATUS` — les opérations de lecture/écriture ne sont pas routées via IPC.

```rust
// servers/virtio_drivers/src/main.rs
match request.msg_type {
    VIRTIO_MSG_HEARTBEAT | VIRTIO_MSG_STATUS => VirtioReply::ok(),
    _ => VirtioReply::error(syscall::EINVAL),  // ← tout le reste rejeté
}
```

Le README confirme explicitement : _"New I/O paths must not assume this service owns virtqueue descriptors until a future refactor."_

**Conséquence v0.2.0 :** les applications Ring3 ne peuvent pas faire d'I/O disque directement. Elles passent obligatoirement par ExoFS (qui utilise le driver kernel interne). Ce chemin indirect est fonctionnel mais rend impossible l'accès brut aux blocs depuis l'espace utilisateur. À documenter comme limitation connue v0.2.0.

---

### IM-05 — Dual calling convention de `SYS_IPC_RECV` non documentée

**Localisation :** `kernel/src/syscall/table.rs` — `normalize_ipc_recv_args()`

Le syscall `SYS_EXO_IPC_RECV` accepte **deux conventions d'appel** différentes, distinguées par une heuristique silencieuse :

```rust
fn normalize_ipc_recv_args(a1: u64, a2: u64, a3: u64, flags: u64) -> (u64, u64, u64, u64) {
    if flags == 0 && a2 <= 65_536 {
        // Mode "shorthand" : a1=buf_ptr, a2=buf_len, a3=flags, endpoint inféré
        let endpoint = primary_ipc_endpoint_for_owner(caller_pid)...;
        (endpoint, a1, a2, a3)
    } else {
        // Mode "explicite" : a1=endpoint, a2=buf_ptr, a3=buf_len, flags=flags
        (a1, a2, a3, flags)
    }
}
```

Le mode "shorthand" est activé lorsque `flags == 0` et `buf_ptr <= 65536`. Cette heuristique peut produire de faux positifs si un buffer userspace légitime est mappé à une adresse < 65536 (inhabituel mais possible). La `syscall_abi` ne documente pas ce mode alternatif.

---

## MINEURES

### Im-01 — `SYS_SIGACTION` alias trompeur

**Localisation :** `servers/syscall_abi/src/lib.rs`

```rust
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_SIGACTION: u64 = SYS_RT_SIGACTION;  // alias
```

Sur Linux x86_64, `SYS_SIGACTION` (l'ancienne interface) n'existe pas au numéro 13 — seul `SYS_RT_SIGACTION = 13` est valide. L'alias `SYS_SIGACTION = 13` laisse croire à une compatibilité avec l'ancienne ABI POSIX signal qui n'est pas implémentée.

---

### Im-02 — Module `arch/aarch64` : `compile_error!` + code fonctionnel coexistent

**Localisation :** `kernel/src/arch/aarch64/mod.rs`

Le module déclare en tête :
```rust
#[cfg(target_arch = "aarch64")]
compile_error!("ExoOS v0.2.0 ne supporte pas encore le boot AArch64 ...");
```

Puis expose 50+ lignes de primitives ARM64 fonctionnelles (`read_tsc`, `halt_cpu`, `irq_disable`, `irq_enable`…). La combinaison d'un `compile_error!` d'interdiction et d'un code fonctionnel est paradoxale : le code existe mais ne peut jamais être compilé. Soit retirer le code (placeholder minimal), soit retirer le `compile_error!` et documenter les limitations.

---

### Im-03 — `loader/` : spin infini si `dynamic_linking` activé par erreur

**Localisation :** `loader/src/main.rs`

```rust
#[cfg(all(target_os = "none", feature = "dynamic_linking"))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop { core::hint::spin_loop(); }  // ← spin infini, aucun log, aucun exit
}
```

Si la feature `dynamic_linking` est activée accidentellement, le binaire bare-metal entre dans une boucle infinie silencieuse. La variante sans `dynamic_linking` appelle correctement `SYS_EXIT(ENOSYS)`. La branche `dynamic_linking` devrait aussi appeler `SYS_EXIT` ou émettre un message de diagnostic avant de boucler.

---

### Im-04 — Tests ExoFS sans preuve d'exécution CI

**Localisation :** `kernel/src/fs/exofs/tests/TESTS_STATUS_REPORT.md`

Le rapport lui-même indique explicitement :

> _"Ce fichier ne remplace pas une execution CI. La validation v0.2.0 doit rester bloquee sur l'execution effective, via WSL, des tests ExoFS critiques."_

Les tiers `tier_4_pipeline` (backend VFS réel) et `tier_6_virtio_vfs` (chemin VirtIO/VFS) n'ont pas de logs d'exécution attachés. La stabilisation complète v0.2.0 est **bloquée** tant que ces runs CI ne sont pas enregistrés.

**Commandes à exécuter et logger :**
```bash
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
  fs::exofs::tests::integration::tier_4_pipeline

cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
  fs::exofs::tests::integration::tier_6_virtio_vfs
```

---

### Im-05 — Layer doc incohérente : security initialisée après process, pas après memory

**Localisation :** `kernel/src/lib.rs` (commentaire d'architecture + `kernel_init`)

Le commentaire de couches déclare :
```
Couche 2b : security/ — dépend de memory/
```
Ce qui impliquerait que `security` peut être initialisé juste après `memory`. Mais `kernel_init()` initialise security à la **Phase 5**, après `process` (Phase 4), car `security_init()` est protégé par `is_security_ready()` pour éviter une double initialisation (une init partielle peut avoir lieu en early boot).

Cette dépendance implicite sur `process/` (pour les hooks OOM, le registre PID…) n'est pas documentée dans le schéma de couches. Un futur refactoring pourrait casser l'ordre en se fiant uniquement au diagramme.

---

## Tableau récapitulatif

| ID | Sévérité | Composant | Description courte | Bloquant v0.2.0 ? |
|---|---|---|---|---|
| IC-01 | CRITIQUE | boot/uefi | `_start` `.code32` exécuté en long mode 64 bits | Oui (boot UEFI cassé) |
| IC-02 | CRITIQUE | ipc/kernel×abi | Enveloppe IPC 256 octets kernel vs 128 octets ABI | Oui (troncature silencieuse) |
| IC-03 | CRITIQUE | init_server | `exo_shield` critique bloqué par deps non critiques | Oui (système sans shell possible) |
| IM-01 | MAJEURE | drivers/ | 54 fichiers drivers vides (audio/gpu/e1000/ahci/nvme…) | Non (QEMU only, à documenter) |
| IM-02 | MAJEURE | crypto_server | Header `CryptoRequest` non standard (u64 vs u32) | Non (risque évolutif) |
| IM-03 | MAJEURE | exophoenix×network | Nommage PhoenixState divergent kernel/network_server | Non (risque évolutif) |
| IM-04 | MAJEURE | virtio_blk | Aucun chemin I/O blocs accessible depuis Ring3 | Non (à documenter) |
| IM-05 | MAJEURE | syscall/ipc | Double convention d'appel `SYS_IPC_RECV` non documentée | Non (risque évolutif) |
| Im-01 | MINEURE | syscall_abi | `SYS_SIGACTION` alias trompeur | Non |
| Im-02 | MINEURE | arch/aarch64 | `compile_error!` + code fonctionnel coexistent | Non |
| Im-03 | MINEURE | loader | Spin infini si `dynamic_linking` activé | Non |
| Im-04 | MINEURE | exofs/tests | Tiers CI 4 et 6 non exécutés | Oui (validation stabilisation) |
| Im-05 | MINEURE | lib.rs | Layer doc security: dépendance implicite sur process non documentée | Non |

---

## Priorités recommandées pour la clôture v0.2.0

1. **IC-01** — Corriger le handoff UEFI (far-jump vers descripteur compat 32 bits ou entrée `_start_uefi` en `.code64`) avant tout test sur machine physique.
2. **IC-03** — Aligner la criticité des deps de `exo_shield` ou implémenter un mode dégradé.
3. **Im-04** — Exécuter et logger les runs CI tier_4 et tier_6 ExoFS, attacher les logs au commit de release.
4. **IC-02** — Documenter formellement la dualité MAX_MSG_SIZE (240) vs IPC_ENVELOPE_SIZE (128) dans `syscall_abi`, et ajouter un `EMSGSIZE` en cas de troncature.
5. **IM-01** — Ajouter une section "Hardware Support Matrix" aux release notes v0.2.0 indiquant que seul QEMU (VirtioBlk + VirtIO-net) est validé.

---

*claude iota — audit statique, aucune exécution de code kernel réelle effectuée*
