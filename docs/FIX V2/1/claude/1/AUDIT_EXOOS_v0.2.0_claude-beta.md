# AUDIT KERNEL ExoOS — Rapport d'incohérences v0.1.0 → v0.2.0

**Auteur :** claude-beta  
**Date :** 2026-05-16  
**Base analysée :** `kernel.zip` — ExoOS v0.1.0 (branche stabilisation v0.2.0)  
**Périmètre :** kernel/, servers/, exo-boot/ — source Rust no_std x86_64

---

## Résumé exécutif

L'audit couvre l'intégralité du code source livré. Dix incohérences ont été identifiées et classifiées selon leur sévérité. Deux sont **critiques** (crash garanti ou corruption silencieuse au démarrage), trois sont **hautes** (comportement indéfini ou fonctionnalité bloquée), deux sont **moyennes** (dégradation de performance ou fiabilité réduite), deux sont **informationnelles**.

| ID           | Sévérité    | Composant                             | Titre court                                  |
|--------------|-------------|---------------------------------------|----------------------------------------------|
| CRIT-01      | 🔴 CRITIQUE | `memory/virtual/page_table/builder.rs`| `map_physmap()` jamais appelée               |
| CRIT-02      | 🔴 CRITIQUE | `process/resource/cgroup.rs` / `lib.rs` | `cgroup::init()` omis en Phase 4          |
| HIGH-01      | 🟠 HAUTE    | `syscall/table.rs`                    | Injection PID via magic number `len == 128`  |
| HIGH-02      | 🟠 HAUTE    | `servers/init_server`                 | Service non-critique bloque `exosh`          |
| HIGH-03      | 🟠 HAUTE    | `fs/elf_loader_impl.rs`               | `USER_ELF_BASE_MIN` = 1 TiB incompatible     |
| MED-01       | 🟡 MOYENNE  | `memory/virtual/page_table/builder.rs`| Physmap mappée en pages 4 KiB               |
| MED-02       | 🟡 MOYENNE  | `fs/exofs/` + `syscall/fs_bridge.rs`  | Pas de writeback périodique automatique      |
| MED-03       | 🟡 MOYENNE  | `servers/network_server`              | `Device::receive()` starve Rx sous pression TX |
| INFO-01      | 🔵 INFO     | `fs/exofs/cache/blob_cache.rs`        | API `flush_all()` trompeuse                  |
| INFO-02      | 🔵 INFO     | `loader/`                             | `exo-loader` squelette sans log de boot      |

---

## CRIT-01 — `map_physmap()` jamais appelée : physmap limitée à 1 GiB

**Fichier :** `kernel/src/memory/virtual/page_table/builder.rs`  
**Fichier secondaire :** `kernel/src/arch/x86_64/boot/memory_map.rs`

### Description

La méthode `PageTableBuilder::map_physmap()` est définie à la ligne 76 de `builder.rs`. Elle est censée établir le mapping complet `PHYS_MAP_BASE → RAM physique 0..ram_size` après la détection mémoire multiboot2. Or, une recherche exhaustive dans l'ensemble du codebase confirme qu'**aucun site d'appel** n'existe en dehors de la définition elle-même.

```
grep -rn "\.map_physmap\b" kernel/src/          → 0 résultats
```

Au démarrage, la physmap repose **uniquement** sur le mapping boot du trampoline assembleur (`main.rs` / `early_init.rs`), qui couvre `PHYS_MAP_BASE + 0 → PHYS_MAP_BASE + 1 GiB` via des pages énormes 2 MiB dans `PML4[256]`. `PHYS_MAP_SIZE` est déclaré à **64 TiB** dans `layout.rs`.

### Impact

Tout appel à `phys_to_virt(phys)` pour une adresse physique `phys ≥ 1 GiB` produit une adresse virtuelle non-mappée. Le premier accès génère une faute de page (`#PF`) en Ring 0, soit une **panique noyau garantie** sur toute machine avec plus de 1 GiB de RAM.

Les sous-systèmes directement exposés :
- `memory/physical/allocator/buddy.rs` — parcourt toute la RAM libre via `phys_to_virt`
- `memory/cow/breaker.rs` — copie-de-page, commentaire interne cite explicitement la physmap
- `fs/exofs/` — accès aux blobs en cache via la physmap
- KPTI `kpti_split.rs` — `map_phys_page()` accède aux tables de pages via la physmap

### Code concerné

```rust
// builder.rs:76
pub fn map_physmap(&mut self, phys_size: u64) -> Result<&mut Self, AllocError> {
    let phys_map_base = crate::memory::core::layout::PHYS_MAP_BASE;
    let n_pages = (phys_size as usize + PAGE_SIZE - 1) / PAGE_SIZE;
    let flags = PageFlags::KERNEL_DATA;
    for i in 0..n_pages {
        let v = VirtAddr::new(phys_map_base.as_u64() + (i * PAGE_SIZE) as u64);
        let p = PhysAddr::new((i * PAGE_SIZE) as u64);
        self.walker.map(v, Frame::containing(p), flags, self.alloc)?;
    }
    Ok(self)
}
```

### Correction requise

Dans `kernel/src/arch/x86_64/boot/memory_map.rs`, après détection de `phys_end_pa`, appeler `map_physmap(phys_end_pa)` via un `PageTableBuilder` sur la PML4 active. Ce remapping doit intervenir **avant** toute utilisation du buddy allocator (Phase 2b) et avant `init_phase3_slab_slub()`.

```rust
// Après la boucle de détection mémoire, avant Phase 2b :
{
    let cr3_phys = read_cr3();
    let alloc = BootFrameAlloc::current();
    let mut builder = PageTableBuilder::from_cr3(cr3_phys, &alloc);
    builder
        .map_physmap(phys_end_pa)
        .expect("CRIT-01: physmap init failed");
    // Invalider le TLB complet
    flush_tlb_all();
}
```

---

## CRIT-02 — `cgroup::init()` omis dans `kernel_init()` Phase 4

**Fichier principal :** `kernel/src/lib.rs` (Phase 4)  
**Fichier secondaire :** `kernel/src/process/mod.rs` + `kernel/src/process/resource/cgroup.rs`

### Description

`process/mod.rs` expose une fonction `pub unsafe fn init()` qui orchestre cinq sous-initialisations, dont `resource::cgroup::init()` à la ligne 108. Le commentaire de `lib.rs` (Phase 4) liste fidèlement ces cinq étapes :

```
// process::init() orchestre :
//   1. pid::init()
//   2. registry::init()
//   3. lifecycle::reap::init_reaper()
//   4. state::wakeup::register_with_dma()
//   5. resource::cgroup::init()     ← listée
```

En pratique, le code appelle **directement** les sous-fonctions individuellement et **omet** l'étape 5 :

```rust
crate::process::core::pid::init(32768, 131072);
crate::process::core::registry::init(32768);
crate::process::lifecycle::reap::init_reaper();
crate::process::state::wakeup::register_with_dma();
// ← cgroup::init() ABSENT
```

### Impact

`cgroup::init()` positionne `CGROUP_TABLE.slots[0].valid = 1` et `CGROUP_TABLE.count = 1`. Sans cet appel, le cgroup racine a `valid = 0`. Le code de gestion des cgroups vérifie systématiquement `valid.load() == 1` avant d'opérer sur un slot. Toute opération cgroup (création de processus, limites CPU/mémoire, comptage de PIDs) échoue silencieusement ou retourne des données incorrectes pour **tous** les processus du système.

### Code concerné

```rust
// process/resource/cgroup.rs:235
pub fn init() {
    let root = &CGROUP_TABLE.slots[0];
    root.refcount.store(1, Ordering::Release);
    root.valid.store(1, Ordering::Release);   // ← jamais exécuté
    CGROUP_TABLE.count.store(1, Ordering::Release);
}
```

### Correction requise

Ajouter l'appel manquant dans `lib.rs` Phase 4, après `register_with_dma()` et avant `drop(process_irq_guard)` :

```rust
// lib.rs — Phase 4, dans le bloc IRQ guard
crate::process::core::pid::init(32768, 131072);
crate::process::core::registry::init(32768);
crate::process::lifecycle::reap::init_reaper();
crate::process::state::wakeup::register_with_dma();
crate::process::resource::cgroup::init();   // ← AJOUTER
drop(process_irq_guard);
```

Alternativement, remplacer les cinq appels manuels par `crate::process::init(&params)` pour garantir la cohérence avec `process/mod.rs`.

---

## HIGH-01 — Injection PID via magic number `len == 128` dans `sys_exo_ipc_send`

**Fichier :** `kernel/src/syscall/table.rs` (ligne ~2695)

### Description

Dans le handler `sys_exo_ipc_send`, une branche conditionnelle injecte silencieusement le PID de l'appelant dans les 4 premiers octets du payload lorsque la taille du message est **exactement** 128 octets :

```rust
if len == 128 {
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
}
```

128 correspond à `IPC_ENVELOPE_SIZE` dans `servers/syscall_abi`. Cette heuristique suppose que tout message de 128 octets est un `IpcEnvelope` dont les 4 premiers octets sont réservés au PID source.

### Impact

Tout message IPC légitime de 128 octets dont les 4 premiers octets ne sont **pas** un champ PID verra ses données **silencieusement écrasées** par le noyau. Il n'y a aucune vérification de type, de flag ou de format. Si un futur serveur envoie un message de 128 octets avec un format différent, cette corruption passe inaperçue à la compilation et au runtime.

De plus, si `IPC_ENVELOPE_SIZE` change dans `syscall_abi`, la constante 128 dans `table.rs` n'est **pas mise à jour automatiquement** : rupture silencieuse garantie.

### Correction requise

Supprimer le magic number. Le PID source doit être injecté soit :
1. Via un flag IPC explicite (`IPC_FLAG_INJECT_SRC_PID`) passé en argument `flags`
2. Via un en-tête de message structuré (`MsgHeader`) vérifié à l'encodage côté userspace

```rust
// Option 1 : flag explicite
const IPC_FLAG_INJECT_SRC_PID: u64 = 0x0002;

if flags & IPC_FLAG_INJECT_SRC_PID != 0 {
    if len < 4 { return EINVAL; }
    let caller_pid = syscall_current_pid();
    payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
}
```

---

## HIGH-02 — Un service non-critique qui timeout bloque définitivement `exosh`

**Fichier :** `servers/init_server/src/supervisor.rs` + `dependency.rs` + `boot_sequence.rs`

### Description

`boot_services()` démarre les services dans l'ordre de résolution de dépendances. Lorsqu'un service atteint son timeout, il est tué (`SIGTERM`) et marqué `dead` (PID = 0). La fonction `dependency_ready()` dans `supervisor.rs` est :

```rust
pub fn dependency_ready(services: &[Service], dep: &str) -> bool {
    dep == "init_server" || service_manager::service_started(services, dep)
}
```

`service_started()` retourne `true` **uniquement si le PID ≠ 0** (service vivant). Un service mort retourne `false`.

`can_start("exo_shield")` vérifie que tous ses `requires` sont `ready`. `exo_shield` dépend entre autres de `network_server` et `scheduler_server` (marqués `critical: false`). Si l'un d'eux échoue :

```
network_server → timeout → dead (pid=0)
can_start("exo_shield") → false (network_server not ready)
can_start("exosh")      → false (exo_shield not ready)
```

La boucle `while progress` ne progressera plus jamais. **L'utilisateur ne voit pas de shell.**

### Impact

Sur une machine sans interface réseau fonctionnelle, ou lors d'un timeout VirtIO au démarrage, le système démarre en silence sans shell accessible. Il n'y a aucun chemin de fallback ni timeout global de la chaîne de dépendances.

### Correction requise

La `dependency_ready()` doit distinguer « service optionnel mort » de « service obligatoire absent » :

```rust
pub fn dependency_ready(services: &[Service], dep: &str) -> bool {
    if dep == "init_server" { return true; }
    // Service démarré → dépendance satisfaite
    if service_manager::service_started(services, dep) { return true; }
    // Service non-critique mort → dépendance satisfaite (best-effort)
    if let Some(meta) = dependency::metadata(dep) {
        if !meta.critical {
            return service_manager::service_dead(services, dep);
        }
    }
    false
}
```

Compléter avec une variable `service_dead()` qui retourne `true` quand `mark_dead()` a été appelé.

---

## HIGH-03 — `USER_ELF_BASE_MIN` = 1 TiB rejette les binaires ELF standards

**Fichier :** `kernel/src/fs/elf_loader_impl.rs` (ligne 34)

### Description

```rust
const USER_ELF_BASE_MIN: u64 = 0x0000_0100_0000_0000; // 1 TiB = 0x100_0000_0000
```

Dans la fonction de chargement ELF, tout segment dont `p_vaddr < USER_ELF_BASE_MIN` est rejeté :

```rust
if start < USER_ELF_BASE_MIN {
    return Err(ElfLoadError::InvalidElf);
}
```

### Impact

La quasi-totalité des binaires ELF x86_64 existants a des segments `PT_LOAD` à des adresses inférieures à 1 TiB :

| Type de binaire | Adresse PT_LOAD typique |
|-----------------|-------------------------|
| Exécutable non-PIE | `0x0000_0000_0040_0000` (~4 MiB) |
| Bibliothèque partagée PIE | `0x0000_7F00_0000_0000` (~127 TiB) — dans l'espace user, mais variable |
| Binaire statique musl | `0x0000_0000_0040_0000` |

Tout binaire compilé sans l'option `-Ttext 0x100000000000` sera refusé. Cela inclut les coreutils, exosh statique, et l'ensemble de l'espace utilisateur prévu pour v0.2.0.

### Correction requise

Aligner `USER_ELF_BASE_MIN` sur la borne basse réelle de l'espace utilisateur, en excluant la page nulle et la zone réservée basse :

```rust
// Correspond à USER_START dans layout.rs (0x0000_0000_0001_0000)
// On ajoute une marge pour éviter la page nulle (NULL dereference)
const USER_ELF_BASE_MIN: u64 = 0x0000_0000_0001_0000; // 64 KiB — base utilisateur réelle
```

---

## MED-01 — `map_physmap()` utilise des pages 4 KiB au lieu de pages énormes 2 MiB

**Fichier :** `kernel/src/memory/virtual/page_table/builder.rs` (ligne 76)

### Description

L'implémentation actuelle de `map_physmap()` mappe la RAM physique page par page en 4 KiB :

```rust
for i in 0..n_pages {
    let v = VirtAddr::new(phys_map_base.as_u64() + (i * PAGE_SIZE) as u64);
    let p = PhysAddr::new((i * PAGE_SIZE) as u64);
    self.walker.map(v, Frame::containing(p), flags, self.alloc)?;
}
```

### Impact

Pour 16 GiB de RAM : 4 194 304 entrées de page table (4M × 8 octets = 32 MiB de tables de pages). Pour 64 TiB (capacité déclarée) : 16 milliards d'entrées — physiquement impossible.

De plus, chaque mapping 4 KiB consomme du budget au boot frame allocator. L'utilisation de pages 2 MiB (`PageFlags::KERNEL_DATA | HUGE`) réduirait le nombre d'entrées par un facteur 512 et éliminerait le niveau PD intermédiaire.

### Correction requise

```rust
pub fn map_physmap(&mut self, phys_size: u64) -> Result<&mut Self, AllocError> {
    const HUGE: u64 = 2 * 1024 * 1024; // 2 MiB
    let phys_map_base = PHYS_MAP_BASE;
    let flags = PageFlags::KERNEL_DATA | PageFlags::HUGE_PAGE;
    let n_huge = (phys_size + HUGE - 1) / HUGE;
    for i in 0..n_huge {
        let v = VirtAddr::new(phys_map_base.as_u64() + i * HUGE);
        let p = PhysAddr::new(i * HUGE);
        self.walker.map_2m(v, p, flags, self.alloc)?;
    }
    Ok(self)
}
```

---

## MED-02 — Aucun writeback périodique automatique des blobs dirty

**Fichier :** `kernel/src/fs/exofs/mod.rs` + `kernel/src/syscall/fs_bridge.rs`

### Description

Le kthread GC `exofs_gc_kthread` effectue uniquement le ramasse-miettes des blobs anciens. Il n'appelle jamais `BLOB_CACHE.collect_dirty()` ni `persist_blob_data_if_disk()`. La fonction `fs_sync()` (qui appelle `collect_dirty` → `persist_blob_data_if_disk`) n'est déclenchée que par les syscalls `sync(2)`, `fsync(2)` et `sync_file_range(2)` émis explicitement par l'espace utilisateur.

```rust
fn exofs_gc_kthread(_arg: usize) -> ! {
    gc_backoff();
    loop {
        let current_epoch = current_epoch();
        if current_epoch > 2 {
            let _ = run_gc_two_phase(epoch_threshold);
        }
        gc_backoff(); // ← yield × 128, aucun writeback
    }
}
```

### Impact

Un processus utilisateur qui écrit via `write(2)` puis se termine sans appeler `sync(2)` perd ses données. Sur coupure d'alimentation ou panique noyau, **toutes les données écrites depuis le dernier sync explicite sont perdues**. Ce comportement est particulièrement critique pour `exosh` et les serveurs de la Phase v0.2.0.

### Correction requise

Ajouter un kthread de writeback périodique distinct du GC, ou intégrer un flush conditionnel dans le GC :

```rust
fn exofs_gc_kthread(_arg: usize) -> ! {
    gc_backoff();
    let mut ticks_since_flush: u64 = 0;
    const FLUSH_INTERVAL_TICKS: u64 = 5000; // ~5 s à HZ=1000

    loop {
        let current_epoch = current_epoch();
        if current_epoch > 2 {
            let _ = run_gc_two_phase(current_epoch - 2);
        }

        ticks_since_flush += 128; // approximation du gc_backoff
        if ticks_since_flush >= FLUSH_INTERVAL_TICKS {
            let _ = crate::syscall::fs_bridge::fs_sync(0 /* kernel */);
            ticks_since_flush = 0;
        }
        gc_backoff();
    }
}
```

---

## MED-03 — `Device::receive()` alloue RxToken + TxToken simultanément : famine Rx sous pression TX

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

### Description

L'implémentation du trait `smoltcp::Device` pour `ExoSmoltcpDevice` alloue systématiquement un slot TX **en même temps** qu'un slot RX dans `receive()` :

```rust
fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
    if !self.pool.ready() { return None; }
    let rx = self.device_mut().pop_rx_for_stack()?;
    let tx_idx = match self.alloc_tx() {
        Some(idx) => idx,
        None => {
            self.device_mut().release_rx(rx.pool_idx);  // ← drop du paquet reçu
            self.device_mut().dropped_tx = ...saturating_add(1);
            return None;
        }
    };
    Some((ExoRxToken { ... }, ExoTxToken { ... }))
}
```

### Impact

Si tous les slots TX sont occupés (rafale d'émission, RTT élevé, pression mémoire), `alloc_tx()` retourne `None`. La trame reçue est **libérée sans traitement** (`release_rx`). Sous pression TX soutenue, **aucun paquet entrant n'est traité** : le stack TCP/IP ne peut pas répondre aux ACKs entrants, ce qui aggrave la congestion TX. C'est une boucle de rétroaction négative.

Le compteur `dropped_tx` est mal nommé : il comptabilise en réalité des paquets **Rx droppés à cause d'une pression TX**.

### Correction requise

Séparer l'allocation TX de la réception. smoltcp permet de retourner `None` pour le `TxToken` sans libérer le `RxToken`. Alternativement, utiliser une file Rx distincte non bloquée par les slots TX :

```rust
fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
    let rx = self.device_mut().pop_rx_for_stack()?;
    // TX optionnel : si pas de slot TX, on traite quand même le paquet Rx
    let tx_idx = self.alloc_tx(); // Option<usize>
    Some((
        ExoRxToken { device: self.device, pool: self.pool, rx },
        ExoTxToken { device: self.device, pool: self.pool, idx: tx_idx },
    ))
}
```

Renommer `dropped_tx` en `dropped_rx_no_tx_slot` pour la précision des métriques.

---

## INFO-01 — API `flush_all()` trompeuse : retourne une erreur au lieu de flusher

**Fichier :** `kernel/src/fs/exofs/cache/blob_cache.rs` (ligne 679)

### Description

```rust
pub fn flush_all(&self) -> ExofsResult<()> {
    let mut inner = self.inner.lock();
    let dirty_count = inner.map.values().filter(|e| e.dirty).count();
    if dirty_count > 0 {
        return Err(ExofsError::DirtyDataLoss(dirty_count)); // ← erreur, pas de flush
    }
    inner.map.clear();
    inner.used = 0;
    Ok(())
}
```

Le nom `flush_all` suggère une écriture forcée. En réalité, la fonction **refuse de s'exécuter** s'il existe des données non persistées. Le seul chemin de flush effectif est `collect_dirty()` + `persist_blob_data_if_disk()` + `mark_clean()`, encapsulé dans `fs_sync()`.

### Impact

Tout code appelant `flush_all()` lors d'un shutdown propre et espérant persister les données sera surpris par un `DirtyDataLoss`. L'erreur n'est pas auto-documentée : il faut lire l'implémentation pour comprendre qu'il faut appeler `fs_sync()` en amont.

### Correction requise

Renommer la fonction en `evict_clean_only()` ou `clear_if_clean()`, et documenter explicitement qu'elle ne fait pas de writeback. Créer une `flush_all_with_persist()` qui appelle `fs_sync()` avant le clear :

```rust
/// Persiste toutes les entrées dirty puis vide le cache.
pub fn flush_all_with_persist(&self) -> ExofsResult<()> {
    // Appel au fs_bridge pour persist + mark_clean
    crate::syscall::fs_bridge::fs_sync(0)?; // ← ou appel direct à persist
    self.flush_all() // maintenant dirty_count == 0, réussit
}
```

---

## INFO-02 — `exo-loader` retourne `ENOSYS` sans log de boot visible

**Fichier :** `loader/src/` + `loader/README.md`

### Description

`exo-loader` est conservé comme squelette pour le futur linkeur dynamique. À l'exécution, il retourne immédiatement `ENOSYS`. Cette information est documentée dans `loader/README.md`, mais **aucun log visible** (port 0xE9 ou serial) n'est émis si quelqu'un lance accidentellement le binaire comme chargeur réel lors d'un boot UEFI ou d'un test intégration.

### Impact

Faible — diagnostic difficile uniquement dans le cas d'un mauvais câblage de boot qui pointerait vers `exo-loader` au lieu du kernel. Pas de crash, mais silence total : l'opérateur ne sait pas pourquoi le système ne démarre pas.

### Correction requise

Ajouter une émission série/port 0xE9 avant le `ENOSYS` :

```rust
// loader/src/main.rs
fn main() -> ! {
    // Émettre un marqueur diagnostique sur le port debug
    unsafe { core::arch::asm!("out 0xE9, al", in("al") b'L', options(nomem, nostack)); }
    unsafe { core::arch::asm!("out 0xE9, al", in("al") b'D', options(nomem, nostack)); }
    // ENOSYS — dynamic_linking non activé
    loop { unsafe { core::arch::asm!("hlt"); } }
}
```

---

## Récapitulatif des corrections prioritaires pour v0.2.0

| Priorité | ID      | Action                                                     | Effort |
|----------|---------|------------------------------------------------------------|--------|
| 1        | CRIT-01 | Appeler `map_physmap()` après détection RAM multiboot2     | Moyen  |
| 2        | CRIT-02 | Ajouter `cgroup::init()` en Phase 4 de `kernel_init()`    | Faible |
| 3        | HIGH-03 | Corriger `USER_ELF_BASE_MIN` → `0x10000`                  | Faible |
| 4        | HIGH-02 | Rendre `dependency_ready()` tolérant aux services morts   | Moyen  |
| 5        | HIGH-01 | Supprimer le magic number `len == 128`, passer par un flag | Moyen  |
| 6        | MED-01  | Convertir `map_physmap()` en huge pages 2 MiB              | Moyen  |
| 7        | MED-02  | Ajouter writeback périodique dans le kthread GC            | Moyen  |
| 8        | MED-03  | Découpler allocation TX et réception Rx dans smoltcp      | Élevé  |
| 9        | INFO-01 | Renommer `flush_all()` → `evict_clean_only()`             | Faible |
| 10       | INFO-02 | Ajouter log port 0xE9 dans `exo-loader`                   | Faible |

---

*Document généré par claude-beta — audit statique exhaustif du code source ExoOS v0.1.0.*  
*Aucune exécution dynamique n'a été réalisée. Les numéros de ligne sont indicatifs et peuvent varier selon les commits ultérieurs.*
