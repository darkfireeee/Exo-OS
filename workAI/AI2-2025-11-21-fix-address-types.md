# [AI2] Implémentation de PhysAddr et VirtAddr

**Date :** 2025-11-21 11:45
**Fichiers modifiés :**

- libs/exo_types/src/address.rs

## Changements

### Ajout de types d'adresse

Implémentation basique de `PhysAddr` et `VirtAddr` pour permettre la compilation de `exo_types`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysAddr(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VirtAddr(u64);
```

## Impact sur l'autre IA

- [ ] Nécessite mise à jour des imports
- [ ] Nécessite changement d'appels de fonction
- [x] Pas d'impact immédiat (le kernel utilise encore `x86_64::PhysAddr` pour l'instant)

## Notes

Ces types sont destinés à remplacer l'utilisation directe de `x86_64::PhysAddr` à terme pour découpler le kernel de l'architecture spécifique dans les interfaces communes.
