# ExoOS — Plan Fondations Userspace : ExoShell
**Auteur :** Claude Alpha  
**Date :** Mai 2026  
**Statut :** Plan architectural v1 — à partir du repo live (commit HEAD, mai 2026)  
**Objectif terminal :** `cd`, `pwd`, `touch`, `cat`, `top`, `kill <pid>` fonctionnels sur bare-metal QEMU

---

## 0. Diagnostic de l'existant

### Ce qui existe (structurellement)

| Composant | Chemin | État réel |
|-----------|--------|-----------|
| Syscall ABI | `servers/syscall_abi/src/lib.rs` | ✅ Complet — syscall0-6, tous SYS_* Linux-compat |
| ELF Loader kernel | `kernel/src/fs/elf_loader_impl.rs` | ✅ Fonctionnel — ExoFS→BlobId→map segments |
| VFS Server (PID 3) | `servers/vfs_server/src/main.rs` | ✅ Partiel — mount, resolve, open IPC |
| IPC Router (PID 2) | `servers/ipc_router/src/main.rs` | ✅ Fonctionnel — registry 64 endpoints |
| Init Server (PID 1) | `servers/init_server/src/` | ✅ Fonctionnel — 9 services Ring1 |
| PS/2 Driver | `drivers/input/ps2/src/` | ⚠️ Stubs vides (i8042.rs, keyboard.rs, mouse.rs) |
| Framebuffer | `drivers/display/framebuffer/src/` | ⚠️ Stubs vides (fb.rs, blit.rs, cursor.rs) |
| virtio-gpu | `drivers/display/virtio_gpu/src/` | ⚠️ Stubs vides |
| Loader userspace | `loader/src/` | ⚠️ ELF parser structuré mais entry.rs vide |
| USB HID | `drivers/input/usb_hid/src/` | ⚠️ Stubs vides |

### Ce qui **manque** complètement

- `input_server` — aucun serveur Ring1 dédié aux événements d'entrée
- `tty_server` — aucune couche TTY/terminal
- `exosh` — aucun binaire shell
- `libexo` — aucune libc minimale userspace
- Procfs `/proc/<pid>/` — nécessaire pour `top`

---

## 1. Architecture globale — Chaîne de flux

```
┌─────────────────────────────────────────────────────────────────────┐
│  HARDWARE                                                           │
│  [PS/2 i8042] ──IRQ1──► [keyboard.rs]                              │
│  [PS/2 Mouse] ──IRQ12─► [mouse.rs]        [virtio-gpu MMIO]        │
│  [USB Host]   ──INTR──► [usb_hid.rs]                               │
└────────────┬───────────────────────────────────────┬───────────────┘
             │ IPC (SYS_IPC_SEND=300)                │ mmap/DMA
             ▼                                       ▼
    ┌─────────────────┐                   ┌─────────────────────┐
    │  input_server   │ (nouveau, Ring1+) │   fb_server /       │
    │  PID 10         │                   │   virtio_gpu_server │
    │  Event queue    │                   │   PID 11            │
    └────────┬────────┘                   └──────────┬──────────┘
             │ IPC INPUT_READ             IPC FB_WRITE│
             ▼                                        ▼
    ┌─────────────────────────────────────────────────────────────┐
    │  tty_server  (PID 12)                                       │
    │  - Line discipline (echo, backspace, Ctrl+C)                │
    │  - stdin/stdout fd abstraction                              │
    │  - VT100 minimal (curseur, clear, couleurs 16)              │
    └────────────────────────┬────────────────────────────────────┘
                             │ pipe IPC / fd hérité
                             ▼
    ┌─────────────────────────────────────────────────────────────┐
    │  exosh  (Ring3, chargé par loader)                          │
    │  stdin ← tty_server  │  stdout/stderr → tty_server         │
    │  fork/exec via SYS_CLONE + SYS_EXECVE                       │
    └─────────────────────────────────────────────────────────────┘
                             │ SYS_EXECVE
                             ▼
    ┌─────────────────────────────────────────────────────────────┐
    │  loader (Ring3, /sbin/exo-loader)                           │
    │  ELF parse → ExoFS BlobId → map segments → jump entry       │
    └─────────────────────────────────────────────────────────────┘
```

**Ordre de démarrage Ring1+ étendu :**
```
PID1(init) → PID2(ipc_router) → PID3(vfs_server) → PID4(crypto_server)
→ PID5(device_server) → PID6(virtio_drivers) → PID7(network_server)
→ PID8(scheduler_server) → PID9(exo_shield)
→ PID10(input_server)   [NOUVEAU]
→ PID11(fb_server)      [NOUVEAU]
→ PID12(tty_server)     [NOUVEAU]
→ PID13(exosh)          [NOUVEAU — premier processus utilisateur]
```

---

## 2. Bloc A — Drivers d'entrée (Input)

### 2.1 PS/2 Keyboard via i8042

**Fichier cible :** `drivers/input/ps2/src/i8042.rs`

Le contrôleur i8042 expose deux ports I/O :
- `0x60` : Data port (lecture scancodes, écriture commandes)
- `0x64` : Status/Command port

**Protocole d'initialisation :**
```
1. Désactiver les ports PS/2 (cmd 0xAD + 0xA7)
2. Flush buffer (lire 0x60 tant que status bit0=1)
3. Lire/modifier Controller Config Byte (cmd 0x20/0x60)
   → désactiver IRQ clavier+souris, désactiver traduction scan
4. Self-test contrôleur (cmd 0xAA → attendre 0x55)
5. Tester port 1 (cmd 0xAB → attendre 0x00)
6. Activer port 1, réactiver IRQ1 dans Config Byte
7. Activer port 1 (cmd 0xAE)
8. Reset clavier (écrire 0xFF sur data port → attendre 0xFA ACK + 0xAA BAT OK)
9. Set Scan Code Set 2 (commande 0xF0, 0x02)
```

**Traitement IRQ1 (ISR — contrainte FIX-108/109) :**
```rust
// RÈGLE : ISR ne peut PAS allouer, PAS acquérir mutex, PAS appeler IPC directement
// Solution : ring buffer lock-free en mémoire partagée

static SCANCODE_RING: ScancodeRing = ScancodeRing::new(); // AtomicU32 head/tail, [u8;256]

pub fn irq1_handler() {
    let raw = inb(0x60);           // lecture obligatoire même si ignoré
    SCANCODE_RING.push(raw);       // CAS atomique — jamais bloquant
    // Signal au input_server via AtomicBool PENDING
    INPUT_PENDING.store(true, Ordering::Release);
}
```

**Scan Code Set 2 → keycode :**

Table de traduction minimale pour le shell (pas de set 1 — set 2 est plus propre) :
- Extended keys : préfixe `0xE0` → arrows, Delete, Home, End, PageUp/Down
- Release : préfixe `0xF0` → key-up event

**Plan du module `keyboard.rs` :**
```rust
pub struct KeyEvent {
    pub keycode: u8,       // code normalisé 0-127
    pub modifiers: u8,     // SHIFT(1) | CTRL(2) | ALT(4) | CAPS(8)
    pub released: bool,
}

pub fn process_scancode(raw: u8, state: &mut KeyboardState) -> Option<KeyEvent>;
pub fn keycode_to_ascii(ev: &KeyEvent) -> Option<u8>;  // pour le shell
```

### 2.2 PS/2 Mouse (IRQ12)

Pour la phase shell texte, la souris est **optionnelle** mais doit être initialisée pour éviter les IRQ12 parasites qui bloquent i8042.

**Initialisation minimale :**
```
1. Activer port 2 (cmd 0xA8)
2. Activer IRQ souris dans Config Byte (bit 1)
3. Activer port 2 (cmd 0xAE)
4. Envoyer Enable Data Reporting au device (0xF4)
```

**ISR IRQ12 :** lire `0x60`, accumuler 3 octets (paquet standard), push dans `MOUSE_RING`.

### 2.3 USB HID (optionnel Phase 2)

Les USB HID keyboards/mice utilisent le protocole HID Report Descriptor. Sur QEMU, le clavier USB est exposé via xHCI ou EHCI. Pour Phase 1 du shell, **PS/2 suffit** — USB HID est documenté dans `drivers/input/usb_hid/` mais hors scope immédiat.

**Condition de déclenchement USB :** si `device_server` détecte classe PCI `0x0C03` (USB) avec sous-classe HID, activer le module `usb_hid`.

---

## 3. Bloc B — Affichage (Display)

### 3.1 Framebuffer (mode QEMU)

QEMU expose un framebuffer linéaire via deux chemins possibles :

**Chemin A — VGA Text Mode (80×25, mode 3) :**
- Buffer à `0xB8000` — 2 octets/cell : `[char][attr]`
- Pas besoin de driver GPU, disponible dès le boot
- **Recommandé pour Phase 1 shell** : zéro dépendance, affichage immédiat

**Chemin B — virtio-gpu (mode graphique) :**
- PCI Vendor `0x1AF4`, Device `0x1050`
- MMIO via BAR0, control queue + cursor queue
- Nécessite allocation DMA, handshake virtio
- **Phase 2** — utile pour une interface graphique future

**Plan pour Phase 1 : VGA Text Mode**

```rust
// drivers/display/vga/src/main.rs
const VGA_BASE: *mut u16 = 0xB8000 as *mut u16;
const COLS: usize = 80;
const ROWS: usize = 25;

pub struct VgaTerminal {
    cursor_col: u8,
    cursor_row: u8,
    color: u8,   // foreground | (background << 4)
}

impl VgaTerminal {
    pub fn write_char(&mut self, c: u8);
    pub fn write_str(&mut self, s: &[u8]);
    pub fn clear(&mut self);
    pub fn scroll_up(&mut self);          // shift lignes vers le haut
    pub fn set_cursor(&mut self, row: u8, col: u8);
    pub fn set_color(&mut self, fg: VgaColor, bg: VgaColor);
    pub fn move_hw_cursor(&self);         // outw(0x3D4/0x3D5) pour le curseur clignotant
}
```

**Gestion du curseur matériel (HW cursor) :**
```rust
fn set_hw_cursor(pos: u16) {
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}
```

### 3.2 fb_server (PID 11)

Le `fb_server` est un Ring1 server qui abstrait la sortie display. Il expose un protocole IPC simple :

```
FB_CLEAR          (type=0) : efface l'écran
FB_WRITE_CHAR     (type=1) : [row][col][char][attr]
FB_WRITE_STRING   (type=2) : [row][col][len][data...]
FB_SCROLL_UP      (type=3) : scroll d'une ligne
FB_SET_CURSOR     (type=4) : [row][col]
```

**Contrainte clé :** Le fb_server est le seul processus qui écrit directement dans `0xB8000`. Tous les autres (tty_server, shell) passent par IPC.

### 3.3 tty_server (PID 12)

Le `tty_server` est la couche de discipline de ligne. C'est le composant central pour le shell.

**Responsabilités :**
- Recevoir les `KeyEvent` bruts depuis `input_server`
- Implémenter la line discipline (mode cooked) :
  - Echo des caractères vers `fb_server`
  - Backspace : efface le dernier caractère
  - `\n` : flush la ligne vers le fd lecteur (shell)
  - `Ctrl+C` : envoyer SIGINT au groupe de processus foreground
  - `Ctrl+D` : EOF
  - `Ctrl+L` : clear screen
- Maintenir un line buffer (max 512 bytes)
- Gérer les fds stdin/stdout pour le processus foreground

**IPC protocol tty_server :**
```
TTY_OPEN      (0) : un processus demande accès au terminal → retourne fd pair
TTY_READ      (1) : lit une ligne (bloquant jusqu'à \n ou EOF)
TTY_WRITE     (2) : écrit bytes sur le terminal (forward → fb_server)
TTY_SETATTR   (3) : raw vs cooked mode (pour future prise en charge de vi, etc.)
TTY_SIGNAL    (4) : envoie signal au foreground process group
```

---

## 4. Bloc C — Loader & Binaires

### 4.1 Architecture du Loader

Le loader (`/sbin/exo-loader`) est un **binaire statique PIE** chargé par le kernel lors de `SYS_EXECVE`. Il s'exécute en Ring3 avant le `_start` du programme cible.

**Flux d'exécution `execve("/bin/exosh", ...)` :**
```
kernel → SYS_EXECVE(59)
  └─► elf_loader_impl.rs :: ExoFsElfLoader::load_elf("/sbin/exo-loader")
        └─► map segments loader en mémoire userspace
        └─► saute au _start du loader
              └─► loader :: parse argv[0] = "/bin/exosh"
              └─► loader :: ExoFS lookup → BlobId
              └─► loader :: ELF parse (loader/src/elf/parser.rs)
              └─► loader :: security check (PIE, signature, capabilities)
              └─► loader :: map segments cible en mémoire
              └─► loader :: reloc (loader/src/elf/relocations.rs)
              └─► loader :: setup stack : argc, argv, envp, auxv
              └─► jmp entry_point (exosh _start)
```

**Fichiers à implémenter :**

`loader/src/entry.rs` — point d'entrée assembleur :
```rust
// entry.rs
#[naked]
#[no_mangle]
unsafe extern "C" fn _start() -> ! {
    core::arch::asm!(
        "xor rbp, rbp",           // ABI : frame de base = 0
        "mov rdi, rsp",           // arg1 = stack pointer (pour load_main)
        "and rsp, -16",           // aligner stack sur 16 bytes
        "call load_main",
        options(noreturn)
    );
}
```

`loader/src/main.rs` — logique principale :
```rust
pub unsafe extern "C" fn load_main(sp: *const u64) -> ! {
    let argc = *sp as usize;
    let argv = sp.add(1) as *const *const u8;
    // argv[0] = "/sbin/exo-loader"
    // argv[1] = chemin du binaire cible (passé par kernel via auxv AT_EXECFN)
    let target_path = get_target_from_auxv(sp);
    let entry = load_elf_and_map(target_path).unwrap_or_else(|_| die(ENOEXEC));
    // Nettoyer les registres sensibles avant jump
    jump_to_entry(entry, sp);
}
```

### 4.2 Format binaire ExoOS

Les binaires ExoOS sont des **ELF64 statiques** ou **dynamiques avec loader** :

```
Type préféré Phase 1 : ELF64 statique no_std
  - Pas de dépendance libc
  - Sections : .text, .rodata, .data, .bss
  - Entry point direct
  - Taille typique : 50-200 KB

Type Phase 2 : ELF64 dynamique (avec exo-loader)
  - PT_INTERP → /sbin/exo-loader
  - Dépendances : libexo.so (notre libc minimale)
```

**Structure du répertoire `/` dans ExoFS :**
```
/sbin/
  exo-init-server
  exo-ipc-router
  exo-memory-server
  exo-vfs-server
  exo-crypto-server
  exo-device-server
  exo-virtio-drivers
  exo-network-server
  exo-scheduler-server
  exo-shield
  exo-input-server    [NOUVEAU]
  exo-fb-server       [NOUVEAU]
  exo-tty-server      [NOUVEAU]
  exo-loader          [NOUVEAU]
/bin/
  exosh               [NOUVEAU]
  cat                 [NOUVEAU — ou builtin]
  top                 [NOUVEAU]
/lib/
  libexo.so           [NOUVEAU — Phase 2]
/proc/                [pseudo-FS via vfs_server]
/dev/
  tty0
  null
  zero
```

### 4.3 Lancement du shell par init_server

Ajout dans `servers/init_server/src/service_table.rs` :

```rust
// Après exo_shield (PID 9), démarrer les services userspace
ServiceMetadata {
    name: "input_server",
    bin_path: b"/sbin/exo-input-server\0",
    requires: &["device_server"],
    ready_timeout_ms: 300,
    critical: false,   // non-critique : dégradé si absent
},
ServiceMetadata {
    name: "fb_server",
    bin_path: b"/sbin/exo-fb-server\0",
    requires: &["device_server"],
    ready_timeout_ms: 300,
    critical: false,
},
ServiceMetadata {
    name: "tty_server",
    bin_path: b"/sbin/exo-tty-server\0",
    requires: &["input_server", "fb_server"],
    ready_timeout_ms: 500,
    critical: false,
},
// Shell : premier vrai processus utilisateur
ServiceMetadata {
    name: "exosh",
    bin_path: b"/bin/exosh\0",
    requires: &["tty_server", "vfs_server"],
    ready_timeout_ms: 1000,
    critical: false,
},
```

**Invariant SRV-01 respecté :** init_server attend `READY` de chaque service avant le suivant.

---

## 5. Bloc D — ExoShell (`/bin/exosh`)

### 5.1 Architecture interne

```
exosh
├── main.rs          — boucle REPL principale
├── readline.rs      — lecture ligne interactive (via tty_server)
├── parser.rs        — tokenizer + parser de commandes
├── executor.rs      — dispatch builtin vs external
├── builtins/
│   ├── cd.rs
│   ├── pwd.rs
│   ├── touch.rs
│   ├── cat.rs
│   ├── top.rs
│   └── kill.rs
├── process.rs       — fork/exec/wait wrappers
└── env.rs           — variables d'environnement, PATH, CWD
```

### 5.2 Boucle REPL

```rust
fn main() -> ! {
    let mut env = ShellEnv::new();
    env.set("PATH", "/bin:/sbin");
    env.set("CWD", "/");
    
    fb_write("ExoOS Shell v0.1\n");
    
    loop {
        // Afficher le prompt
        let cwd = env.get("CWD").unwrap_or("/");
        fb_write("exosh:");
        fb_write(cwd);
        fb_write("$ ");
        
        // Lire une ligne depuis tty_server (IPC TTY_READ)
        let line = readline::read_line();
        if line.is_empty() { continue; }
        
        // Parser
        let cmd = parser::parse(&line);
        
        // Exécuter
        executor::execute(cmd, &mut env);
    }
}
```

### 5.3 Builtins détaillés

#### `pwd`
```rust
pub fn builtin_pwd(env: &ShellEnv) {
    let cwd = env.get("CWD").unwrap_or("/");
    tty_write(cwd.as_bytes());
    tty_write(b"\n");
}
```
**Syscall utilisé :** aucun — CWD est maintenu en mémoire par le shell.  
**Note :** `SYS_GETCWD (79)` existe dans l'ABI mais pour un shell minimaliste, la variable interne suffit en Phase 1.

#### `cd <path>`
```rust
pub fn builtin_cd(args: &[&str], env: &mut ShellEnv) {
    let target = args.get(1).copied().unwrap_or("/");
    let resolved = resolve_path(env.get("CWD").unwrap_or("/"), target);
    
    // Vérifier que le chemin existe via VFS
    // SYS_ACCESS(21) : access(path, F_OK=0)
    let ret = unsafe { syscall2(SYS_ACCESS, resolved.as_ptr() as u64, 0) };
    if ret < 0 {
        tty_write(b"cd: no such directory\n");
        return;
    }
    env.set("CWD", &resolved);
}

fn resolve_path(cwd: &str, target: &str) -> FixedString<256> {
    if target.starts_with('/') {
        // Absolu
        normalize(target)
    } else {
        // Relatif
        normalize(&[cwd, "/", target].concat())
    }
}
// normalize : gère ".", "..", supprime doubles "/"
```

#### `touch <file>`
```rust
pub fn builtin_touch(args: &[&str], env: &ShellEnv) {
    let Some(path) = args.get(1) else {
        tty_write(b"touch: missing operand\n"); return;
    };
    let abs_path = make_absolute(env.get("CWD").unwrap_or("/"), path);
    
    // SYS_OPEN(2) avec flags O_CREAT|O_WRONLY|O_TRUNC, mode 0644
    let flags: i32 = 0x41 | 0x200; // O_WRONLY=1, O_CREAT=0x40, dans notre ABI
    let ret = unsafe {
        syscall3(SYS_OPEN, abs_path.as_ptr() as u64, flags as u64, 0o644)
    };
    if ret < 0 {
        tty_write(b"touch: cannot create file\n"); return;
    }
    let fd = ret as u64;
    unsafe { syscall1(SYS_CLOSE, fd); }
    // Si le fichier existe déjà → mettre à jour mtime via SYS_FUTIMESAT(261)
    // Pour Phase 1 : l'O_CREAT seul suffit
}
```

#### `cat <file>`
```rust
pub fn builtin_cat(args: &[&str], env: &ShellEnv) {
    let Some(path) = args.get(1) else {
        tty_write(b"cat: missing operand\n"); return;
    };
    let abs_path = make_absolute(env.get("CWD").unwrap_or("/"), path);
    
    // SYS_OPEN(2) O_RDONLY=0
    let fd = unsafe { syscall3(SYS_OPEN, abs_path.as_ptr() as u64, 0, 0) };
    if fd < 0 { tty_write(b"cat: no such file\n"); return; }
    
    let mut buf = [0u8; 4096];
    loop {
        // SYS_READ(0)
        let n = unsafe { syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, 4096) };
        if n <= 0 { break; }
        // SYS_WRITE(1) → fd 1 (stdout → tty_server)
        unsafe { syscall3(SYS_WRITE, 1, buf.as_ptr() as u64, n as u64); }
    }
    unsafe { syscall1(SYS_CLOSE, fd as u64); }
}
```

#### `kill <pid>` et `kill -9 <pid>`
```rust
pub fn builtin_kill(args: &[&str]) {
    // kill [-SIG] PID
    let (sig, pid_str) = if args.get(1).map(|s| s.starts_with('-')).unwrap_or(false) {
        let sig_str = &args[1][1..];
        let sig = match sig_str {
            "9" | "KILL"  => 9u64,
            "15" | "TERM" => 15,
            "2" | "INT"   => 2,
            _ => { tty_write(b"kill: unknown signal\n"); return; }
        };
        (sig, args.get(2))
    } else {
        (15u64, args.get(1))  // défaut SIGTERM
    };
    
    let Some(pid_s) = pid_str else {
        tty_write(b"kill: usage: kill [-SIG] PID\n"); return;
    };
    let pid = parse_u64(pid_s.as_bytes()).unwrap_or(0);
    if pid == 0 { tty_write(b"kill: invalid PID\n"); return; }
    
    // SYS_KILL(62) : kill(pid, sig)
    let ret = unsafe { syscall2(SYS_KILL, pid, sig) };
    if ret < 0 { tty_write(b"kill: operation not permitted\n"); }
}
```

#### `top` (lecture procfs)

`top` nécessite `/proc/<pid>/stat` — ce pseudo-FS est géré par `vfs_server/src/compat/procfs.rs`.

**Plan procfs minimal requis :**
```
/proc/
├── <pid>/
│   ├── stat        → "pid (name) R ppid ... utime stime ..."
│   ├── status      → "Name: ...\nPid: ...\nState: ...\nVmRSS: ..."
│   └── cmdline     → argv[0]\0argv[1]\0...
└── meminfo         → "MemTotal: X kB\nMemFree: Y kB\n..."
```

```rust
pub fn builtin_top(env: &ShellEnv) {
    tty_write(b"\x1b[2J\x1b[H");  // clear screen (VT100)
    tty_write(b"PID  NAME            STATE  MEM(KB)\n");
    tty_write(b"---  ----            -----  -------\n");
    
    // Lister /proc/ via SYS_GETDENTS64(217)
    let proc_fd = unsafe { syscall3(SYS_OPEN, b"/proc\0".as_ptr() as u64, 0, 0) };
    if proc_fd < 0 { tty_write(b"top: cannot open /proc\n"); return; }
    
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { syscall3(SYS_GETDENTS64, proc_fd as u64, buf.as_mut_ptr() as u64, 4096) };
        if n <= 0 { break; }
        // Parser les dirents, pour chaque entrée numérique (PID),
        // lire /proc/<pid>/status → extraire Name, State, VmRSS
        parse_and_display_proc_entries(&buf[..n as usize]);
    }
    unsafe { syscall1(SYS_CLOSE, proc_fd as u64); }
}
```

**Affichage `top` Phase 1 :** snapshot unique (pas de rafraîchissement temps réel — pas de `nanosleep` loop pour simplifier). Phase 2 : boucle avec `SYS_NANOSLEEP(35)` et gestion `Ctrl+C`.

---

## 6. Servers nécessaires — Spécifications IPC

### 6.1 input_server (PID 10)

**Rôle :** Collecter les scancodes bruts (via ring buffer partagé avec ISR), les convertir en `KeyEvent`, et les distribuer aux clients (tty_server, futurs WM).

**Enregistrement IPC :** `SYS_IPC_REGISTER(304, "input_server", cap)`

**Boucle principale :**
```
loop:
  if INPUT_PENDING.load(Relaxed):
    drain SCANCODE_RING
    convert → KeyEvent
    INPUT_PENDING.store(false, Release)
    IPC_SEND → tty_server (TTY_KEY_EVENT)
  else:
    SYS_SCHED_YIELD(24)  // céder le CPU
```

**Mémoire partagée keyboard ring :**
```rust
#[repr(C, align(64))]
struct ScancodeRing {
    head: AtomicU32,
    tail: AtomicU32,
    _pad: [u8; 56],
    buf: [u8; 256],
}
```
La mémoire partagée entre le driver (Ring1) et `input_server` est établie via `SYS_MMAP` avec flags de partage.

### 6.2 fb_server (PID 11)

**Rôle :** Propriétaire exclusif du buffer VGA `0xB8000`. Reçoit des commandes d'affichage par IPC.

**Contrainte de sécurité :** Seul fb_server a la capability `CAP_FB_ACCESS`. Aucun autre Ring1+ process ne peut accéder directement à `0xB8000` (ExoCage).

**Boucle principale :**
```
loop:
  msg = IPC_RECV(301)
  match msg.type:
    FB_WRITE_CHAR   → VGA_BASE[row*80+col] = (attr<<8) | char
    FB_WRITE_STRING → pour chaque char : write_char avec gestion newline/scroll
    FB_CLEAR        → memset VGA_BASE, 0x0720 (espace, gris sur noir)
    FB_SCROLL_UP    → memmove(VGA_BASE, VGA_BASE+80, 24*80*2), clear ligne 24
    FB_SET_CURSOR   → set_hw_cursor(row*80+col)
```

### 6.3 tty_server (PID 12)

**Rôle :** Couche de discipline de ligne. Colle `input_server` ↔ `fb_server` et expose stdin/stdout aux processus applicatifs.

**State machine de la line discipline :**
```
État: NORMAL
  KeyEvent(printable) → echo sur fb + append line_buf
  KeyEvent(BACKSPACE) → si buf non vide : effacer dernier char fb + pop buf
  KeyEvent(ENTER)     → flush line_buf vers processus foreground + clear buf
  KeyEvent(CTRL+C)    → SYS_KILL(foreground_pid, SIGINT)
  KeyEvent(CTRL+D)    → EOF vers foreground
  KeyEvent(CTRL+L)    → FB_CLEAR + réafficher prompt

TTY_WRITE(data) depuis processus → forward vers fb_server FB_WRITE_STRING
```

---

## 7. Syscalls critiques à valider avant intégration

Vérifier dans `kernel/src/syscall/table.rs` que ces entrées existent et sont routées :

| SYS | NR | Utilisé par | État à vérifier |
|-----|-----|------------|-----------------|
| SYS_READ | 0 | cat, shell stdin | ✅ Standard POSIX |
| SYS_WRITE | 1 | cat, stdout | ✅ Standard POSIX |
| SYS_OPEN | 2 | cat, touch, top | ✅ Standard POSIX |
| SYS_CLOSE | 3 | cat, touch | ✅ Standard POSIX |
| SYS_STAT | 4 | cd (vérif dir) | ✅ Standard POSIX |
| SYS_ACCESS | 21 | cd | ✅ Standard POSIX |
| SYS_GETPID | 39 | shell, top | ✅ Standard POSIX |
| SYS_CLONE | 56 | fork | ✅ Défini dans ABI |
| SYS_EXECVE | 59 | exec externe | ✅ Implémenté (ElfLoader) |
| SYS_KILL | 62 | kill builtin | ✅ Standard POSIX |
| SYS_GETCWD | 79 | pwd (fallback) | ⚠️ À vérifier dans table.rs |
| SYS_GETDENTS64 | 217 | top (ls /proc) | ⚠️ À vérifier dans table.rs |
| SYS_IPC_SEND | 300 | tous servers | ✅ Défini ExoOS |
| SYS_IPC_RECV | 301 | tous servers | ✅ Défini ExoOS |
| SYS_IPC_REGISTER | 304 | servers Ring1 | ✅ Défini ExoOS |

**Risque P0 identifié :** `SYS_GETDENTS64 (217)` et `SYS_GETCWD (79)` ne sont pas listés dans `servers/syscall_abi/src/lib.rs` (fichier lu : constants jusqu'à SYS_SEMGET=64). Ces numéros doivent être ajoutés à la lib ABI et vérifiés dans `kernel/src/syscall/table.rs` avant d'implémenter `top` et `pwd`.

---

## 8. Roadmap d'implémentation — Phases

### Phase 1 — Foundation (objectif : affichage + entrée clavier)

| Tâche | Fichier | Durée estimée | Dépendance |
|-------|---------|--------------|------------|
| i8042 init + IRQ1 ISR | `drivers/input/ps2/src/i8042.rs` | 1 session | — |
| Scancode Set 2 → KeyEvent | `drivers/input/ps2/src/keyboard.rs` | 1 session | i8042 |
| VGA text mode driver | `drivers/display/vga/src/main.rs` | 1 session | — |
| fb_server (VGA) | `servers/fb_server/src/main.rs` | 1 session | VGA driver |
| input_server | `servers/input_server/src/main.rs` | 1 session | i8042 |
| tty_server | `servers/tty_server/src/main.rs` | 2 sessions | input + fb |

**Validation Phase 1 :** taper des caractères sur QEMU, les voir affichés sur l'écran VGA.

### Phase 2 — Loader + Shell minimal

| Tâche | Fichier | Durée estimée | Dépendance |
|-------|---------|--------------|------------|
| loader entry.rs + main.rs | `loader/src/` | 2 sessions | ElfLoader kernel |
| exosh REPL + pwd/cd | `userspace/exosh/src/` | 2 sessions | tty_server, loader |
| exosh touch/cat | `userspace/exosh/src/builtins/` | 1 session | vfs_server |
| init_server : spawn shell | `servers/init_server/src/service_table.rs` | 0.5 session | tout |

**Validation Phase 2 :** `pwd`, `cd /`, `touch test.txt`, `cat test.txt` fonctionnels.

### Phase 3 — top/kill + procfs

| Tâche | Fichier | Durée estimée | Dépendance |
|-------|---------|--------------|------------|
| procfs minimal (/proc/PID/stat) | `servers/vfs_server/src/compat/procfs.rs` | 2 sessions | scheduler_server |
| SYS_GETDENTS64 dans table.rs | `kernel/src/syscall/table.rs` | 0.5 session | — |
| exosh top | `userspace/exosh/src/builtins/top.rs` | 1 session | procfs |
| exosh kill | `userspace/exosh/src/builtins/kill.rs` | 0.5 session | SYS_KILL |

**Validation Phase 3 :** `top` liste les PIDs actifs, `kill 5` envoie SIGTERM.

---

## 9. Risques et points de vigilance (Claude Alpha)

### RISK-01 — Absence de libexo (libc)

**Problème :** Le shell et les builtins nécessitent `memcpy`, `strlen`, `itoa`, `atoi`, formatage de strings — fonctions absentes en `no_std`.

**Solution :** Créer `libs/libexo_user/` avec un ensemble minimal :
- `mem.rs` : `memcpy`, `memset`, `memmove`, `memcmp`
- `str.rs` : `strlen`, `strcmp`, `strcpy`, `itoa`, `atoi`, `parse_u64`
- `fmt.rs` : formatage minimal sans `format!` macro (pas de heap)
- `fixed_str.rs` : `FixedString<N>` — déjà dans `libs/exo_types/src/fixed_string.rs` ✅

### RISK-02 — Heap pour le shell

**Problème :** `exosh` tourne en Ring3 sans allocateur heap configuré.

**Solution :** Configurer `exo_allocator` (déjà dans `libs/exo_allocator/`) comme allocateur global pour les processus Ring3. Appel `SYS_BRK(12)` pour étendre le segment data.

### RISK-03 — procfs.rs est un stub

**Vérification :** `servers/vfs_server/src/compat/procfs.rs` — à lire en detail avant Phase 3. Si vide, l'implémentation de `top` sera bloquée sur la collecte de données PID.

**Solution si vide :** `top` peut interroger `scheduler_server` directement via IPC `SCHED_LIST_TASKS` en contournant procfs pour Phase 3 MVP.

### RISK-04 — Synchronisation ISR ↔ input_server

**Problème :** Le `ScancodeRing` est partagé entre le contexte IRQ1 (ISR) et `input_server` (Ring1 process). Si le kernel n'expose pas de mémoire partagée ISR↔Ring1, il faut passer par un syscall `SYS_READ_SCANCODE` dédié.

**Solution recommandée :** Définir `SYS_INPUT_READ (540+n)` dans le namespace ExoOS (les syscalls >500 sont ExoOS-spécifiques selon la table). L'ISR écrit dans un buffer kernel, `SYS_INPUT_READ` le draine côté userspace.

### RISK-05 — Capabilities pour accès VGA 0xB8000

**Problème :** Accéder à `0xB8000` depuis Ring3/Ring1 userspace nécessite un IOPL ou un mapping mémoire explicite autorisé par le kernel.

**Solution :** fb_server demande au `device_server` un mapping physique `0xB8000..0xB8FFF` via `SYS_DMA_ALLOC (534)` avec flags `PHYS_REMAP`. Le kernel vérifie la capability `CAP_PHYSMAP` avant d'accorder.

---

## 10. Structure de fichiers à créer

```
servers/
├── input_server/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          — boucle IPC + drain ring
│       ├── scancode_ring.rs — ring buffer lock-free
│       └── protocol.rs      — INPUT_KEY_EVENT IPC types
├── fb_server/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          — boucle IPC + dispatch VGA
│       ├── vga.rs           — VgaTerminal impl
│       └── protocol.rs      — FB_WRITE_CHAR etc.
└── tty_server/
    ├── Cargo.toml
    └── src/
        ├── main.rs          — boucle IPC
        ├── line_discipline.rs
        └── protocol.rs

userspace/                   [NOUVEAU répertoire]
└── exosh/
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── readline.rs
        ├── parser.rs
        ├── executor.rs
        ├── env.rs
        ├── process.rs
        └── builtins/
            ├── mod.rs
            ├── cd.rs
            ├── pwd.rs
            ├── touch.rs
            ├── cat.rs
            ├── top.rs
            └── kill.rs

loader/src/
├── main.rs          — IMPLÉMENTER (était vide)
├── entry.rs         — IMPLÉMENTER (était vide)
├── elf/             — ✅ déjà structuré
└── security/        — ✅ déjà structuré

drivers/
├── input/ps2/src/
│   ├── i8042.rs     — IMPLÉMENTER
│   ├── keyboard.rs  — IMPLÉMENTER
│   └── mouse.rs     — IMPLÉMENTER (minimal)
└── display/vga/src/
    └── main.rs      — IMPLÉMENTER
```

---

## 11. Critères de succès (Definition of Done)

La session userspace/shell est **complète** quand les scénarios suivants passent sur QEMU bare-metal :

```bash
# Démarrage
[kernel] boot → 9 stages → Ring1 chain → input_server → fb_server → tty_server → exosh

# Affichage attendu sur écran VGA :
ExoOS v0.1 — Kernel ready
[OK] ipc_router
[OK] memory_server
[OK] vfs_server
...
[OK] tty_server
ExoOS Shell v0.1
exosh:/$ _

# Scénario 1 — Navigation
exosh:/$ pwd
/
exosh:/$ cd /tmp
exosh:/tmp$ pwd
/tmp

# Scénario 2 — Fichiers
exosh:/$ touch hello.txt
exosh:/$ cat hello.txt
(vide — fichier vide créé)

# Scénario 3 — Processus
exosh:/$ top
PID  NAME            STATE  MEM(KB)
1    init_server     R      128
2    ipc_router      S      64
3    vfs_server      S      256
...

exosh:/$ kill 5
(SIGTERM envoyé à PID 5)

# Scénario 4 — Erreurs gérées
exosh:/$ cd /nonexistent
cd: no such directory
exosh:/$ cat /nonexistent
cat: no such file
exosh:/$ kill 99999
kill: operation not permitted
```

---

*Plan rédigé par Claude Alpha — Mai 2026*  
*Basé sur lecture live du repo ExoOS (commit HEAD)*  
*Prochaine étape recommandée : implémenter Bloc A (i8042 + keyboard.rs) en premier — c'est le point de départ de toute la chaîne*
