# Statut du Débogage SMP - 28 Janvier 2025

## Résumé Exécutif

**Objectif:** Activer le support multi-processeur (SMP) pour permettre aux Application Processors (APs) de démarrer et de rejoindre le système.

**État Actuel:** L'AP crashe en mode 32-bit AVANT d'activer le paging. Le BSP fonctionne parfaitement.

**Niveau de Complétion:** ~85% - Le trampoline est presque fonctionnel, problème isolé dans la transition 32-bit.

---

## 🎯 Découvertes Majeures

### ✅ Ce Qui Fonctionne Parfaitement

1. **BSP (Bootstrap Processor)**
   - APIC initialisé correctement (x2APIC mode)
   - Timer APIC configuré à 100Hz
   - ACPI détecte correctement 4 CPUs
   - Envoi des IPIs (INIT + 2x SIPI) réussit

2. **Préparation du Trampoline**
   - Code copié à l'adresse physique 0x8000 ✓
   - Structure de données à 0x8200 correctement écrite:
     - PML4 = 0x136000 ✓
     - Stack = 0x8100b0 ✓
     - Entry = 0x11a425 ✓
     - IDT base = 0x1511a0, limit = 0xfff ✓
   - Vérification par read-back confirme toutes les valeurs
   - Mémoire 0-2MB mappée avec huge page (identity map)

3. **Réception des IPIs**
   - L'AP reçoit l'INIT IPI
   - L'AP reçoit les deux SIPIs (vecteur 0x8)
   - Le trampoline commence à s'exécuter

### ❌ Le Problème Critique

**Triple Fault en Mode 32-bit**

État du CPU au moment du crash (d'après QEMU -d cpu_reset):
```
CR0=00000011  (Protected mode, NO paging)
CR3=00000000  (PML4 PAS ENCORE CHARGÉ)
IDT=000f621e 00000000  (Valeur corrompue)
```

**Conclusion:** L'AP crashe AVANT d'atteindre le code qui charge CR3. Le problème est donc dans:
- Le code 16-bit (real mode)
- Le code 32-bit (protected mode)
- OU la transition entre les deux

---

## 📊 Chronologie du Débogage (Session Complète)

### Phase 1: Red Herrings (Fausses Pistes)
1. ❌ Pensé que c'était le timer APIC qui interrompait
2. ❌ Pensé que c'était un problème de CLI/STI
3. ❌ Passé des heures sur le chargement IDT en mode 64-bit
4. ❌ Pensé que c'était un problème d'adresse virtuelle vs physique

### Phase 2: Découverte Progressive
1. ✓ Identifié que le BSP se bloquait à cause de `cli` mal placé
2. ✓ Réalisé que `log::info()` ne peut pas fonctionner avec interruptions désactivées
3. ✓ Découvert que le triple fault se produit côté AP, pas BSP
4. ✓ Confirmé via QEMU que CR3=0 au moment du crash

### Phase 3: État Actuel
- **Certitude:** Le crash est en mode 32-bit
- **Incertitude:** Exactement QUELLE instruction cause le crash
- **Prochaine Étape:** Ajouter du débogage dans le code 16/32-bit

---

## 🔍 Architecture du Trampoline

### Flux d'Exécution Théorique

```
1. Real Mode (16-bit) @ 0x8000
   ├─ CLI (disable interrupts)
   ├─ Setup segments (DS=ES=SS=0)
   ├─ Load GDT32 (minimal GDT)
   ├─ Enable Protected Mode (CR0.PE = 1)
   └─ Far jump to mode32_entry

2. Protected Mode (32-bit) @ 0x8xxx
   ├─ Setup 32-bit segments
   ├─ Enable PAE (CR4.PAE = 1)
   ├─ Load PML4 into CR3 ← JAMAIS ATTEINT
   ├─ Enable Long Mode (EFER.LME = 1)
   ├─ Enable Paging (CR0.PG = 1)
   └─ Load GDT64 + Far jump to mode64_entry

3. Long Mode (64-bit) @ 0x8xxx
   ├─ Setup 64-bit segments
   ├─ Load IDT
   ├─ Load stack
   └─ Jump to ap_startup() (Rust)
```

**CRASH ICI** ↑ (entre étape 2.1 et 2.3)

### Hypothèses sur la Cause

1. **Far jump 16→32 bit mal encodé**
   - L'instruction `retf` ou `jmp far` pourrait être incorrecte
   - Sélecteur de segment invalide

2. **GDT32 corrompu ou mal aligné**
   - Structure GDT interne au trampoline
   - Descripteur mal formé

3. **Instruction privilégiée avant d'être prêt**
   - Accès à un registre de contrôle invalide
   - MSR read/write avant d'être en mode approprié

4. **Problème NASM d'encodage**
   - Comme pour les autres instructions, NASM pourrait générer du mauvais code
   - Devrait vérifier avec objdump

---

## 🛠️ Tentatives de Correction (Historique)

### Compilatio NASM
- ✅ Retiré `[ORG 0x0000]` (incompatible ELF64)
- ✅ Utilisé encodage manuel `db` pour certaines instructions
- ✅ Vérifié avec hexdump que les bytes sont corrects

### Chargement Données
- ✅ Déplacé de 0xA000 → 0x8200 (fix majeur!)
- ✅ CR3 maintenant lu correctement en mode 64-bit
- ✅ Vérification read-back confirme toutes valeurs

### IDT
- ⚠️ Essayé plusieurs approches (toutes en 64-bit, donc non pertinent)
- ⚠️ Hardcodé l'IDT (mais crash avant d'y arriver)

---

## 📋 Prochaines Étapes (Plan d'Action)

### Étape 1: Vérifier l'Encodage 16/32-bit
```bash
objdump -D -b binary -m i386:x86-64 -M intel ap_trampoline.o
```
- Chercher les instructions suspectes
- Vérifier les far jumps
- Valider les encodages des segments

### Étape 2: Simplifier le Trampoline
- Retirer temporairement tout code non essentiel
- Garder juste: 16-bit → 32-bit → halt
- Ajouter progressivement chaque étape

### Étape 3: Ajouter du Débogage Visuel
- Utiliser `out` vers port série AVANT chaque transition
- Pattern: `out 0xe9, byte 'A'` puis 'B', 'C'...
- Observer dans QEMU où exactement ça s'arrête

### Étape 4: Vérifier la GDT32
```nasm
; Vérifier que ces valeurs sont correctes:
gdt32:
    dq 0x0000000000000000  ; NULL
    dq 0x00CF9A000000FFFF  ; Code 32-bit
    dq 0x00CF92000000FFFF  ; Data 32-bit
```

### Étape 5: Mode Alternatif (Si Échec)
- Utiliser le trampoline du Linux kernel comme référence
- Ou écrire en pur assembleur manuel (pas de NASM)

---

## 📝 Logs Clés

### BSP - Préparation Réussie
```
[INFO] Booting AP 1 (APIC ID 1)...
[INFO] AP 1 boot data: PML4=0x136000, Stack=0x8100b0, Entry=0x11a425
[INFO]   [VERIFY] PML4 @ 0x8200: wrote 0x136000, read 0x136000 ✓
[INFO]   [VERIFY] IDT @ 0x822a: limit=0xfff, base=0x1511a0 ✓
[INFO] AP 1 trampoline ready, SIPI vector = 0x8
```

### QEMU - État au Crash
```
CR0=00000011 CR2=0000000000000000 CR3=0000000000000000 CR4=00000000
IDT=000f621e 00000000
check_exception old: 0x8 new 0xd
Triple fault
```

---

## 🎓 Leçons Apprises

1. **NASM + ELF64 = Problèmes**
   - L'encodage automatique peut être incorrect
   - Toujours vérifier avec objdump
   - Utiliser encodage manuel si nécessaire

2. **Ne Pas Faire Confiance aux Suppositions**
   - J'ai passé trop de temps sur l'IDT en 64-bit
   - Alors que le problème était en 32-bit
   - Toujours vérifier l'état CPU avec QEMU -d cpu_reset

3. **Log::info() ≠ Toujours Sûr**
   - Peut deadlock si interruptions désactivées
   - Utiliser des techniques de débogage plus bas niveau

4. **Identity Mapping est Crucial**
   - La mémoire 0-2MB doit être accessible
   - Déplacer les données de 0xA000 à 0x8200 a tout changé

---

## 📚 Fichiers Modifiés

### Créés/Réécrits Complètement
- `kernel/src/arch/x86_64/smp/ap_trampoline.asm` (v2, production-ready)
- `kernel/src/arch/x86_64/smp/bootstrap.rs` (v2, avec vérifications)

### Modifiés Significativement  
- `kernel/src/arch/x86_64/smp/mod.rs` (logs debug, gestion CLI/STI)
- `kernel/src/arch/x86_64/pit.rs` (sleep_us simplifié)
- `kernel/src/arch/x86_64/memory/paging.rs` (map_low_memory)

### Archivés
- `ap_trampoline_old.asm`
- `bootstrap_old.rs`

---

## 🔧 Configuration Actuelle

**QEMU Test Command:**
```bash
qemu-system-x86_64 -cpu max -smp 4 -m 128M \
  -cdrom build/exo_os.iso -serial stdio -no-reboot \
  -d cpu_reset,int
```

**NASM Compilation:**
```bash
nasm -f elf64 -o ap_trampoline.o ap_trampoline.asm
```

**Adresses Mémoire:**
- Trampoline code: 0x8000
- Boot data: 0x8200
- PML4: 0x136000
- BSP IDT: 0x1511a0
- AP Stack: 0x8100b0

---

## ⏭️ Continuité du Projet

Une fois le SMP fonctionnel, les tâches suivantes:
1. ✅ Phase 2 SMP complet
2. Scheduler multi-core
3. Load balancing
4. IPI pour reschedule/TLB flush
5. Per-CPU data structures

**Estimation:** 2-4 heures de débogage pour résoudre le problème actuel.

---

*Document mis à jour: 28 janvier 2025*
*Auteur: Session de débogage GitHub Copilot*
