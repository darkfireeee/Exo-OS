Voici le document de référence unifié, synthèse des quatre plans d'architecture, assurant la cohérence technique et l'intégration complète du projet **ExoOS**.

---

# ExoOS — Plan d'Intégration Unifié : Fondations Userspace & ExoShell
**Référence :** `USERSPACE-UNIFIED-001`  
**Date :** 2026-05-05  
**Cible :** Shell interactif fonctionnel sur bare-metal QEMU (`cd`, `pwd`, `ls`, `touch`, `cat`, `top`, `kill`, `echo`, `exit`)  
**Dépôt :** https://github.com/darkfireeee/Exo-OS.git

---

## 1. Philosophie & Objectif Terminal

L'approche **shell-first** est la méthode de validation la plus robuste pour un microkernel. Chaque commande exerce une couche distincte du système :
- `cd`/`pwd` → VFS + chemins + syscalls `chdir`/`getcwd`
- `touch`/`cat` → ExoFS + descripteurs de fichiers + `open`/`read`/`write`
- `top` → `procfs` ou IPC vers le scheduler
- `kill` → Signaux + gestion des processus

**Objectif terminal :** démarrer sur QEMU bare-metal, afficher un prompt `exosh:/$`, et exécuter une session shell complète.

---

## 2. Architecture Globale Unifiée

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  RING 3 — Userspace                                                         │
│  ┌───────────────────────────────────────────────────────────────────────┐ │
│  │  exosh (shell) + /bin/cat, touch, ls, top, kill, echo                │ │
│  │  libexo.so / statique (wrappers syscall, IPC, fmt)                   │ │
│  └────────────────────────┬──────────────────────────────────────────────┘ │
│                           │ fd 0/1/2 (stdin/stdout/stderr)                │
└───────────────────────────┼─────────────────────────────────────────────────┘
                            │ IPC ExoOS (SYS_EXO_IPC_SEND/RECV)
┌───────────────────────────▼─────────────────────────────────────────────────┐
│  RING 1 — Serveurs & Drivers Userspace                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐   │
│  │ input_server │  │  tty_server  │  │  fb_server   │  │  vfs_server  │   │
│  │  (PID 11)    │──│  (PID 12)    │──│  (PID 13)    │  │  (PID 3)     │   │
│  │ agrégation   │  │ line disc.   │  │ VGA/FB owner │  │ VFS + procfs │   │
│  │ evdev        │  │ VT100 / PTY  │  │ unique       │  │ tmpfs→ExoFS  │   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘   │
│         │                 │                 │                  │          │
│  ┌──────▼───────┐  ┌──────▼───────┐  ┌──────▼───────┐  ┌──────▼───────┐   │
│  │ ps2_driver   │  │ terminal_srv │  │ vga_driver   │  │ process_srv  │   │
│  │ (IRQ1/12)    │  │ (rendu txt)  │  │ (0xB8000)    │  │ (spawn/kill) │   │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘   │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  ipc_router (PID 2) — graphe d'autorisation étendu                 │   │
│  │  init_server (PID 1) — séquence de boot supervisée                 │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                            │ Syscalls + Capabilities
┌───────────────────────────▼─────────────────────────────────────────────────┐
│  RING 0 — Microkernel ExoOS                                                 │
│  Scheduler · Mémoire (paging) · Syscalls · IRQ relay · ELF Loader          │
│  [CORRECTIONS CRITIQUES : cr3 dynamique, PTE_USER, BSS zero, CLOEXEC]      │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Ordre de Démarrage (Bootstrap)

```
PID 1   init_server        Superviseur de boot, capability broker
PID 2   ipc_router         Routage inter-processus (graph AUTHORIZED_GRAPH)
PID 3   vfs_server         Système de fichiers virtuel (tmpfs → ExoFS)
PID 4   memory_server      Allocateur physique / virtuel
PID 5   device_server      Registre périphériques PCI/USB
PID 6   virtio_drivers     Drivers VirtIO (net, blk, gpu)
PID 7   network_server     Stack réseau
PID 8   scheduler_server   Ordonnancement + métriques CPU
PID 9   exo_shield         Vérification signatures/capabilities
──────────────────────────────────────────────────────────────
PID 10  ps2_driver         Clavier + souris PS/2 (IRQ1/12)
PID 11  input_server       Agrégation InputEvent → tty_server
PID 12  fb_server          Framebuffer/VGA owner exclusif
PID 13  tty_server         Discipline de ligne + PTY maître
──────────────────────────────────────────────────────────────
PID 14  exo-loader         /sbin/exo-loader (ELF static PIE)
PID 15  exosh              /bin/exosh (premier processus utilisateur)
```

**Règle SRV-01 :** `init_server` attend le signal `READY` de chaque service avant de lancer le suivant.

---

## 4. Bloc Input — Clavier & Souris

### 4.1 Type Canonique : `InputEvent`

Tous les drivers d'entrée (PS/2, USB HID, futurs) produisent **un seul type** partagé. Respecte IPC-02 (taille fixe).

```rust
// libs/exo_types/src/input_event.rs
#[repr(C, packed)]
#[derive(Copy, Clone, Debug)]
pub struct InputEvent {
    pub kind:  u8,      // EventType
    pub code:  u16,     // Scancode normalisé (HID-like) ou bouton souris
    pub value: i16,     // 1=press, 0=release, 2=repeat ; dx/dy souris
    pub mods:  u8,      // SHIFT(1) | CTRL(2) | ALT(4) | META(8)
    pub _pad:  [u8; 10],
}

#[repr(u8)]
pub enum EventType { Key = 0x01, RelAbs = 0x02, Button = 0x03, Sync = 0xFF }
```

### 4.2 Driver PS/2 (`drivers/input/ps2/`)

#### `i8042.rs` — Contrôleur bas-niveau
```rust
pub const PS2_DATA:   u16 = 0x60;
pub const PS2_STATUS: u16 = 0x64; // bit0=OBF, bit1=IBF
pub const PS2_CMD:    u16 = 0x64;

pub unsafe fn init_controller() {
    // 1. Désactiver ports
    send_cmd(0xAD); send_cmd(0xA7);
    // 2. Flush buffer
    while inb(PS2_STATUS) & 0x01 != 0 { let _ = inb(PS2_DATA); }
    // 3. Config Byte : IRQ1 + IRQ12 activés, translation OFF (set 2 brut)
    send_cmd(0x20);
    let mut ccb = read_data() & !0x43 | 0x03;
    send_cmd(0x60); send_data(ccb);
    // 4. Self-test (0xAA → 0x55)
    // 5. Réactiver
    send_cmd(0xAE); send_cmd(0xA8);
}
```

#### `keyboard.rs` — Décodeur Set 2
```rust
pub struct KeyboardDecoder {
    extended: bool,
    release:  bool,
    shift: bool, ctrl: bool, alt: bool, meta: bool,
}

impl KeyboardDecoder {
    pub fn feed(&mut self, raw: u8) -> Option<InputEvent> {
        match raw {
            0xE0 => { self.extended = true; None }
            0xF0 => { self.release  = true; None }
            code => {
                let hid = scancode_to_hid(code, self.extended);
                let val = if self.release { 0i16 } else { 1i16 };
                let ev = InputEvent {
                    kind: EventType::Key as u8, code: hid, value: val,
                    mods: self.build_mods(), _pad: [0; 10],
                };
                self.update_mods(hid, val != 0);
                self.extended = false; self.release = false;
                Some(ev)
            }
        }
    }
}
```

#### `main.rs` — Boucle driver
- Enregistre IRQ1 et IRQ12 via `SYS_IRQ_REGISTER (530)`
- **ISR contrainte :** pas d'allocation, pas de mutex bloquant. Ring buffer lock-free vers `input_server`.
- `SYS_IRQ_ACK (531)` systématique après traitement (DRV-08).

### 4.3 `input_server` (PID 11)

Agrège les événements bruts, déduplique (rate-limit 5ms), et transmet à `tty_server`.

```rust
loop {
    let mut msg = [0u8; 64];
    let ret = syscall3(SYS_EXO_IPC_RECV, msg.as_mut_ptr() as u64, 64, u64::MAX);
    if msg_type == MSG_INPUT_EVENT {
        let ev = unsafe { &*(msg.as_ptr().add(4) as *const InputEvent) };
        if filter.accept(ev) {
            // Retransmettre à tty_server (PID 12)
            ipc_send(TTY_SERVER_PID, MSG_INPUT_EVENT, ev as *const _ as u64, 16);
        }
    }
}
```

---

## 5. Bloc Display — Affichage

### 5.1 Stratégie Dual-Mode

| Mode | Usage | Adresse |
|------|-------|---------|
| **VGA Text 80×25** | Fallback immédiat, Phase 1 | `0xB8000` |
| **Framebuffer linéaire** | Cible principale | Via UEFI GOP / VirtIO-GPU |

### 5.2 `vga_driver` — Fallback Texte

```rust
const VGA_BASE: *mut u16 = 0xB8000 as *mut u16;
const COLS: usize = 80;
const ROWS: usize = 25;

pub fn write_char(row: u8, col: u8, c: u8, attr: u8) {
    let off = row as usize * COLS + col as usize;
    unsafe { VGA_BASE.add(off).write_volatile((attr as u16) << 8 | c as u16); }
}

pub fn set_hw_cursor(pos: u16) {
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}
```

### 5.3 `fb_server` / `terminal_server` (PID 12/13)

- **Seul** processus autorisé à écrire sur `0xB8000` (capability `CAP_FB_ACCESS`).
- Expose une interface IPC :
  - `FB_CLEAR (0)` — efface écran
  - `FB_WRITE_CHAR (1)` — `[row][col][char][attr]`
  - `FB_WRITE_STRING (2)` — chaîne avec gestion `\n` / scroll
  - `FB_SCROLL_UP (3)` — défilement d'une ligne
  - `FB_SET_CURSOR (4)` — curseur matériel

**Console Framebuffer (Phase 2) :**
- Police bitmap 8×16 embarquée statiquement (`include_bytes!("font8x16.bin")`) — pas de dépendance VFS au boot.
- Rendu par `blit_glyph(col*8, row*16, ch, fg, bg)`.

---

## 6. Bloc TTY — Discipline de Ligne

### 6.1 `tty_server` (PID 13) — Responsabilités

- Reçoit `InputEvent` depuis `input_server`
- Mode **Canonique** par défaut (ligne par ligne)
- Mode **Raw** pour `top`, éditeurs futurs
- Gère `stdin`/`stdout`/`stderr` comme des fd virtuels
- PTY maître/esclave pour isolation du shell

### 6.2 Line Discipline

```rust
pub struct LineDiscipline {
    buf: [u8; 512],
    len: usize,
    echo: bool,
    mode: TtyMode, // Canon | Raw
}

impl LineDiscipline {
    pub fn feed(&mut self, ev: &InputEvent) -> Option<&[u8]> {
        if ev.value == 0 { return None; } // key-up ignoré
        let ch = hid_to_ascii(ev.code, ev.mods);
        match ch {
            b'\x03' => { /* Ctrl+C → SIGINT foreground */ sys_kill(fg_pgid, SIGINT); None }
            b'\x04' => { /* Ctrl+D → EOF */ None }
            b'\x7F' | b'\x08' => { /* Backspace */ self.backspace(); None }
            b'\r' | b'\n' => { /* Enter → flush ligne */ self.flush_line() }
            c if c != 0 && self.len < 511 => {
                self.buf[self.len] = c; self.len += 1;
                if self.echo { echo_char(c); }
                None
            }
            _ => None
        }
    }
}
```

### 6.3 VT100 Minimal

Séquences reconnues pour le shell :
- `ESC[2J` — clear screen
- `ESC[H` — home
- `ESC[K` — clear line
- `ESC[A/B/C/D` — flèches (historique Phase 2)

### 6.4 Protocole IPC TTY

```
TTY_OPEN      (0x120) → retourne fd pair maître/esclave
TTY_READ      (0x111) → bloquant jusqu'à \n ou EOF
TTY_WRITE     (0x110) → forward vers fb_server
TTY_SETATTR   (0x130) → canonique vs raw
TTY_SIGNAL    (0x140) → SIGINT au foreground
MSG_SHELL_REGISTER (0x131) — enregistrement dynamique du PID shell
```

---

## 7. Bloc Loader ELF — Corrections & Implémentation

### 7.1 Corrections Critiques Pré-requises

#### BUG-CRIT-01 — `cr3` hardcodé
```rust
// AVANT (bug) : unsafe { asm!("mov cr3, {}", in(reg) 0x1000u64); }
// APRÈS : Le scheduler charge cr3 lors du context switch.
//         Le loader kernel crée un NOUVEAU page table par process.
let pt = PageTable::new_user()?;
process.page_table = pt;
```

#### BUG-CRIT-02 — `PTE_USER` manquant
```rust
// Les pages userspace DOIVENT avoir le bit U/S = 1
let flags = PteFlags::PRESENT | PteFlags::USER;
// Code : PRESENT | USER (pas WRITABLE, pas NX)
// Données : PRESENT | USER | WRITABLE | NO_EXECUTE
```

#### BUG-CRIT-03 — BSS non initialisé
```rust
if seg.memsz > seg.filesz {
    core::ptr::write_bytes(
        (vaddr + seg.filesz) as *mut u8, 0,
        (seg.memsz - seg.filesz) as usize
    );
}
```

#### BUG-CRIT-04 — `O_CLOEXEC` non géré dans `execve`
Parcourir la FD table avant `execve` et fermer tous les fd marqués `FD_CLOEXEC`.

### 7.2 Architecture Loader

```
[Shell] → SYS_EXECVE(path)
              │
          kernel : elf_loader_impl.rs
              │
          1. Ouvre binaire via VFS (SYS_EXOFS_OPEN_BY_PATH)
          2. Parse ELF64 (ET_EXEC ou ET_DYN/PIE)
          3. Mappe PT_LOAD via SYS_MMAP (PROT selon flags)
          4. Setup stack (argc, argv, envp, auxv)
          5. Jump _start (Ring 3)
```

### 7.3 `loader/src/elf/parser.rs`

```rust
pub fn parse_elf64(data: &[u8]) -> Option<ElfInfo> {
    if data.len() < 64 || &data[0..4] != b"\x7FELF" { return None; }
    if data[4] != 2 || data[5] != 1 { return None; } // 64-bit LE
    let hdr = unsafe { &*(data.as_ptr() as *const Elf64Header) };
    if hdr.e_type != ET_EXEC && hdr.e_type != ET_DYN { return None; }
    if hdr.e_machine != EM_X86_64 { return None; }

    // Parse Program Headers → segments PT_LOAD (max 8)
    // Vérification bounds : phoff + phnum*phentsize <= data.len()
}
```

### 7.4 `loader/src/main.rs`

```rust
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    // argv[0] = binaire cible (via AT_EXECFN dans auxv)
    let target = get_target_from_auxv();
    let fd = syscall4(SYS_EXOFS_OPEN_BY_PATH, target.as_ptr() as u64, 0, 0, CAP_READ);
    let n = syscall3(SYS_READ, fd, ELF_BUF.as_mut_ptr() as u64, ELF_BUF.len() as u64);
    syscall1(SYS_CLOSE, fd);

    let elf = elf::parser::parse(&ELF_BUF[..n as usize]).unwrap_or_else(|_| die(ENOEXEC));
    map_segments(&elf, &ELF_BUF);
    setup_stack(elf.entry);
    // Jump
    core::arch::asm!("mov rsp, {}; jmp {}", in(reg) STACK_TOP, in(reg) elf.entry, options(noreturn));
}
```

---

## 8. Bloc VFS & Procfs

### 8.1 VFS Server — Corrections P0

| Problème | Correction |
|----------|------------|
| `SYS_OPEN` retourne fd=3 hardcodé | Implémenter `alloc_fd()` / `free_fd()` — table `[Option<OpenFile>; 1024]` |
| `SYS_GETCWD` absent | Stocker `cwd` dans le PCB ; mettre à jour sur `SYS_CHDIR` |
| `SYS_GETDENTS64` format inconnu | Retourner `dirent64` : `d_ino`, `d_off`, `d_reclen`, `d_type`, `d_name` |
| `SYS_CHDIR` ne met pas à jour le PCB | Appel interne `pcb.set_cwd(path)` |

### 8.2 Arborescence Minimale `/`

```
/               (tmpfs root)
├── bin/
│   ├── exosh
│   ├── cat
│   ├── touch
│   ├── ls
│   ├── top
│   ├── kill
│   └── echo
├── sbin/
│   ├── exo-loader
│   ├── exo-input-server
│   ├── exo-tty-server
│   └── exo-fb-server
├── dev/
│   ├── tty0
│   ├── null
│   └── zero
├── tmp/
├── proc/         (pseudo-FS généré par vfs_server)
│   ├── meminfo
│   └── <pid>/
│       ├── stat
│       ├── status
│       └── cmdline
└── lib/
    └── libexo.so
```

### 8.3 Procfs Minimal (pour `top`)

```rust
// /proc/<pid>/stat
format!("{} ({}) {} {} ... {} {} {}\n", pid, name, state, ppid, utime, stime, vsize);

// /proc/<pid>/status
format!("Name: {}\nPid: {}\nState: {}\nVmRSS: {} kB\n", name, pid, state, rss);
```

**Alternative si procfs vide :** `top` interroge `scheduler_server` via IPC `SCHED_LIST_TASKS` en Phase 1.

---

## 9. Bloc Shell — ExoShell (`/bin/exosh`)

### 9.1 Architecture

```
exosh/
├── main.rs        — REPL : prompt → read → parse → execute
├── parser.rs      — Tokenisation simple (espaces, pas de quotes complexes)
├── builtins.rs    — cd, pwd, echo, exit, export
├── commands.rs    — touch, cat, ls, top, kill (wrappers syscalls)
├── ipc.rs         — Helpers TTY (tty_read, tty_write)
├── env.rs         — HashMap PATH, HOME, CWD
└── process.rs     — fork / execve / waitpid
```

### 9.2 REPL Principal

```rust
fn main() -> ! {
    register_with_tty(); // MSG_SHELL_REGISTER avec notre PID
    tty_write(b"ExoOS Shell v0.1\nexo$ ");

    loop {
        let mut buf = [0u8; 256];
        let n = tty_read(&mut buf);
        let line = trim_newline(&buf[..n]);
        let (cmd, args) = parse_line(line);

        match cmd {
            b"exit" => sys_exit(0),
            b"cd"   => builtin_cd(args),
            b"pwd"  => builtin_pwd(),
            b"echo" => builtin_echo(args),
            b"touch"=> cmd_touch(args),
            b"cat"  => cmd_cat(args),
            b"ls"   => cmd_ls(args),
            b"top"  => cmd_top(),
            b"kill" => cmd_kill(args),
            _       => tty_write(b"exosh: command not found\n"),
        }
        tty_write(b"exo$ ");
    }
}
```

### 9.3 Builtins Détaillés

#### `cd <path>`
```rust
pub fn builtin_cd(args: &[&[u8]]) {
    let target = args.get(1).unwrap_or(b"/");
    let abs = resolve_path(cwd(), target);
    let ret = unsafe { syscall2(SYS_CHDIR, abs.as_ptr() as u64, abs.len() as u64) };
    if ret < 0 { tty_write(b"cd: no such directory\n"); }
    else { update_cwd(&abs); }
}
```

#### `pwd`
```rust
pub fn builtin_pwd() {
    let mut buf = [0u8; 256];
    let ret = unsafe { syscall2(SYS_GETCWD, buf.as_mut_ptr() as u64, 256) };
    if ret >= 0 { tty_write(&buf[..ret as usize]); tty_write(b"\n"); }
}
```

#### `touch <file>`
```rust
let fd = syscall4(SYS_OPEN, path.as_ptr() as u64,
                  (O_CREAT | O_WRONLY | O_TRUNC) as u64,
                  0o644, 0);
if fd >= 0 { syscall1(SYS_CLOSE, fd as u64); }
```

#### `cat <file>`
```rust
let fd = syscall3(SYS_OPEN, path.as_ptr() as u64, O_RDONLY as u64, 0);
if fd < 0 { tty_write(b"cat: no such file\n"); return; }
let mut buf = [0u8; 4096];
loop {
    let n = syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, 4096);
    if n <= 0 { break; }
    tty_write(&buf[..n as usize]);
}
syscall1(SYS_CLOSE, fd as u64);
```

#### `top`
```rust
pub fn cmd_top() {
    tty_write(b"\x1b[2J\x1b[H"); // clear screen VT100
    tty_write(b"PID  NAME            STATE  MEM(KB)\n");
    // Lire /proc/ via SYS_GETDENTS64
    let fd = syscall3(SYS_OPEN, b"/proc\0".as_ptr() as u64, O_RDONLY as u64, 0);
    // Parser les entrées numériques, lire /proc/<pid>/status
}
```

#### `kill <pid>`
```rust
let pid = parse_u64(args[1]);
let sig = if args[1].starts_with(b"-") { parse_signal(args[1]) } else { 15 }; // SIGTERM
let ret = unsafe { syscall2(SYS_KILL, pid, sig) };
if ret < 0 { tty_write(b"kill: operation not permitted\n"); }
```

---

## 10. Bloc libexo — Bibliothèque Userspace

```
libexo/
├── syscall.rs    — syscall0..6, NR constants
├── ipc.rs        — send/recv wrappers
├── vfs.rs        — open, read, write, close, stat
├── fmt.rs        — print!, println!, eprintln! (no_std, no heap)
├── str.rs        — strlen, strcmp, itoa, parse_u64
├── mem.rs        — memcpy, memset, memmove
└── start.rs      — _start (ABI SysV AMD64) → app_main
```

**Runtime `_start` :**
```rust
#[naked]
#[no_mangle]
unsafe extern "C" fn _start() -> ! {
    core::arch::asm!(
        "xor rbp, rbp",
        "mov rdi, rsp",      // arg1 = stack top (argc, argv...)
        "and rsp, -16",      // align 16
        "call exo_main",
        "mov rax, 60", "xor rdi, rdi", "syscall", // exit
        options(noreturn)
    );
}
```

---

## 11. Graphe d'Autorisation IPC (ipc_router)

Ajouts dans `servers/ipc_router/src/lib.rs` :

```rust
pub enum ServiceId {
    // ... existants 1-10 ...
    InputServer = 11,
    TtyServer   = 12,
    FbServer    = 13,
}

// Nouveaux edges
AuthEdge::new(ServiceId::InputServer, ServiceId::Device,     2, 500_000),
AuthEdge::new(ServiceId::TtyServer,   ServiceId::InputServer, 2, 500_000),
AuthEdge::new(ServiceId::TtyServer,   ServiceId::FbServer,    2, 500_000),
AuthEdge::new(ServiceId::TtyServer,   ServiceId::Vfs,         2, 50_000),
AuthEdge::new(ServiceId::Init,        ServiceId::TtyServer,   2, 10_000),
```

---

## 12. Syscalls Critiques à Valider

| SYS | NR | Utilisé par | État |
|-----|-----|------------|------|
| READ | 0 | cat, shell stdin | ✅ |
| WRITE | 1 | echo, stdout | ✅ |
| OPEN | 2 | touch, cat, ls | ✅ |
| CLOSE | 3 | tous | ✅ |
| STAT | 4 | ls, cd | ✅ |
| MMAP | 9 | loader | ✅ |
| GETPID | 39 | shell, top | ✅ |
| FORK | 57 | exec externe | ✅ |
| EXECVE | 59 | loader | ✅ |
| KILL | 62 | kill, Ctrl+C | ✅ |
| GETCWD | 79 | pwd | ⚠️ À vérifier |
| CHDIR | 80 | cd | ⚠️ À vérifier |
| GETDENTS64 | 217 | ls, top | ⚠️ À vérifier |
| EXO_IPC_SEND | 300 | serveurs | ✅ |
| EXO_IPC_RECV | 301 | serveurs | ✅ |
| EXO_IPC_CREATE | 302 | registration | ✅ |
| EXO_IPC_REGISTER | 304 | serveurs | ✅ |
| IRQ_REGISTER | 530 | ps2_driver | ✅ |
| IRQ_ACK | 531 | ps2_driver | ✅ |
| MMIO_MAP | 532 | vga/fb | ✅ |
| EXOFS_OPEN_BY_PATH | 519 | loader, shell | ✅ |

---

## 13. Roadmap d'Implémentation Unifiée

### Phase 1 — Fondations Bas Niveau (Semaine 1)
| Tâche | Fichier | Livrable |
|-------|---------|----------|
| Corriger cr3 + PTE_USER | `kernel/loader/elf_loader.rs`, `kernel/memory/paging.rs` | Binaires Ring 3 chargés sans triple fault |
| i8042 init + IRQ1/12 | `drivers/input/ps2/src/i8042.rs` | Clavier détecté |
| Scancode Set 2 → InputEvent | `drivers/input/ps2/src/keyboard.rs` | Touches traduites |
| VGA text mode driver | `drivers/display/vga/src/main.rs` | Texte sur `0xB8000` |
| `input_server` | `servers/input_server/src/main.rs` | Événements routés |

### Phase 2 — Pipeline TTY & Affichage (Semaine 2)
| Tâche | Fichier | Livrable |
|-------|---------|----------|
| `fb_server` (VGA owner) | `servers/fb_server/src/main.rs` | Écriture exclusive VGA |
| `tty_server` | `servers/tty_server/src/main.rs` | Echo, Backspace, Enter |
| Line discipline canonique | `servers/tty_server/src/line_disc.rs` | Ligne bufferisée |
| VT100 minimal | `servers/tty_server/src/vt100.rs` | Clear screen, curseur |
| Police 8×16 statique | `drivers/display/framebuffer/font/` | Rendu glyph |

### Phase 3 — Loader & Binaires (Semaine 3)
| Tâche | Fichier | Livrable |
|-------|---------|----------|
| ELF parser 64 | `loader/src/elf/parser.rs` | Validation ELF |
| Segments PT_LOAD | `loader/src/elf/segments.rs` | Mapping mémoire |
| `loader/src/main.rs` + `entry.rs` | `loader/src/` | Saut vers Ring 3 |
| `libexo` (syscall + fmt) | `userspace/libexo/` | Hello world static |
| VFS fd table réelle | `servers/vfs_server/src/` | `open` retourne fd >= 3 |

### Phase 4 — Shell MVP (Semaine 4)
| Tâche | Fichier | Livrable |
|-------|---------|----------|
| `exosh` REPL | `userspace/exosh/src/main.rs` | Prompt interactif |
| Builtins cd/pwd/echo/exit | `userspace/exosh/src/builtins.rs` | Navigation |
| touch/cat/ls | `userspace/exosh/src/commands.rs` | Manipulation fichiers |
| `top` (procfs ou scheduler IPC) | `userspace/exosh/src/commands.rs` | Liste processus |
| `kill` | `userspace/exosh/src/commands.rs` | Envoi signal |

### Phase 5 — Intégration & Tests (Semaine 5)
- Chaîne complète : boot → init → input → tty → shell
- Tests d'acceptance (voir §14)

---

## 14. Critères de Succès (Definition of Done)

Le système est validé quand la session suivante s'exécute sans crash sur QEMU :

```bash
[boot] ExoOS v0.1 — Kernel ready
[OK] ipc_router
[OK] vfs_server
[OK] tty_server
ExoOS Shell v0.1
exo$ pwd
/
exo$ cd /tmp
exo:/tmp$ pwd
/tmp
exo:/tmp$ touch hello.txt
exo:/tmp$ cat hello.txt
(vide)
exo:/tmp$ echo "ExoOS" > hello.txt
exo:/tmp$ cat hello.txt
ExoOS
exo:/tmp$ ls /
tmp/  bin/  sbin/  dev/  proc/
exo:/tmp$ top
PID  NAME            STATE  MEM(KB)
1    init_server     R      128
2    ipc_router      S      64
...
exo:/tmp$ kill 5
exo:/tmp$ cd /nonexistent
cd: no such directory
exo:/tmp$ exit
Bye!
[kernel] shell exited — system halting
```

---

## 15. Risques & Points de Vigilance

| ID | Risque | Mitigation |
|----|--------|------------|
| R-01 | Absence `libexo` → pas de `memcpy`/`strlen` en Ring 3 | Créer `libexo` dès Phase 3 |
| R-02 | Heap non configuré pour le shell | `SYS_BRK(12)` + `exo_allocator` global |
| R-03 | `procfs.rs` est un stub vide | `top` utilise IPC `SCHED_LIST_TASKS` en fallback |
| R-04 | Synchronisation ISR ↔ input_server | Utiliser `SYS_INPUT_READ` dédié (syscall > 500) ou ring buffer lock-free |
| R-05 | Accès VGA `0xB8000` depuis Ring 1 | `fb_server` demande mapping via `SYS_MMIO_MAP` avec `CAP_PHYSMAP` |
| R-06 | PID shell hardcodé dans tty_server | Mécanisme `MSG_SHELL_REGISTER` dynamique |
| R-07 | Deadlock IPC synchrone | Canaux asynchrones non-bloquants + timeout 100ms |

---

## 16. Structure de Fichiers à Créer / Modifier

```
NOUVEAUX RÉPERTOIRES :
├── servers/input_server/
├── servers/fb_server/
├── servers/tty_server/
├── userspace/exosh/
├── userspace/libexo/
├── userspace/bin/cat/
├── userspace/bin/touch/
├── userspace/bin/ls/
├── userspace/bin/top/
├── userspace/bin/kill/
├── userspace/bin/echo/
├── drivers/display/vga/
└── drivers/display/framebuffer/font/

FICHIERS À IMPLÉMENTER (actuellement vides/stubs) :
├── drivers/input/ps2/src/i8042.rs
├── drivers/input/ps2/src/keyboard.rs
├── drivers/input/ps2/src/mouse.rs
├── drivers/input/ps2/src/main.rs
├── drivers/display/vga/src/main.rs
├── drivers/display/framebuffer/src/main.rs
├── loader/src/entry.rs
├── loader/src/main.rs
├── loader/src/elf/parser.rs
└── loader/src/elf/segments.rs

FICHIERS À MODIFIER :
├── servers/init_server/src/service_table.rs    (+ input, fb, tty, shell)
├── servers/ipc_router/src/lib.rs               (+ ServiceId 11-13, edges)
├── servers/vfs_server/src/translation_layer/   (+ fd table, getcwd, getdents)
├── kernel/src/fs/elf_loader_impl.rs            (+ cr3, PTE_USER, BSS)
├── kernel/src/syscall/handlers/process.rs      (+ cloexec, argv copy)
└── libs/exo_types/src/input_event.rs           (+ type canonique)
```

---

*Document unifié produit le 2026-05-05 — Synthèse des plans Claude Alpha, Beta, Iota & Gamma.*  
*Référence d'implémentation : https://github.com/darkfireeee/Exo-OS.git*