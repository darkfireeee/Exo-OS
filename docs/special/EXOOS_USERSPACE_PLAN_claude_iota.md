# ExoOS — Plan Userspace : Shell & Fondations Réelles
**Auteur : Claude Iota**  
**Date : 2026-05-05**  
**Phase : Userspace Phase 1 — Shell fonctionnel**

---

## 0. Contexte & Objectif

Le kernel ExoOS est maintenant stable (boot, IDT, SMP, ExoFS, IPC, scheduler, syscalls 0–546).
Les serveurs Ring-1 (`init_server`, `vfs_server`, `ipc_router`, `memory_server`) existent en squelette.

L'objectif de cette phase est de **rendre le système utilisable** depuis un shell interactif capable d'exécuter :

```
cd /  pwd  ls  touch <f>  cat <f>  top  kill <pid>
```

Pour y arriver, 5 composants doivent être implantés dans l'ordre suivant :

```
1. Drivers Input    → clavier PS/2 fonctionnel
2. Driver TTY       → ligne de saisie canonique
3. Driver Display   → framebuffer + console texte
4. Loader ELF       → chargement de binaires userspace
5. Shell (exosh)    → application builtin + exec externe
```

---

## 1. Drivers Input — Clavier & Souris

### 1.1 État actuel

```
drivers/input/ps2/src/keyboard.rs  ← VIDE
drivers/input/ps2/src/mouse.rs     ← VIDE
drivers/input/ps2/src/i8042.rs     ← VIDE
drivers/input/ps2/src/main.rs      ← VIDE
drivers/input/evdev/src/events.rs  ← VIDE
drivers/input/usb_hid/             ← VIDE
```

Les IRQs sont routables (IRQ 1 = clavier, IRQ 12 = souris PS/2) via `sys_irq_register` (SYS 533).

### 1.2 Plan d'implémentation — PS/2 Keyboard

#### Étape 1 : `i8042.rs` — Contrôleur PS/2

```rust
// drivers/input/ps2/src/i8042.rs
//
// REGISTRES :
//   0x60 = DATA  (lecture scancode / écriture commande)
//   0x64 = STATUS (read) / COMMAND (write)
//
// RÈGLE : Attendre Status.IBF==0 avant toute écriture.
//         Attendre Status.OBF==1 avant toute lecture.
// MAX_WAIT = 10_000 itérations (évite boucle infinie si HW mort)

pub const PS2_DATA:    u16 = 0x60;
pub const PS2_STATUS:  u16 = 0x64;
pub const PS2_CMD:     u16 = 0x64;

pub fn wait_write_ready() -> bool {
    for _ in 0..10_000 {
        if inb(PS2_STATUS) & 0x02 == 0 { return true; }
        core::hint::spin_loop();
    }
    false  // Timeout — log + ignorer
}

pub fn wait_read_ready() -> bool {
    for _ in 0..10_000 {
        if inb(PS2_STATUS) & 0x01 != 0 { return true; }
        core::hint::spin_loop();
    }
    false
}

pub fn init_controller() {
    // 1. Désactiver ports pendant init
    write_cmd(0xAD); write_cmd(0xA7);
    // 2. Flush buffer de sortie
    while inb(PS2_STATUS) & 0x01 != 0 { let _ = inb(PS2_DATA); }
    // 3. Config byte : IRQ1 activé, IRQ12 activé, translation scancode off
    write_cmd(0x20);
    let config = read_data() & !0x43 | 0x01;
    write_cmd(0x60); write_data(config);
    // 4. Self-test + port test
    // 5. Réactiver port 1 (clavier)
    write_cmd(0xAE);
}
```

#### Étape 2 : `keyboard.rs` — Décodeur scancodes Set 2

```rust
// drivers/input/ps2/src/keyboard.rs
//
// Scancode Set 2 (défaut PS/2 moderne).
// État : Normal, Extended (0xE0), Release (0xF0), ExtRelease (0xE0 0xF0)
//
// RING BUFFER statique (pas d'alloc) : 64 KeyEvent
// Partagé entre ISR (push) et consommateur TTY (pop)

#[derive(Copy, Clone)]
pub struct KeyEvent {
    pub keycode: u8,
    pub pressed: bool,
    pub modifiers: Modifiers,  // Shift, Ctrl, Alt, AltGr
}

// Table de translation scancode Set 2 → ASCII / keycode
// Seuls les codes utiles pour le shell sont nécessaires :
// Lettres, chiffres, espace, entrée, backspace, flèches, Ctrl-C, Ctrl-D

static KEY_RING: SpscRing<KeyEvent, 64> = SpscRing::new();

/// Appelée depuis l'ISR IRQ1 — AUCUNE allocation, AUCUN mutex bloquant
pub fn handle_scancode(byte: u8) {
    // Machine à états : décode séquences multi-octets
    // Push dans KEY_RING (CAS non-bloquant)
}

/// Appelée par le driver TTY — consomme les événements
pub fn poll_key() -> Option<KeyEvent> {
    KEY_RING.pop()
}
```

#### Étape 3 : `main.rs` — Enregistrement IRQ

```rust
// drivers/input/ps2/src/main.rs
//
// Process userspace qui :
//  1. Appelle init_controller()
//  2. Enregistre IRQ 1 via sys_irq_register(1, endpoint)
//  3. Boucle sur IPC recv → handle_scancode() → publie KeyEvent vers TTY server

pub fn main() {
    i8042::init_controller();
    let ep = ipc::register_endpoint("input.keyboard");
    sys_irq_register(1, ep);   // IRQ 1 = keyboard
    loop {
        let msg = ipc::recv_irq();
        let scancode = inb(PS2_DATA);
        keyboard::handle_scancode(scancode);
        sys_irq_ack(1, msg.generation);
        // Notifier le TTY server si KeyEvent disponible
        if let Some(ev) = keyboard::poll_key() {
            ipc::send("tty.server", InputMsg::Key(ev));
        }
    }
}
```

### 1.3 Corrections nécessaires

| Fichier | Problème | Correction |
|---------|----------|------------|
| `i8042.rs` | Pas de timeout sur les lectures | `wait_read_ready()` avec MAX_WAIT |
| `keyboard.rs` | Ring buffer vide | Implémenter `SpscRing<KeyEvent, 64>` |
| `main.rs` | IRQ ACK manquant | Appeler `sys_irq_ack` après chaque scancode |

---

## 2. Driver TTY — Discipline de ligne

### 2.1 État actuel

```
drivers/tty/src/console.rs  ← squelette basique
drivers/tty/src/line_disc.rs ← VIDE
drivers/tty/src/vt100.rs    ← VIDE
drivers/tty/src/pty.rs      ← VIDE
```

### 2.2 Architecture TTY ExoOS

```
[PS2 Driver] ──KeyEvent──► [TTY Server]
                                │
                    ┌───────────┼───────────┐
                    │           │           │
              Line Disc     VT100 Esc    Echo
              (canon/raw)   Sequences    (→ FB)
                    │
               [Shell stdin]
```

### 2.3 Plan — `line_disc.rs` — Mode Canonique

```rust
// drivers/tty/src/line_disc.rs
//
// MODE CANONIQUE : traitement ligne par ligne (défaut)
//   - Backspace : supprime dernier char du buffer
//   - Enter     : valide la ligne → envoie au processus lecteur
//   - Ctrl-C    : envoie SIGINT au foreground process group
//   - Ctrl-D    : EOF si buffer vide, sinon flush
//
// MODE RAW : chaque keypress transmis immédiatement
//   (pour top, vi, etc.)

pub struct LineDisc {
    buf:    [u8; 4096],    // Buffer de saisie canonique
    len:    usize,
    mode:   TtyMode,       // Canonical | Raw
    fg_pgid: u32,          // Process group foreground (pour signaux)
}

impl LineDisc {
    pub fn push_key(&mut self, ev: KeyEvent) -> Option<&[u8]> {
        match self.mode {
            TtyMode::Canonical => self.canonical_push(ev),
            TtyMode::Raw       => self.raw_push(ev),
        }
    }

    fn canonical_push(&mut self, ev: KeyEvent) -> Option<&[u8]> {
        if !ev.pressed { return None; }
        match ev.keycode {
            KEY_ENTER     => { let line = &self.buf[..self.len]; self.len = 0; Some(line) }
            KEY_BACKSPACE => { if self.len > 0 { self.len -= 1; self.echo(b"\x08 \x08"); } None }
            KEY_CTRL_C    => { sys_kill(self.fg_pgid, SIGINT); None }
            KEY_CTRL_D    => { if self.len == 0 { Some(&[]) } else { /* flush */ None } }
            _ if ev.ascii != 0 => {
                if self.len < self.buf.len() {
                    self.buf[self.len] = ev.ascii;
                    self.len += 1;
                    self.echo(&[ev.ascii]); // Echo vers framebuffer
                }
                None
            }
            _ => None,
        }
    }
}
```

### 2.4 Plan — `vt100.rs` — Séquences d'échappement

```rust
// drivers/tty/src/vt100.rs
//
// Séquences minimales pour le shell :
//   ESC[A   = curseur haut     (historique)
//   ESC[B   = curseur bas
//   ESC[C   = curseur droite
//   ESC[D   = curseur gauche
//   ESC[2J  = effacer écran (clear)
//   ESC[H   = home (0,0)
//   ESC[K   = effacer fin de ligne
//   ESC[?m  = couleur (optionnel phase 1)

pub struct VtParser {
    state: VtState,
    params: [u16; 8],
    n_params: usize,
}
```

---

## 3. Driver Display — Framebuffer Console

### 3.1 État actuel

```
drivers/display/framebuffer/src/fb.rs      ← squelette (BlitOp, PixelFormat)
drivers/display/framebuffer/src/cursor.rs  ← squelette
drivers/display/framebuffer/src/blit.rs    ← squelette
drivers/display/framebuffer/src/main.rs    ← VIDE
```

Le framebuffer est initialisé par `exo-boot` (UEFI/BIOS GOP), les coordonnées sont transmises au kernel dans le `BootInfo`. Le kernel dispose déjà de `kernel/src/arch/x86_64/framebuffer_early.rs`.

### 3.2 Plan — `fb.rs` — Framebuffer Driver

```rust
// drivers/display/framebuffer/src/fb.rs
//
// Accès au framebuffer physique via mmap (SYS_MMAP sur l'adresse physique
// transmise dans BootInfo → passée au driver via IPC init_server)
//
// Structure :
//   - Base:    *mut u8  (adresse virtuelle mappée)
//   - Stride:  u32      (bytes par ligne)
//   - Width/Height: u32
//   - Format:  PixelFormat (RGBA32, BGR24, etc.)

pub struct Framebuffer {
    base:   *mut u8,
    stride: usize,
    width:  u32,
    height: u32,
    format: PixelFormat,
}

impl Framebuffer {
    pub fn put_pixel(&mut self, x: u32, y: u32, color: Rgb) {
        let offset = y as usize * self.stride + x as usize * self.format.bytes_per_pixel();
        let ptr = unsafe { self.base.add(offset) };
        self.format.write(ptr, color);
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: Rgb) {
        // Optimisation : si stride == width*bpp → memset ligne
        for row in y..y+h {
            for col in x..x+w {
                self.put_pixel(col, row, color);
            }
        }
    }
}
```

### 3.3 Plan — Console Texte sur Framebuffer

```rust
// drivers/display/framebuffer/src/console.rs
//
// Console texte 80×25 (ou résolution native / 8×16 par glyphe)
// Police bitmap 8×16 (PSF1 ou table statique embarquée)
//
// État : curseur (col, row), couleur fg/bg
// RÈGLE : Scroll = memmove(ligne 1..N-1 → 0..N-2) + clear ligne N-1

pub struct TextConsole {
    fb:         Framebuffer,
    font:       &'static [u8],  // Table glyphes 256 × 16 bytes
    cols:       u32,
    rows:       u32,
    cursor_col: u32,
    cursor_row: u32,
    fg:         Rgb,
    bg:         Rgb,
}

impl TextConsole {
    pub fn write_char(&mut self, c: u8) {
        match c {
            b'\n'      => self.newline(),
            b'\r'      => self.cursor_col = 0,
            b'\x08'    => self.backspace(),  // Backspace echo
            0x20..=0x7E => self.put_glyph(c),
            _          => {}
        }
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        for &b in data { self.write_char(b); }
    }

    fn newline(&mut self) {
        self.cursor_col = 0;
        self.cursor_row += 1;
        if self.cursor_row >= self.rows { self.scroll_up(); }
    }

    fn scroll_up(&mut self) {
        // Déplace lignes 1..rows-1 vers 0..rows-2
        let row_bytes = self.cols as usize * 8 * 16 * 3; // 8px wide, 16px tall, RGB
        // memmove ou blit row par row
        self.cursor_row = self.rows - 1;
        self.clear_row(self.cursor_row);
    }
}
```

### 3.4 Police Bitmap embarquée

```rust
// Inclure une police PSF2 minimale (ASCII 32-126) directement dans le binaire
// Option 1 : include_bytes!("font_8x16.psf")  (police libre ~3KB)
// Option 2 : table const statique générée
//
// PHASE 1 : table statique suffit pour ASCII

const FONT_8X16: &[u8; 256 * 16] = include_bytes!("font8x16.bin");
```

---

## 4. Loader ELF — Chargement de Binaires

### 4.1 État actuel

```
loader/src/elf/parser.rs      ← partiel
loader/src/elf/segments.rs    ← partiel
loader/src/elf/relocations.rs ← partiel
loader/src/elf/tls.rs         ← squelette
loader/src/elf/validator.rs   ← squelette
loader/src/dynamic_linker/    ← squelette
loader/src/entry.rs           ← VIDE
loader/src/main.rs            ← VIDE
```

Le kernel dispose de `kernel/src/fs/elf_loader_impl.rs` pour les exécutables statiques.

### 4.2 Architecture Loader

```
[Shell] → execve(path, argv, envp)
             │
          SYS_EXECVE (59)
             │
          kernel/process/lifecycle/exec.rs
             │
          Lit ELF depuis ExoFS (SYS_EXOFS_OPEN_BY_PATH=519)
             │
     ┌────────────────────┐
     │  elf_loader_impl   │
     │  1. Parse ELF64    │
     │  2. Map segments   │  PT_LOAD → mmap(PROT_R|W|X selon flags)
     │  3. Setup stack    │  argv, envp, auxv
     │  4. Jump entry     │  → _start (crt0 musl-exo)
     └────────────────────┘
```

### 4.3 Plan — `loader/src/elf/parser.rs`

```rust
// loader/src/elf/parser.rs
//
// Valide et parse un ELF64 x86_64
// ERREURS SILENCIEUSES à corriger :
//   ❌ Ne pas vérifier e_ident[EI_MAG] → peut crasher sur binaire non-ELF
//   ❌ Ne pas vérifier e_machine = EM_X86_64 → rejeter ARM/RISC-V
//   ❌ e_phoff peut pointer hors fichier → bounds check impératif

pub fn parse_elf64(data: &[u8]) -> Result<Elf64Info, ElfError> {
    // 1. Magic bytes : 0x7F 'E' 'L' 'F'
    if data.len() < 64 { return Err(ElfError::TooShort); }
    if &data[0..4] != b"\x7FELF" { return Err(ElfError::NotElf); }
    if data[4] != 2 { return Err(ElfError::Not64Bit); }
    if data[18..20] != [0x3E, 0x00] { return Err(ElfError::WrongArch); } // EM_X86_64

    let header = read_elf64_header(data)?;

    // 2. Lire les Program Headers (segments PT_LOAD)
    let mut load_segments = [None::<LoadSegment>; 8];
    let mut n_load = 0usize;

    for i in 0..header.e_phnum {
        let phoff = header.e_phoff as usize + i as usize * header.e_phentsize as usize;
        if phoff + 56 > data.len() { return Err(ElfError::PhdrOutOfBounds); }
        let ph = read_phdr64(&data[phoff..])?;
        if ph.p_type == PT_LOAD {
            if n_load >= 8 { return Err(ElfError::TooManyLoadSegments); }
            load_segments[n_load] = Some(LoadSegment {
                vaddr:  ph.p_vaddr,
                filesz: ph.p_filesz,
                memsz:  ph.p_memsz,
                offset: ph.p_offset,
                flags:  ph.p_flags,    // PF_R | PF_W | PF_X
            });
            n_load += 1;
        }
    }

    Ok(Elf64Info {
        entry:         header.e_entry,
        segments:      load_segments,
        n_segments:    n_load,
        interp:        find_interp(data, &header),  // PT_INTERP (dynamic linking)
        is_pie:        header.e_type == ET_DYN,
    })
}
```

### 4.4 Plan — `loader/src/elf/segments.rs` — Mapping mémoire

```rust
// loader/src/elf/segments.rs
//
// Mappe les segments ELF dans l'espace virtuel du nouveau processus
// via SYS_MMAP (9).
//
// RÈGLES :
//   - PF_X seul (code)   → PROT_READ | PROT_EXEC
//   - PF_W seul (rare)   → PROT_READ | PROT_WRITE  (pas d'exec → W^X)
//   - PF_W | PF_R (data) → PROT_READ | PROT_WRITE
//   - Alignement : p_vaddr doit être aligné sur PAGE_SIZE (4096)
//     Si non aligné → mmap à page_align_down(p_vaddr), ajuster offset
//
// BSS : si p_memsz > p_filesz → memset(vaddr + filesz, 0, memsz - filesz)

pub fn map_segments(elf: &Elf64Info, elf_data: &[u8]) -> Result<(), MapError> {
    let base_offset = if elf.is_pie { choose_pie_base() } else { 0u64 };

    for i in 0..elf.n_segments {
        let seg = elf.segments[i].unwrap();
        let vaddr = base_offset + seg.vaddr;
        let page_start = page_align_down(vaddr);
        let page_end   = page_align_up(vaddr + seg.memsz);

        let prot = elf_flags_to_prot(seg.flags);

        // mmap anonyme puis copier les données
        let ptr = sys_mmap(page_start, page_end - page_start, prot,
                           MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;

        let file_start = seg.offset as usize;
        let file_end   = file_start + seg.filesz as usize;
        let dst_start  = (vaddr - page_start) as usize;

        unsafe {
            core::ptr::copy_nonoverlapping(
                elf_data[file_start..file_end].as_ptr(),
                (ptr as *mut u8).add(dst_start),
                seg.filesz as usize,
            );
            // BSS : zéro le reste
            if seg.memsz > seg.filesz {
                let bss_ptr = (ptr as *mut u8).add(dst_start + seg.filesz as usize);
                core::ptr::write_bytes(bss_ptr, 0, (seg.memsz - seg.filesz) as usize);
            }
        }
    }
    Ok(())
}
```

### 4.5 Setup Stack — `loader/src/entry.rs`

```rust
// loader/src/entry.rs
//
// Construit la pile initiale du processus selon l'ABI Linux x86_64 :
//
// [rsp]    argc
// [rsp+8]  argv[0] ... argv[argc-1]  (pointeurs)
// [rsp+x]  NULL (fin argv)
// [rsp+y]  envp[0] ... envp[n]  (pointeurs)
// [rsp+z]  NULL (fin envp)
// [rsp+w]  auxv : AT_PHDR, AT_PHENT, AT_PHNUM, AT_ENTRY, AT_RANDOM, AT_NULL
//
// Puis jump sur elf.entry (ou ASLR-adjusted entry pour PIE)
//
// musl-exo/_start lit exactement ce format → compatible sans patch

pub fn setup_stack_and_jump(
    stack_top:  u64,
    elf:        &Elf64Info,
    argv:       &[&[u8]],
    envp:       &[&[u8]],
    base_offset: u64,
) -> ! {
    // ... pousser argc, argv, envp, auxv
    // jmp elf.entry + base_offset
    unsafe {
        core::arch::asm!(
            "mov rsp, {stack}",
            "jmp {entry}",
            stack = in(reg) stack_top,
            entry = in(reg) elf.entry + base_offset,
            options(noreturn)
        )
    }
}
```

---

## 5. Serveurs — Complétions nécessaires

### 5.1 TTY Server (nouveau : `servers/tty_server/`)

Le TTY server est le **pivot** entre les drivers input et le shell. Il n'existe pas encore.

```
servers/tty_server/
├── Cargo.toml
└── src/
    ├── main.rs          ← PID 7 (après vfs_server)
    ├── session.rs       ← Sessions TTY (un shell = une session)
    ├── discipline.rs    ← Wrapping de line_disc.rs
    └── protocol.rs      ← Messages IPC: READ, WRITE, IOCTL(TCGETS/TCSETS)
```

**Protocole IPC TTY :**

```
TTY_READ(fd, len)          → data bytes ou EOF
TTY_WRITE(fd, data)        → ok
TTY_IOCTL(TCGETS)          → struct termios
TTY_IOCTL(TCSETS)          → set mode (canonical/raw, echo on/off)
TTY_SIGNAL(SIGINT/SIGTSTP) → kill fg process group
```

**Syscalls TTY mappés :**
- `SYS_READ(fd=0)` → `TTY_READ` si fd est un TTY
- `SYS_WRITE(fd=1/2)` → `TTY_WRITE` → framebuffer console
- `SYS_IOCTL(TCGETS/TCSETS)` → `TTY_IOCTL` (mode canonique ↔ raw)

### 5.2 Complétion `servers/vfs_server/`

Pour que `cat`, `touch`, `ls` fonctionnent, le VFS server doit implémenter :

| Syscall | Handler VFS | Priorité |
|---------|-------------|----------|
| `SYS_OPEN (2)` | `SYS_EXOFS_OPEN_BY_PATH (519)` | P0 |
| `SYS_READ (0)` | `SYS_EXOFS_OBJECT_READ (509)` | P0 |
| `SYS_WRITE (1)` | `SYS_EXOFS_OBJECT_WRITE (510)` | P0 |
| `SYS_CLOSE (3)` | libérer fd + SYS_EXOFS_OBJECT_FD (507) | P0 |
| `SYS_STAT (4)` | `SYS_EXOFS_OBJECT_STAT (506)` | P0 |
| `SYS_GETDENTS (78)` | `SYS_EXOFS_READDIR (521)` | P0 |
| `SYS_MKDIR (83)` | `SYS_EXOFS_OBJECT_CREATE (501)` | P1 |
| `SYS_UNLINK (87)` | `SYS_EXOFS_OBJECT_DELETE (502)` | P1 |
| `SYS_GETCWD (79)` | tracking interne VFS | P0 |
| `SYS_CHDIR (80)` | mise à jour cwd | P0 |

**Corrections VFS silencieuses connues :**

```rust
// ERREUR : SYS_OPEN retourne fd=0 au lieu d'un vrai fd table index
// FIX : Maintenir une FD table globale [Option<OpenFile>; 1024]
//       fd 0,1,2 = stdin/stdout/stderr (TTY) — réservés
//       Allouer fd >= 3 pour les fichiers ouverts

// ERREUR : SYS_GETCWD pas implémenté → ls/cd crashent
// FIX : Stocker cwd comme chemin string dans le PCB userspace
//       Mettre à jour sur SYS_CHDIR

// ERREUR : SYS_GETDENTS retourne struct dirent64 attendue par musl
// Doit retourner : d_ino, d_off, d_reclen, d_type, d_name
// d_type : DT_REG=8, DT_DIR=4, DT_LNK=10
```

### 5.3 Complétion `servers/init_server/`

L'init server doit démarrer la chaîne dans l'ordre suivant (mis à jour) :

```
PID 1 : init_server     ← superviseur
PID 2 : ipc_router      ← routage IPC
PID 3 : vfs_server      ← filesystem
PID 4 : memory_server   ← allocateur userspace
PID 5 : device_server   ← périphériques
PID 6 : drivers/ps2     ← input keyboard
PID 7 : drivers/tty     ← TTY (discipline + console FB)
PID 8 : exosh           ← shell interactif
```

---

## 6. Shell `exosh` — Application Userspace

### 6.1 Architecture

```
userspace/exosh/
├── Cargo.toml
└── src/
    ├── main.rs      ← boucle principale REPL
    ├── parser.rs    ← tokeniseur argv simple
    ├── builtins.rs  ← cd, pwd, exit, help
    ├── exec.rs      ← fork+execve pour binaires externes
    ├── env.rs       ← variables d'environnement
    └── history.rs   ← historique flèche haut/bas (optionnel phase 1)
```

### 6.2 Boucle REPL

```rust
// userspace/exosh/src/main.rs
//
// Shell minimaliste : Read → Eval → Print → Loop
// Utilise musl-exo pour toutes les syscalls (read, write, fork, execve, waitpid)

fn main() -> ! {
    let mut env = Env::default();
    env.set("PATH", "/bin:/usr/bin");
    env.set("HOME", "/root");
    env.set("SHELL", "/bin/exosh");

    // Setup TTY : mode canonique, echo activé
    let tty = open("/dev/tty0", O_RDWR).expect("can't open TTY");
    // TCGETS puis si nécessaire TCSETS

    loop {
        // Afficher prompt
        let cwd = env.cwd.as_str();
        write_str(STDOUT, &format!("exosh:{cwd}$ "));

        // Lire ligne (mode canonique → line_disc fait le travail)
        let mut line = [0u8; 4096];
        let n = read(STDIN, &mut line);
        if n == 0 { break; } // Ctrl-D EOF

        let input = core::str::from_utf8(&line[..n]).unwrap_or("").trim();
        if input.is_empty() { continue; }

        // Parser
        let tokens = parser::tokenize(input);
        if tokens.is_empty() { continue; }

        // Exécuter
        match tokens[0] {
            "cd"   => builtins::cd(&tokens[1..], &mut env),
            "pwd"  => builtins::pwd(&env),
            "exit" => builtins::exit(&tokens[1..]),
            "help" => builtins::help(),
            cmd    => exec::run(cmd, &tokens, &env),
        }
    }
    // Ctrl-D → exit propre
    sys_exit(0);
}
```

### 6.3 Builtins

```rust
// userspace/exosh/src/builtins.rs

pub fn cd(args: &[&str], env: &mut Env) {
    let target = args.get(0).copied().unwrap_or(env.get("HOME").unwrap_or("/"));
    let new_path = resolve_path(env.cwd.as_str(), target);
    // Vérifier que c'est un répertoire avec stat()
    match sys_stat(&new_path) {
        Ok(st) if st.is_dir() => {
            sys_chdir(&new_path).expect("chdir failed");
            env.cwd = new_path;
        }
        Ok(_)  => eprintln!("cd: not a directory: {target}"),
        Err(e) => eprintln!("cd: {target}: {e}"),
    }
}

pub fn pwd(env: &Env) {
    println!("{}", env.cwd);
}

pub fn exit(args: &[&str]) -> ! {
    let code: i32 = args.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    sys_exit(code);
}
```

### 6.4 Commandes externes via fork+execve

```rust
// userspace/exosh/src/exec.rs

pub fn run(cmd: &str, argv: &[&str], env: &Env) {
    // Résoudre le chemin : "touch" → "/bin/touch"
    let path = resolve_in_path(cmd, env.get("PATH").unwrap_or("/bin"));
    let Some(path) = path else {
        eprintln!("exosh: {cmd}: command not found");
        return;
    };

    match sys_fork() {
        0 => {
            // Enfant : execve
            sys_execve(&path, argv, &env.to_envp())
                .unwrap_or_else(|_| sys_exit(127));
        }
        pid => {
            // Parent : attendre la fin
            let mut status = 0i32;
            sys_waitpid(pid, &mut status, 0);
            let code = (status >> 8) & 0xFF;
            if code != 0 {
                // Afficher code d'erreur si != 0 (optionnel)
            }
        }
    }
}
```

---

## 7. Binaires `/bin/` — Implémentation des commandes

Ces binaires sont compilés avec `musl-exo` (libc statique d'ExoOS).

### 7.1 `touch`

```rust
// userspace/bin/touch/src/main.rs
fn main() {
    for arg in args().skip(1) {
        // Ouvrir ou créer le fichier
        let fd = open(&arg, O_WRONLY | O_CREAT, 0o644)
            .unwrap_or_else(|e| { eprintln!("touch: {arg}: {e}"); exit(1); });
        // Mettre à jour timestamps via futimens
        futimens(fd, &[UTIME_NOW, UTIME_NOW]).ok();
        close(fd);
    }
}
```

### 7.2 `cat`

```rust
fn main() {
    let files: Vec<_> = args().skip(1).collect();
    if files.is_empty() {
        // cat stdin
        copy_fd(STDIN_FILENO, STDOUT_FILENO);
    } else {
        for f in &files {
            match open(f, O_RDONLY, 0) {
                Ok(fd) => { copy_fd(fd, STDOUT_FILENO); close(fd); }
                Err(e) => eprintln!("cat: {f}: {e}"),
            }
        }
    }
}

fn copy_fd(src: i32, dst: i32) {
    let mut buf = [0u8; 4096];
    loop {
        let n = read(src, &mut buf).unwrap_or(0);
        if n == 0 { break; }
        write_all(dst, &buf[..n]);
    }
}
```

### 7.3 `ls`

```rust
fn main() {
    let dir = args().nth(1).unwrap_or(".".into());
    let path = if dir == "." { getcwd() } else { dir };

    let mut entries = Vec::new();
    readdir(&path, |entry| entries.push(entry));
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    for e in &entries {
        // Filtrer . et .. (sauf avec -a)
        if e.name.starts_with('.') { continue; }
        let suffix = if e.d_type == DT_DIR { "/" } else { "" };
        println!("{}{suffix}", e.name);
    }
}
```

### 7.4 `top` — Moniteur de processus

```rust
// userspace/bin/top/src/main.rs
//
// Lit /proc/*/stat via VFS server (procfs compat dans vfs_server/compat/procfs.rs)
// Mode RAW (TCSETS) pour affichage dynamique
// Ctrl-C pour quitter

fn main() {
    set_raw_mode();
    clear_screen();

    loop {
        let procs = read_proc_list();  // lire /proc/<pid>/stat
        display_top(&procs);
        nanosleep(1_000_000_000);     // 1 seconde

        // Non-blocking check pour touche 'q' ou Ctrl-C
        if check_input() == Some(b'q') { break; }
    }

    restore_canonical_mode();
}
```

**Correction nécessaire :** `vfs_server/compat/procfs.rs` doit implémenter la lecture de `/proc/<pid>/stat` en interrogeant le scheduler server via IPC.

### 7.5 `kill`

```rust
// userspace/bin/kill/src/main.rs
fn main() {
    // kill [-SIGNAL] <pid>
    let mut signum = SIGTERM;
    let mut args: Vec<_> = args().skip(1).collect();

    if let Some(sig_arg) = args.first().filter(|a| a.starts_with('-')) {
        signum = parse_signal(&sig_arg[1..]).unwrap_or(SIGTERM);
        args.remove(0);
    }

    for arg in &args {
        match arg.parse::<i32>() {
            Ok(pid) => {
                if let Err(e) = sys_kill(pid, signum) {
                    eprintln!("kill: ({pid}) - {e}");
                }
            }
            Err(_) => eprintln!("kill: {arg}: invalid pid"),
        }
    }
}
```

---

## 8. Corrections du Code Existant

### 8.1 `kernel/src/fs/elf_loader_impl.rs`

```
PROBLÈME : Vérification de l'alignement des segments absente
CORRECTION : Vérifier que p_align est une puissance de 2 et que
             p_vaddr % p_align == p_offset % p_align (invariant ELF)

PROBLÈME : BSS non initialisé à zéro si segment non-anonyme
CORRECTION : memset(vaddr + filesz, 0, memsz - filesz) impératif
             sinon données garbage dans variables globales non-initialisées
```

### 8.2 `kernel/src/syscall/handlers/process.rs` — `execve`

```
PROBLÈME : Le gestionnaire execve ne ferme pas les fd O_CLOEXEC
CORRECTION : Parcourir la FD table et fermer tous les fd avec flag FD_CLOEXEC
             avant de charger le nouveau binaire (POSIX impératif)

PROBLÈME : Les arguments argv/envp ne sont pas copiés depuis userspace
           avant de libérer l'ancien espace d'adressage
CORRECTION : Copier argv/envp dans un buffer kernel temporaire AVANT
             sys_munmap de l'ancien mapping
```

### 8.3 `servers/vfs_server/src/translation_layer/syscalls.rs`

```
PROBLÈME : SYS_CHDIR ne met pas à jour le cwd dans le PCB
CORRECTION : Appeler kernel::process::core::pcb::set_cwd(pid, new_path)
             via SYS 530+ dédié ou via IPC process_server

PROBLÈME : SYS_OPEN retourne toujours fd=3 (hardcodé)
CORRECTION : Implémenter une vraie FD table avec alloc_fd() / free_fd()
```

### 8.4 `drivers/tty/src/main.rs`

```
PROBLÈME : Echo des caractères non implémenté (résultat : frappe aveugle)
CORRECTION : Sur chaque KeyEvent reçu, si echo_enabled → écrire vers
             le display server (framebuffer console)

PROBLÈME : Ctrl-C ne génère pas SIGINT
CORRECTION : Sur KEY_CTRL_C → sys_kill(-fg_pgid, SIGINT)
             (tuer tout le process group foreground)
```

---

## 9. Ordre d'Implémentation Recommandé

```
Semaine 1 : Fondations bas niveau
  ├── i8042.rs + keyboard.rs (scancodes → KeyEvent)
  ├── framebuffer/fb.rs (put_pixel, fill_rect)
  └── framebuffer/console.rs (write_char, scroll)

Semaine 2 : Pipeline TTY
  ├── tty/line_disc.rs (mode canonique + echo)
  ├── tty/vt100.rs (séquences escape minimales)
  └── servers/tty_server/ (IPC read/write/ioctl)

Semaine 3 : Loader & Exécution
  ├── loader/elf/parser.rs (validation ELF64)
  ├── loader/elf/segments.rs (mapping mémoire)
  ├── loader/entry.rs (stack ABI + jump)
  └── kernel/fs/elf_loader_impl.rs (corrections BSS + cloexec)

Semaine 4 : Shell & Commandes
  ├── userspace/exosh/ (REPL + builtins)
  ├── userspace/bin/touch, cat, ls
  ├── userspace/bin/kill
  └── userspace/bin/top (avec procfs)

Semaine 5 : Intégration & Tests
  ├── Chaîne complète : boot → init → tty → shell
  ├── Test : cd /tmp && pwd → /tmp
  ├── Test : touch test.txt && cat test.txt
  ├── Test : ls /bin
  ├── Test : top (1 seconde d'affichage)
  └── Test : kill -9 <pid>
```

---

## 10. Tests d'Acceptance

```bash
# Boot → Shell prompt visible sur framebuffer
exosh:/$ 

# Navigation filesystem
exosh:/$ cd /
exosh:/$ pwd
/

exosh:/$ cd /tmp
exosh:/tmp$ pwd
/tmp

# Création et lecture fichier
exosh:/tmp$ touch hello.txt
exosh:/tmp$ cat hello.txt
(vide — fichier créé)

# Listing
exosh:/tmp$ ls
hello.txt

# Processus
exosh:/$ top
  PID  CPU%  MEM   CMD
    1   0.1%   32k  init_server
    2   0.0%   16k  ipc_router
    ...
^C

# Signal
exosh:/$ kill 2    # SIGTERM
exosh:/$ kill -9 2 # SIGKILL
```

---

## 11. Références Syscalls Utilisés

| Syscall | Numéro | Utilisation |
|---------|--------|-------------|
| `read` | 0 | shell stdin, cat |
| `write` | 1 | shell stdout, echo |
| `open` | 2 | cat, touch, ls |
| `close` | 3 | fermeture fd |
| `stat` | 4 | ls, cd |
| `mmap` | 9 | loader segments |
| `getpid` | 39 | top |
| `fork` | 57 | exec externe |
| `execve` | 59 | lancement binaires |
| `exit` | 60 | exit shell |
| `waitpid` | 61 | wait enfant |
| `kill` | 62 | kill, Ctrl-C |
| `getdents64` | 217 | ls |
| `getcwd` | 79 | pwd |
| `chdir` | 80 | cd |
| `ioctl(TCGETS/TCSETS)` | 16 | TTY mode |
| `futimens` | 280 | touch timestamps |
| `sys_irq_register` | 533 | PS/2 keyboard |
| `sys_irq_ack` | 534 | ACK IRQ 1 |
| `SYS_EXOFS_OPEN_BY_PATH` | 519 | VFS open |
| `SYS_EXOFS_OBJECT_READ` | 509 | VFS read |
| `SYS_EXOFS_OBJECT_WRITE` | 510 | VFS write |
| `SYS_EXOFS_READDIR` | 521 | VFS ls |

---

*— Claude Iota, ExoOS Userspace Phase 1*
