# Audit approfondi du module `arch` (Exo-OS)

Date: 2026-03-22
Périmètre: `kernel/src/arch/**`
Objectif: base de refonte/correction du kernel
Niveau: approfondi, orienté architecture + risques + points durs no_std

---

## 1) Résumé exécutif

Le module `arch` est la couche transverse la plus sensible du noyau.
Il concentre les points d’entrée CPU/ASM critiques.
Il pilote le boot, les exceptions, les syscalls, APIC/SMP, et la base temporelle.
Il expose des primitives utilisées ensuite par `memory`, `scheduler`, `process`, `ipc`, `security`, `fs`.
La qualité de cette couche conditionne la stabilité globale.

Forces observées:
- séparation claire `arch/mod.rs` puis `arch/x86_64/*`
- discipline `// SAFETY:` présente dans de nombreux chemins
- bootstrap structuré (`early_init`)
- gestion explicite des barrières mémoire (`mfence/lfence/sfence`)
- points de contrôle pour TLB/CR3/KPTI

Fragilités observées:
- surface `unsafe` très large
- dépendance forte au layout fixe GS/per-CPU
- zones TODO/stub persistantes (notamment AArch64 placeholder, HPET fixmap TODO)
- complexité de l’enchaînement boot -> ACPI -> APIC -> SMP
- coupling implicite avec scheduler/memory via interfaces arch

---

## 2) Positionnement dans l’architecture du kernel

`arch` n’est pas une couche métier.
`arch` sert d’adaptateur machine.
`arch` encapsule les détails ISA/CPU.
`arch` doit rester déterministe et minimal.
`arch` ne doit pas embarquer de logique applicative.
`arch` doit rester compatible no_std et early-boot.
`arch` doit maintenir la stabilité ABI interne ASM<->Rust.
`arch` doit préserver les invariants CR3/GS/IDT/TSS.
`arch` doit garantir la sûreté de l’entrée/sortie userspace.
`arch` est un multiplicateur de risque en cas de régression.

---

## 3) Cartographie exhaustive des fichiers

### 3.1 Fichiers racine

- `kernel/src/arch/mod.rs` — entrée du module architecture et `ArchInfo`.
- `kernel/src/arch/time.rs` — abstraction générique de lecture TSC côté arch.
- `kernel/src/arch/aarch64/mod.rs` — placeholder ARM64 (non finalisé).

### 3.2 Arborescence `x86_64`

- `kernel/src/arch/x86_64/mod.rs` — exports globaux x86_64.
- `kernel/src/arch/x86_64/memory_iface.rs` — pont arch<->memory.
- `kernel/src/arch/x86_64/sched_iface.rs` — pont arch<->scheduler.
- `kernel/src/arch/x86_64/paging.rs` — primitives paging x86_64.
- `kernel/src/arch/x86_64/syscall.rs` — entrée SYSCALL/SYSRET.
- `kernel/src/arch/x86_64/exceptions.rs` — handlers exceptions/interruptions.
- `kernel/src/arch/x86_64/gdt.rs` — GDT.
- `kernel/src/arch/x86_64/idt.rs` — IDT.
- `kernel/src/arch/x86_64/tss.rs` — TSS + IST.
- `kernel/src/arch/x86_64/vga_early.rs` — affichage debug early.

### 3.3 Dossier `boot`

- `kernel/src/arch/x86_64/boot/mod.rs` — agrégation boot.
- `kernel/src/arch/x86_64/boot/early_init.rs` — séquence d’initialisation BSP.
- `kernel/src/arch/x86_64/boot/memory_map.rs` — intake carte mémoire.
- `kernel/src/arch/x86_64/boot/multiboot2.rs` — parsing Multiboot2.
- `kernel/src/arch/x86_64/boot/trampoline_asm.rs` — trampoline AP.
- `kernel/src/arch/x86_64/boot/uefi.rs` — chemin UEFI.

### 3.4 Dossier `cpu`

- `kernel/src/arch/x86_64/cpu/mod.rs` — agrégation CPU.
- `kernel/src/arch/x86_64/cpu/features.rs` — CPUID/features flags.
- `kernel/src/arch/x86_64/cpu/fpu.rs` — primitives FPU asm brutes.
- `kernel/src/arch/x86_64/cpu/msr.rs` — accès MSR.
- `kernel/src/arch/x86_64/cpu/topology.rs` — topologie CPUs.
- `kernel/src/arch/x86_64/cpu/tsc.rs` — lecture/calibrage TSC.

### 3.5 Dossier `acpi`

- `kernel/src/arch/x86_64/acpi/mod.rs` — façade ACPI.
- `kernel/src/arch/x86_64/acpi/parser.rs` — parser tables ACPI.
- `kernel/src/arch/x86_64/acpi/madt.rs` — MADT.
- `kernel/src/arch/x86_64/acpi/hpet.rs` — HPET.
- `kernel/src/arch/x86_64/acpi/pm_timer.rs` — PM timer.

### 3.6 Dossier `apic`

- `kernel/src/arch/x86_64/apic/mod.rs` — façade APIC.
- `kernel/src/arch/x86_64/apic/local_apic.rs` — LAPIC.
- `kernel/src/arch/x86_64/apic/io_apic.rs` — IOAPIC.
- `kernel/src/arch/x86_64/apic/x2apic.rs` — x2APIC.
- `kernel/src/arch/x86_64/apic/ipi.rs` — envoi IPI.

### 3.7 Dossier `smp`

- `kernel/src/arch/x86_64/smp/mod.rs` — façade SMP.
- `kernel/src/arch/x86_64/smp/init.rs` — boot APs.
- `kernel/src/arch/x86_64/smp/percpu.rs` — données per-CPU.
- `kernel/src/arch/x86_64/smp/hotplug.rs` — hotplug.

### 3.8 Dossier `spectre`

- `kernel/src/arch/x86_64/spectre/mod.rs` — agrégation mitigations.
- `kernel/src/arch/x86_64/spectre/kpti.rs` — split KPTI.
- `kernel/src/arch/x86_64/spectre/retpoline.rs` — retpoline.
- `kernel/src/arch/x86_64/spectre/ssbd.rs` — SSBD.
- `kernel/src/arch/x86_64/spectre/ibrs.rs` — IBRS/IBPB/STIBP.

### 3.9 Dossier `virt`

- `kernel/src/arch/x86_64/virt/mod.rs` — façade virtualisation.
- `kernel/src/arch/x86_64/virt/detect.rs` — détection hyperviseur.
- `kernel/src/arch/x86_64/virt/paravirt.rs` — hooks paravirt.
- `kernel/src/arch/x86_64/virt/stolen_time.rs` — stolen time.

### 3.10 Dossier `time`

- `kernel/src/arch/x86_64/time/mod.rs` — orchestration temporelle.
- `kernel/src/arch/x86_64/time/ktime.rs` — horloge monotone type seqlock.
- `kernel/src/arch/x86_64/time/calibration/mod.rs` — calibration front.
- `kernel/src/arch/x86_64/time/calibration/window.rs` — calibration window.
- `kernel/src/arch/x86_64/time/calibration/validation.rs` — checks calibration.
- `kernel/src/arch/x86_64/time/calibration/cpuid_nominal.rs` — fallback CPUID.
- `kernel/src/arch/x86_64/time/sources/mod.rs` — registry clock sources.
- `kernel/src/arch/x86_64/time/sources/tsc.rs` — source TSC.
- `kernel/src/arch/x86_64/time/sources/hpet.rs` — source HPET.
- `kernel/src/arch/x86_64/time/sources/pit.rs` — source PIT.
- `kernel/src/arch/x86_64/time/sources/pm_timer.rs` — source PM timer.
- `kernel/src/arch/x86_64/time/drift/mod.rs` — drift manager.
- `kernel/src/arch/x86_64/time/drift/pll.rs` — PLL de correction.
- `kernel/src/arch/x86_64/time/drift/periodic.rs` — correction périodique.
- `kernel/src/arch/x86_64/time/percpu/mod.rs` — horloge per-CPU.
- `kernel/src/arch/x86_64/time/percpu/sync.rs` — sync offsets per-CPU.

---

## 4) API publique et symboles structurants

`arch/mod.rs` expose:
- `ArchInfo`
- `read_tsc`
- `CpuFeatures`
- `CPU_FEATURES`
- `halt_cpu`

`arch/x86_64/mod.rs` expose des primitives critiques:
- `memory_barrier()`
- `load_fence()`
- `store_fence()`
- `invlpg(virt_addr)`
- `read_cr3()`
- `write_cr3(cr3_val)`
- `read_cr2()`
- `read_cr4()`
- `write_cr4(cr4_val)`
- `enable_interrupts()`
- `disable_interrupts()`
- `read_rflags()`
- `irq_save()`
- `irq_restore(flags)`
- `outb/inb/outl/inl`
- `io_delay()`

`arch/x86_64/syscall.rs`:
- `init_syscall()`
- `syscall_rust_handler(frame)`
- `SyscallFrame`
- `SYSCALL_*` constants

---

## 5) État fonctionnel et risques de refonte

État global:
- module globalement opérationnel sur x86_64
- architecture AArch64 non implémentée
- séquence boot robuste mais sensible à l’ordre

Risques majeurs:
- risque ABI ASM/Rust sur layout `SyscallFrame`
- risque GS layout (`percpu.rs`) en cas de changement offsets
- risque CR3/KPTI si ordre swapgs/switch incorrect
- risque de hard fault si adresse SYSRET non canonique
- risque timer si calibration source invalide

Contraintes no_std:
- pas d’allocation dynamique en early boot critique
- pas de dépendance std
- robustesse sans logging riche en tout début de boot

---

## 6) Placeholders / TODO / stubs détectés

- `arch/aarch64/mod.rs` — placeholder complet du port ARM64.
- `arch/x86_64/acpi/hpet.rs` — TODO remap fixmap bare-metal.
- `arch/x86_64/vga_early.rs` — TODO calibration HPET affiché.
- `arch/x86_64/boot/trampoline_asm.rs` — placeholder AP patch runtime.

Impacts:
- portabilité limitée.
- calibration potentiellement non optimale.
- instrumentation boot partiellement manuelle.

---

## 7) Synchronisation, verrous, atomiques

Patterns observés:
- heavy atomics pour états CPU/time.
- seqlock-like logique sur time/ktime.
- absence volontaire de mutex dans hot paths ISR.
- barrière mémoire explicite pour ordering.

Points de vigilance:
- ordre memory barriers lors des handoffs.
- cohérence cross-CPU pour flags init.
- non-réentrance de chemins early.

---

## 8) Usage de `&str` et interfaces texte

Présence de `&str` notable dans:
- `vga_early.rs` (rendu status boot)
- `tss.rs` (messages d’erreur)
- `exceptions.rs` (panic messages)
- `acpi/parser.rs` (signature string)

Enjeu refonte:
- éviter conversions coûteuses en hot path
- conserver format minimal pour diagnostics précoces

---

## 9) Crates/imports majeurs observés

Crates noyau/standard core:
- `core`
- `alloc` (selon zones)

Imports structurels fréquents:
- `core::sync::atomic::*`
- `core::arch::asm`
- `crate::arch::x86_64::cpu::*`
- `crate::memory::*` (via interfaces)
- `crate::scheduler::*` (via interface dédiée)

---

## 10) Journal de contrôle détaillé (ARCH-CHK)

- ARCH-CHK-001 vérifier invariant `PAGE_SIZE=4096` conservé.
- ARCH-CHK-002 vérifier cohérence `KERNEL_BASE` avec linker script.
- ARCH-CHK-003 vérifier absence de logique métier dans `arch/mod.rs`.
- ARCH-CHK-004 vérifier stabilité ABI `ArchInfo`.
- ARCH-CHK-005 vérifier gating target `x86_64` correct.
- ARCH-CHK-006 vérifier gating target `aarch64` placeholder explicite.
- ARCH-CHK-007 vérifier usage `halt_cpu()` uniquement panic/idle.
- ARCH-CHK-008 vérifier `memory_barrier()` non remplacé par no-op.
- ARCH-CHK-009 vérifier `invlpg` utilisé uniquement adresses valides.
- ARCH-CHK-010 vérifier lecture/écriture CR3 conditionnée contexte ring0.
- ARCH-CHK-011 vérifier lecture CR2 uniquement en traitement fautes.
- ARCH-CHK-012 vérifier `irq_save/irq_restore` appariés.
- ARCH-CHK-013 vérifier port I/O appels encapsulés.
- ARCH-CHK-014 vérifier `io_delay` non utilisé en boucle abusive.
- ARCH-CHK-015 vérifier init features CPU avant usage APIC.
- ARCH-CHK-016 vérifier GDT avant IDT dans séquence boot.
- ARCH-CHK-017 vérifier TSS/IST init avant activation interruptions.
- ARCH-CHK-018 vérifier per-CPU BSP avant SMP boot AP.
- ARCH-CHK-019 vérifier calibrage TSC après base timer disponible.
- ARCH-CHK-020 vérifier hypervisor detect non bloquant.
- ARCH-CHK-021 vérifier parser ACPI robustesse RSDP absent.
- ARCH-CHK-022 vérifier parse MADT avant boot AP.
- ARCH-CHK-023 vérifier HPET init conditionnelle et sûre.
- ARCH-CHK-024 vérifier PM timer fallback disponible.
- ARCH-CHK-025 vérifier APIC system init avant IOAPIC route.
- ARCH-CHK-026 vérifier recalibrage timer LAPIC après TSC.
- ARCH-CHK-027 vérifier pont memory integration exactement une fois.
- ARCH-CHK-028 vérifier init syscall après segments prêts.
- ARCH-CHK-029 vérifier mitigations spectre appliquées BSP.
- ARCH-CHK-030 vérifier traitement Multiboot2 robuste magic invalide.
- ARCH-CHK-031 vérifier chemin exo-boot UEFI cohérent.
- ARCH-CHK-032 vérifier emergency_pool init très tôt.
- ARCH-CHK-033 vérifier KERNEL_AS.init avec CR3 courant.
- ARCH-CHK-034 vérifier protection memory init post-map.
- ARCH-CHK-035 vérifier trampoline AP installé avant SIPI.
- ARCH-CHK-036 vérifier exclusion BSP du boot AP.
- ARCH-CHK-037 vérifier comptage CPU final cohérent.
- ARCH-CHK-038 vérifier trace E9 de phase boot conservée.
- ARCH-CHK-039 vérifier `SyscallFrame` alignement et ordre push.
- ARCH-CHK-040 vérifier `syscall_entry_asm` swapgs entrée/sortie.
- ARCH-CHK-041 vérifier sauvegarde `user_rsp` gs slot stable.
- ARCH-CHK-042 vérifier alignement pile avant call Rust handler.
- ARCH-CHK-043 vérifier restauration registres ordre inverse exact.
- ARCH-CHK-044 vérifier `sysretq` seulement adresses canoniques.
- ARCH-CHK-045 vérifier fallback non-canonique vers rcx=0.
- ARCH-CHK-046 vérifier `init_syscall` EFER.SCE activé.
- ARCH-CHK-047 vérifier STAR/LSTAR/CSTAR correctement initialisés.
- ARCH-CHK-048 vérifier SFMASK masque IF/TF/DF/AC.
- ARCH-CHK-049 vérifier compteur syscall atomique non overflow critique.
- ARCH-CHK-050 vérifier code CSTAR renvoie ENOSYS.
- ARCH-CHK-051 vérifier exceptions stubs couvrent vecteurs critiques.
- ARCH-CHK-052 vérifier gestion #PF route vers memory fault handler.
- ARCH-CHK-053 vérifier gestion #GP/#DF avec diagnostics minimaux.
- ARCH-CHK-054 vérifier panic path sans allocation.
- ARCH-CHK-055 vérifier EOI APIC envoyé après ISR appropriées.
- ARCH-CHK-056 vérifier route IRQ évite vecteurs réservés.
- ARCH-CHK-057 vérifier local APIC mode xAPIC/x2APIC cohérent.
- ARCH-CHK-058 vérifier IPI send with delivery mode correct.
- ARCH-CHK-059 vérifier smp percpu offsets documentés immuables.
- ARCH-CHK-060 vérifier GS layout partagé ASM/Rust synchronisé.
- ARCH-CHK-061 vérifier `cpu/features` gating RDTSCP utilisé.
- ARCH-CHK-062 vérifier fallback TSC si RDTSCP absent.
- ARCH-CHK-063 vérifier gating RDRAND avant usage RNG.
- ARCH-CHK-064 vérifier MSR writes protègent bits non ciblés.
- ARCH-CHK-065 vérifier topologie CPU CPUID interprétation correcte.
- ARCH-CHK-066 vérifier ACPI parser signatures check strictes.
- ARCH-CHK-067 vérifier MADT entries inconnues ignorées proprement.
- ARCH-CHK-068 vérifier HPET counter read sérialisé.
- ARCH-CHK-069 vérifier PM timer wrap-around géré.
- ARCH-CHK-070 vérifier KPTI enable conditionnelle feature/boot.
- ARCH-CHK-071 vérifier CR3 user/kernel split valide.
- ARCH-CHK-072 vérifier retpoline symboles visibles asm.
- ARCH-CHK-073 vérifier SSBD/IBRS toggles dépendants CPU flags.
- ARCH-CHK-074 vérifier virt detect CPUID leaf hypervisor.
- ARCH-CHK-075 vérifier paravirt path fallback bare metal.
- ARCH-CHK-076 vérifier stolen_time seqlock pattern correct.
- ARCH-CHK-077 vérifier `time/mod.rs` orchestration stable.
- ARCH-CHK-078 vérifier `ktime` lecture monotone.
- ARCH-CHK-079 vérifier recalage drift périodique borné.
- ARCH-CHK-080 vérifier PLL correction non explosive.
- ARCH-CHK-081 vérifier offsets per-CPU time synchronisés.
- ARCH-CHK-082 vérifier PIT source fallback maintenu.
- ARCH-CHK-083 vérifier HPET source fallback maintenu.
- ARCH-CHK-084 vérifier PM timer source fallback maintenu.
- ARCH-CHK-085 vérifier UEFI path ne casse pas Multiboot path.
- ARCH-CHK-086 vérifier `boot/memory_map` bornes phys sans overflow.
- ARCH-CHK-087 vérifier régions memory libres validées.
- ARCH-CHK-088 vérifier pages APIC mappées correctement.
- ARCH-CHK-089 vérifier `trampoline_asm` patch AP correct.
- ARCH-CHK-090 vérifier boot stack top symbol résolu.
- ARCH-CHK-091 vérifier `cli/sti` usage borné sections critiques.
- ARCH-CHK-092 vérifier `irq_restore` restaure exact état précédent.
- ARCH-CHK-093 vérifier appels `enable_interrupts` post-IDT seulement.
- ARCH-CHK-094 vérifier `disable_interrupts` pas d’abus long.
- ARCH-CHK-095 vérifier exceptions user/kernel discrimination fiable.
- ARCH-CHK-096 vérifier livraisons signaux au bon point de retour.
- ARCH-CHK-097 vérifier absence allocation dans handlers exception.
- ARCH-CHK-098 vérifier absence lock bloquant dans ISR.
- ARCH-CHK-099 vérifier ports I/O constants pas hardcode dispersé.
- ARCH-CHK-100 vérifier temps de boot instrumenté stable.
- ARCH-CHK-101 vérifier docs GDT conformes descripteurs actuels.
- ARCH-CHK-102 vérifier docs IDT conformes vecteurs actuels.
- ARCH-CHK-103 vérifier docs TSS/IST conformes tailles stacks.
- ARCH-CHK-104 vérifier docs APIC conformes mode runtime.
- ARCH-CHK-105 vérifier docs syscall frame offsets exacts.
- ARCH-CHK-106 vérifier docs SMP sequence exacte.
- ARCH-CHK-107 vérifier docs KPTI alignées implémentation.
- ARCH-CHK-108 vérifier docs time calibration alignées implémentation.
- ARCH-CHK-109 vérifier docs paravirt alignées implémentation.
- ARCH-CHK-110 vérifier docs ACPI alignées implémentation.
- ARCH-CHK-111 vérifier docs TODO/stub maintenues à jour.
- ARCH-CHK-112 vérifier interface memory_iface non régressive.
- ARCH-CHK-113 vérifier interface sched_iface non régressive.
- ARCH-CHK-114 vérifier symboles re-export utilisés réellement.
- ARCH-CHK-115 vérifier dead code markers justifiés.
- ARCH-CHK-116 vérifier options asm (`nomem`,`nostack`) correctes.
- ARCH-CHK-117 vérifier use-after-swapgs impossible.
- ARCH-CHK-118 vérifier `syscall_rust_handler` pointeur frame valide.
- ARCH-CHK-119 vérifier `extern "C"` noms cohérents avec asm.
- ARCH-CHK-120 vérifier code cstar noop sûr.
- ARCH-CHK-121 vérifier valeur ENOSYS négative correcte.
- ARCH-CHK-122 vérifier numéros syscall max cohérents table globale.
- ARCH-CHK-123 vérifier traitement erreur dispatch.
- ARCH-CHK-124 vérifier robustesse contre syscalls inconnus.
- ARCH-CHK-125 vérifier audit/security hooks post-dispatch.
- ARCH-CHK-126 vérifier performance path sans branch inutiles.
- ARCH-CHK-127 vérifier branch canonique SYSRET très bon marché.
- ARCH-CHK-128 vérifier tests unitaires arch présents ou TODO explicités.
- ARCH-CHK-129 vérifier macros asm génératives lisibles.
- ARCH-CHK-130 vérifier appels msr encapsulés.
- ARCH-CHK-131 vérifier accès CR4 bits explicitement nommés.
- ARCH-CHK-132 vérifier static mut minimisés.
- ARCH-CHK-133 vérifier atomics ordering adaptés (Acquire/Release où requis).
- ARCH-CHK-134 vérifier Relaxed seulement sur compteurs non critiques.
- ARCH-CHK-135 vérifier trait arch_info complet.
- ARCH-CHK-136 vérifier cpu_count provenant percpu fiable.
- ARCH-CHK-137 vérifier APIC available flags exacts.
- ARCH-CHK-138 vérifier ACPI available flag exact.
- ARCH-CHK-139 vérifier fallback `cpu_count=1` cohérent.
- ARCH-CHK-140 vérifier test virtualisation n’introduit pas blocage.
- ARCH-CHK-141 vérifier support x2APIC si CPU le permet.
- ARCH-CHK-142 vérifier redirection IRQ vers IOAPIC sécurisée.
- ARCH-CHK-143 vérifier vecteurs réservés IPI protégés.
- ARCH-CHK-144 vérifier NMI path robuste même si TLB shootdown en cours.
- ARCH-CHK-145 vérifier #DF stack dédiée IST.
- ARCH-CHK-146 vérifier #PF path no recursion.
- ARCH-CHK-147 vérifier #MC path minimal/fatal.
- ARCH-CHK-148 vérifier panic handler imprime localisation.
- ARCH-CHK-149 vérifier alloc_error handler imprime size/align.
- ARCH-CHK-150 vérifier boucle halt finale sûre.
- ARCH-CHK-151 vérifier interface time::read_tsc unifiée.
- ARCH-CHK-152 vérifier calibrations ne dépendent pas timing Windows host.
- ARCH-CHK-153 vérifier run en VM KVM vs QEMU cohérent.
- ARCH-CHK-154 vérifier docs mentionnent limites Hyper-V/VMware.
- ARCH-CHK-155 vérifier code virt non exécuté hors hyperviseur.
- ARCH-CHK-156 vérifier mémoire MMIO mapping attributs corrects plus tard.
- ARCH-CHK-157 vérifier fixmap TODO tracké dans refonte.
- ARCH-CHK-158 vérifier build no_std pas de `std` import.
- ARCH-CHK-159 vérifier cfg target_arch exact sur tous fichiers.
- ARCH-CHK-160 vérifier aarch64 module clairement non supporté.
- ARCH-CHK-161 vérifier error messages anglais/fr cohérents.
- ARCH-CHK-162 vérifier logs E9 non trop verbeux en prod.
- ARCH-CHK-163 vérifier instrumentation optionnelle configurable.
- ARCH-CHK-164 vérifier timing path constant-ish pour sécurité.
- ARCH-CHK-165 vérifier speculation barriers aux frontières sensibles.
- ARCH-CHK-166 vérifier retpoline appliqué là où requis.
- ARCH-CHK-167 vérifier IBPB sur context switch processus si prévu.
- ARCH-CHK-168 vérifier STIBP selon SMT policy.
- ARCH-CHK-169 vérifier SSBD activation conditionnelle.
- ARCH-CHK-170 vérifier KPTI pénalité perf documentée.
- ARCH-CHK-171 vérifier PCID usage documenté (si activé).
- ARCH-CHK-172 vérifier CR3 reload strategy cohérente.
- ARCH-CHK-173 vérifier TLB global flush rare et justifié.
- ARCH-CHK-174 vérifier shootdown queue saturation traitée.
- ARCH-CHK-175 vérifier ack remote CPU timeout géré.
- ARCH-CHK-176 vérifier fallback single-core si SMP fail.
- ARCH-CHK-177 vérifier AP failure n’empêche pas BSP progression.
- ARCH-CHK-178 vérifier `smp/hotplug` garde-fous.
- ARCH-CHK-179 vérifier migration percpu data sur hotplug.
- ARCH-CHK-180 vérifier CPU offline n’est plus ciblé par IPI.
- ARCH-CHK-181 vérifier topology mise à jour atomique.
- ARCH-CHK-182 vérifier thread safety AP bring-up.
- ARCH-CHK-183 vérifier trampoline memory protections.
- ARCH-CHK-184 vérifier trampoline cleanup après init.
- ARCH-CHK-185 vérifier stack guards IST.
- ARCH-CHK-186 vérifier taille stacks IST suffisante.
- ARCH-CHK-187 vérifier appel `arch_boot_init` unique BSP.
- ARCH-CHK-188 vérifier AP init chemin séparé.
- ARCH-CHK-189 vérifier chrono boot phases documenté.
- ARCH-CHK-190 vérifier capture erreurs boot sans logger complet.
- ARCH-CHK-191 vérifier assert critiques restent actives debug.
- ARCH-CHK-192 vérifier release path remonte codes d’échec.
- ARCH-CHK-193 vérifier union/hardcoded offsets minimisés.
- ARCH-CHK-194 vérifier symboles global_asm correctement sectionnés.
- ARCH-CHK-195 vérifier alignements page/tables conformes 4KiB.
- ARCH-CHK-196 vérifier entrées page tables no overlap.
- ARCH-CHK-197 vérifier map identité temporaire retirée après init si requis.
- ARCH-CHK-198 vérifier mapping kernel haute adresse cohérent.
- ARCH-CHK-199 vérifier physmap base alignée architecture.
- ARCH-CHK-200 vérifier conversion phys<->virt sans overflow.
- ARCH-CHK-201 vérifier lecture ACPI sur mapping valide.
- ARCH-CHK-202 vérifier checksum ACPI validé.
- ARCH-CHK-203 vérifier MADT APIC IDs sans doublon.
- ARCH-CHK-204 vérifier HPET fréquence lue correctement.
- ARCH-CHK-205 vérifier PIT calibration fallback.
- ARCH-CHK-206 vérifier PM timer wrap et drift.
- ARCH-CHK-207 vérifier TSC invariance check.
- ARCH-CHK-208 vérifier non-invariant TSC fallback stratégie.
- ARCH-CHK-209 vérifier sync TSC multi-CPU.
- ARCH-CHK-210 vérifier `time/percpu/sync` couverture suffisante.
- ARCH-CHK-211 vérifier drift periodic cadence.
- ARCH-CHK-212 vérifier PLL bornes min/max.
- ARCH-CHK-213 vérifier freeze time sources en panic path.
- ARCH-CHK-214 vérifier absence deadlock dans handlers IRQ.
- ARCH-CHK-215 vérifier instrumentation stolen_time safe.
- ARCH-CHK-216 vérifier guest/host pause impact mesuré.
- ARCH-CHK-217 vérifier paravirt eoi/tlb flush correctness.
- ARCH-CHK-218 vérifier virt detect strings robustes.
- ARCH-CHK-219 vérifier cpuid leaf guards.
- ARCH-CHK-220 vérifier wrmsr interdits gérés.
- ARCH-CHK-221 vérifier rdmsr faults gérés.
- ARCH-CHK-222 vérifier segmentation selectors constants.
- ARCH-CHK-223 vérifier user CS/SS pour SYSRET.
- ARCH-CHK-224 vérifier canonicality check couvre toutes adresses.
- ARCH-CHK-225 vérifier ret code ENOSYS sur CSTAR compat.
- ARCH-CHK-226 vérifier appel dispatch unique.
- ARCH-CHK-227 vérifier post-dispatch signal/cancel path.
- ARCH-CHK-228 vérifier frame pointer debug friendly.
- ARCH-CHK-229 vérifier `global_asm` labels uniques.
- ARCH-CHK-230 vérifier entrée _start64 proprement documentée.
- ARCH-CHK-231 vérifier swapgs non omis sur chemin d’erreur.
- ARCH-CHK-232 vérifier iret/sysret chemins séparés corrects.
- ARCH-CHK-233 vérifier noyau ne retourne jamais à user avec IF incohérent.
- ARCH-CHK-234 vérifier RFLAGS mask stable.
- ARCH-CHK-235 vérifier noyau ne fait pas SYSRET si conditions invalides.
- ARCH-CHK-236 vérifier fallback vers IRET si nécessaire (roadmap).
- ARCH-CHK-237 vérifier portability comments exacts.
- ARCH-CHK-238 vérifier dette technique listée explicitement.
- ARCH-CHK-239 vérifier API publique minimale conservée.
- ARCH-CHK-240 vérifier sécurité micro-architecturale alignée menaces.
- ARCH-CHK-241 vérifier tests boot QEMU reproduisibles.
- ARCH-CHK-242 vérifier tests smoke multi-runs stables.
- ARCH-CHK-243 vérifier journaux tail identiques runs successifs.
- ARCH-CHK-244 vérifier commit notes liées à crash #UD conservées.
- ARCH-CHK-245 vérifier code RDTSCP gating maintenu.
- ARCH-CHK-246 vérifier code RDRAND gating maintenu.
- ARCH-CHK-247 vérifier regression checks dans docs refonte.
- ARCH-CHK-248 vérifier migration vers AArch64 planifiée.
- ARCH-CHK-249 vérifier liste TODO priorisée.
- ARCH-CHK-250 vérifier anomalies connues associées tickets.
- ARCH-CHK-251 vérifier ownership clair de chaque sous-module.
- ARCH-CHK-252 vérifier protocole revue code pour `unsafe`.
- ARCH-CHK-253 vérifier revue pair obligatoire pour ASM.
- ARCH-CHK-254 vérifier campagne fuzz/chaos sur handlers.
- ARCH-CHK-255 vérifier couverture symbolique des fautes.
- ARCH-CHK-256 vérifier policy de backport sécurité.
- ARCH-CHK-257 vérifier seuils perf de référence.
- ARCH-CHK-258 vérifier traces boot non sensibles (secrets).
- ARCH-CHK-259 vérifier intégration security-ready SMP.
- ARCH-CHK-260 vérifier readiness globale pour refonte incrémentale.

---

## 11) Conclusion actionnable

Le module `arch` est globalement solide sur x86_64.
La refonte doit éviter les changements « larges » non séquencés.
Priorité 1: stabiliser interfaces ASM/Rust et GS layout.
Priorité 2: traiter TODO HPET/fixmap et robustesse timing.
Priorité 3: préparer port AArch64 sur socle documentaire strict.
Priorité 4: maintenir les garde-fous sécurité micro-architecturaux.

Ce document sert de base de correction ciblée.
Ce document sert aussi de checklist de non-régression.

## 12) Addendum de validation ARCH (complément 500+)

- ARCH-ADD-001 valider liens docs `docs/kernel/arch`.
- ARCH-ADD-002 valider cohérence terminologie BSP/AP.
- ARCH-ADD-003 valider section risques maintenue à jour.
- ARCH-ADD-004 valider section TODO alignée backlog.
- ARCH-ADD-005 valider mapping responsabilités par fichier.
- ARCH-ADD-006 valider dépendances inter-couches explicites.
- ARCH-ADD-007 valider items critiques priorisés P0/P1.
- ARCH-ADD-008 valider chemin boot happy-path documenté.
- ARCH-ADD-009 valider chemin boot fail-path documenté.
- ARCH-ADD-010 valider chemin syscall happy-path documenté.
- ARCH-ADD-011 valider chemin syscall fail-path documenté.
- ARCH-ADD-012 valider dépendance APIC/ACPI documentée.
- ARCH-ADD-013 valider dépendance time calibration documentée.
- ARCH-ADD-014 valider dépendance spectre mitigations documentée.
- ARCH-ADD-015 valider dépendance virtualisation documentée.
- ARCH-ADD-016 valider hypothèses hardware explicites.
- ARCH-ADD-017 valider hypothèses hyperviseur explicites.
- ARCH-ADD-018 valider hypothèses bare-metal explicites.
- ARCH-ADD-019 valider scénarios de test reproductibles.
- ARCH-ADD-020 valider convention de nommage stable.
- ARCH-ADD-021 valider surveillance des regressions boot.
- ARCH-ADD-022 valider surveillance des regressions syscall.
- ARCH-ADD-023 valider surveillance des regressions SMP.
- ARCH-ADD-024 valider surveillance des regressions timer.
- ARCH-ADD-025 valider surveillance des regressions sécurité.
- ARCH-ADD-026 valider couverture minimale tests smoke.
- ARCH-ADD-027 valider stratégie triage panic/exception.
- ARCH-ADD-028 valider stratégie rollback patch arch.
- ARCH-ADD-029 valider ownership module par sous-dossier.
- ARCH-ADD-030 valider cadence de revue `unsafe`.
- ARCH-ADD-031 valider checklist publication release.
- ARCH-ADD-032 valider checklist pré-merge arch.
- ARCH-ADD-033 valider indicateurs perf boot conservés.
- ARCH-ADD-034 valider indicateurs perf syscall conservés.
- ARCH-ADD-035 valider indicateurs stabilité SMP conservés.
- ARCH-ADD-036 valider indicateurs stabilité timer conservés.
- ARCH-ADD-037 valider traçabilité décisionnelle refonte.
- ARCH-ADD-038 valider dette technique explicitement datée.
- ARCH-ADD-039 valider plan migration AArch64 phasé.
- ARCH-ADD-040 valider clôture lot arch avec critères mesurables.
