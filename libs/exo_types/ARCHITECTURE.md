# Architecture Optimisée - exo_types

## 🎯 Vision
Bibliothèque de types système zero-cost, type-safe, et production-ready pour Exo-OS.

## 📐 Structure Optimale

```
exo_types/
├── src/
│   ├── lib.rs                 # Point d'entrée, réexportations
│   │
│   ├── primitives/            # Types primitifs (Layer 0)
│   │   ├── mod.rs
│   │   ├── address.rs         # PhysAddr, VirtAddr [PRIORITÉ 1]
│   │   ├── pid.rs             # Process ID
│   │   ├── fd.rs              # File Descriptor
│   │   └── uid_gid.rs         # User/Group IDs
│   │
│   ├── error/                 # Gestion d'erreurs (Layer 1)
│   │   ├── mod.rs
│   │   └── errno.rs           # Codes errno POSIX + custom [PRIORITÉ 2]
│   │
│   ├── time/                  # Types temporels (Layer 1)
│   │   ├── mod.rs
│   │   ├── timestamp.rs       # Timestamp monotonique/realtime
│   │   └── duration.rs        # Duration
│   │
│   ├── ipc/                   # IPC types (Layer 2)
│   │   ├── mod.rs
│   │   ├── signal.rs          # Signaux POSIX
│   │   └── capability.rs      # Capability-based security [PRIORITÉ 3]
│   │
│   ├── syscall/               # Syscalls (Layer 3)
│   │   ├── mod.rs
│   │   ├── numbers.rs         # SyscallNumber enum
│   │   └── raw.rs             # Assembly syscall wrappers
│   │
│   └── prelude.rs             # Réexportations communes
│
├── tests/
│   ├── primitives/
│   │   ├── address_tests.rs
│   │   ├── pid_tests.rs
│   │   └── ...
│   ├── integration/
│   │   └── full_workflow.rs
│   └── property/
│       └── address_properties.rs
│
├── benches/
│   ├── address_bench.rs
│   ├── errno_bench.rs
│   └── ...
│
├── examples/
│   ├── basic_usage.rs
│   ├── error_handling.rs
│   └── ...
│
├── Cargo.toml
├── README.md
└── ARCHITECTURE.md (ce fichier)
```

## 🔄 Plan de Migration (Ordre Prioritaire)

### Phase 1: Foundation (Primitives + Errors)
```
1. ✅ address.rs      - COMPLET avec tests exhaustifs
2. ⏳ errno.rs       - Fusion error.rs, système unifié
3. ⏳ pid.rs         - Correction bug KERNEL
4. ⏳ fd.rs          - RAII robuste
5. ⏳ uid_gid.rs     - Validation complète
```

### Phase 2: Time & IPC
```
6. ⏳ time/*         - Split timestamp/duration
7. ⏳ signal.rs      - Display + SignalSet
8. ⏳ capability.rs  - Éliminer allocations
```

### Phase 3: Syscalls & Integration
```
9. ⏳ syscall/*      - Sécuriser assembly
10. ⏳ lib.rs        - Organisation modules
11. ⏳ prelude.rs    - API ergonomique
```

### Phase 4: Quality & Documentation
```
12. Tests intégration
13. Benchmarks complets
14. Documentation exhaustive
15. Examples pratiques
```

## 🏗️ Principes d'Architecture

### 1. Layering (Dépendances)
```
Layer 3: syscall (utilise Layer 0-2)
   ↓
Layer 2: ipc, capability (utilise Layer 0-1)
   ↓
Layer 1: error, time (utilise Layer 0)
   ↓
Layer 0: primitives (NO dependencies)
```

### 2. Zero-Cost Abstractions
- `#[repr(transparent)]` partout
- `#[inline(always)]` sur hot paths
- Pas d'allocations sauf nécessité absolue
- const fn maximal

### 3. Type Safety
- NewType pattern pour tous les types systèmes
- TryFrom au lieu de From paniquant
- Validation stricte aux boundaries

### 4. API Ergonomique
- Méthodes descriptives (page_align_up vs align_up)
- Conversions naturelles (From, Into, TryFrom)
- Display/Debug informatifs

## 📊 Métriques de Qualité

| Critère | Cible | Status |
|---------|-------|--------|
| Code coverage | >90% | TBD |
| Inline hot paths | 100% | 60% |
| Const fn | >80% | 40% |
| Zero allocations | Oui | Non (capability) |
| Doc coverage | 100% | 30% |
| Benchmarks | All modules | 0% |

## 🎓 État Actuel

### ✅ TERMINÉ
- address.rs: Structure de base optimisée

### 🔄 EN COURS
- address.rs: Tests exhaustifs

### ⏳ À FAIRE
- Tout le reste selon plan ci-dessus
