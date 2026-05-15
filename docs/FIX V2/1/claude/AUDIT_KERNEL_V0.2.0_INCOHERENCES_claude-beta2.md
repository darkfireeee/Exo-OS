# ExoOS — Rapport d'audit des incohérences du kernel
## Cible : stabilisation v0.2.0

**Auteur :** claude-beta  
**Date :** 2026-05-14  
**Version analysée :** ExoOS v0.1.0 (commit Fix 7.1, 162 fichiers)  
**Objectif :** Identifier toutes les incohérences bloquant la stabilisation complète avant le portage Wayland et l'installation visuelle  
**Périmètre :** kernel/, servers/, drivers/ — architecture x86_64 bare-metal

---

## Résumé exécutif

L'analyse statique du code source révèle **10 incohérences** réparties sur trois niveaux de sévérité. Trois d'entre elles sont de niveau P0 (blocantes pour la sécurité ou la stabilité système) et doivent être corrigées avant toute promotion en v0.2.0. Les sept autres constituent des dettes techniques dont le traitement est requis pour atteindre la stabilisation complète promise par la v0.2.0.

| Niveau | Nombre | Impact |
|--------|--------|--------|
| **P0 — Critique** | 3 | Sécurité noyau compromise ou crash garanti |
| **P1 — Grave** | 4 | Comportement incorrect, régression latente |
| **P2 — Mineur** | 3 | Débogage difficile, incomplétude fonctionnelle |

---

## P0 — Incohérences critiques

### P0-01 · KPTI inactif sur le chemin d'entrée syscall

**Fichiers :** `kernel/src/arch/x86_64/syscall.rs`, `kernel/src/arch/x86_64/spectre/kpti.rs`

**Description :**  
Le module `spectre/kpti.rs` expose deux fonctions de commutation CR3 — `kpti_switch_to_kernel()` et `kpti_switch_to_user()` — mais celles-ci ne sont **jamais appelées** depuis `syscall_entry_asm`. L'assembleur d'entrée syscall (`syscall_entry_asm`) effectue SWAPGS puis empile les registres, mais ne commute pas CR3. Le commentaire de `kpti.rs` indique que « Le switch CR3 se fait dans le code ASM de bas niveau (switch_asm.s) », ce qui est exact pour les context switches scheduler, mais **pas** pour le chemin syscall.

**Conséquence :**  
Lors d'un appel système, le CPU entre en Ring 0 avec le CR3 *user* encore actif. Le kernel s'exécute donc sans isolation des tables de pages vis-à-vis de l'espace utilisateur. L'objectif premier de KPTI — contrer Meltdown en séparant les tables de pages kernel et user — est neutralisé sur ce chemin, qui est le plus fréquemment emprunté.

**Correction requise :**  
Ajouter dans `syscall_entry_asm`, immédiatement après `swapgs`, un switch vers `cr3_kernel` (lu depuis la zone per-CPU via `gs:[offset_cr3_kernel]`). Ajouter le switch inverse vers `cr3_user` juste avant le second `swapgs` de sortie. Cette séquence doit être atomique vis-à-vis des NMI (placer le switch CR3 avant tout accès à la pile kernel).

```asm
; Après SWAPGS (CR3 kernel non encore actif — fenêtre critique minimale)
mov   rax, qword ptr gs:[CR3_KERNEL_OFFSET]
mov   cr3, rax
; ... corps du handler ...
; Avant SWAPGS de sortie
mov   rax, qword ptr gs:[CR3_USER_OFFSET]
mov   cr3, rax
swapgs
sysretq
```

---

### P0-02 · Collision de vecteur IDT : IPI reschedule = signal freeze ExoPhoenix

**Fichiers :** `kernel/src/arch/x86_64/idt.rs`

**Description :**  
Les constantes suivantes sont définies dans `idt.rs` :

```rust
pub const VEC_EXOPHOENIX_FREEZE: u8 = 0xF1;
pub const VEC_IPI_RESCHEDULE:    u8 = VEC_EXOPHOENIX_FREEZE; // = 0xF1
```

Le vecteur `0xF1` est ainsi à la fois le signal de freeze ExoPhoenix (utilisé par le Kernel B pour geler le Kernel A en cas d'anomalie) et l'IPI de reschedule du scheduler (émis à haute fréquence par le tick timer vers les CPUs cibles). Ces deux rôles sont **incompatibles** et partagent le même handler IDT.

**Conséquence :**  
Tout IPI de reschedule envoyé au cours du fonctionnement normal sera interprété comme un événement de freeze ExoPhoenix. Inversement, un freeze ExoPhoenix légitime ne pourra pas être distingué d'un reschedule banal. La double utilisation rend le protocole ExoPhoenix inopérant et le scheduler instable sous charge SMP.

**Correction requise :**  
Allouer deux vecteurs distincts. ExoPhoenix dispose de la plage réservée `0xF0–0xFF`. Utiliser par exemple :

```rust
pub const VEC_EXOPHOENIX_FREEZE: u8 = 0xF0; // ExoPhoenix exclusif
pub const VEC_IPI_RESCHEDULE:    u8 = 0xE0; // scheduler IPI (hors plage ExoPhoenix)
```

Mettre à jour les entrées IDT correspondantes et les handlers associés.

---

### P0-03 · User shadow PML4 sans mapping des stubs de transition kernel

**Fichiers :** `kernel/src/memory/virtual/page_table/kpti_split.rs`

**Description :**  
La fonction `build_user_shadow_pml4()` crée la table de pages user (CR3 user) en copiant uniquement les entrées PML4[0..255], correspondant à l'espace d'adressage utilisateur :

```rust
for i in 0..256 {
    user_pml4[i] = kernel_pml4[i];
}
```

Aucune entrée kernel haute (PML4[256..511]) n'est copiée. Le commentaire du module indique explicitement que « les stubs de transition doivent être explicitement mappés dans une page trampoline dédiée », mais ce mapping **n'est pas implémenté** dans `build_user_shadow_pml4()`.

**Conséquence :**  
Lorsque KPTI est actif et que le CR3 user est chargé, toute exception ou NMI survenant avant le switch vers CR3 kernel (y compris une #PF dans la fenêtre de transition) déclenchera un #PF en Ring 0 sur le handler d'exception lui-même — car les handlers IDT sont en espace kernel, non mappé dans la PML4 user. Cela produit un double fault (#DF), puis un triple fault et un reset CPU. La condition est reproductible sous NMI watchdog actif ou lors d'un tick LAPIC pendant la transition.

**Correction requise :**  
Mapper explicitement dans la PML4 user les pages strictement nécessaires à la transition Ring3→Ring0 : stubs d'entrée syscall, stubs d'exception, et page trampoline SMP (`TRAMPOLINE_PHYS = 0x6000`). Ces pages doivent être en exécution Ring 0 uniquement (non accessibles en Ring 3) dans la PML4 user.

---

## P1 — Incohérences graves

### P1-01 · Champ `preempt_count` fantôme dans PerCpuData

**Fichiers :** `kernel/src/arch/x86_64/smp/percpu.rs`, `kernel/src/scheduler/core/preempt.rs`

**Description :**  
La structure `PerCpuData` contient le champ :

```rust
pub preempt_count: u64, // 0x30 réservé (ne pas utiliser comme compteur canonique)
```

Ce champ occupe l'offset `GS:[0x30]` et est initialisé à zéro. Le compteur de préemption canonique réside dans `scheduler::core::preempt::PREEMPT_COUNT`, un tableau statique per-CPU distinct. Les fonctions `preempt_disable()` et `preempt_enable()` de `percpu.rs` sont marquées `#[deprecated]` et délèguent au compteur du scheduler — mais le champ `preempt_count` dans `PerCpuData` n'est jamais mis à jour.

**Conséquence :**  
Tout code accédant à `GS:[0x30]` pour lire un état de préemption (pattern courant dans du code bas niveau écrit à la main) obtiendra toujours `0`, indiquant « préemptible » même si la préemption est désactivée. Un futur développement assembleur ou une intégration de driver pourrait introduire silencieusement cette lecture erronée. De plus, la coexistence des deux structures entretient une confusion architecturale.

**Correction requise :**  
Supprimer le champ `preempt_count` de `PerCpuData`, ou le remplacer par un alias qui lit directement `scheduler::core::preempt::PREEMPT_COUNT[cpu_id]` pour garantir la cohérence. Mettre à jour les assertions de layout si nécessaire.

---

### P1-02 · Bug sémantique dans `init_fpu_for_cpu` : lecture de CR4 pour la variable `cr0`

**Fichier :** `kernel/src/arch/x86_64/cpu/fpu.rs`, ligne 263

**Description :**  
La fonction `init_fpu_for_cpu()` contient :

```rust
// Lecture CR0 courant
let cr0 = super::super::read_cr4();  // ← lit CR4, pas CR0
let _ = cr0; // utilisation future
```

La variable est nommée `cr0` et le commentaire indique « Lecture CR0 courant », mais l'appel effectué est `read_cr4()`. La variable est immédiatement abandonnée (`let _ = cr0`), donc il n'y a pas d'impact runtime direct. Cependant, la lecture de CR4 à cet endroit est sémantiquement incorrecte et masque une intention de code non implémentée.

**Conséquence :**  
L'initialisation FPU ne vérifie pas l'état de CR0 avant de le modifier (bits MP, NE, EM, TS). Si CR0 contient des états inattendus à ce stade (EM=1, par exemple, en raison d'une configuration matérielle non standard), la modification aveugle de CR0 peut corrompre l'état FPU. Par ailleurs, le code futur qui exploiterait la variable `cr0` lirait une valeur de CR4 sans le savoir.

**Correction requise :**  
Remplacer l'appel par `read_cr0()` (ou lire directement CR0 via l'instruction `mov {}, cr0`). Utiliser effectivement la valeur lue pour vérifier l'état courant avant modification.

---

### P1-03 · MSR SFMASK ne masque pas NT (bit 14) ni RF (bit 16)

**Fichier :** `kernel/src/arch/x86_64/syscall.rs`, fonction `init_syscall()`

**Description :**  
La configuration du MSR SFMASK masque :

```rust
let sfmask = (1 << 9)   // IF
           | (1 << 8)   // TF
           | (1 << 10)  // DF
           | (1 << 18); // AC
```

Les flags **NT (Nested Task, bit 14)** et **RF (Resume Flag, bit 16)** ne sont pas masqués.

**Conséquence :**  
- **NT non masqué :** Si un processus utilisateur place NT=1 dans ses RFLAGS avant d'émettre un SYSCALL, ce flag est propagé dans RFLAGS kernel. Lors d'un IRETQ ultérieur (depuis un handler d'exception par exemple), NT=1 active le mécanisme de retour via la chaîne TSS (task switching x86 legacy), provoquant soit un #GP, soit un comportement indéterminé selon l'état du TSS courant. C'est une surface d'attaque exploitable localement pour provoquer un #GP en Ring 0 ou un comportement imprévisible du scheduler.
- **RF non masqué :** RF=1 supprime les breakpoints matériels (#DB) pour une instruction. Bien que moins critique, cela peut interférer avec le débogage kernel.

**Correction requise :**

```rust
let sfmask = (1 << 9)   // IF
           | (1 << 8)   // TF
           | (1 << 10)  // DF
           | (1 << 14)  // NT ← à ajouter
           | (1 << 16)  // RF ← à ajouter
           | (1 << 18); // AC
```

---

### P1-04 · `SmoltcpIface` est un stub sans intégration réelle à la pile smoltcp

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

**Description :**  
La structure `SmoltcpIface` importe `smoltcp::time::Instant` mais n'instancie aucune `smoltcp::iface::Interface`, aucun `SocketSet`, et n'appelle aucune méthode de traitement du protocole. `poll_one()` délègue directement à `device.poll_ingress_single()` (lecture de paquets bruts), et `poll_egress()` libère simplement les buffers TX. Il n'y a pas de traitement ARP, IP, TCP, ou UDP par smoltcp.

**Conséquence :**  
Les opérations réseau de haut niveau dispatché par `NetworkService::dispatch()` — `NET_OP_CONNECT`, `NET_OP_ACCEPT`, `NET_OP_SENDTO`, `NET_OP_RECVFROM`, etc. — transitent par `SocketTable` mais ne sont jamais soumises au moteur smoltcp. La pile TCP/IP est absente : aucun paquet TCP n'est construit, aucun état de connexion n'est géré. Le network_server ne peut pas établir de connexion réseau fonctionnelle.

**Correction requise :**  
Intégrer `smoltcp::iface::Interface` et `smoltcp::socket::SocketSet` dans `SmoltcpIface`. Connecter les opérations socket de `SocketTable` aux sockets smoltcp correspondants. Appeler `iface.poll()` dans la boucle principale avec un `Instant` calibré sur le TSC.

---

## P2 — Incohérences mineures

### P2-01 · `RAW_MSG_SIZE` (240 B) supérieur à `IPC_INLINE_PAYLOAD_SIZE` (120 B)

**Fichiers :** `servers/network_server/src/protocol.rs`, `servers/syscall_abi/src/lib.rs`

**Description :**  
Le `network_server` définit `RAW_MSG_SIZE = 240` et alloue un buffer `[u8; 240]` pour `recv_raw()`. Or, la structure `IpcMessage` du syscall ABI définit :

```rust
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 120;
pub const IPC_ENVELOPE_SIZE: usize = 8 + 120; // = 128 octets total
```

Le kernel IPC ne peut transporter que 120 octets de payload inline. Tout message `network_server` dépassant 120 octets sera silencieusement tronqué au niveau du kernel.

**Conséquence :**  
Les messages dépassant la capacité inline (cas `NET_OP_SENDMSG`, `NET_OP_RECVMSG` avec données) seront corrompus sans qu'aucune erreur ne soit signalée. Le `recv_raw()` retournera un `rc` positif mais les octets 120–239 seront toujours zéro.

**Correction requise :**  
Aligner `RAW_MSG_SIZE` sur `IPC_ENVELOPE_SIZE` (128 octets), ou implémenter le mécanisme de shared memory pour les transferts dépassant la capacité inline. La `NetMsg` (48 octets + header) tient dans la payload de 120 octets ; s'assurer que le chemin de données brutes n'excède pas cette limite.

---

### P2-02 · Handler `#DF` (Double Fault) silencieux — aucun diagnostic avant halt

**Fichier :** `kernel/src/arch/x86_64/exceptions.rs`, fonction `do_double_fault()`

**Description :**  
Le handler de double fault est :

```rust
extern "C" fn do_double_fault(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[8].fetch_add(1, Ordering::Relaxed);
    super::halt_cpu();
}
```

Le frame d'exception est ignoré. Aucune information — adresse fautive, registres, CPU source — n'est produite avant l'arrêt. Le `#DF` est l'exception finale avant un triple fault ; c'est le dernier moment possible pour collecter du diagnostic.

**Conséquence :**  
En cas de `#DF` (double stack overflow, #PF en Ring 0 sur pile kernel invalide, collision KPTI comme décrite en P0-03), le système s'arrête sans laisser de trace. Le débogage post-mortem est impossible sans un JTAG ou un analyseur logique.

**Correction requise :**  
Exploiter la pile IST dédiée (#DF utilise IST4 selon `tss.rs`) pour écrire un diagnostic minimal sur le port E9 ou le framebuffer, puis appeler `halt_cpu()`. Les contraintes NO-ALLOC sont respectables : utiliser uniquement l'écriture directe sur port I/O.

```rust
extern "C" fn do_double_fault(frame: *mut ExceptionFrame) {
    let frame = unsafe { &*frame };
    // Écriture port E9 — pas d'allocation
    crate::arch::x86_64::terminal::debug_write(b"[#DF DOUBLE FAULT]\n");
    // Écrire RIP, CS, RSP depuis frame...
    super::halt_cpu();
}
```

---

### P2-03 · Constante `XSAVE_MASK_MINIMAL` définie sans AVX — risque de confusion future

**Fichier :** `kernel/src/arch/x86_64/cpu/fpu.rs`

**Description :**  
La constante est définie comme :

```rust
pub const XSAVE_MASK_MINIMAL: u64 = XSAVE_X87 | XSAVE_SSE;
// ← XSAVE_AVX absent
```

Le code de `save_restore.rs` utilise correctement `!0u64` (tout sauvegarder) pour les appels `arch_xsave64` et `arch_xrstor64`. Cependant, `XSAVE_MASK_MINIMAL` est publique et pourrait être utilisée par du code future ou des drivers, avec la croyance erronée qu'elle constitue un masque « suffisant » pour la sauvegarde de contexte.

**Conséquence :**  
Tout code utilisant `XSAVE_MASK_MINIMAL` comme masque de sauvegarde sur un CPU avec AVX activé omettrait les registres YMM (upper 128 bits). Le résultat serait une corruption silencieuse d'état AVX pour les threads utilisant des intrinsèques AVX, sans aucun signal d'erreur.

**Correction requise :**  
Soit renommer la constante en `XSAVE_MASK_X87_SSE_ONLY` pour clarifier son usage restreint, soit y inclure `XSAVE_AVX` et documenter que le masque minimal garanti la cohérence pour toute application POSIX générique. Ajouter un commentaire d'avertissement dans tous les cas.

---

## Matrice de correction pour v0.2.0

| ID | Titre abrégé | Priorité | Fichier principal | Complexité |
|----|-------------|----------|-------------------|------------|
| P0-01 | KPTI inactif sur syscall path | **CRITIQUE** | `arch/x86_64/syscall.rs` | Élevée |
| P0-02 | Collision vecteur 0xF1 IPI/ExoPhoenix | **CRITIQUE** | `arch/x86_64/idt.rs` | Faible |
| P0-03 | User PML4 sans mapping stubs kernel | **CRITIQUE** | `memory/page_table/kpti_split.rs` | Élevée |
| P1-01 | Champ `preempt_count` mort dans PerCpuData | Grave | `arch/x86_64/smp/percpu.rs` | Faible |
| P1-02 | `read_cr4()` à la place de `read_cr0()` dans FPU init | Grave | `arch/x86_64/cpu/fpu.rs` | Triviale |
| P1-03 | SFMASK manque NT(14) et RF(16) | Grave | `arch/x86_64/syscall.rs` | Triviale |
| P1-04 | SmoltcpIface stub sans TCP/IP réel | Grave | `servers/network_server/smoltcp_iface.rs` | Très élevée |
| P2-01 | RAW_MSG_SIZE > IPC_INLINE_PAYLOAD_SIZE | Mineur | `servers/network_server/protocol.rs` | Faible |
| P2-02 | Double Fault handler silencieux | Mineur | `arch/x86_64/exceptions.rs` | Faible |
| P2-03 | `XSAVE_MASK_MINIMAL` sans AVX | Mineur | `arch/x86_64/cpu/fpu.rs` | Triviale |

---

## Notes sur les points validés (non bloquants)

Les éléments suivants ont été vérifiés et sont **conformes** pour la v0.2.0 :

- **Layout GDT/STAR MSR :** `GDT_USER_CS32(0x18) → GDT_USER_DS(0x20) → GDT_USER_CS64(0x28)` — espacement de 8 octets correct pour SYSRET 64 bits. L'assertion compile-time confirme la cohérence.
- **IST assignments :** #DF→IST4, #NMI→IST3, #PF→IST2, ExoPhoenix→IST1 — correctement câblés dans `tss.rs` et `idt.rs` (après résolution de P0-02).
- **Séquence d'initialisation scheduler :** `preempt::init()` → `runqueue::init_percpu()` → `fpu::save_restore::init()` → `fpu::lazy::init()` — ordre respecté dans `scheduler/mod.rs`.
- **MAX_CPUS cohérent :** Valeur 256 uniforme dans `percpu.rs`, `preempt.rs`, `topology.rs`, `memory/arch_iface.rs` — pas de divergence.
- **init_server service_table :** 12 services référencés dans `CANONICAL_SERVICES` avec graphe de dépendances complet (ipc_router → memory_server → vfs_server → … → exo_shield). La liste est complète.
- **context_switch_asm :** Sauvegarde des 6 registres callee-saved uniquement (rbx, rbp, r12–r15), sans MXCSR ni FCW — conforme à V7-C-02. Le switch CR3 dans switch_asm.s est correct pour le chemin scheduler.
- **XSAVE mask en save_restore :** `!0u64` utilisé pour xsave/xrstor — sauvegarde complète de tous les composants XCR0, correct.
- **Livraison de signaux :** `proc_signal_on_exception_return` est bien défini (`#[no_mangle] pub unsafe extern "C"` dans `process/signal/delivery.rs`) et appelé depuis `exception_return_to_user()` dans `exceptions.rs`.

---

*Rapport produit par claude-beta — ExoOS audit kernel v0.1.0 → v0.2.0 stabilisation*
