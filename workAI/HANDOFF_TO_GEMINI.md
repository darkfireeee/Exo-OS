# 🤝 HANDOFF: Copilot → Gemini

**Date** : 23 novembre 2025 - 14:15
**De** : Copilot (Claude Sonnet 4.5)
**À** : Gemini
**Contexte** : Phase 1 Boot terminée, passage à Phase 2 Drivers

---

## ✅ Travail Terminé par Copilot

### Boot System (60% → prêt pour test)
Fichiers créés :
- `kernel/src/arch/x86_64/boot/boot.asm` (400+ lignes)
- `kernel/src/arch/x86_64/boot/boot.c` (350+ lignes)
- `link_boot.ps1` (script de linkage Windows)
- `link_boot.sh` (script de linkage Linux)
- `kernel/build.rs` (mis à jour)

**État** : Code écrit, en attente test compilation.

**Workflow de build** :
```powershell
.\link_boot.ps1   # Compile boot.asm + boot.c → libboot_combined.a
cargo build       # Compile kernel + link
```

Documentation complète : `workAI/BUILD_PROCESS.md`

---

## 🎯 TON TRAVAIL (Autorisé maintenant)

### Priorité 1 : VGA Driver
**Fichier** : `kernel/src/drivers/video/vga.rs`
**Objectif** : Affichage texte 80x25

**Spécifications** :
```rust
pub struct VgaDriver {
    buffer: &'static mut [[ScreenChar; 80]; 25],
    cursor_x: usize,
    cursor_y: usize,
    color: ColorCode,
}

impl VgaDriver {
    pub fn write_char(&mut self, c: char);
    pub fn write_string(&mut self, s: &str);
    pub fn set_color(&mut self, fg: Color, bg: Color);
    pub fn clear_screen(&mut self);
    pub fn move_cursor(&mut self, x: usize, y: usize);
}
```

**Adresses** :
- Buffer VGA : 0xB8000
- Port curseur : 0x3D4 (command), 0x3D5 (data)
- Format : [char: u8, color: u8] par cellule

**Référence** : Le boot.c a déjà du code VGA que tu peux adapter.

---

### Priorité 2 : Keyboard Driver
**Fichier** : `kernel/src/drivers/input/keyboard.rs`
**Objectif** : Lecture clavier PS/2

**Spécifications** :
```rust
pub struct KeyboardDriver {
    buffer: CircularBuffer<u8, 256>,
}

impl KeyboardDriver {
    pub fn read_scancode(&mut self) -> Option<u8>;
    pub fn scancode_to_ascii(scancode: u8) -> Option<char>;
    pub fn has_key(&self) -> bool;
}
```

**Ports** :
- Data : 0x60
- Status : 0x64
- IRQ : 1 (INT 0x21)

**Scancode mapping** : US QWERTY standard (Set 1)

---

## 📚 Ressources pour Toi

### Documentation à Lire
1. **OBLIGATOIRE** : `workAI/BUILD_PROCESS.md` - Workflow de compilation
2. **OBLIGATOIRE** : `workAI/DIRECTIVES.md` - Standards de code
3. **RÉFÉRENCE** : `kernel/src/drivers/char/serial.rs` - Exemple de driver
4. **RÉFÉRENCE** : `kernel/src/arch/x86_64/boot/boot.c` (lignes 40-80) - Code VGA existant

### Style de Code
Suis DIRECTIVES.md :
- Commentaires en français
- Zero-copy quand possible
- Lock-free patterns
- Inline functions pour perf
- Mesure avec rdtsc

### Exemple - Driver Pattern
```rust
use crate::drivers::{Driver, DeviceInfo, DriverError, DriverResult};
use spin::Mutex;

pub struct VgaDriver {
    // fields
}

impl VgaDriver {
    pub const fn new() -> Self {
        // construction
    }
}

impl Driver for VgaDriver {
    fn name(&self) -> &str {
        "VGA Text Mode Driver"
    }
    
    fn init(&mut self) -> DriverResult<()> {
        // initialization
        Ok(())
    }
    
    fn probe(&self) -> DriverResult<DeviceInfo> {
        Ok(DeviceInfo {
            name: "VGA Compatible",
            vendor_id: 0,
            device_id: 0,
        })
    }
}
```

---

## 🔄 Coordination

### Mise à Jour STATUS_GEMINI.md
Mets à jour toutes les 30 minutes avec :
- Fichiers créés
- Tests réussis
- Problèmes rencontrés
- % completion

### Si Tu Bloques
1. Documente dans `workAI/PROBLEMS.md`
2. Mets ton statut en BLOCKED
3. Pose la question dans STATUS_GEMINI.md

### Quand Tu Termines VGA + Keyboard
1. Mets STATUS à 100% pour Drivers Phase 1
2. Écris dans STATUS_GEMINI : "VGA + Keyboard terminés, attente Memory API"
3. Je publierai alors Memory API dans INTERFACES.md
4. Tu pourras commencer Filesystem

---

## ⚠️ Points d'Attention

### Ne PAS Faire
- ❌ Modifier boot.asm ou boot.c (c'est ma zone)
- ❌ Modifier memory/ (c'est ma zone)
- ❌ Modifier ipc/ (c'est ma zone)
- ❌ Commencer network ou filesystem (attends APIs)

### Tu PEUX Faire
- ✅ Créer/modifier tout dans `drivers/`
- ✅ Ajouter utilitaires dans `utils/`
- ✅ Ajouter tests dans `tests/`
- ✅ Documenter dans `workAI/`

---

## 🎯 Objectif de Cette Étape

**Milestone** : Drivers VGA + Keyboard fonctionnels
**ETA** : 2-3 heures
**Critères de succès** :
- [ ] VGA affiche du texte
- [ ] VGA scroll fonctionne
- [ ] Curseur VGA se déplace
- [ ] Keyboard lit scancodes
- [ ] Keyboard convertit en ASCII
- [ ] Tests unitaires passent
- [ ] Code compile sans warnings

**Après** : Je publierai Memory API et tu pourras commencer tmpfs.

---

## 💬 Communication

**Questions** : Écris dans STATUS_GEMINI.md section "Questions pour Copilot"
**Urgences** : Mets BLOCKED dans statut
**Updates** : Toutes les 30 minutes

---

## 🚀 TU PEUX COMMENCER MAINTENANT

Tout est prêt pour toi :
- ✅ Boot code écrit
- ✅ Build system documenté
- ✅ Trait Driver défini
- ✅ Serial driver comme exemple
- ✅ Directives claires
- ✅ Tests framework existe

**Go code VGA + Keyboard! 🎮**

---

**Bon courage!**
— Copilot

P.S. : N'oublie pas de tester avec QEMU après chaque étape.
