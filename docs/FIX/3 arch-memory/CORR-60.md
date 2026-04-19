# CORR-60 — KPTI : user_pml4 jamais allouée → triple fault au premier IRETQ Ring3 (CRIT-10)

**Sévérité :** 🔴 CRITIQUE — Triple fault au premier retour userspace  
**Fichiers :** `kernel/src/arch/x86_64/spectre/kpti.rs`, `kernel/src/memory/virtual/page_table/kpti_split.rs`  
**Impact :** `kpti_switch_to_user()` charge CR3=0 → triple fault immédiat

---

## Problème

```rust
// kpti.rs:92–115
pub fn init_kpti() {
    // ... initialise KPTI_ENABLED ...
    KPTI_ENABLED.store(true, Ordering::Release);  // ← activé
    // MAIS : KptiTable::register_cpu() n'est jamais appelé !
    // → states[cpu_id].user_pml4 = PhysAddr::NULL pour tous les CPUs
}

// kpti_split.rs:103
pub fn switch_to_user(&self, cpu_id: usize) {
    let pml4 = self.states[cpu_id].user_pml4;
    // pml4 = PhysAddr::NULL (0x0) car register_cpu() jamais appelé
    unsafe { write_cr3(pml4); }  // → CR3 = 0 → triple fault
}
```

---

## Correction

### Étape 1 — Créer la shadow PML4 user dans `init_kpti()`

```rust
// kernel/src/arch/x86_64/spectre/kpti.rs

pub fn init_kpti() {
    use crate::memory::physical::frame_allocator::{alloc_frame, AllocFlags};
    use crate::memory::core::PHYS_MAP_BASE;
    use crate::arch::x86_64::cpu::topology::MAX_CPUS;

    // Allouer et initialiser la shadow PML4 user pour chaque CPU.
    // La shadow PML4 ne contient que :
    //   - Le trampoline de switch (partagé, 1 entrée PML4)
    //   - Les mappings userspace du processus courant (gérés par le scheduler)
    // Elle N'a PAS les mappings kernel → protection KPTI effective.

    let cpu_count = crate::arch::x86_64::smp::init::smp_cpu_count() as usize;
    let cpu_count = cpu_count.min(MAX_CPUS);

    for cpu_id in 0..cpu_count {
        // Allouer une frame physique pour la shadow PML4.
        let user_pml4_frame = match alloc_frame(AllocFlags::ZEROED | AllocFlags::KERNEL) {
            Some(f) => f,
            None => {
                // Allocation échouée → désactiver KPTI pour ce CPU.
                // Logger l'échec mais ne pas paniquer (dégradation gracieuse).
                crate::logger::warn!(
                    "KPTI: failed to alloc user_pml4 for CPU {} — KPTI disabled",
                    cpu_id
                );
                continue;
            }
        };

        // Allouer le trampoline physique (page partagée entre kernel et user PML4).
        let trampoline_frame = match alloc_frame(AllocFlags::ZEROED | AllocFlags::KERNEL) {
            Some(f) => f,
            None => {
                crate::logger::warn!("KPTI: failed to alloc trampoline for CPU {}", cpu_id);
                continue;
            }
        };

        // Récupérer le CR3 kernel courant.
        let kernel_pml4 = crate::arch::x86_64::registers::cr3_read_phys();

        // Copier les entrées userspace (PML4 indices 0–255) depuis le kernel PML4.
        // Les entrées kernel (indices 256–511) ne sont PAS copiées.
        copy_user_pml4_entries(kernel_pml4, user_pml4_frame.phys_addr());

        // Installer le trampoline dans les deux PML4 (même entrée PML4).
        install_trampoline(kernel_pml4, user_pml4_frame.phys_addr(), trampoline_frame.phys_addr());

        // Enregistrer dans KptiTable.
        // SAFETY: cpu_id < MAX_CPUS, tous les PhysAddr sont valides et non-nuls.
        unsafe {
            KPTI.register_cpu(
                cpu_id,
                kernel_pml4,
                user_pml4_frame.phys_addr(),
                trampoline_frame.phys_addr(),
            );
        }
    }

    // N'activer KPTI que si au moins un CPU a une shadow PML4 valide.
    if KPTI.any_cpu_registered() {
        KPTI_ENABLED.store(true, Ordering::Release);
        crate::logger::info!("KPTI: enabled for {} CPUs", cpu_count);
    } else {
        crate::logger::warn!("KPTI: disabled — no CPU could allocate shadow PML4");
    }
}

/// Copie les entrées PML4 de la zone userspace (indices 0–255)
/// depuis la kernel PML4 vers la shadow user PML4.
fn copy_user_pml4_entries(
    kernel_pml4: crate::memory::core::PhysAddr,
    user_pml4: crate::memory::core::PhysAddr,
) {
    use crate::memory::core::PHYS_MAP_BASE;
    let k_virt = (PHYS_MAP_BASE.as_u64() + kernel_pml4.as_u64()) as *const u64;
    let u_virt = (PHYS_MAP_BASE.as_u64() + user_pml4.as_u64()) as *mut u64;

    // Copier uniquement les 256 premières entrées (espace user, < 0x8000_0000_0000).
    for i in 0..256usize {
        unsafe {
            u_virt.add(i).write_volatile(k_virt.add(i).read_volatile());
        }
    }
    // Laisser les entrées 256–511 (kernel) à zéro dans la shadow user PML4.
}
```

### Étape 2 — Ajouter `any_cpu_registered()` à `KptiTable`

```rust
// kernel/src/memory/virtual/page_table/kpti_split.rs

impl KptiTable {
    /// Retourne true si au moins un CPU a une shadow PML4 non nulle.
    pub fn any_cpu_registered(&self) -> bool {
        self.states.iter().any(|s| s.kernel_pml4.as_u64() != 0)
    }
}
```

### Étape 3 — Guard dans `switch_to_user()` (défense en profondeur)

```rust
// kernel/src/memory/virtual/page_table/kpti_split.rs:103

pub fn switch_to_user(&self, cpu_id: usize) {
    let pml4 = self.states[cpu_id].user_pml4;
    
    // GUARD: ne jamais charger CR3=NULL — provoquerait un triple fault.
    if pml4.as_u64() == 0 {
        // KPTI non initialisé pour ce CPU — continuer avec le CR3 kernel.
        // C'est une dégradation de sécurité (pas d'isolation KPTI) mais pas un crash.
        return;
    }
    
    unsafe { write_cr3(pml4); }
}
```

### Étape 4 — Appel de `init_kpti()` dans la séquence de boot

```rust
// kernel/src/boot/stage0.rs ou init sequence (étape 12) — VÉRIFIER QUE :
// init_kpti() est appelé APRÈS :
//   - init de la physmap (étape 11)
//   - init de l'allocateur physique de frames
//   - setup SMP (pour connaître cpu_count)
// Et AVANT le premier IRETQ vers Ring3.

// Exemple d'appel dans la séquence boot :
// ...
// step_11_init_physmap();
// step_12_init_kpti(); // ← ajouter ici si absent
// step_13_start_ring3_servers();
```

---

## Vérification

```bash
# Vérifier que register_cpu est appelé
grep -rn "register_cpu" kernel/src/arch/x86_64/spectre/
# Doit retourner au moins un appel dans init_kpti()

# Vérifier qu'aucun CR3=0 n'est possible
grep -n "write_cr3\|PhysAddr::NULL" kernel/src/memory/virtual/page_table/kpti_split.rs
```

---

**Priorité :** Triple fault garanti au premier IRETQ vers Ring3  
**Dépendances :** Allocateur de frames physiques opérationnel (étape 11 boot)
