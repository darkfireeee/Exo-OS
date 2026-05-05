# ExoOS — Plan Fondations Userspace & ExoShell
**Auteur :** claude-gamma  
**Date :** 2026-05-05  
**Référence :** USERSPACE-001 — Fondations réelles d'utilisation (Shell First)  
**Dépendances :** FIX-4 (commit 74c3659e), Security-Unification, ExoPhoenix

---

## 0. Philosophie & Objectif

L'approche *shell-first* est la plus robuste pour valider les fondations d'un microkernel : chaque commande (`cd`, `pwd`, `cat`, `top`, `kill`) exerce une couche différente du système. Avant d'avoir un shell fonctionnel, il faut que quatre sous-systèmes soient opérationnels et coordonnés :

```
┌──────────────────────────────────────────────────────┐
│                    ExoShell (userspace)               │
├────────────┬──────────────┬──────────────┬────────────┤
│ Input Drv  │ Display Drv  │ ELF Loader   │  Servers   │
│ (kbd/mouse)│ (framebuf)   │ (binaries)   │(VFS/Proc..)│
├────────────┴──────────────┴──────────────┴────────────┤
│               IPC / Capability Layer (ExoIPC)         │
├───────────────────────────────────────────────────────┤
│        ExoOS Microkernel (arch + memory + sched)      │
└───────────────────────────────────────────────────────┘
```

---

## 1. Plan — Drivers d'Entrée (Input)

### 1.1 Architecture générale

Les drivers d'entrée tournent en **Ring-3 userspace** sous forme de serveurs dédiés communiquant via IPC avec les consommateurs (ExoShell, futurs apps graphiques). Le kernel ne fait qu'exposer les IRQ via capability.

```
[Keyboard IRQ #1] → [kernel IRQ relay] → CAP_IRQ_KBD
                                              │
                                    [kbd_server (userspace)]
                                              │ IPC MSG_KEY_EVENT
                                    [ExoShell / WM]
```

### 1.2 Driver Clavier PS/2

**Fichier :** `drivers/input/ps2_kbd.rs`

```rust
// Registres PS/2
const PS2_DATA:    u16 = 0x60;
const PS2_STATUS:  u16 = 0x64;
const PS2_CMD:     u16 = 0x64;

pub struct Ps2Keyboard {
    irq_cap: IrqCapability,    // capability obtenue du kernel
    event_tx: IpcChannel,      // canal vers les abonnés
    scancode_state: ScancodeState,
}

impl Ps2Keyboard {
    pub fn init(irq_cap: IrqCapability, tx: IpcChannel) -> Result<Self, InputError> {
        // 1. Flush output buffer
        while unsafe { inb(PS2_STATUS) } & 0x01 != 0 {
            unsafe { inb(PS2_DATA) };
        }
        // 2. Activer les interruptions (bit 0 de la config KBC)
        Self::send_cmd(0x20)?; // Lire config actuelle
        // 3. Mettre en mode scancode set 2 (recommandé)
        Self::send_data(0xF0)?;
        Self::send_data(0x02)?;
        Ok(Self { irq_cap, event_tx: tx, scancode_state: ScancodeState::default() })
    }

    pub fn handle_irq(&mut self) {
        let raw = unsafe { inb(PS2_DATA) };
        if let Some(event) = self.scancode_state.process(raw) {
            let msg = KeyMessage {
                keycode: event.keycode,
                state: event.state,        // Pressed / Released
                modifiers: event.mods,     // Shift, Ctrl, Alt, Super
            };
            // Envoi non-bloquant — si le canal est plein, on drop l'événement
            let _ = self.event_tx.try_send(IpcMessage::Key(msg));
        }
    }
}

// Traducteur scancode set 2 → KeyCode interne ExoOS
struct ScancodeState {
    extended: bool,
    release_next: bool,
}

impl ScancodeState {
    fn process(&mut self, byte: u8) -> Option<RawKeyEvent> {
        match byte {
            0xE0 => { self.extended = true; None }
            0xF0 => { self.release_next = true; None }
            code => {
                let state = if self.release_next { KeyState::Released } else { KeyState::Pressed };
                self.extended = false;
                self.release_next = false;
                let keycode = scancode2_to_keycode(code, self.extended);
                keycode.map(|k| RawKeyEvent { keycode: k, state })
            }
        }
    }
}
```

**Table de mapping à implémenter** (`scancode2_keycode.rs`) : mapping minimal fonctionnel couvrant a-z, 0-9, F1-F12, flèches, Enter, Backspace, Tab, Escape, Ctrl/Alt/Shift/Super.

### 1.3 Driver Souris PS/2

**Fichier :** `drivers/input/ps2_mouse.rs`

```rust
pub struct Ps2Mouse {
    irq_cap: IrqCapability,
    event_tx: IpcChannel,
    packet_buf: [u8; 4],
    packet_idx: usize,
    has_scroll: bool,   // IntelliMouse (5-byte extension)
}

impl Ps2Mouse {
    pub fn handle_irq(&mut self) {
        let byte = unsafe { inb(PS2_DATA) };
        self.packet_buf[self.packet_idx] = byte;
        self.packet_idx += 1;

        let packet_size = if self.has_scroll { 4 } else { 3 };
        if self.packet_idx >= packet_size {
            self.packet_idx = 0;
            if let Some(evt) = self.decode_packet() {
                let _ = self.event_tx.try_send(IpcMessage::Mouse(evt));
            }
        }
    }

    fn decode_packet(&self) -> Option<MouseEvent> {
        let flags = self.packet_buf[0];
        // Vérification bit de sync (bit 3 toujours à 1)
        if flags & 0x08 == 0 { return None; }

        let dx = Self::sign_extend(self.packet_buf[1], flags & 0x10 != 0);
        let dy = -Self::sign_extend(self.packet_buf[2], flags & 0x20 != 0); // Y inversé
        let scroll = if self.has_scroll {
            (self.packet_buf[3] as i8) as i32
        } else { 0 };

        Some(MouseEvent {
            dx, dy, scroll,
            buttons: MouseButtons {
                left:   flags & 0x01 != 0,
                right:  flags & 0x02 != 0,
                middle: flags & 0x04 != 0,
            },
        })
    }

    fn sign_extend(val: u8, sign_bit: bool) -> i32 {
        if sign_bit { (val as i32) - 256 } else { val as i32 }
    }
}
```

### 1.4 Serveur Input (input_server)

**Fichier :** `userspace/servers/input_server/main.rs`

Le serveur agrège kbd + mouse et distribue les événements aux abonnés enregistrés via capability.

```rust
pub struct InputServer {
    kbd: Ps2Keyboard,
    mouse: Ps2Mouse,
    subscribers: Vec<(IpcChannel, InputFilter)>,
}

// InputFilter : masque ce qu'un abonné veut recevoir
pub struct InputFilter {
    pub keyboard: bool,
    pub mouse: bool,
    pub focused_only: bool, // futur : focus window manager
}

impl InputServer {
    pub fn run(&mut self) -> ! {
        loop {
            // Attente IRQ via capability (syscall bloquant)
            let irq = self.wait_irq();
            match irq {
                IrqSource::Keyboard => self.kbd.handle_irq(),
                IrqSource::Mouse    => self.mouse.handle_irq(),
            }
            // La diffusion est faite inside handle_irq via les canaux IPC
        }
    }

    pub fn register_subscriber(&mut self, ch: IpcChannel, filter: InputFilter) {
        self.subscribers.push((ch, filter));
    }
}
```

---

## 2. Plan — Driver d'Affichage (Display)

### 2.1 Architecture

Pour la phase shell, on utilise un **framebuffer linéaire** obtenu via UEFI GOP (Graphics Output Protocol) ou BIOS VESA. Pas de GPU driver dans cette phase — ce sera ExoDisplay v2.

```
[UEFI GOP / VESA] → framebuffer physique
                           │
              [fb_driver (kernel driver, Ring-0)]
              │  expose: CAP_FB_MAP (mmap userspace)
              │
   [terminal_server (userspace, Ring-3)]
   │  - rendu texte (glyph bitmap)
   │  - gestion curseur
   │  - scroll buffer
   │  IPC: MSG_TERM_WRITE / MSG_TERM_CLEAR / MSG_TERM_SCROLL
   │
[ExoShell]
```

### 2.2 Driver Framebuffer Kernel

**Fichier :** `kernel/drivers/display/framebuffer.rs`

```rust
pub struct FramebufferInfo {
    pub base_phys: PhysAddr,
    pub base_virt: VirtAddr,   // mappé en kernel space au boot
    pub width:     u32,
    pub height:    u32,
    pub pitch:     u32,        // bytes par ligne
    pub bpp:       u8,         // bits par pixel (32 typique)
    pub format:    PixelFormat, // RGB / BGR / RGBX / BGRX
}

pub struct FramebufferDriver {
    info: FramebufferInfo,
}

impl FramebufferDriver {
    /// Initialisation depuis les infos GOP transmises par le bootloader
    pub fn init_from_bootinfo(bootinfo: &BootInfo) -> Result<Self, FbError> {
        let fb = &bootinfo.framebuffer;
        // Mapper le framebuffer en kernel space (write-combining idéalement)
        let virt = memory::map_physical_region(
            fb.base_phys,
            fb.size_bytes(),
            PageFlags::PRESENT | PageFlags::WRITE | PageFlags::PWT, // write-through
        )?;
        Ok(Self { info: FramebufferInfo { base_virt: virt, ..fb.into() } })
    }

    /// Expose une capability de mapping userspace
    pub fn create_user_cap(&self, caps: &mut CapabilityTable) -> CapabilityId {
        caps.insert(Capability::FramebufferMap {
            phys: self.info.base_phys,
            size: self.info.size_bytes(),
            info: self.info.clone(),
        })
    }

    /// Blit direct (utilisé seulement par le terminal_server via mmap)
    #[inline(always)]
    pub fn put_pixel(&mut self, x: u32, y: u32, color: Color) {
        let offset = (y * self.info.pitch + x * (self.info.bpp as u32 / 8)) as usize;
        let ptr = (self.info.base_virt.as_u64() as *mut u8);
        unsafe {
            let pixel = ptr.add(offset) as *mut u32;
            *pixel = color.to_u32(self.info.format);
        }
    }
}
```

### 2.3 Terminal Server

**Fichier :** `userspace/servers/terminal_server/main.rs`

```rust
const COLS: usize = 80;
const ROWS: usize = 25;
const GLYPH_W: u32 = 8;
const GLYPH_H: u32 = 16;

pub struct TerminalServer {
    fb: *mut u32,            // mmap du framebuffer
    fb_info: FramebufferInfo,
    font: BitmapFont,        // police 8x16 (PC Screen Font ou custom)
    cells: [[Cell; COLS]; ROWS],
    cursor: (usize, usize),  // (col, row)
    scroll_offset: usize,
    color_fg: Color,
    color_bg: Color,
    scroll_history: VecDeque<[Cell; COLS]>,
}

#[derive(Clone, Copy)]
struct Cell {
    ch:  char,
    fg:  Color,
    bg:  Color,
    dirty: bool,
}

impl TerminalServer {
    pub fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.newline(),
            '\r' => { self.cursor.0 = 0; }
            '\x08' => self.backspace(),
            '\x1b' => { /* ANSI escape — phase 2 */ }
            c => {
                self.cells[self.cursor.1][self.cursor.0] = Cell {
                    ch: c, fg: self.color_fg, bg: self.color_bg, dirty: true
                };
                self.cursor.0 += 1;
                if self.cursor.0 >= COLS { self.newline(); }
            }
        }
        self.flush_dirty();
    }

    fn newline(&mut self) {
        self.cursor.0 = 0;
        self.cursor.1 += 1;
        if self.cursor.1 >= ROWS {
            self.scroll_up();
        }
    }

    fn scroll_up(&mut self) {
        self.scroll_history.push_back(self.cells[0]);
        if self.scroll_history.len() > 1000 { self.scroll_history.pop_front(); }
        for row in 0..ROWS-1 {
            self.cells[row] = self.cells[row + 1];
            for col in 0..COLS { self.cells[row][col].dirty = true; }
        }
        self.cells[ROWS-1] = [Cell::blank(); COLS];
        self.cursor.1 = ROWS - 1;
    }

    fn flush_dirty(&mut self) {
        for row in 0..ROWS {
            for col in 0..COLS {
                let cell = &mut self.cells[row][col];
                if cell.dirty {
                    self.render_glyph(col as u32 * GLYPH_W, row as u32 * GLYPH_H, cell.ch, cell.fg, cell.bg);
                    cell.dirty = false;
                }
            }
        }
        // Dessiner le curseur
        self.render_cursor();
    }

    fn render_glyph(&self, px: u32, py: u32, ch: char, fg: Color, bg: Color) {
        let glyph = self.font.get_glyph(ch);
        for row in 0..GLYPH_H as usize {
            let row_bits = glyph[row];
            for col in 0..GLYPH_W as usize {
                let color = if row_bits & (0x80 >> col) != 0 { fg } else { bg };
                unsafe {
                    let offset = (py as usize + row) * self.fb_info.pitch as usize / 4
                               + (px as usize + col);
                    *self.fb.add(offset) = color.to_u32(self.fb_info.format);
                }
            }
        }
    }
}
```

**Police recommandée :** embarquer la police PSF2 (PC Screen Font) standard — `font/lat9-16.psf` — en tant que tableau `[u8]` dans le binaire. Évite toute dépendance au VFS au démarrage.

---

## 3. Plan — Loader ELF & Binaires

### 3.1 Corrections critiques pré-requises (issues FIX-4 connues)

Avant de charger des binaires userspace, deux bugs critiques identifiés en FIX-4 **doivent être corrigés** :

#### BUG-CRIT-01 — `cr3=0x1000` hardcodé dans l'ELF loader

**Fichier :** `kernel/loader/elf_loader.rs`

```rust
// ❌ AVANT (bug — utilise toujours le même page table)
unsafe { asm!("mov cr3, {}", in(reg) 0x1000u64); }

// ✅ APRÈS — utiliser le cr3 du nouveau process
pub fn load_elf(elf_data: &[u8], process: &mut Process) -> Result<VirtAddr, LoadError> {
    let pt = PageTable::new_user()?;         // Nouveau page table pour ce process
    let entry = parse_and_map_elf(elf_data, &mut pt)?;
    process.page_table = pt;
    // cr3 sera chargé par le scheduler lors du context switch
    // NE PAS toucher cr3 ici — c'est le rôle du scheduler
    Ok(entry)
}
```

#### BUG-CRIT-02 — `PTE_USER` manquant dans les mappings userspace

**Fichier :** `kernel/memory/paging.rs`

```rust
// ❌ AVANT — les pages user ne sont pas accessibles depuis Ring-3
fn map_user_page(pt: &mut PageTable, virt: VirtAddr, phys: PhysAddr) {
    let flags = PteFlags::PRESENT | PteFlags::WRITABLE;  // PTE_USER manquant !
    // ...
}

// ✅ APRÈS
fn map_user_page(pt: &mut PageTable, virt: VirtAddr, phys: PhysAddr, writable: bool) {
    let mut flags = PteFlags::PRESENT | PteFlags::USER;  // Bit U/S = 1
    if writable { flags |= PteFlags::WRITABLE; }
    flags |= PteFlags::NO_EXECUTE; // Données : NX
    map_page_internal(pt, virt, phys, flags);
}

// Pour les segments exécutables ELF :
fn map_exec_segment(pt: &mut PageTable, virt: VirtAddr, phys: PhysAddr) {
    let flags = PteFlags::PRESENT | PteFlags::USER;  // Pas WRITABLE, pas NX
    map_page_internal(pt, virt, phys, flags);
}
```

### 3.2 ELF Loader complet

**Fichier :** `kernel/loader/elf_loader.rs`

```rust
use core::mem;

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

pub struct ElfLoader;

impl ElfLoader {
    pub fn load(
        elf_data: &[u8],
        pt: &mut PageTable,
        phys_alloc: &mut PhysAllocator,
    ) -> Result<ElfLoadResult, LoadError> {
        // 1. Vérification header ELF
        let header = Self::parse_header(elf_data)?;
        if header.e_type != ET_EXEC && header.e_type != ET_DYN {
            return Err(LoadError::UnsupportedType);
        }
        if header.e_machine != EM_X86_64 {
            return Err(LoadError::WrongArchitecture);
        }

        // 2. Parser et mapper les Program Headers (PT_LOAD)
        let mut load_bias: u64 = 0;
        if header.e_type == ET_DYN {
            // PIE : choisir une adresse de base aléatoire (ASLR basique)
            load_bias = Self::choose_load_bias();
        }

        let mut brk: VirtAddr = VirtAddr::zero();
        for i in 0..header.e_phnum {
            let phdr = Self::get_phdr(elf_data, &header, i)?;
            if phdr.p_type != PT_LOAD { continue; }

            let vaddr = VirtAddr::new(phdr.p_vaddr + load_bias);
            let memsz = phdr.p_memsz as usize;
            let filesz = phdr.p_filesz as usize;

            // Allouer et mapper les pages
            let pages_needed = (memsz + 0xFFF) / 0x1000;
            for page_idx in 0..pages_needed {
                let phys = phys_alloc.allocate_zeroed()?;
                let virt = vaddr + (page_idx * 0x1000) as u64;

                // Copier les données du fichier dans les premières pages
                let file_offset = phdr.p_offset as usize + page_idx * 0x1000;
                let copy_size = if file_offset < phdr.p_offset as usize + filesz {
                    core::cmp::min(0x1000, (phdr.p_offset as usize + filesz) - file_offset)
                } else { 0 };

                if copy_size > 0 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            elf_data.as_ptr().add(file_offset),
                            phys.to_virt().as_mut_ptr(),
                            copy_size,
                        );
                    }
                }

                // Droits selon flags ELF
                let writable = phdr.p_flags & PF_W != 0;
                let executable = phdr.p_flags & PF_X != 0;
                match (writable, executable) {
                    (_, true)  => map_exec_segment(pt, virt, phys),
                    (true, _)  => map_user_page(pt, virt, phys, true),
                    _          => map_user_page(pt, virt, phys, false),
                }

                let end = virt + 0x1000u64;
                if end > brk { brk = end; }
            }
        }

        // 3. Stack userspace (8 pages = 32 KiB par défaut)
        let stack_top = Self::setup_user_stack(pt, phys_alloc)?;

        // 4. Pousser les arguments (argc/argv) sur la stack
        // Pour phase 1 : argc=0, argv=null

        Ok(ElfLoadResult {
            entry_point: VirtAddr::new(header.e_entry + load_bias),
            stack_top,
            brk,
        })
    }

    fn setup_user_stack(
        pt: &mut PageTable,
        alloc: &mut PhysAllocator,
    ) -> Result<VirtAddr, LoadError> {
        const STACK_PAGES: usize = 8;
        const STACK_BASE: u64 = 0x0000_7FFF_FFFF_0000;
        
        for i in 0..STACK_PAGES {
            let phys = alloc.allocate_zeroed()?;
            let virt = VirtAddr::new(STACK_BASE - (i as u64 + 1) * 0x1000);
            map_user_page(pt, virt, phys, true);
        }
        Ok(VirtAddr::new(STACK_BASE))
    }
}
```

### 3.3 Format binaire ExoOS

Les binaires ExoOS utilisent le format **ELF64** statique linkés contre `libexo` (la bibliothèque standard minimale d'ExoOS).

**Toolchain :** `x86_64-unknown-none` avec linker script custom.

**Linker script** (`userspace/link.ld`) :

```ld
ENTRY(_start)

SECTIONS {
    . = 0x400000;        /* Adresse de base pour binaires statiques */

    .text   : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }

    . = ALIGN(4096);
    .data   : { *(.data .data.*) }
    .bss    : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Heap initial (brk) */
    . = ALIGN(4096);
    __heap_start = .;

    /DISCARD/ : { *(.eh_frame) *(.note*) }
}
```

**Runtime minimal** (`userspace/libexo/start.rs`) :

```rust
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    core::arch::asm!(
        "xor rbp, rbp",          // Clear frame pointer (ABI)
        "mov rdi, rsp",          // Passer le stack pointer à exo_main
        "call exo_main",
        "mov rax, 60",           // syscall exit
        "xor rdi, rdi",
        "syscall",
        options(noreturn)
    );
}

#[no_mangle]
extern "C" fn exo_main(sp: *const u64) -> i32 {
    // Parser argc/argv depuis la stack
    let argc = unsafe { *sp } as usize;
    let argv = unsafe { sp.add(1) as *const *const u8 };
    // Appeler main
    extern "Rust" { fn main(args: &[&str]) -> i32; }
    unsafe { main(core::slice::from_raw_parts(argv as *const &str, argc)) }
}
```

---

## 4. Plan — Serveurs de Fonctionnement

### 4.1 Vue d'ensemble des serveurs requis

| Serveur | Rôle | Priorité |
|---|---|---|
| `vfs_server` | Système de fichiers virtuel (VFS) | 🔴 Critique |
| `process_server` | Création/gestion de processus | 🔴 Critique |
| `device_server` | Registre des devices | 🟡 Important |
| `input_server` | Agrégation entrées kbd/mouse | 🟡 Important |
| `terminal_server` | Rendu texte framebuffer | 🟡 Important |
| `time_server` | Horloge, timers | 🟢 Utile |
| `net_server` | Réseau (phase ultérieure) | ⚪ Futur |

### 4.2 VFS Server

**Fichier :** `userspace/servers/vfs_server/main.rs`

Le VFS server implémente une hiérarchie de fichiers en mémoire (tmpfs) pour la phase 1, avec un backend ExoFS en phase 2.

```rust
// Interface IPC du VFS server
pub enum VfsRequest {
    Open   { path: PathBuf, flags: OpenFlags },
    Close  { fd: FileDescriptor },
    Read   { fd: FileDescriptor, count: usize },
    Write  { fd: FileDescriptor, data: Vec<u8> },
    Stat   { path: PathBuf },
    Readdir{ path: PathBuf },
    Mkdir  { path: PathBuf },
    Unlink { path: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
}

pub enum VfsResponse {
    Ok,
    Fd(FileDescriptor),
    Data(Vec<u8>),
    Stat(FileStat),
    DirEntries(Vec<DirEntry>),
    Error(VfsError),
}

// Arbre VFS en mémoire
pub struct VfsNode {
    name:     String,
    kind:     NodeKind,
    children: BTreeMap<String, Box<VfsNode>>,
    data:     Vec<u8>,          // pour les fichiers réguliers
    stat:     FileStat,
}

pub struct VfsServer {
    root: VfsNode,
    open_files: HashMap<FileDescriptor, OpenFile>,
    next_fd: u64,
    // Backends montés
    mounts: Vec<(PathBuf, Box<dyn FsBackend>)>,
}

impl VfsServer {
    pub fn new() -> Self {
        let mut srv = Self {
            root: VfsNode::new_dir("/"),
            open_files: HashMap::new(),
            next_fd: 3, // 0=stdin, 1=stdout, 2=stderr réservés
            mounts: Vec::new(),
        };
        // Arborescence minimale
        srv.mkdir_p("/dev");
        srv.mkdir_p("/tmp");
        srv.mkdir_p("/bin");
        srv.mkdir_p("/home");
        srv.mkdir_p("/proc");
        // Fichiers virtuels /proc
        srv.create_virtual("/proc/version", ProcVersion);
        srv.create_virtual("/proc/uptime",  ProcUptime);
        srv
    }

    fn handle_request(&mut self, req: VfsRequest) -> VfsResponse {
        match req {
            VfsRequest::Open { path, flags } => self.open(path, flags),
            VfsRequest::Read { fd, count }   => self.read(fd, count),
            VfsRequest::Write{ fd, data }    => self.write(fd, data),
            VfsRequest::Stat { path }        => self.stat(path),
            VfsRequest::Readdir{ path }      => self.readdir(path),
            VfsRequest::Mkdir { path }       => self.mkdir(path),
            VfsRequest::Unlink{ path }       => self.unlink(path),
            VfsRequest::Close { fd }         => self.close(fd),
            VfsRequest::Rename{ from, to }   => self.rename(from, to),
        }
    }
}
```

### 4.3 Process Server

**Fichier :** `userspace/servers/process_server/main.rs`

```rust
pub enum ProcRequest {
    Spawn  { path: PathBuf, args: Vec<String>, env: HashMap<String,String> },
    Kill   { pid: Pid, signal: Signal },
    Wait   { pid: Pid },
    GetPid,
    GetPPid,
    List,           // pour `top` et `ps`
    GetInfo(Pid),
}

pub struct ProcessEntry {
    pub pid:        Pid,
    pub ppid:       Pid,
    pub name:       String,
    pub state:      ProcessState,
    pub cpu_usage:  u8,    // pourcentage
    pub mem_pages:  usize,
    pub start_time: u64,
}

impl ProcessServer {
    fn spawn(&mut self, path: PathBuf, args: Vec<String>) -> Result<Pid, ProcError> {
        // 1. Demander au VFS de lire le binaire
        let elf_data = self.vfs_client.read_file(&path)?;
        // 2. Créer un nouveau process via syscall kernel
        let pid = syscall::create_process()?;
        // 3. Loader l'ELF dans l'espace d'adressage du process
        let load_result = elf_loader::load_into_process(pid, &elf_data)?;
        // 4. Configurer stdin/stdout/stderr via VFS
        self.setup_stdio(pid)?;
        // 5. Démarrer le process (syscall resume)
        syscall::resume_process(pid, load_result.entry_point)?;
        Ok(pid)
    }

    fn kill(&mut self, pid: Pid, signal: Signal) -> Result<(), ProcError> {
        // Vérifier capability (un process ne peut tuer que ses enfants ou lui-même)
        // sauf si possède CAP_KILL_ANY
        self.check_kill_cap(pid, signal)?;
        syscall::send_signal(pid, signal)
    }
}
```

### 4.4 Device Server

```rust
// Registre des devices (type /dev/*)
pub struct DeviceServer {
    devices: HashMap<String, Box<dyn Device>>,
}

// Devices minimaux requis pour le shell
impl DeviceServer {
    pub fn new() -> Self {
        let mut srv = Self { devices: HashMap::new() };
        srv.register("null",    NullDevice);
        srv.register("zero",    ZeroDevice);
        srv.register("tty",     TtyDevice::new());     // lié au terminal_server
        srv.register("tty0",    TtyDevice::new());
        srv.register("random",  RandomDevice::new());  // PRNG simple
        srv
    }
}
```

---

## 5. Plan — ExoShell

### 5.1 Architecture du shell

```
ExoShell
├── repl.rs          — Read-Eval-Print Loop principal
├── lexer.rs         — Tokenisation de la ligne de commande
├── parser.rs        — Parsing : pipes, redirections, substitutions
├── builtins/        — Commandes intégrées (cd, pwd, exit, export...)
│   ├── cd.rs
│   ├── pwd.rs
│   ├── exit.rs
│   └── export.rs
├── executor.rs      — Exécution : builtins vs externes, fork/exec
├── env.rs           — Variables d'environnement + PATH
├── history.rs       — Historique de commandes (flèche haut/bas)
└── completion.rs    — Tab-completion (phase 2)
```

### 5.2 REPL Principal

**Fichier :** `userspace/apps/exoshell/repl.rs`

```rust
pub struct Shell {
    env:         Environment,
    history:     History,
    term_client: TerminalClient,    // IPC vers terminal_server
    input_client: InputClient,      // IPC vers input_server
    vfs_client:  VfsClient,
    proc_client: ProcessClient,
    cwd:         PathBuf,
}

impl Shell {
    pub fn run(&mut self) -> ! {
        self.print_banner();
        loop {
            self.print_prompt();
            let line = self.read_line();
            if line.trim().is_empty() { continue; }
            self.history.push(line.clone());
            match self.eval(&line) {
                Ok(ExitCode(0)) => {},
                Ok(ExitCode(n)) => self.print_error(n),
                Err(e)         => self.print_err(&format!("exosh: {}", e)),
            }
        }
    }

    fn print_prompt(&mut self) {
        let user = self.env.get("USER").unwrap_or("user");
        let cwd_str = self.format_cwd();
        self.term_client.write(&format!(
            "\x1b[32m{}@exoos\x1b[0m:\x1b[34m{}\x1b[0m$ ",
            user, cwd_str
        ));
    }

    fn format_cwd(&self) -> String {
        let home = self.env.get("HOME").unwrap_or("/home/user");
        let path = self.cwd.to_str().unwrap_or("?");
        if path.starts_with(home) {
            format!("~{}", &path[home.len()..])
        } else {
            path.to_string()
        }
    }
}
```

### 5.3 Lexer & Parser

```rust
// Tokens
#[derive(Debug, PartialEq)]
pub enum Token {
    Word(String),
    Pipe,               // |
    RedirectIn,         // <
    RedirectOut,        // >
    RedirectAppend,     // >>
    Background,         // &
    Semicolon,          // ;
    And,                // &&
    Or,                 // ||
    VarExpand(String),  // $VAR
    Subshell(String),   // $(...)
}

// AST
pub enum Command {
    Simple { args: Vec<String>, redirects: Vec<Redirect> },
    Pipeline(Vec<Command>),
    Sequence(Vec<Command>),
    Background(Box<Command>),
    And(Box<Command>, Box<Command>),
    Or(Box<Command>, Box<Command>),
}
```

### 5.4 Commandes Intégrées (Builtins)

#### `cd`
```rust
pub fn builtin_cd(shell: &mut Shell, args: &[&str]) -> Result<ExitCode, ShellError> {
    let target = match args.get(0) {
        Some(&"-") => shell.env.get("OLDPWD")
                         .map(PathBuf::from)
                         .unwrap_or_else(|| PathBuf::from("/")),
        Some(path) => {
            if path.starts_with('/') {
                PathBuf::from(path)
            } else {
                shell.cwd.join(path)
            }
        }
        None => PathBuf::from(
            shell.env.get("HOME").unwrap_or("/home/user")
        ),
    };

    // Résolution des .. et .
    let resolved = resolve_path(&target);

    match shell.vfs_client.stat(&resolved) {
        Ok(stat) if stat.is_dir() => {
            shell.env.set("OLDPWD", shell.cwd.to_str().unwrap_or("/"));
            shell.cwd = resolved;
            shell.env.set("PWD", shell.cwd.to_str().unwrap_or("/"));
            Ok(ExitCode(0))
        }
        Ok(_) => Err(ShellError::NotADirectory(resolved)),
        Err(_) => Err(ShellError::NoSuchFile(resolved)),
    }
}
```

#### `pwd`
```rust
pub fn builtin_pwd(shell: &Shell, _args: &[&str]) -> Result<ExitCode, ShellError> {
    shell.term_client.writeln(shell.cwd.to_str().unwrap_or("?"));
    Ok(ExitCode(0))
}
```

#### `exit`
```rust
pub fn builtin_exit(shell: &Shell, args: &[&str]) -> Result<ExitCode, ShellError> {
    let code = args.get(0)
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    syscall::exit(code);
}
```

### 5.5 Commandes Externes (binaires)

Les commandes comme `cat`, `touch`, `top`, `kill` sont des **binaires ELF userspace** dans `/bin/`.

#### `/bin/cat` — `userspace/apps/cat/main.rs`
```rust
fn main(args: &[&str]) -> i32 {
    if args.is_empty() {
        // Lire stdin
        return cat_fd(STDIN_FD);
    }
    let mut code = 0;
    for path in args {
        match vfs::open(path, OpenFlags::RDONLY) {
            Ok(fd) => { cat_fd(fd); vfs::close(fd); }
            Err(e) => { eprintln!("cat: {}: {}", path, e); code = 1; }
        }
    }
    code
}

fn cat_fd(fd: FileDescriptor) -> i32 {
    let mut buf = [0u8; 4096];
    loop {
        match vfs::read(fd, &mut buf) {
            Ok(0)  => break,
            Ok(n)  => { stdout_write(&buf[..n]); }
            Err(e) => { eprintln!("cat: read error: {}", e); return 1; }
        }
    }
    0
}
```

#### `/bin/touch` — `userspace/apps/touch/main.rs`
```rust
fn main(args: &[&str]) -> i32 {
    if args.is_empty() {
        eprintln!("touch: missing operand");
        return 1;
    }
    let mut code = 0;
    for path in args {
        let result = vfs::open(path, OpenFlags::WRONLY | OpenFlags::CREATE)
            .and_then(|fd| vfs::close(fd));
        if let Err(e) = result {
            eprintln!("touch: {}: {}", path, e);
            code = 1;
        }
    }
    code
}
```

#### `/bin/top` — `userspace/apps/top/main.rs`
```rust
fn main(_args: &[&str]) -> i32 {
    let proc_client = ProcessClient::connect();
    loop {
        term::clear_screen();
        term::move_cursor(0, 0);
        println!("ExoOS top — {} processes", proc_client.count());
        println!("{:<8} {:<20} {:<8} {:<8} {}", "PID", "NAME", "CPU%", "MEM(KB)", "STATE");
        println!("{}", "─".repeat(60));

        let mut procs = proc_client.list().unwrap_or_default();
        procs.sort_by(|a, b| b.cpu_usage.cmp(&a.cpu_usage));

        for p in &procs {
            println!("{:<8} {:<20} {:<8} {:<8} {:?}",
                p.pid, &p.name[..p.name.len().min(20)],
                p.cpu_usage, p.mem_pages * 4, p.state);
        }

        // Attendre 1s puis refresh (ou 'q' pour quitter)
        if let Some(key) = input::poll_key(Duration::from_secs(1)) {
            if key == KeyCode::Char('q') { break; }
        }
    }
    0
}
```

#### `/bin/kill` — `userspace/apps/kill/main.rs`
```rust
fn main(args: &[&str]) -> i32 {
    let (signal, pid_strs) = parse_kill_args(args);

    let mut code = 0;
    let proc_client = ProcessClient::connect();
    for pid_str in pid_strs {
        match pid_str.parse::<u64>() {
            Ok(pid) => {
                if let Err(e) = proc_client.kill(pid, signal) {
                    eprintln!("kill: ({}): {}", pid, e);
                    code = 1;
                }
            }
            Err(_) => { eprintln!("kill: invalid PID: {}", pid_str); code = 1; }
        }
    }
    code
}

fn parse_kill_args<'a>(args: &'a [&str]) -> (Signal, &'a [&'a str]) {
    if args.get(0).map(|s| s.starts_with('-')).unwrap_or(false) {
        let sig_str = &args[0][1..];
        let sig = Signal::from_name(sig_str)
            .or_else(|| sig_str.parse::<u32>().ok().map(Signal::from_num))
            .unwrap_or(Signal::SIGTERM);
        (sig, &args[1..])
    } else {
        (Signal::SIGTERM, args)
    }
}
```

---

## 6. Bibliothèque Userspace `libexo`

`libexo` est la bibliothèque standard minimale d'ExoOS. Elle fournit les bindings syscall, les abstractions IPC et les types partagés.

### 6.1 Structure

```
userspace/libexo/
├── src/
│   ├── lib.rs
│   ├── syscall.rs      — Wrappers syscall (read, write, exit, mmap, ...)
│   ├── ipc.rs          — Client IPC générique
│   ├── vfs.rs          — Client VFS (open/read/write/close/stat/...)
│   ├── proc.rs         — Client Process server
│   ├── input.rs        — Client Input server
│   ├── term.rs         — Client Terminal server (write, clear, ...)
│   ├── env.rs          — Variables d'environnement
│   ├── path.rs         — Manipulation de chemins
│   ├── fmt.rs          — Formatage (print!, println!, eprintln!)
│   └── start.rs        — Point d'entrée _start
└── Cargo.toml
```

### 6.2 Wrappers Syscall

```rust
// userspace/libexo/src/syscall.rs

pub mod nr {
    pub const READ:      u64 = 0;
    pub const WRITE:     u64 = 1;
    pub const OPEN:      u64 = 2;
    pub const CLOSE:     u64 = 3;
    pub const MMAP:      u64 = 9;
    pub const EXIT:      u64 = 60;
    pub const GETPID:    u64 = 39;
    pub const IPC_SEND:  u64 = 200;  // ExoOS custom
    pub const IPC_RECV:  u64 = 201;
    pub const CAP_GRANT: u64 = 210;
}

#[inline(always)]
pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr,
        in("rdi") a1,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

// ... syscall2, syscall3, syscall4, syscall5, syscall6

pub fn write(fd: u64, buf: &[u8]) -> Result<usize, SyscallError> {
    let ret = unsafe { syscall3(nr::WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) };
    if ret < 0 { Err(SyscallError::from_errno(-ret as u32)) } else { Ok(ret as usize) }
}

pub fn exit(code: i32) -> ! {
    unsafe { syscall1(nr::EXIT, code as u64); }
    unreachable!()
}
```

---

## 7. Feuille de Route d'Implémentation

### Phase 1 — Fondations (Priorité absolue)

| # | Tâche | Fichier(s) | Effort |
|---|---|---|---|
| P1-01 | Corriger BUG-CRIT-01 cr3 hardcodé | `kernel/loader/elf_loader.rs` | 1h |
| P1-02 | Corriger BUG-CRIT-02 PTE_USER manquant | `kernel/memory/paging.rs` | 1h |
| P1-03 | Driver framebuffer depuis UEFI GOP | `kernel/drivers/display/framebuffer.rs` | 3h |
| P1-04 | Terminal server (texte, scroll) | `userspace/servers/terminal_server/` | 4h |
| P1-05 | Driver clavier PS/2 + scancode set 2 | `drivers/input/ps2_kbd.rs` | 3h |
| P1-06 | Input server | `userspace/servers/input_server/` | 2h |
| P1-07 | libexo (syscall wrappers + start.rs) | `userspace/libexo/` | 4h |
| P1-08 | ELF loader robuste | `kernel/loader/elf_loader.rs` | 5h |
| P1-09 | VFS server (tmpfs) | `userspace/servers/vfs_server/` | 6h |
| P1-10 | Process server (spawn + kill + list) | `userspace/servers/process_server/` | 5h |

### Phase 2 — ExoShell fonctionnel

| # | Tâche | Fichier(s) | Effort |
|---|---|---|---|
| P2-01 | ExoShell REPL + lexer + parser | `userspace/apps/exoshell/` | 6h |
| P2-02 | Builtins : cd, pwd, exit, export | `exoshell/builtins/` | 2h |
| P2-03 | `/bin/cat` | `userspace/apps/cat/` | 1h |
| P2-04 | `/bin/touch` | `userspace/apps/touch/` | 1h |
| P2-05 | `/bin/kill` | `userspace/apps/kill/` | 1h |
| P2-06 | `/bin/top` | `userspace/apps/top/` | 3h |
| P2-07 | `/bin/ls` | `userspace/apps/ls/` | 2h |
| P2-08 | `/bin/echo`, `/bin/env`, `/bin/mkdir`, `/bin/rm` | divers | 2h |
| P2-09 | Historique flèche haut/bas | `exoshell/history.rs` | 1h |
| P2-10 | Pipes et redirections | `exoshell/executor.rs` | 4h |

### Phase 3 — Consolidation

| # | Tâche | Effort |
|---|---|---|
| P3-01 | ExoFS backend VFS (remplace tmpfs) | 8h |
| P3-02 | Persistence filesystem (lecture disque) | 6h |
| P3-03 | Tab-completion | 3h |
| P3-04 | ANSI escape codes complets | 2h |
| P3-05 | Signaux SIGINT (Ctrl+C), SIGTSTP (Ctrl+Z) | 3h |
| P3-06 | Job control (fg, bg, jobs) | 4h |

---

## 8. Points de Vigilance & Risques

### 8.1 IPC & Deadlocks

La chaîne `ExoShell → VFS server → Process server` peut créer des deadlocks si les canaux IPC sont synchrones et que deux serveurs s'attendent mutuellement. **Solution :** utiliser exclusivement des canaux IPC **asynchrones non-bloquants** pour les requêtes inter-serveurs, avec timeout (100ms par défaut).

### 8.2 Sécurité Capability

Tout accès à un device ou serveur doit passer par une capability obtenue au démarrage via le kernel. ExoShell ne doit **jamais** avoir `CAP_RAW_IO` — il passe par `input_server` et `terminal_server`.

Matrice capability minimale pour ExoShell :

| Capability | Justification |
|---|---|
| `CAP_VFS_CLIENT` | Accès lecture/écriture fichiers |
| `CAP_PROC_SPAWN` | Lancer des sous-processus |
| `CAP_PROC_KILL` (restreinte) | Kill ses propres enfants seulement |
| `CAP_INPUT_SUBSCRIBE` | Recevoir les événements clavier |
| `CAP_TERM_WRITE` | Écrire dans le terminal |

### 8.3 Bootstrap Order

L'ordre de démarrage des serveurs est critique :

```
1. kernel init (arch, memory, sched)
2. framebuffer driver (kernel)
3. terminal_server  ← premier affichage possible
4. input_server
5. vfs_server (tmpfs)
6. device_server
7. process_server
8. exoshell         ← premier prompt
```

Si un serveur manque dans la chaîne, tous les suivants paniquent. Implémenter un **watcher de boot** qui affiche l'état de chaque serveur sur le terminal pendant l'initialisation.

### 8.4 Police de Caractères

Embarquer la police en dur dans le binaire `terminal_server` (tableau `[u8; N]`). Ne **pas** dépendre du VFS pour charger la police — c'est un bootstrap circulaire.

---

## 9. Structure de Répertoires Proposée

```
ExoOS/
├── kernel/
│   ├── arch/x86_64/
│   ├── memory/          (paging.rs — BUG-CRIT-02 ici)
│   ├── scheduler/
│   ├── ipc/
│   ├── loader/          (elf_loader.rs — BUG-CRIT-01 ici)
│   ├── drivers/
│   │   └── display/     (framebuffer.rs — NOUVEAU)
│   └── security/        (exoshield)
│
├── drivers/
│   └── input/           (ps2_kbd.rs, ps2_mouse.rs — NOUVEAU)
│
└── userspace/
    ├── libexo/           (NOUVEAU — bibliothèque standard minimale)
    ├── servers/
    │   ├── input_server/ (NOUVEAU)
    │   ├── terminal_server/ (NOUVEAU)
    │   ├── vfs_server/   (NOUVEAU)
    │   ├── process_server/ (NOUVEAU)
    │   └── device_server/  (NOUVEAU)
    └── apps/
        ├── exoshell/     (NOUVEAU)
        ├── cat/          (NOUVEAU)
        ├── touch/        (NOUVEAU)
        ├── top/          (NOUVEAU)
        ├── kill/         (NOUVEAU)
        ├── ls/           (NOUVEAU)
        ├── echo/         (NOUVEAU)
        ├── mkdir/        (NOUVEAU)
        └── rm/           (NOUVEAU)
```

---

## 10. Critère de Validation

Le plan est considéré accompli lorsque la séquence suivante fonctionne sans crash ni triple fault :

```
exoos boot...
[OK] framebuffer
[OK] terminal_server
[OK] input_server
[OK] vfs_server (tmpfs)
[OK] process_server
[OK] exoshell

user@exoos:/$ pwd
/
user@exoos:/$ mkdir /tmp/test
user@exoos:/$ cd /tmp/test
user@exoos:/tmp/test$ touch hello.txt
user@exoos:/tmp/test$ echo "bonjour ExoOS" > hello.txt
user@exoos:/tmp/test$ cat hello.txt
bonjour ExoOS
user@exoos:/tmp/test$ top
[affichage des processus]
user@exoos:/tmp/test$ kill 5
user@exoos:/tmp/test$ cd ..
user@exoos:/tmp$ pwd
/tmp
```

---

*Document produit par claude-gamma — USERSPACE-001 — 2026-05-05*  
*Prochain document : USERSPACE-002 — Implémentation Phase 1 (corrections + drivers)*
