# Index global des audits noyau (Exo-OS)

Date: 2026-03-22
Périmètre: `arch`, `memory`, `scheduler`, `ipc`, `fs`, `security`, `exophoenix`

---

## 1) Livrables d’audit générés

- Principaux:
	- `docs/Audite/AUDIT_ARCH_2026-03-22.md` (513 lignes)
	- `docs/Audite/AUDIT_MEMORY_2026-03-22.md` (512 lignes)
	- `docs/Audite/AUDIT_SCHEDULER_2026-03-22.md` (508 lignes)
	- `docs/Audite/AUDIT_IPC_2026-03-22.md` (514 lignes)
	- `docs/Audite/AUDIT_FS_2026-03-22.md` (726 lignes)
	- `docs/Audite/AUDIT_SECURITY_2026-03-22.md` (508 lignes)
	- `docs/Audite/AUDIT_EXOPHOENIX_2026-03-22.md` (544 lignes)

- Secondaires:
	- `docs/Audite/AUDIT_ARCH_SECONDARY_2026-03-22.md` (500 lignes)
	- `docs/Audite/AUDIT_MEMORY_SECONDARY_2026-03-22.md` (505 lignes)
	- `docs/Audite/AUDIT_SCHEDULER_SECONDARY_2026-03-22.md` (521 lignes)
	- `docs/Audite/AUDIT_IPC_SECONDARY_2026-03-22.md` (502 lignes)
	- `docs/Audite/AUDIT_FS_SECONDARY_2026-03-22.md` (290 lignes)
	- `docs/Audite/AUDIT_SECURITY_SECONDARY_2026-03-22.md` (513 lignes)
	- `docs/Audite/AUDIT_EXOPHOENIX_SECONDARY_2026-03-22.md` (473 lignes)

Total principaux: 3825 lignes.
Total secondaires: 3304 lignes.
Total global documentation modules: 7129 lignes.

---

## 2) Compteurs consolidés par module (principal + secondaire)

- `ARCH`: principal 513 + secondaire 500 = **1013**
- `MEMORY`: principal 512 + secondaire 505 = **1017**
- `SCHEDULER`: principal 508 + secondaire 521 = **1029**
- `IPC`: principal 514 + secondaire 502 = **1016**
- `FS`: principal 726 + secondaire 290 = **1016**
- `SECURITY`: principal 508 + secondaire 513 = **1021**
- `EXOPHOENIX`: principal 544 + secondaire 473 = **1017**

Total global (somme des modules): **7129** lignes.

---

## 3) Résumé quantitatif des fichiers source audités

- `kernel/src/arch/**`: 64 fichiers
- `kernel/src/memory/**`: 128 fichiers
- `kernel/src/scheduler/**`: 43 fichiers
- `kernel/src/ipc/**`: 54 fichiers
- `kernel/src/fs/**`: 289 fichiers
- `kernel/src/security/**`: 44 fichiers
- `kernel/src/exophoenix/**`: 8 fichiers

Complément d’intégration ExoPhoenix hors module: 6 fichiers (`kernel/src/lib.rs`, `arch/x86_64/{idt.rs,exceptions.rs,tss.rs,boot/memory_map.rs,apic/io_apic.rs}`).

Total périmètre audité (modules): 630 fichiers source.

---

## 4) Index des fichiers audités (exhaustif)

### MODULE: arch
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\time.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\aarch64\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\exceptions.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\gdt.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\idt.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\memory_iface.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\paging.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\sched_iface.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\syscall.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\tss.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\vga_early.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\acpi\hpet.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\acpi\madt.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\acpi\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\acpi\parser.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\acpi\pm_timer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\apic\io_apic.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\apic\ipi.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\apic\local_apic.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\apic\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\apic\x2apic.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\early_init.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\memory_map.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\multiboot2.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\trampoline_asm.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\uefi.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\cpu\features.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\cpu\fpu.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\cpu\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\cpu\msr.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\cpu\topology.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\cpu\tsc.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\smp\hotplug.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\smp\init.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\smp\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\smp\percpu.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\spectre\ibrs.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\spectre\kpti.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\spectre\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\spectre\retpoline.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\spectre\ssbd.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\ktime.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\calibration\cpuid_nominal.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\calibration\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\calibration\validation.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\calibration\window.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\drift\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\drift\periodic.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\drift\pll.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\percpu\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\percpu\sync.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\sources\hpet.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\sources\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\sources\pit.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\sources\pm_timer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\time\sources\tsc.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\virt\detect.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\virt\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\virt\paravirt.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\virt\stolen_time.rs

### MODULE: memory
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\arch_iface.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\numa.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\core\address.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\core\constants.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\core\layout.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\core\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\core\types.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\cow\breaker.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\cow\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\cow\tracker.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\channels\affinity.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\channels\channel.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\channels\manager.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\channels\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\channels\priority.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\completion\handler.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\completion\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\completion\polling.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\completion\wakeup.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\core\descriptor.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\core\error.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\core\mapping.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\core\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\core\types.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\core\wakeup_iface.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\engines\ahci_dma.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\engines\idxd.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\engines\ioat.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\engines\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\engines\nvme_dma.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\engines\virtio_dma.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\iommu\amd_iommu.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\iommu\arm_smmu.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\iommu\domain.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\iommu\intel_vtd.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\iommu\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\iommu\page_table.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\ops\cyclic.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\ops\interleaved.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\ops\memcpy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\ops\memset.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\ops\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\ops\scatter_gather.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\stats\counters.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\dma\stats\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\allocator\global.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\allocator\hybrid.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\allocator\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\allocator\size_classes.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\large\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\large\vmalloc.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\thread_local\cache.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\thread_local\drain.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\thread_local\magazine.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\heap\thread_local\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\huge_pages\hugetlbfs.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\huge_pages\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\huge_pages\split.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\huge_pages\thp.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\integrity\canary.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\integrity\guard_pages.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\integrity\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\integrity\sanitizer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\stats.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\bitmap.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\buddy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\numa_aware.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\numa_hints.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\slab.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\allocator\slub.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\frame\descriptor.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\frame\emergency_pool.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\frame\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\frame\pool.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\frame\reclaim.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\frame\ref_count.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\numa\distance.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\numa\migration.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\numa\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\numa\node.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\numa\policy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\zone\dma.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\zone\dma32.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\zone\high.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\zone\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\zone\movable.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\physical\zone\normal.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\protection\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\protection\nx.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\protection\pku.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\protection\smap.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\protection\smep.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\swap\backend.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\swap\cluster.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\swap\compress.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\swap\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\swap\policy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\utils\futex_table.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\utils\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\utils\oom_killer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\utils\shrinker.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\mmap.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\address_space\kernel.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\address_space\mapper.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\address_space\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\address_space\tlb.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\address_space\user.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\fault\cow.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\fault\demand_paging.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\fault\handler.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\fault\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\fault\swap_in.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\page_table\builder.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\page_table\kpti_split.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\page_table\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\page_table\walker.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\page_table\x86_64.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\vma\cow.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\vma\descriptor.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\vma\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\vma\operations.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\memory\virtual\vma\tree.rs

### MODULE: scheduler
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\asm\fast_path.s
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\asm\switch_asm.s
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\core\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\core\pick_next.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\core\preempt.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\core\runqueue.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\core\switch.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\core\task.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\energy\c_states.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\energy\frequency.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\energy\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\energy\power_profile.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\fpu\lazy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\fpu\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\fpu\save_restore.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\fpu\state.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\policies\cfs.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\policies\deadline.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\policies\idle.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\policies\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\policies\realtime.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\smp\affinity.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\smp\load_balance.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\smp\migration.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\smp\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\smp\topology.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\stats\latency.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\stats\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\stats\per_cpu.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\barrier.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\condvar.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\mutex.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\rwlock.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\seqlock.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\spinlock.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\sync\wait_queue.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\timer\clock.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\timer\deadline_timer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\timer\hrtimer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\timer\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\scheduler\timer\tick.rs

### MODULE: ipc
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\async.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\broadcast.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\mpmc.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\raw.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\streaming.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\sync.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\channel\typed.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\core\constants.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\core\fastcall_asm.s
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\core\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\core\sequence.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\core\transfer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\core\types.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\endpoint\connection.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\endpoint\descriptor.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\endpoint\lifecycle.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\endpoint\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\endpoint\registry.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\message\builder.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\message\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\message\priority.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\message\router.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\message\serializer.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\batch.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\fusion.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\mpmc.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\slot.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\spsc.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\ring\zerocopy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\rpc\client.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\rpc\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\rpc\protocol.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\rpc\raw.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\rpc\server.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\rpc\timeout.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\allocator.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\descriptor.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\mapping.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\numa_aware.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\page.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\shared_memory\pool.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\stats\counters.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\stats\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\barrier.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\event.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\futex.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\rendezvous.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\sched_hooks.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\ipc\sync\wait_queue.rs

### MODULE: fs
Voir la liste exhaustive complète dans:
- `docs/Audite/AUDIT_FS_2026-03-22.md` section « Arborescence exhaustive du module fs ».
(289 fichiers déjà listés unitairement dans ce document de module.)

### MODULE: security
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\access_control\checker.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\access_control\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\access_control\object_types.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\audit\logger.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\audit\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\audit\rules.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\audit\syscall_audit.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\delegation.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\namespace.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\revocation.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\rights.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\table.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\token.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\capability\verify.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\aes_gcm.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\blake3.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\ed25519.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\kdf.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\rng.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\x25519.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\crypto\xchacha20_poly1305.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\exploit_mitigations\cet.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\exploit_mitigations\cfg.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\exploit_mitigations\kaslr.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\exploit_mitigations\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\exploit_mitigations\safe_stack.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\exploit_mitigations\stack_protector.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\integrity_check\code_signing.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\integrity_check\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\integrity_check\runtime_check.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\integrity_check\secure_boot.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\isolation\domains.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\isolation\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\isolation\namespaces.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\isolation\pledge.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\isolation\sandbox.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\zero_trust\context.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\zero_trust\labels.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\zero_trust\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\zero_trust\policy.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\security\zero_trust\verify.rs

### MODULE: exophoenix
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\mod.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\ssr.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\stage0.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\interrupts.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\sentinel.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\handoff.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\forge.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\exophoenix\isolate.rs

### INTÉGRATION EXOPHOENIX (hors module)
C:\Users\xavie\Desktop\Exo-OS\kernel\src\lib.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\idt.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\exceptions.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\tss.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\boot\memory_map.rs
C:\Users\xavie\Desktop\Exo-OS\kernel\src\arch\x86_64\apic\io_apic.rs

---

## 5) Notes de cohérence

- Les 7 audits principaux sont ≥ 500 lignes chacun.
- Les 7 audits secondaires complètent la couverture « fichiers secondaires ».
- Chaque module atteint désormais ≥ 1000 lignes (principal + secondaire).
- Les listes exhaustives module sont intégrées dans les audits dédiés.
- Le présent index centralise les livrables et l’inventaire global mis à jour.
