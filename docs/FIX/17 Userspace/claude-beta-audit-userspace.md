# ExoOS — Audit Userspace/Shell : Rapport Claude-Beta
**Commit analysé :** `kernel.zip` (dernier état post-corrections Codex, ~2026-05-07)  
**Portée :** `fork`, `exec`, `CoW`, `page fault handler`, `scheduler switch`, `syscall dispatch`  
**Objectif :** Identifier toutes les incohérences bloquant le démarrage du terminal/shell

---

## Résumé exécutif

Deux bugs P0 conjugués expliquent pourquoi `init_server` ne dépasse jamais `init: start ipc_router` :

| # | Priorité | Fichier | Symptôme observé |
|---|----------|---------|-----------------|
| **BUG-01** | **P0** | `memory/virtual/address_space/fork_impl.rs` | Enfant meurt sur premier #PF (VMA tree vide) |
| **BUG-02** | **P0** | `memory/virtual/address_space/fork_impl.rs` | Parent meurt sur premier write CoW (VMA flag `COW` absent) |
| BUG-03 | P1 | `arch/x86_64/address_space/tlb.rs` | TLB local non flushé après marquage CoW |
| BUG-04 | P1 | `syscall/dispatch.rs` + `process/lifecycle/exec.rs` | SYSRETQ pour execve — chemins risqués sans IRETQ |
| BUG-05 | P1 | `process/lifecycle/fork.rs` | RFLAGS_FORCE_CLR ne masque pas RF ni VM |
| BUG-06 | P2 | `arch/x86_64/syscall.rs` | `CSTAR` noop ne fait pas `movq gs:[0x08], rsp` avant save |

---

## BUG-01 — P0 CRITIQUE : VMA tree absent dans l'espace d'adressage fils

### Localisation
`kernel/src/memory/virtual/address_space/fork_impl.rs`, fonction `clone_cow` (ligne ~86)

### Description
`clone_cow` clone correctement les tables de pages (PML4 → PT) et marque les frames en CoW. Il crée ensuite un `UserAddressSpace` **vide** pour le fils :

```rust
let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
    Some(addr_space) => addr_space,
    ...
};
```

`UserAddressSpace::new()` initialise un `VmaTree::new()` vide. **Aucune VMA du parent n'est copiée.**

### Chaîne de mort

```
child runs → stack write → #PF (write, present=1, user=1)
  ↓
exceptions.rs::do_page_fault()
  → user_as.find_vma(fault_addr)   // VmaTree VIDE
  → None
  ↓
handler.rs::handle_page_fault()
  → ctx.find_vma() → None
  → FaultResult::Segfault
  ↓
exception_return_to_user() → SIGSEGV livré
  → enfant tué avant _start
```

Tout #PF dans l'enfant (CoW break, demand paging de la pile, accès au TLS, etc.) aboutit à `FaultResult::Segfault`. L'enfant ne peut jamais exécuter une seule instruction userspace.

### Preuve dans le code

`exceptions.rs` (lignes 582-587) :
```rust
if let Some(vma) = user_as.find_vma(fault_addr) {
    ctx = ctx.with_vma(vma);
}
// si None → ctx.vma_ptr reste null
```

`handler.rs` (ligne ~65) :
```rust
let vma = match ctx.find_vma(ctx.fault_addr) {
    Some(v) => v,
    None => {
        return FaultResult::Segfault { addr: ctx.fault_addr };
    }
};
```

`user.rs` (ligne ~135) :
```rust
pub fn find_vma(&self, addr: VirtAddr) -> Option<*const VmaDescriptor> {
    let inner = self.inner.lock();
    inner.vma_tree.find(addr).map(|v| v as *const _)
    // VIDE après fork_impl::clone_cow → toujours None
}
```

### Correction

Ajouter une méthode `clone_vma_tree_for_fork(&self) -> Option<VmaTree>` dans `UserAddressSpace`, et l'appeler dans `clone_cow`. Les VMAs doivent être clonées avec leurs flags (en marquant celles qui sont WRITE comme COW → voir BUG-02). Exemple de structure :

```rust
// Dans UserAddressSpace
pub fn clone_for_fork(&self) -> Option<UserAddressSpace> {
    let inner = self.inner.lock();
    let cloned_tree = inner.vma_tree.clone_cow_snapshot()?; // deep clone
    let child_as = UserAddressSpace {
        inner: Mutex::new(UserAsInner {
            vma_tree: cloned_tree,
            mmap_hint: inner.mmap_hint,
            stack_bottom: inner.stack_bottom,
        }),
        stats: UserAsStats::new(),
        pml4_phys: /* child PML4 — passé en paramètre */,
        pid: 0,   // mis à jour plus bas
        heap_end: AtomicU64::new(
            self.heap_end.load(Ordering::Acquire)
        ),
    };
    Some(child_as)
}
```

Dans `VmaTree::clone_cow_snapshot`, chaque VMA avec le flag `WRITE` reçoit le flag `COW` en plus (pour BUG-02). `fork_impl::clone_cow` doit passer le `child_pml4_phys` à `clone_for_fork` et utiliser son résultat.

---

## BUG-02 — P0 CRITIQUE : VMA COW flag absent dans le parent après fork

### Localisation
`kernel/src/memory/virtual/address_space/fork_impl.rs`, fonctions `clone_pt`, `clone_pd`, `clone_pdpt`

### Description
Lors du fork, `clone_userspace_tables` marque les PTEs des pages writables en CoW (`PTE_COW | ~PTE_WRITABLE`). **Les VMA correspondantes dans l'espace du PARENT ne reçoivent pas le flag `VmaFlags::COW`.**

Après fork, quand le PARENT écrit sur la stack (premier retour de fork()) :
- PTE lue = présente, read-only, `PTE_COW=1`
- Le handler reçoit : `cause=Write`, `vma.flags = WRITE | ANON` (pas de COW)

Voici le dispatcher dans `handler.rs` :
```rust
// CoW break path — SAUTÉ car vma.flags n'a PAS COW
if ctx.cause == FaultCause::Write && vma.flags.contains(VmaFlags::COW) {
    return cow::handle_cow_fault(ctx, vma, alloc);
}

// protection write check
match ctx.cause {
    FaultCause::Write => {
        // VMA a WRITE → pas de Segfault immédiat ici
    }
}

// Tombe dans demand_paging (page PRÉSENTE → undefined behavior)
if vma.flags.contains(VmaFlags::ANONYMOUS) ... {
    demand_paging::handle_demand_paging(ctx, vma, alloc)
}
```

`demand_paging` reçoit une page **déjà présente** avec `error_code.P=1`. Selon l'implémentation, cela retourne `Segfault` ou corrompt la page (remapping destructif). Dans les deux cas : le parent meurt ou ses données sont corrompues.

### Symptôme observé (Codex)
> "Le retour SYSRET est atteint, mais PID1 ne logue plus après fork"

SYSRETQ s'est exécuté. Le parent revient en Ring 3. Sa première instruction écrit sur la stack (prologue ABI, sauvegarde de rbx/rbp). Cette écriture déclenche le #PF. Le handler ne trouve pas le chemin CoW → SIGSEGV → PID1 mort.

### Correction

Dans `clone_userspace_tables` (ou idéalement dans `clone_for_fork` côté `UserAddressSpace`), après le marquage CoW des PTEs, parcourir les VMAs du parent et ajouter `VmaFlags::COW` à toutes celles qui ont `VmaFlags::WRITE` :

```rust
// Après clone_userspace_tables :
if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    parent_as.mark_writeable_vmas_cow(); // nouveau helper
}
```

```rust
// Dans UserAddressSpace :
pub fn mark_writeable_vmas_cow(&self) {
    let mut inner = self.inner.lock();
    inner.vma_tree.for_each_mut(|vma| {
        if vma.flags.contains(VmaFlags::WRITE) {
            vma.flags |= VmaFlags::COW;
        }
    });
}
```

**Important :** après execve(), les VMAs COW doivent être recréées sans ce flag (déjà géré car execve reconstruit l'AS depuis zéro).

---

## BUG-03 — P1 : TLB local non flushé après marquage CoW (parent)

### Localisation
`memory/virtual/address_space/fork_impl.rs`, `flush_tlb_after_fork`

### Description
```rust
fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
    unsafe {
        shootdown_sync(TlbFlushType::All, smp_cpu_count());
    }
}
```

`shootdown_sync` envoie des IPIs aux **autres** CPUs. Le CPU courant (qui vient d'appeler `do_fork()`) doit aussi invalider son propre TLB. Sans ça, le parent dispose d'entrées TLB cacheant les PTEs comme writables, même si les PTEs ont été modifiées (read-only CoW). Le parent écrit à travers le TLB cache sans déclencher de #PF, contaminant le frame partagé avec l'enfant.

### Correction

Ajouter un flush local explicite :
```rust
fn flush_tlb_after_fork(&self, parent_cr3: u64) {
    // Flush local : réécrire CR3 invalide toutes les non-global entries
    unsafe {
        crate::arch::x86_64::write_cr3(parent_cr3); // flush local TLB
        shootdown_sync(TlbFlushType::All, smp_cpu_count());
    }
}
```

**Note :** Si PCID est actif, utiliser `write_cr3(cr3 & !CR3_PCID_MASK)` pour forcer le flush même si le PCID ne change pas.

---

## BUG-04 — P1 : execve retourne via SYSRETQ sans IRETQ propre

### Localisation
`syscall/dispatch.rs`, `handle_execve_inplace` (ligne ~749)

### Description
Après un `do_execve()` réussi, le dispatch met à jour la `SyscallFrame` et retourne via `SYSRETQ` :
```rust
frame.rcx = new_rip;    // entry_point du nouvel ELF
frame.rsp = new_rsp;
frame.r11 = 0x0202;
```

Deux risques non mitigés :

1. **Les registres callee-saved ne sont pas zérisés.** `rbx`, `rbp`, `r12-r15` du *caller* de fork() (init_server) fuient dans le nouvel espace d'adressage. Si le nouvel ELF est différent (exec dans l'enfant après fork), ces registres contiennent des données de l'ancienne image qui peuvent être interprétées comme pointeurs ou valeurs par la libc.

2. **SYSRETQ est interdit si `rcx[63:48] != 0`.** Le check `is_user_return_addr` couvre ça, mais uniquement pour le cas où `new_rip` est invalide. Il n'y a pas de fallback vers IRETQ si le check échoue — le processus reçoit juste RIP=0 → triple-fault potentiel en Ring 3.

### Correction

Après execve, zériser les registres callee-saved dans la frame avant SYSRETQ :
```rust
frame.rbx = 0;
frame.rbp = 0;
frame.r12 = 0; frame.r13 = 0; frame.r14 = 0; frame.r15 = 0;
frame.rcx = new_rip;
frame.rsp = new_rsp;
frame.r11 = 0x0202;
frame.rax = 0;
```

---

## BUG-05 — P1 : RFLAGS_FORCE_CLR incomplet dans do_fork

### Localisation
`process/lifecycle/fork.rs`, ligne ~275

### Description
```rust
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // TF=0, NT=0, RF=0, VM=0
```

La valeur `0x0004_0100` efface uniquement :
- bit 8 = TF (Trap Flag) ✓
- bit 14 = NT (Nested Task) ✓

Elle **ne masque pas** :
- bit 16 = RF (Resume Flag)
- bit 17 = VM (Virtual-8086 mode)

Le commentaire "RF=0, VM=0" est trompeur. En pratique, `RFLAGS_SAFE_MASK = 0x200CD5` n'inclut pas RF ni VM, donc ils sont déjà à 0 après l'AND. Ce bug est cosmétique (pas fonctionnel) mais crée une fausse assurance de sécurité.

### Correction
```rust
const RFLAGS_FORCE_CLR: u64 = 
    (1 << 8)  |  // TF
    (1 << 14) |  // NT
    (1 << 16) |  // RF
    (1 << 17);   // VM = 0x0003_4100
```

---

## BUG-06 — P2 : CSTAR noop — fuite de RSP userspace en Ring 0

### Localisation
`arch/x86_64/syscall.rs`, `syscall_cstar_noop` (stub compat 32-bit)

### Description
Le stub CSTAR (SYSCALL compat) fait :
```asm
swapgs
mov qword ptr gs:[0x08], rsp  // save user RSP
mov rsp, qword ptr gs:[0x00]  // load kernel RSP
mov eax, -38
mov rsp, qword ptr gs:[0x08]  // restore user RSP
swapgs
sysret
```

Ce stub est correct sur le papier, mais si un signal ou IRQ se produit entre le `swapgs` initial et le `mov rsp, gs:[0x00]`, le CPU est en Ring 0 avec la PILE USERSPACE active. Tout write sur stack corrompt la pile utilisateur. La fenêtre est ultra-courte (2 instructions) mais non-nulle sur hardware SMP. Linux utilise `IST` ou `ESPFIX` pour ce cas.

En pratique, pour ExoOS, CSTAR ne sera jamais appelé légitimement (pas de processus 32-bit). Mais si un process malformé envoie un SYSCALL en compat mode, il peut ouvrir cette fenêtre.

### Correction (minimale)
Désactiver les interruptions dès l'entrée CSTAR :
```asm
syscall_cstar_noop:
    cli            // ferme la fenêtre IST pendant le switch de stack
    swapgs
    mov qword ptr gs:[0x08], rsp
    mov rsp, qword ptr gs:[0x00]
    mov eax, -38   // ENOSYS
    mov rsp, qword ptr gs:[0x08]
    swapgs
    sti
    sysret
```

---

## Récapitulatif des corrections prioritaires

```
BUG-01 (P0) fork_impl.rs   : Implémenter VmaTree::clone_cow_snapshot() 
                              Appeler depuis clone_cow() pour copier les VMAs du parent
                              
BUG-02 (P0) fork_impl.rs   : Après clone_userspace_tables(), ajouter VmaFlags::COW
                              à toutes les VMAs WRITE du PARENT via mark_writeable_vmas_cow()
                              
BUG-03 (P1) fork_impl.rs   : flush_tlb_after_fork() : ajouter write_cr3(parent_cr3)
                              AVANT shootdown_sync pour flush le CPU local
                              
BUG-04 (P1) dispatch.rs    : Zériser rbx,rbp,r12-r15 dans la frame avant SYSRETQ execve
                              
BUG-05 (P1) fork.rs        : Corriger RFLAGS_FORCE_CLR pour inclure bits 16 (RF) et 17 (VM)
                              
BUG-06 (P2) syscall.rs     : Ajouter cli/sti dans CSTAR noop (sécurité défensive)
```

---

## Note sur la piste Codex

Codex a progressivement levé les blocages dans l'ordre :
1. ✅ Build sans warning
2. ✅ Panic `extend_from_slice` dans `try_clone_for_fork` (table FD)
3. ✅ Triple fault (ajout du mapping noyau dans user PML4[256:512])
4. ✅ SYSRETQ atteint par le parent

**Point d'arrêt actuel** : parent meurt juste après SYSRETQ (BUG-02), enfant meurt sur premier #PF (BUG-01). Le chemin emprunté par Codex était globalement correct. Les corrections ci-dessus sont les deux derniers maillons manquants pour avoir un `ipc_router` fonctionnel.
