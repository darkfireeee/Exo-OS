# Analyse Exhaustive Ligne par Ligne — `kernel/src/memory/protection/`

## 1. Vue d'Ensemble du Module de Protection Matérielle

Le sous-système `kernel/src/memory/protection/` implémente les mécanismes de défense en profondeur du processeur x86_64 contre les attaques d'élévation de privilèges, de corruption mémoire et d'exécution de code malveillant. Il configure et contrôle les registres matériels CPU (`CR4`, `EFER`, `PKRU`) et fournit des abstractions RAII garantissant que les privilèges temporaires sont révoqués de façon déterministe.

Le module regroupe 5 composants de sécurité matérielle :
- `nx.rs` : Protection No-Execute (`EFER.NXE` - bit 11) interdisant l'exécution de code depuis les pages de données (pile, tas, buffers DMA).
- `smep.rs` : Supervisor Mode Execution Prevention (`CR4.SMEP` - bit 20) empêchant le noyau (Ring 0) d'exécuter du code situé dans des pages utilisateur (Ring 3), neutralisant les attaques de type *ret2usr*.
- `smap.rs` : Supervisor Mode Access Prevention (`CR4.SMAP` - bit 21) bloquant toute lecture ou écriture du noyau dans l'espace utilisateur en l'absence explicite du drapeau `EFLAGS.AC` (instructions `stac` / `clac`).
- `umip.rs` : User-Mode Instruction Prevention (`CR4.UMIP` - bit 11) bloquant l'exécution des instructions `SGDT`, `SIDT`, `SLDT`, `SMSW`, `STR` depuis l'espace utilisateur pour prévenir les fuites d'adresses noyau (KASLR bypass).
- `pku.rs` : Protection Keys for Userspace (`CR4.PKE` - bit 22) et gestion du registre `PKRU` (Protection Key Rights for Userspace), permettant une compartimentation mémoire fine (16 clés distinctes avec droits AD - Access Disable et WD - Write Disable modifiables en user-space sans TLB flush).

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 Protection d'Accès Superviseur (`smap.rs`)
- **Fenêtre d'Accès Contrôlée (`SmapAccessGuard`) :**
  - Tout transfert de données entre noyau et espace utilisateur (`copy_from_user`, `copy_to_user`, `zero_user`) est obligatoirement enveloppé dans un garde RAII `SmapAccessGuard`.
  - Le constructeur active `stac()` (Set Alignment Check / AC = 1), autorisant temporairement l'accès aux pages Ring 3.
  - L'implémentation de `Drop` appelle impérativement `clac()` (Clear Alignment Check / AC = 0), refermant la fenêtre même en cas d'erreur ou d'interruption.
- **Documentation et En-têtes de Sécurité :**
  - Toutes les fonctions d'accès direct matériel et de copie mémoire non vérifiée sont annotées d'un bloc `# Safety` spécifiant les invariants requis (CPL 0, validité des pointeurs, bornes des tampons).

### 2.2 Clés de Protection Mémoire (`pku.rs`)
- **Attribution et Réservation des Clés (`PkuKeyAllocator`) :**
  - Clés 0 à 3 réservées au noyau (`PKU_DEFAULT_KEY = 0`, `PKU_KERNEL_HEAP_KEY = 1`, `PKU_GUARD_KEY = 2`, `PKU_MMIO_KEY = 3`).
  - Clés 4 à 15 attribuables dynamiquement aux threads utilisateurs.
  - La méthode `free` valide désormais l'intervalle avec l'idiome Rust `!(4..PKU_KEY_COUNT).contains(&k)`.
- **Manipulation Atomique de `PKRU` :**
  - Lectures et écritures directes via les instructions processeur x86 `rdpkru` et `wrpkru`.
  - `PkuAccessGuard` applique le pattern RAII pour accorder des droits d'accès temporaires à une clé et restaurer l'état `saved_pkru` au `Drop`.

### 2.3 Prévention d'Exécution et Fuites d'Adresses (`nx.rs`, `smep.rs`, `umip.rs`)
- **Immuabilité Post-Boot :**
  - Une fois activés au boot (`init`), les bits `CR4.SMEP`, `CR4.UMIP` et `EFER.NXE` restent verrouillés en permanence.
  - Tout manquement ou violation matérielle déclenche un `#GP` ou un `#PF` non récupérable avec journalisation d'audit immédiate.

---

## 3. Synthèse des Corrections Appliquées

| Fichier | Problème Détecté | Correction Appliquée |
| :--- | :--- | :--- |
| `pku.rs` | Absence de `Default` sur `PkuStats` et `PkuKeyAllocator` | `impl Default for PkuStats` & `impl Default for PkuKeyAllocator` |
| `pku.rs` | Test d'intervalle non idiomatique (`k < 4 \|\| k >= PKU_KEY_COUNT`) | `!(4..PKU_KEY_COUNT).contains(&k)` |
| `pku.rs` | En-têtes doc `# Safety` non conformes | Remplacement des deux-points par un titre Markdown propre sur 8 fonctions |
| `smap.rs` | Absence de `Default` sur `SmapStats` | `impl Default for SmapStats` |
| `smap.rs` | En-têtes doc `# Safety` non conformes | Remplacement des deux-points par un titre Markdown propre sur 7 fonctions |

---

## 4. Statut de Validation

Le sous-système `kernel/src/memory/protection/` est rigoureusement conforme aux standards de sécurité matérielle du noyau Exo-OS.
