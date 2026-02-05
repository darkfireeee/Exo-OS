# Plan Tests CoW RÉELS - Sans Simplification

**Date**: 24 Janvier 2025  
**Objectif**: Valider 100% du CoW avec pages réelles AVANT toute autre intégration  
**Principe**: ZÉRO frame synthétique, ZÉRO simplification

---

## 🎯 Problème Actuel

### Tests Existants (Simplifiés)
```rust
// TEST 0b - SIMPLIFIÉ ❌
let frames = vec![
    PhysicalAddress::new(0x10000),  // Adresse inventée
    PhysicalAddress::new(0x20000),  // Adresse inventée
    PhysicalAddress::new(0x30000),  // Adresse inventée
];
// Teste seulement mark_cow(), pas l'infrastructure complète
```

**Problèmes**:
- ❌ Pas de vraies pages mappées
- ❌ walk_pages() non testé avec page tables réelles
- ❌ fork_cow() non testé avec UserAddressSpace réel
- ❌ Intégration incomplète

### Blocage Découvert
```rust
// UserAddressSpace::new() DEADLOCK dans kernel threads
let mut space = UserAddressSpace::new()?; // ⚠️ BLOQUE
```

**Root Cause**: alloc_page_table() utilise heap allocator qui deadlock en kernel thread context.

---

## 🚀 Solution: Tests avec Page Tables Réelles

### Approche 1: Scanner Page Tables Kernel Actuelles ⭐

**Principe**: Utiliser CR3 actuel, scanner PML4→PDPT→PD→PT existantes

**Avantages**:
- ✅ Pas besoin de créer UserAddressSpace
- ✅ Vraies structures de page tables
- ✅ Vraies adresses physiques
- ✅ Teste walk_pages() sur données réelles

**Implémentation**:

```rust
/// Test walk_pages() sur page tables RÉELLES (kernel)
pub fn test_walk_pages_kernel_real() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST WALK_PAGES: Vraies Page Tables Kernel     ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    // 1. Obtenir CR3 (PML4 physique)
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    let pml4_phys = PhysicalAddress::new((cr3 & 0x000F_FFFF_FFFF_F000) as usize);
    
    // 2. Mapper PML4 en virtuel (identity-mapped ou via kernel mapping)
    let pml4_virt = VirtualAddress::new(pml4_phys.value() + KERNEL_OFFSET);
    let pml4 = unsafe { &*(pml4_virt.as_ptr::<PageTable>()) };
    
    // 3. Scanner VRAIES entrées PML4
    let mut page_count = 0;
    let mut cow_candidate_count = 0;
    
    for (pml4_idx, pml4_entry) in pml4.entries.iter().enumerate() {
        if pml4_entry.present() {
            // Descendre dans PDPT
            let pdpt_phys = PhysicalAddress::new((pml4_entry.value() & 0x000F_FFFF_FFFF_F000) as usize);
            let pdpt_virt = VirtualAddress::new(pdpt_phys.value() + KERNEL_OFFSET);
            let pdpt = unsafe { &*(pdpt_virt.as_ptr::<PageTable>()) };
            
            for (pdpt_idx, pdpt_entry) in pdpt.entries.iter().enumerate() {
                if pdpt_entry.present() && !pdpt_entry.is_huge_page() {
                    // Descendre dans PD
                    let pd_phys = PhysicalAddress::new((pdpt_entry.value() & 0x000F_FFFF_FFFF_F000) as usize);
                    let pd_virt = VirtualAddress::new(pd_phys.value() + KERNEL_OFFSET);
                    let pd = unsafe { &*(pd_virt.as_ptr::<PageTable>()) };
                    
                    for (pd_idx, pd_entry) in pd.entries.iter().enumerate() {
                        if pd_entry.present() && !pd_entry.is_huge_page() {
                            // Descendre dans PT
                            let pt_phys = PhysicalAddress::new((pd_entry.value() & 0x000F_FFFF_FFFF_F000) as usize);
                            let pt_virt = VirtualAddress::new(pt_phys.value() + KERNEL_OFFSET);
                            let pt = unsafe { &*(pt_virt.as_ptr::<PageTable>()) };
                            
                            for (pt_idx, pt_entry) in pt.entries.iter().enumerate() {
                                if pt_entry.present() {
                                    page_count += 1;
                                    
                                    // Calculer adresse virtuelle
                                    let virt_addr = (pml4_idx << 39) | (pdpt_idx << 30) | 
                                                   (pd_idx << 21) | (pt_idx << 12);
                                    let virt = VirtualAddress::new(virt_addr);
                                    
                                    // Adresse physique
                                    let phys = PhysicalAddress::new((pt_entry.value() & 0x000F_FFFF_FFFF_F000) as usize);
                                    
                                    // Vérifier si writable (candidat pour CoW)
                                    if pt_entry.writable() {
                                        cow_candidate_count += 1;
                                        
                                        // Afficher les 10 premières
                                        if cow_candidate_count <= 10 {
                                            let s = alloc::format!(
                                                "[PAGE {}] Virt: {:#x}, Phys: {:#x}, Writable: ✅\n",
                                                cow_candidate_count, virt.value(), phys.value()
                                            );
                                            crate::logger::early_print(&s);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    let s = alloc::format!("\n[STATS] Total pages: {}\n", page_count);
    crate::logger::early_print(&s);
    let s = alloc::format!("[STATS] Pages writable (candidats CoW): {}\n", cow_candidate_count);
    crate::logger::early_print(&s);
    
    // Validation
    if page_count > 100 {
        crate::logger::early_print("[PASS] ✅ Scanner trouve >100 pages réelles\n");
    } else {
        crate::logger::early_print("[FAIL] ❌ Trop peu de pages trouvées\n");
    }
    
    if cow_candidate_count > 10 {
        crate::logger::early_print("[PASS] ✅ Trouve >10 pages writable pour tester CoW\n");
    } else {
        crate::logger::early_print("[FAIL] ❌ Pas assez de pages writable\n");
    }
}
```

**Résultat Attendu**:
```
[PAGE 1] Virt: 0xffff800000001000, Phys: 0x123000, Writable: ✅
[PAGE 2] Virt: 0xffff800000002000, Phys: 0x124000, Writable: ✅
...
[STATS] Total pages: 1547
[STATS] Pages writable (candidats CoW): 234
[PASS] ✅ Scanner trouve >100 pages réelles
[PASS] ✅ Trouve >10 pages writable pour tester CoW
```

---

### Approche 2: Tester fork_cow() avec Pages Kernel Réelles

**Principe**: Prendre 10 pages kernel writable, appliquer la logique fork_cow()

```rust
pub fn test_fork_cow_kernel_pages() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST FORK_COW: Pages Kernel Réelles             ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    // 1. Scanner et collecter 10 pages writable réelles
    let mut test_pages: Vec<(VirtualAddress, PhysicalAddress)> = Vec::new();
    
    // ... (même scanner que ci-dessus mais collecte pages)
    
    crate::logger::early_print("[SETUP] Collected 10 real writable pages from kernel\n");
    
    // 2. Pour chaque page, tester le workflow CoW
    for (i, (virt, phys)) in test_pages.iter().enumerate().take(10) {
        let s = alloc::format!("\n[TEST {}] Virt: {:#x}, Phys: {:#x}\n", i+1, virt.value(), phys.value());
        crate::logger::early_print(&s);
        
        // a) Marquer comme CoW
        let refcount = cow_manager::mark_cow(*phys);
        let s = alloc::format!("  → mark_cow() → refcount: {}\n", refcount);
        crate::logger::early_print(&s);
        
        // b) Vérifier refcount
        if refcount == 1 {
            crate::logger::early_print("  ✅ Refcount correct (première page)\n");
        } else {
            crate::logger::early_print("  ❌ Refcount inattendu\n");
        }
        
        // c) Simuler deuxième référence (fork)
        let refcount2 = cow_manager::mark_cow(*phys);
        let s = alloc::format!("  → mark_cow() [fork] → refcount: {}\n", refcount2);
        crate::logger::early_print(&s);
        
        if refcount2 == 2 {
            crate::logger::early_print("  ✅ Refcount partagé correct\n");
        } else {
            crate::logger::early_print("  ❌ Refcount partagé incorrect\n");
        }
        
        // d) Simuler CoW fault (copie)
        match cow_manager::handle_cow_fault(*virt, *phys) {
            Ok(new_phys) => {
                let s = alloc::format!("  → handle_cow_fault() → new phys: {:#x}\n", new_phys.value());
                crate::logger::early_print(&s);
                
                if new_phys != *phys {
                    crate::logger::early_print("  ✅ Nouvelle page allouée\n");
                } else {
                    crate::logger::early_print("  ❌ Pas de nouvelle page\n");
                }
                
                // e) Vérifier refcount après copie
                let refcount_after = cow_manager::get_refcount(new_phys).unwrap_or(0);
                let s = alloc::format!("  → get_refcount(new) → {}\n", refcount_after);
                crate::logger::early_print(&s);
                
                if refcount_after == 1 {
                    crate::logger::early_print("  ✅ Refcount nouvelle page = 1\n");
                } else {
                    crate::logger::early_print("  ⚠️  Refcount nouvelle page != 1\n");
                }
            }
            Err(e) => {
                let s = alloc::format!("  ❌ handle_cow_fault failed: {:?}\n", e);
                crate::logger::early_print(&s);
            }
        }
    }
    
    crate::logger::early_print("\n[SUMMARY] Tested fork_cow workflow on 10 real kernel pages\n");
}
```

**Résultat Attendu**:
```
[TEST 1] Virt: 0xffff800000123000, Phys: 0x456000
  → mark_cow() → refcount: 1
  ✅ Refcount correct (première page)
  → mark_cow() [fork] → refcount: 2
  ✅ Refcount partagé correct
  → handle_cow_fault() → new phys: 0x789000
  ✅ Nouvelle page allouée
  → get_refcount(new) → 1
  ✅ Refcount nouvelle page = 1
...
[SUMMARY] Tested fork_cow workflow on 10 real kernel pages
```

---

### Approche 3: Mini Process avec Pages Heap Réelles

**Principe**: Allouer pages via heap (Box), obtenir adresses physiques réelles, tester CoW

```rust
pub fn test_cow_with_heap_pages() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST COW: Pages Heap Réelles                    ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    // 1. Allouer pages via heap (garantit pages physiques réelles)
    let num_pages = 10;
    let mut heap_pages: Vec<Box<[u8; 4096]>> = Vec::new();
    
    for i in 0..num_pages {
        let page = Box::new([0u8; 4096]);
        heap_pages.push(page);
    }
    
    let s = alloc::format!("[SETUP] Allocated {} real heap pages\n", num_pages);
    crate::logger::early_print(&s);
    
    // 2. Pour chaque page, obtenir adresse physique RÉELLE
    for (i, page) in heap_pages.iter().enumerate() {
        let virt_addr = page.as_ptr() as usize;
        let virt = VirtualAddress::new(virt_addr);
        
        // Traduire virt→phys via page tables
        // Option A: Si identity-mapped en kernel
        let phys = PhysicalAddress::new(virt_addr - KERNEL_OFFSET);
        
        // Option B: Via page table walk (plus correct)
        // let phys = translate_virt_to_phys(virt)?;
        
        let s = alloc::format!("\n[PAGE {}] Heap page at virt: {:#x}, phys: {:#x}\n", 
                              i+1, virt.value(), phys.value());
        crate::logger::early_print(&s);
        
        // 3. Écrire données test
        unsafe {
            let data_ptr = virt_addr as *mut u64;
            *data_ptr = 0xDEADBEEF_00000000 | i as u64;
        }
        
        // 4. Tester workflow CoW complet
        let refcount1 = cow_manager::mark_cow(phys);
        let s = alloc::format!("  → mark_cow() → refcount: {}\n", refcount1);
        crate::logger::early_print(&s);
        
        let refcount2 = cow_manager::mark_cow(phys);
        let s = alloc::format!("  → mark_cow() [fork] → refcount: {}\n", refcount2);
        crate::logger::early_print(&s);
        
        // 5. Trigger CoW fault
        match cow_manager::handle_cow_fault(virt, phys) {
            Ok(new_phys) => {
                let s = alloc::format!("  → handle_cow_fault() → new: {:#x}\n", new_phys.value());
                crate::logger::early_print(&s);
                
                // 6. Vérifier données copiées
                let original_data = unsafe { *(virt_addr as *const u64) };
                let expected_data = 0xDEADBEEF_00000000 | i as u64;
                
                if original_data == expected_data {
                    crate::logger::early_print("  ✅ Données préservées après CoW\n");
                } else {
                    let s = alloc::format!("  ❌ Données corrompues: {:#x} != {:#x}\n", 
                                          original_data, expected_data);
                    crate::logger::early_print(&s);
                }
            }
            Err(e) => {
                let s = alloc::format!("  ❌ handle_cow_fault failed: {:?}\n", e);
                crate::logger::early_print(&s);
            }
        }
    }
    
    // 7. Cleanup (pages libérées automatiquement via Drop)
    crate::logger::early_print("\n[CLEANUP] Heap pages will be freed on drop\n");
    core::mem::drop(heap_pages);
    
    crate::logger::early_print("[COMPLETE] ✅ Tested CoW with real heap pages\n");
}
```

**Résultat Attendu**:
```
[SETUP] Allocated 10 real heap pages

[PAGE 1] Heap page at virt: 0xffff800001234000, phys: 0x1234000
  → mark_cow() → refcount: 1
  → mark_cow() [fork] → refcount: 2
  → handle_cow_fault() → new: 0x5678000
  ✅ Données préservées après CoW
...
[CLEANUP] Heap pages will be freed on drop
[COMPLETE] ✅ Tested CoW with real heap pages
```

---

## 📋 Plan d'Implémentation

### Étape 1: Test walk_pages() RÉEL ⭐ PRIORITÉ 1
**Fichier**: `kernel/src/tests/cow_real_tests.rs` (nouveau)

**Tâches**:
1. [ ] Créer cow_real_tests.rs
2. [ ] Implémenter test_walk_pages_kernel_real()
3. [ ] Ajouter constantes KERNEL_OFFSET si manquantes
4. [ ] Intégrer dans mod.rs
5. [ ] Compiler et tester dans QEMU

**Critères de Succès**:
- [ ] Trouve >100 pages kernel
- [ ] Trouve >10 pages writable
- [ ] Affiche vraies adresses phys/virt
- [ ] ZÉRO adresse synthétique

**Temps estimé**: 2h

---

### Étape 2: Test fork_cow() RÉEL ⭐ PRIORITÉ 2
**Fichier**: `kernel/src/tests/cow_real_tests.rs`

**Tâches**:
1. [ ] Implémenter test_fork_cow_kernel_pages()
2. [ ] Tester workflow complet sur 10 pages réelles
3. [ ] Valider refcount 1→2→1
4. [ ] Valider nouvelle page allouée

**Critères de Succès**:
- [ ] 10/10 pages testées avec succès
- [ ] Refcount correct à chaque étape
- [ ] handle_cow_fault() alloue nouvelle page
- [ ] Refcount nouvelle page = 1

**Temps estimé**: 2h

---

### Étape 3: Test CoW avec Heap Pages ⭐ PRIORITÉ 3
**Fichier**: `kernel/src/tests/cow_real_tests.rs`

**Tâches**:
1. [ ] Implémenter test_cow_with_heap_pages()
2. [ ] Allouer 10 pages heap
3. [ ] Obtenir adresses physiques réelles
4. [ ] Tester CoW avec données
5. [ ] Valider préservation données

**Critères de Succès**:
- [ ] 10 pages heap allouées
- [ ] Adresses physiques obtenues
- [ ] Données préservées après CoW
- [ ] Cleanup automatique (Drop)

**Temps estimé**: 3h

---

### Étape 4: Documentation et Validation

**Tâches**:
1. [ ] Documenter résultats dans JOUR_4_COW_INTEGRATION.md
2. [ ] Créer section "Tests RÉELS" avec métriques
3. [ ] Valider 100% CoW fonctionnel
4. [ ] Marquer Phase 4 comme complète

**Critères de Succès**:
- [ ] Tous tests RÉELS passent
- [ ] ZÉRO simplification
- [ ] Documentation complète
- [ ] Prêt pour intégrations autres modules

**Temps estimé**: 1h

---

## 🎯 Checklist Validation CoW 100%

### Infrastructure ✅
- [x] CoW Manager (mark_cow, handle_cow_fault, get_stats)
- [x] Page Fault Handler intégré
- [x] walk_pages() implémenté
- [x] fork_cow() implémenté
- [x] sys_fork() intégré

### Tests Synthétiques ✅
- [x] TEST 0: 6 pages CoW tracked
- [x] TEST 0b: 3 frames synthétiques
- [x] TEST refcount
- [x] TEST latency

### Tests RÉELS ⏳ EN COURS
- [ ] walk_pages() sur page tables kernel réelles
- [ ] fork_cow() sur 10 pages kernel réelles
- [ ] CoW avec heap pages et données réelles
- [ ] Validation refcount complet
- [ ] Validation copie de page
- [ ] Validation isolation parent/child

### Intégration Complète ⏳ APRÈS TESTS RÉELS
- [ ] Process Table avec UserAddressSpace
- [ ] sys_fork() avec vraies pages
- [ ] Tests userspace (après ELF loader)

---

## 📊 Métriques Finales Requises

| Métrique | Valeur | Status |
|----------|--------|--------|
| Pages kernel scannées | >100 | ⏳ |
| Pages writable trouvées | >10 | ⏳ |
| Tests CoW workflow réussis | 10/10 | ⏳ |
| Refcount 1→2→1 correct | ✅ | ⏳ |
| Nouvelle page allouée | ✅ | ⏳ |
| Données préservées | ✅ | ⏳ |
| Cleanup sans leak | ✅ | ⏳ |

---

## 🚀 Prochaine Action IMMÉDIATE

**Créer cow_real_tests.rs maintenant**:
```bash
touch kernel/src/tests/cow_real_tests.rs
```

**Implémenter test_walk_pages_kernel_real() d'abord**

**OBJECTIF**: Prouver que le scanner trouve >100 pages réelles AVANT toute autre intégration.

---

**PRINCIPE DIRECTEUR**: "Aucune simplification, aucun raccourci - seulement des tests avec des vraies pages physiques et des vraies structures de page tables."
