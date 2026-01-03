#!/bin/bash
# Test Page Fault Handler + CoW Integration

echo "════════════════════════════════════════════════════════════"
echo "  TEST Page Fault Handler + CoW Manager Integration"
echo "════════════════════════════════════════════════════════════"
echo ""

cat > /tmp/test_page_fault_cow.rs << 'EOF'
// Test d'intégration: Page Fault Handler + CoW Manager
// Simule le workflow complet: fork() → write → page fault → CoW copy

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::alloc::{alloc, dealloc, Layout};

// ============================================================================
// TYPES SIMULÉS
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PhysicalAddress(usize);
impl PhysicalAddress {
    fn new(addr: usize) -> Self { Self(addr & !0xFFF) }
    fn value(&self) -> usize { self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct VirtualAddress(usize);
impl VirtualAddress {
    fn new(addr: usize) -> Self { Self(addr) }
    fn value(&self) -> usize { self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UserPageFlags(u64);
impl UserPageFlags {
    fn empty() -> Self { Self(0) }
    fn writable(self) -> Self { Self(self.0 | (1 << 1)) }
    fn user(self) -> Self { Self(self.0 | (1 << 2)) }
    fn present(self) -> Self { Self(self.0 | (1 << 0)) }
    
    fn contains_writable(&self) -> bool { (self.0 & (1 << 1)) != 0 }
    fn remove_writable(self) -> Self { Self(self.0 & !(1 << 1)) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CowError {
    OutOfMemory,
    NotCowPage,
}

const PAGE_SIZE: usize = 4096;

// ============================================================================
// MOCK FRAME ALLOCATOR
// ============================================================================

struct MockFrameAllocator {
    allocated: BTreeMap<usize, *mut u8>,
    next_addr: usize,
}

impl MockFrameAllocator {
    fn new() -> Self {
        Self {
            allocated: BTreeMap::new(),
            next_addr: 0x10000,
        }
    }

    fn allocate(&mut self) -> Result<PhysicalAddress, CowError> {
        unsafe {
            let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
            let ptr = alloc(layout);
            if ptr.is_null() {
                return Err(CowError::OutOfMemory);
            }
            
            let addr = self.next_addr;
            self.next_addr += PAGE_SIZE;
            self.allocated.insert(addr, ptr);
            
            Ok(PhysicalAddress::new(ptr as usize))
        }
    }

    fn deallocate(&mut self, addr: PhysicalAddress) {
        let addr_val = addr.value();
        
        let mut to_remove = None;
        for (logical_addr, &ptr) in &self.allocated {
            if ptr as usize == addr_val {
                to_remove = Some(*logical_addr);
                break;
            }
        }
        
        if let Some(logical_addr) = to_remove {
            if let Some(&ptr) = self.allocated.get(&logical_addr) {
                unsafe {
                    let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
                    dealloc(ptr, layout);
                }
                self.allocated.remove(&logical_addr);
            }
        }
    }
}

static mut MOCK_ALLOCATOR: Option<MockFrameAllocator> = None;

fn init_mock_allocator() {
    unsafe {
        MOCK_ALLOCATOR = Some(MockFrameAllocator::new());
    }
}

fn allocate_frame() -> Result<PhysicalAddress, CowError> {
    unsafe {
        MOCK_ALLOCATOR.as_mut().unwrap().allocate()
    }
}

fn deallocate_frame(addr: PhysicalAddress) {
    unsafe {
        MOCK_ALLOCATOR.as_mut().unwrap().deallocate(addr);
    }
}

// ============================================================================
// COW MANAGER
// ============================================================================

struct RefCountEntry {
    refcount: AtomicU32,
}

impl RefCountEntry {
    fn new(count: u32) -> Self {
        Self { refcount: AtomicU32::new(count) }
    }
    fn get(&self) -> u32 {
        self.refcount.load(Ordering::SeqCst)
    }
    fn increment(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn decrement(&self) -> u32 {
        self.refcount.fetch_sub(1, Ordering::SeqCst) - 1
    }
}

struct CowManager {
    refcounts: BTreeMap<PhysicalAddress, RefCountEntry>,
}

impl CowManager {
    fn new() -> Self {
        Self { refcounts: BTreeMap::new() }
    }

    fn mark_cow(&mut self, phys: PhysicalAddress) -> u32 {
        if let Some(entry) = self.refcounts.get(&phys) {
            entry.increment()
        } else {
            self.refcounts.insert(phys, RefCountEntry::new(2));
            2
        }
    }

    fn is_cow(&self, phys: PhysicalAddress) -> bool {
        self.refcounts.contains_key(&phys)
    }

    fn get_refcount(&self, phys: PhysicalAddress) -> Option<u32> {
        self.refcounts.get(&phys).map(|e| e.get())
    }

    fn decrement(&mut self, phys: PhysicalAddress) -> u32 {
        if let Some(entry) = self.refcounts.get(&phys) {
            let new_count = entry.decrement();
            if new_count == 0 {
                self.refcounts.remove(&phys);
            }
            new_count
        } else {
            0
        }
    }

    fn handle_cow_fault(&mut self, _virt: VirtualAddress, phys: PhysicalAddress) 
        -> Result<PhysicalAddress, CowError> 
    {
        if !self.is_cow(phys) {
            return Err(CowError::NotCowPage);
        }

        if let Some(count) = self.get_refcount(phys) {
            if count == 1 {
                self.refcounts.remove(&phys);
                return Ok(phys);
            }
        }

        self.copy_page(phys)
    }

    fn copy_page(&mut self, src_phys: PhysicalAddress) -> Result<PhysicalAddress, CowError> {
        let new_phys = allocate_frame()?;

        unsafe {
            let src = src_phys.value() as *const u8;
            let dst = new_phys.value() as *mut u8;
            std::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
        }

        self.decrement(src_phys);

        Ok(new_phys)
    }

    fn clone_address_space(
        &mut self, 
        pages: &[(VirtualAddress, PhysicalAddress, UserPageFlags)]
    ) -> Result<Vec<(VirtualAddress, PhysicalAddress, UserPageFlags)>, CowError> {
        let mut new_pages = Vec::with_capacity(pages.len());

        for &(virt, phys, flags) in pages {
            if flags.contains_writable() {
                self.mark_cow(phys);
                let cow_flags = flags.remove_writable();
                new_pages.push((virt, phys, cow_flags));
            } else {
                new_pages.push((virt, phys, flags));
            }
        }

        Ok(new_pages)
    }
}

// ============================================================================
// PAGE TABLE SIMULATOR
// ============================================================================

struct PageTable {
    mappings: BTreeMap<VirtualAddress, (PhysicalAddress, UserPageFlags)>,
}

impl PageTable {
    fn new() -> Self {
        Self {
            mappings: BTreeMap::new(),
        }
    }

    fn map(&mut self, virt: VirtualAddress, phys: PhysicalAddress, flags: UserPageFlags) {
        self.mappings.insert(virt, (phys, flags));
    }

    fn get_physical(&self, virt: VirtualAddress) -> Option<PhysicalAddress> {
        self.mappings.get(&virt).map(|(phys, _)| *phys)
    }

    fn get_flags(&self, virt: VirtualAddress) -> Option<UserPageFlags> {
        self.mappings.get(&virt).map(|(_, flags)| *flags)
    }

    fn set_flags(&mut self, virt: VirtualAddress, flags: UserPageFlags) {
        if let Some((phys, _)) = self.mappings.get(&virt) {
            let phys = *phys;
            self.mappings.insert(virt, (phys, flags));
        }
    }

    fn remap(&mut self, virt: VirtualAddress, new_phys: PhysicalAddress) {
        if let Some((_, flags)) = self.mappings.get(&virt) {
            let flags = *flags;
            self.mappings.insert(virt, (new_phys, flags));
        }
    }
}

// ============================================================================
// PAGE FAULT HANDLER
// ============================================================================

fn handle_cow_page_fault(
    page_table: &mut PageTable,
    cow_manager: &mut CowManager,
    virt: VirtualAddress
) -> Result<(), &'static str> {
    // Obtenir adresse physique actuelle
    let current_phys = page_table.get_physical(virt)
        .ok_or("Page not mapped")?;

    // Gérer le fault CoW
    let new_phys = cow_manager.handle_cow_fault(virt, current_phys)
        .map_err(|_| "CoW fault failed")?;

    // Obtenir flags actuels
    let mut flags = page_table.get_flags(virt)
        .ok_or("No flags")?;

    // Ajouter flag writable
    flags = flags.writable();

    if new_phys == current_phys {
        // Refcount==1: juste changer les flags
        page_table.set_flags(virt, flags);
    } else {
        // Refcount>1: remapper avec nouvelle page
        page_table.remap(virt, new_phys);
        page_table.set_flags(virt, flags);
    }

    Ok(())
}

// ============================================================================
// TESTS
// ============================================================================

fn main() {
    init_mock_allocator();
    
    let mut passed = 0;
    let failed = 0;

    // Test 9: Workflow complet fork() + write
    {
        let mut cow_manager = CowManager::new();
        let mut parent_pt = PageTable::new();

        // Parent: allouer et mapper 2 pages
        let page1_phys = allocate_frame().unwrap();
        let page2_phys = allocate_frame().unwrap();

        // Écrire pattern dans les pages
        unsafe {
            let ptr1 = page1_phys.value() as *mut u8;
            let ptr2 = page2_phys.value() as *mut u8;
            for i in 0..PAGE_SIZE {
                *ptr1.add(i) = 0xAA;
                *ptr2.add(i) = 0xBB;
            }
        }

        // Mapper dans parent (RW)
        let virt1 = VirtualAddress::new(0x400000);
        let virt2 = VirtualAddress::new(0x401000);
        let flags_rw = UserPageFlags::empty().present().user().writable();

        parent_pt.map(virt1, page1_phys, flags_rw);
        parent_pt.map(virt2, page2_phys, flags_rw);

        // FORK: cloner address space
        let parent_pages = vec![
            (virt1, page1_phys, flags_rw),
            (virt2, page2_phys, flags_rw),
        ];

        let child_pages = cow_manager.clone_address_space(&parent_pages).unwrap();

        // Créer page table child
        let mut child_pt = PageTable::new();
        for (virt, phys, flags) in child_pages {
            child_pt.map(virt, phys, flags);
        }

        // Vérifier que child partage les pages physiques
        assert_eq!(child_pt.get_physical(virt1), Some(page1_phys),
                   "❌ Test 9.1 failed: child should share page1");
        assert_eq!(child_pt.get_physical(virt2), Some(page2_phys),
                   "❌ Test 9.2 failed: child should share page2");

        // Vérifier que les pages sont read-only
        assert!(!child_pt.get_flags(virt1).unwrap().contains_writable(),
                "❌ Test 9.3 failed: page1 should be read-only");

        // Vérifier refcount=2
        assert_eq!(cow_manager.get_refcount(page1_phys), Some(2),
                   "❌ Test 9.4 failed: refcount should be 2");

        // WRITE dans child → page fault
        let result = handle_cow_page_fault(&mut child_pt, &mut cow_manager, virt1);
        assert!(result.is_ok(), "❌ Test 9.5 failed: page fault handler failed");

        // Vérifier que child a maintenant une copie privée
        let child_page1 = child_pt.get_physical(virt1).unwrap();
        assert_ne!(child_page1, page1_phys,
                   "❌ Test 9.6 failed: child should have private copy");

        // Vérifier que la copie a le même contenu
        unsafe {
            let original_ptr = page1_phys.value() as *const u8;
            let copy_ptr = child_page1.value() as *const u8;
            
            for i in 0..PAGE_SIZE {
                let original_val = *original_ptr.add(i);
                let copy_val = *copy_ptr.add(i);
                assert_eq!(original_val, copy_val,
                           "❌ Test 9.7 failed: copy content mismatch at offset {}", i);
            }
        }

        // Vérifier que page est maintenant writable
        assert!(child_pt.get_flags(virt1).unwrap().contains_writable(),
                "❌ Test 9.8 failed: page1 should be writable after CoW");

        // Vérifier refcount décrémenté
        assert_eq!(cow_manager.get_refcount(page1_phys), Some(1),
                   "❌ Test 9.9 failed: refcount should be 1 after CoW");

        // Cleanup
        deallocate_frame(page1_phys);
        deallocate_frame(page2_phys);
        deallocate_frame(child_page1);

        println!("✅ Test 9: Workflow fork() + write + CoW - PASSED");
        passed += 1;
    }

    // Test 10: Page fault avec refcount=1 (optimisation)
    {
        let mut cow_manager = CowManager::new();
        let mut page_table = PageTable::new();

        let phys = allocate_frame().unwrap();
        let virt = VirtualAddress::new(0x500000);
        let flags = UserPageFlags::empty().present().user();

        // Marquer CoW (refcount=2)
        cow_manager.mark_cow(phys);

        // Décrémenter à 1
        cow_manager.decrement(phys);

        page_table.map(virt, phys, flags);

        // Page fault avec refcount=1 → pas de copie
        let result = handle_cow_page_fault(&mut page_table, &mut cow_manager, virt);
        assert!(result.is_ok(), "❌ Test 10.1 failed");

        // Vérifier même adresse physique
        assert_eq!(page_table.get_physical(virt), Some(phys),
                   "❌ Test 10.2 failed: should keep same physical address");

        // Vérifier writable
        assert!(page_table.get_flags(virt).unwrap().contains_writable(),
                "❌ Test 10.3 failed: should be writable");

        // Vérifier plus CoW
        assert!(!cow_manager.is_cow(phys),
                "❌ Test 10.4 failed: should not be CoW anymore");

        deallocate_frame(phys);

        println!("✅ Test 10: Page fault refcount=1 optimisation - PASSED");
        passed += 1;
    }

    // Résumé
    println!("");
    println!("════════════════════════════════════════════════════════════");
    println!("  RÉSULTATS PAGE FAULT + CoW INTEGRATION");
    println!("════════════════════════════════════════════════════════════");
    println!("  ✅ Passed: {}", passed);
    println!("  ❌ Failed: {}", failed);
    println!("  📊 Total:  {}", passed + failed);
    println!("════════════════════════════════════════════════════════════");

    if failed == 0 {
        println!("\n🎉 TOUS LES TESTS D'INTÉGRATION PASSÉS");
        std::process::exit(0);
    } else {
        println!("\n⚠️  ÉCHECS DÉTECTÉS");
        std::process::exit(1);
    }
}
EOF

echo "📝 Compilation..."
rustc /tmp/test_page_fault_cow.rs -o /tmp/test_page_fault_cow 2>&1

if [ $? -eq 0 ]; then
    echo "✅ Compilation réussie"
    echo ""
    echo "🧪 Exécution des tests..."
    echo ""
    /tmp/test_page_fault_cow
    exit_code=$?
    
    echo ""
    if [ $exit_code -eq 0 ]; then
        echo "════════════════════════════════════════════════════════════"
        echo "  ✅ VALIDATION PAGE FAULT + CoW COMPLÈTE"
        echo "════════════════════════════════════════════════════════════"
    fi
    
    exit $exit_code
else
    echo "❌ Erreur de compilation"
    exit 1
fi
