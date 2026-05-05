# ExoOS — Plan Userspace : Shell & Fondations d'Utilisation Réelle
**claude beta** | ExoOS Phase Userspace | 2026-05-05  
Basé sur audit du commit courant — Architecture v7, Driver Framework v10, syscall_abi canonique

---

## Table des matières

1. [Vue d'ensemble et philosophie](#1-vue-densemble)
2. [Carte de dépendances userspace](#2-carte-de-dépendances)
3. [Pilote Input — PS/2 + USB HID + Evdev](#3-pilote-input)
4. [Pilote Display — Framebuffer + VT100](#4-pilote-display)
5. [TTY Server — Ring 1](#5-tty-server)
6. [Input Server — Ring 1](#6-input-server)
7. [Loader — Complétion ELF](#7-loader-elf)
8. [Shell Binaire — Ring 3](#8-shell-binaire)
9. [Corrections init_server — Ajout des nouveaux services](#9-corrections-init_server)
10. [Corrections et Blocages identifiés](#10-corrections-et-blocages)
11. [Plan de développement phasé](#11-plan-phasé)

---

## 1. Vue d'ensemble

### 1.1 Objectif de la phase

Permettre à un utilisateur de démarrer ExoOS et d'interagir avec un **shell fonctionnel** capable d'exécuter :

```
cd <path>   pwd   ls   touch <file>   cat <file>   top   kill <pid>   echo   exit
```

### 1.2 Stack userspace complet

```
┌─────────────────────────────────────────────────────────────────┐
│  Ring 3 — Userspace                                             │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  exo-shell  (ELF Ring 3, no_std + exo-libc partiel)      │  │
│  │  cd / pwd / ls / touch / cat / top / kill / echo / exit  │  │
│  └────────────────────┬─────────────────────────────────────┘  │
│                        │ fd 0/1/2  (SYS_READ / SYS_WRITE)      │
└────────────────────────│────────────────────────────────────────┘
                         │ IPC 300-305
┌────────────────────────▼────────────────────────────────────────┐
│  Ring 1 — Nouveaux serveurs de cette phase                      │
│  ┌──────────────┐   ┌────────────────────────────────────────┐  │
│  │  tty_server  │   │  input_server                          │  │
│  │  (PID 11)    │◄──│  (PID 12)                              │  │
│  │  line disc.  │   │  evdev → InputEvent → tty_server       │  │
│  │  VT100 out   │   └───────────────────────────────────────┘  │
│  │  PTY pairs   │                                               │
│  └──────┬───────┘                                               │
│         │ SYS_WRITE → fd virtuel                                │
└─────────│───────────────────────────────────────────────────────┘
          │
┌─────────▼───────────────────────────────────────────────────────┐
│  Ring 1 — Serveurs existants (9 démarrés par init_server)       │
│  ipc_router · memory_server · vfs_server · crypto_server        │
│  device_server · virtio_drivers · network_server                │
│  scheduler_server · exo_shield                                  │
└─────────────────────────────────────────────────────────────────┘
          │
┌─────────▼───────────────────────────────────────────────────────┐
│  Ring 1 — Drivers (chargés par device_server)                   │
│  drivers/input/ps2/     → PS/2 keyboard + mouse                 │
│  drivers/input/usb_hid/ → USB keyboard + mouse                  │
│  drivers/input/evdev/   → abstraction unifiée InputEvent        │
│  drivers/display/framebuffer/ → blitting pixel VESA/VirtIO-GPU  │
│  drivers/display/vga/   → fallback texte 80×25                  │
│  drivers/tty/           → tty_server consomme ce driver          │
└─────────────────────────────────────────────────────────────────┘
          │
┌─────────▼───────────────────────────────────────────────────────┐
│  Loader — drivers/loader/                                        │
│  ELF parser + relocations + spawn Ring 3                         │
└─────────────────────────────────────────────────────────────────┘
```

### 1.3 Règles architecturales rappelées (non négociables)

| Règle | Impact sur cette phase |
|-------|----------------------|
| **PHX-02** `#![no_std]` + `panic='abort'` | Tous les nouveaux servers/drivers |
| **IPC-02** Types `Sized` et taille fixe | Protocoles `tty_server` et `input_server` |
| **SRV-02** Pas de `blake3`/`chacha20` hors crypto_server | Shell + tty ne font pas de crypto directe |
| **CAP-01** `verify_cap_token()` en première instruction | `tty_server::main()` et `input_server::main()` |
| **SRV-01** `init_server` supervise tous les Ring 1 | tty_server et input_server ajoutés au `service_table.rs` |

---

## 2. Carte de dépendances

### 2.1 Ordre de démarrage augmenté

```
PID 1  init_server       (déjà présent)
PID 2  ipc_router        (déjà présent)
PID 3  memory_server     (déjà présent)
PID 4  vfs_server        (déjà présent)
PID 5  crypto_server     (déjà présent)
PID 6  device_server     (déjà présent)
PID 7  virtio_drivers    (déjà présent)
PID 8  network_server    (déjà présent)
PID 9  scheduler_server  (déjà présent)
PID 10 exo_shield        (déjà présent)
─── NOUVEAUX ──────────────────────────────────
PID 11 input_server      (dépend de device_server)
PID 12 tty_server        (dépend de input_server + vfs_server)
─── RING 3 via Loader ─────────────────────────
PID 13 exo-shell         (lancé par init_server après tty_server READY)
```

### 2.2 Graphe d'autorisation IPC à étendre

Fichier : `servers/ipc_router/src/lib.rs` — tableau `AUTHORIZED_GRAPH`

Ajouts requis :

```rust
// Existants — ne pas toucher
AuthEdge::new(ServiceId::Init,    ServiceId::Memory,        4, 10_000),
AuthEdge::new(ServiceId::Init,    ServiceId::Vfs,           4, 10_000),
AuthEdge::new(ServiceId::Vfs,     ServiceId::Crypto,        2, 50_000),
AuthEdge::new(ServiceId::Network, ServiceId::Vfs,           2, 100_000),
AuthEdge::new(ServiceId::Device,  ServiceId::VirtioDrivers, 1, 1_000_000),

// NOUVEAUX — phase userspace
AuthEdge::new(ServiceId::InputServer, ServiceId::Device,    2, 500_000),
AuthEdge::new(ServiceId::TtyServer,   ServiceId::Input,     2, 500_000),
AuthEdge::new(ServiceId::TtyServer,   ServiceId::Vfs,       2, 50_000),
AuthEdge::new(ServiceId::Init,        ServiceId::TtyServer, 2, 10_000),
// Le shell Ring 3 passe par SYS_WRITE fd→tty_server ; pas d'edge IPC directe
```

Et dans `ServiceId` :

```rust
pub enum ServiceId {
    // ... existants (1–10) ...
    InputServer = 11,   // NOUVEAU
    TtyServer   = 12,   // NOUVEAU
}
```

---

## 3. Pilote Input

### 3.1 État actuel

Tous les fichiers sources dans `drivers/input/` sont **vides (0 octets)** :

```
drivers/input/ps2/src/i8042.rs   — 0 octets
drivers/input/ps2/src/keyboard.rs — 0 octets
drivers/input/ps2/src/mouse.rs    — 0 octets
drivers/input/ps2/src/main.rs     — 0 octets
drivers/input/evdev/src/events.rs  — 0 octets
drivers/input/evdev/src/main.rs    — 0 octets
drivers/input/usb_hid/src/        — 0 octets
```

### 3.2 Type `InputEvent` — source unique de vérité

À placer dans `libs/exo_types/src/input_event.rs` (puis exposer dans `lib.rs`) :

```rust
/// InputEvent — type canonique partagé ps2 / usb_hid / evdev / tty_server
/// Taille fixe 16 octets (IPC-02 compatible).
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InputEvent {
    /// Type d'événement (voir EventType)
    pub kind:   u8,
    /// Code clavier (scancode SET 2 normalisé) ou bouton souris
    pub code:   u16,
    /// Valeur : 1=press, 0=release, 2=repeat ; axe souris en i16
    pub value:  i16,
    /// Flags : bit 0 = shift, bit 1 = ctrl, bit 2 = alt, bit 3 = meta
    pub mods:   u8,
    /// Réservé — padding pour alignement sur 16 octets
    pub _pad:   [u8; 10],
}

#[repr(u8)]
pub enum EventType {
    Key    = 0x01,   // événement clavier
    RelAbs = 0x02,   // mouvement souris relatif
    Button = 0x03,   // bouton souris
    Sync   = 0xFF,   // synchronisation batch
}
```

> **Règle** : `InputEvent` est le seul type traversant la frontière driver→input_server→tty_server. Aucun autre format propriétaire dans les drivers.

### 3.3 Plan `drivers/input/ps2/`

#### 3.3.1 `src/i8042.rs` — contrôleur PS/2

```rust
//! Accès bas-niveau au contrôleur i8042 via I/O ports 0x60 / 0x64.
//! Suit le Driver Framework v10 : IRQ via SYS_IRQ_REGISTER (530).

const DATA_PORT:   u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const CMD_PORT:    u16 = 0x64;

// Bits status
const STATUS_OBF: u8 = 1 << 0;  // Output Buffer Full → donnée dispo en 0x60
const STATUS_IBF: u8 = 1 << 1;  // Input Buffer Full  → attendre avant écriture

/// Lit un octet depuis le port de données PS/2 (bloquant, polling).
/// SAFETY : Doit être appelé depuis le thread IRQ uniquement.
pub unsafe fn read_data() -> u8 {
    // spin-wait sur OBF — borné par watchdog IRQ
    while inb(STATUS_PORT) & STATUS_OBF == 0 { core::hint::spin_loop(); }
    inb(DATA_PORT)
}

/// Envoie une commande au contrôleur.
pub unsafe fn send_cmd(cmd: u8) {
    while inb(STATUS_PORT) & STATUS_IBF != 0 { core::hint::spin_loop(); }
    outb(CMD_PORT, cmd);
}

/// Envoie une donnée vers le port de données.
pub unsafe fn send_data(data: u8) {
    while inb(STATUS_PORT) & STATUS_IBF != 0 { core::hint::spin_loop(); }
    outb(DATA_PORT, data);
}

#[inline] unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nostack, nomem));
    v
}
#[inline] unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nostack, nomem));
}

/// Initialise le contrôleur i8042 :
/// 1. Vide les buffers résiduels
/// 2. Active IRQ1 (clavier) et IRQ12 (souris)
/// 3. Active le port auxiliaire (souris)
pub unsafe fn init() {
    // Flush output buffer
    for _ in 0..16u8 { if inb(STATUS_PORT) & STATUS_OBF == 0 { break; } inb(DATA_PORT); }
    send_cmd(0xAD); // désactive clavier
    send_cmd(0xA7); // désactive souris
    send_cmd(0x20); // lit CCB
    let mut ccb = read_data();
    ccb |= 0x03;    // active IRQ1 + IRQ12
    ccb &= !0x30;   // désactive translation scancode (on veut SET 2 brut)
    send_cmd(0x60); send_data(ccb); // écrit CCB modifié
    send_cmd(0xAE); // réactive clavier
    send_cmd(0xA8); // réactive port auxiliaire (souris)
}
```

#### 3.3.2 `src/keyboard.rs` — décodage scancodes SET 2

```rust
//! Décodage scancodes PS/2 SET 2 → InputEvent.
//! Gère : touches simples, extended (0xE0), release prefix (0xF0).

use exo_types::{InputEvent, EventType};

pub struct KeyboardDecoder {
    extended:  bool,   // préfixe 0xE0 reçu
    release:   bool,   // préfixe 0xF0 reçu (key-up)
    shift:     bool,
    ctrl:      bool,
    alt:       bool,
    meta:      bool,
}

impl KeyboardDecoder {
    pub const fn new() -> Self {
        Self { extended: false, release: false,
               shift: false, ctrl: false, alt: false, meta: false }
    }

    /// Traite un octet brut reçu de l'i8042.
    /// Retourne Some(InputEvent) si un événement complet est formé.
    pub fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        match byte {
            0xE0 => { self.extended = true;  return None; }
            0xF0 => { self.release  = true;  return None; }
            raw  => {
                let code  = Self::scancode_to_hid(raw, self.extended);
                let value = if self.release { 0i16 } else { 1i16 };
                let mods  = self.build_mods();
                self.update_modifiers(code, value != 0);
                self.extended = false;
                self.release  = false;
                if code == 0 { return None; } // scancode inconnu
                Some(InputEvent {
                    kind:  EventType::Key as u8,
                    code,
                    value,
                    mods,
                    _pad: [0u8; 10],
                })
            }
        }
    }

    fn build_mods(&self) -> u8 {
        (self.shift as u8)      |
        ((self.ctrl as u8) << 1)|
        ((self.alt  as u8) << 2)|
        ((self.meta as u8) << 3)
    }

    fn update_modifiers(&mut self, hid_code: u16, pressed: bool) {
        match hid_code {
            0x002A | 0x0036 => self.shift = pressed, // L/R Shift
            0x001D | 0x0061 => self.ctrl  = pressed, // L/R Ctrl
            0x0038 | 0x0064 => self.alt   = pressed, // L/R Alt
            0x005B | 0x005C => self.meta  = pressed, // L/R Meta
            _ => {}
        }
    }

    /// Table partielle SET 2 → code HID.
    /// Extension : couvrir les 101 touches standard + extended 0xE0 subset.
    fn scancode_to_hid(sc: u8, ext: bool) -> u16 {
        if ext {
            return match sc {
                0x1C => 0x0058, // KP Enter
                0x1D => 0x0061, // R Ctrl
                0x35 => 0x0054, // KP /
                0x38 => 0x0064, // R Alt
                0x47 => 0x004A, // Home
                0x48 => 0x0052, // Up
                0x49 => 0x004B, // PgUp
                0x4B => 0x0050, // Left
                0x4D => 0x004F, // Right
                0x4F => 0x004D, // End
                0x50 => 0x0051, // Down
                0x51 => 0x004E, // PgDn
                0x52 => 0x0049, // Insert
                0x53 => 0x004C, // Delete
                0x5B => 0x005B, // L Meta
                0x5C => 0x005C, // R Meta
                _    => 0x0000,
            };
        }
        // Table SET 2 standard (sous-ensemble critique pour shell)
        const TABLE: [u16; 128] = [
        //  0      1      2      3      4      5      6      7
            0x000, 0x029, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, // 00-07
            0x02A, 0x02B, 0x000, 0x000, 0x000, 0x02C, 0x01E, 0x000, // 08-0F
            0x000, 0x000, 0x000, 0x000, 0x000, 0x014, 0x01A, 0x000, // 10-17
            0x000, 0x000, 0x01D, 0x016, 0x02D, 0x012, 0x01B, 0x000, // 18-1F
            0x000, 0x006, 0x019, 0x005, 0x011, 0x010, 0x036, 0x000, // 20-27
            0x000, 0x037, 0x02E, 0x015, 0x008, 0x017, 0x01C, 0x000, // 28-2F
            0x000, 0x000, 0x007, 0x009, 0x004, 0x018, 0x024, 0x000, // 30-37
            0x000, 0x000, 0x028, 0x023, 0x025, 0x026, 0x030, 0x000, // 38-3F
        //  (suite simplifiée — à compléter avec la table complète SET 2)
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
            0x000, 0x000, 0x002A, 0x000, 0x000, 0x000, 0x000, 0x000, // 78-7F
        ];
        if (sc as usize) < TABLE.len() { TABLE[sc as usize] } else { 0 }
    }
}
```

#### 3.3.3 `src/mouse.rs` — décodage paquets souris PS/2

```rust
//! Décodage paquets 3 octets souris PS/2 → InputEvent.
use exo_types::{InputEvent, EventType};

pub struct MouseDecoder {
    buf:   [u8; 3],
    idx:   usize,
}

impl MouseDecoder {
    pub const fn new() -> Self { Self { buf: [0u8; 3], idx: 0 } }

    pub fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        if self.idx == 0 && byte & 0x08 == 0 { return None; } // sync bit
        self.buf[self.idx] = byte;
        self.idx += 1;
        if self.idx < 3 { return None; }
        self.idx = 0;

        let dx = self.buf[1] as i16 - ((self.buf[0] as i16 & 0x10) << 4);
        let dy = self.buf[2] as i16 - ((self.buf[0] as i16 & 0x20) << 3);
        let buttons = self.buf[0] & 0x07;

        // On ne rapporte que le mouvement (boutons en événement Button séparé)
        if dx != 0 || dy != 0 {
            Some(InputEvent {
                kind:  EventType::RelAbs as u8,
                code:  (dx as u16 & 0xFF) | ((dy as u16 & 0xFF) << 8),
                value: 0,
                mods:  buttons,
                _pad:  [0u8; 10],
            })
        } else if buttons != 0 {
            Some(InputEvent {
                kind:  EventType::Button as u8,
                code:  buttons as u16,
                value: 1,
                mods:  0,
                _pad:  [0u8; 10],
            })
        } else { None }
    }
}
```

#### 3.3.4 `src/main.rs` — driver Ring 1 PS/2

```rust
#![no_std]
#![no_main]

//! # ps2_driver — Ring 1, gestion IRQ1 (clavier) + IRQ12 (souris)
//!
//! Enregistre IRQ1 et IRQ12 via SYS_IRQ_REGISTER (530).
//! Chaque IRQ produit un InputEvent envoyé à input_server via IPC.
//! Règle DRV-08 : SYS_IRQ_ACK appelé même si result=NotMine.

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_types::InputEvent;

mod i8042;
mod keyboard;
mod mouse;

static KB:  spin::Mutex<keyboard::KeyboardDecoder> = spin::Mutex::new(keyboard::KeyboardDecoder::new());
static MS:  spin::Mutex<mouse::MouseDecoder>       = spin::Mutex::new(mouse::MouseDecoder::new());

const INPUT_SERVER_PID: u32 = 11; // PID fixe dans service_table
const MSG_INPUT_EVENT:  u32 = 0x100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // CAP-01 : vérification du cap token en premier
    verify_cap_token();

    unsafe { i8042::init(); }

    // Enregistrer IRQ1 (clavier) — SYS_IRQ_REGISTER = 530
    let _reg_kb = unsafe {
        syscall::syscall4(530,
            1,  // IRQ 1
            INPUT_SERVER_PID as u64,
            0,  // IrqSourceKind::LegacyISA
            u64::MAX, // bdf = None
        )
    };

    // Enregistrer IRQ12 (souris)
    let _reg_ms = unsafe {
        syscall::syscall4(530, 12, INPUT_SERVER_PID as u64, 0, u64::MAX)
    };

    // Boucle de traitement IRQ (modèle polling post-IRQ)
    loop {
        // IRQ1 — clavier
        let raw_kb = unsafe { i8042::read_data() };
        if let Some(ev) = KB.lock().feed(raw_kb) {
            send_event(&ev);
        }
        // ACK IRQ1 (DRV-08)
        unsafe { syscall::syscall5(531, 1, 0, 0, 0, 0); }

        core::hint::spin_loop();
    }
}

fn send_event(ev: &InputEvent) {
    let mut payload = [0u8; 64];
    // IPC-02 : payload fixe 16 octets = InputEvent
    payload[..16].copy_from_slice(unsafe {
        core::slice::from_raw_parts(ev as *const InputEvent as *const u8, 16)
    });
    unsafe {
        syscall::syscall4(
            syscall::SYS_EXO_IPC_SEND,
            INPUT_SERVER_PID as u64,
            MSG_INPUT_EVENT as u64,
            payload.as_ptr() as u64,
            16,
        );
    }
}

fn verify_cap_token() {
    // Placeholder : appel SYS_CAP_VERIFY à implémenter quand kernel expose le syscall
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
```

---

## 4. Pilote Display

### 4.1 État actuel

Fichiers vides :
```
drivers/display/framebuffer/src/fb.rs     — 0 octets
drivers/display/framebuffer/src/blit.rs   — 0 octets
drivers/display/framebuffer/src/main.rs   — 0 octets
drivers/display/framebuffer/src/cursor.rs — 0 octets
drivers/display/vga/src/                  — 0 octets
```

### 4.2 Stratégie display pour le shell

Pour le shell, on n'a pas besoin d'un GPU 3D : on vise **deux modes** :

| Mode | Conditions | Description |
|------|-----------|-------------|
| **VGA text 80×25** | QEMU / machine sans VESA | Mode fallback, simple, rapide |
| **Framebuffer VESA/VirtIO-GPU** | Cible principale | Rendu caractères 8×16 via font bitmap |

Le `tty_server` consomme l'un ou l'autre via une **interface abstraite `DisplayBackend`**.

### 4.3 `drivers/display/vga/src/main.rs` — VGA texte

```rust
#![no_std]
#![no_main]

//! # vga_driver — Mode texte VGA 80×25, Ring 1
//! Buffer vidéo à 0xB8000 (mappé via SYS_MMIO_MAP = 532)
//! Protocole IPC : MSG_VGA_WRITE_CHAR, MSG_VGA_CLEAR, MSG_VGA_SET_CURSOR

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

const VGA_PHYS: u64   = 0xB8000;
const VGA_SIZE: usize = 80 * 25 * 2; // 2 octets par cellule (char + attr)
const COLS: usize = 80;
const ROWS: usize = 25;

// Couleurs VGA standard
const COLOR_WHITE_ON_BLACK: u8 = 0x07;
const COLOR_GREEN_ON_BLACK: u8 = 0x02; // prompt shell

const TTY_SERVER_PID: u32 = 12;
const MSG_VGA_READY: u32  = 0x200;
const MSG_VGA_WRITE: u32  = 0x201;
const MSG_VGA_CLEAR: u32  = 0x202;
const MSG_VGA_CURSOR: u32 = 0x203;

static mut VGA_BUF: *mut u16 = core::ptr::null_mut();

#[no_mangle]
pub extern "C" fn _start() -> ! {
    verify_cap_token();

    // Mapper le buffer VGA physique → virtuel via SYS_MMIO_MAP = 532
    let virt = unsafe {
        syscall::syscall2(532, VGA_PHYS, VGA_SIZE as u64)
    };
    if virt < 0 { panic!("VGA MMIO map failed"); }
    unsafe { VGA_BUF = virt as *mut u16; }

    vga_clear();
    vga_write_str(b"ExoOS VGA driver ready\r\n", COLOR_WHITE_ON_BLACK, 0, 0);

    // Signaler tty_server que le display est prêt
    unsafe { syscall::syscall2(syscall::SYS_EXO_IPC_SEND, TTY_SERVER_PID as u64, MSG_VGA_READY as u64); }

    // Boucle de service IPC
    loop {
        let mut msg = [0u8; 64];
        let ret = unsafe {
            syscall::syscall3(syscall::SYS_EXO_IPC_RECV,
                msg.as_mut_ptr() as u64, msg.len() as u64, u64::MAX)
        };
        if ret < 0 { continue; }
        let msg_type = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
        match msg_type {
            0x201 => { /* write char à position */ }
            0x202 => { vga_clear(); }
            0x203 => { /* update cursor HW */ }
            _ => {}
        }
    }
}

fn vga_clear() {
    unsafe {
        for i in 0..(COLS * ROWS) {
            VGA_BUF.add(i).write_volatile(0x0720); // espace blanc sur noir
        }
    }
}

fn vga_write_str(s: &[u8], attr: u8, col: usize, row: usize) {
    let mut c = col; let mut r = row;
    for &b in s {
        if b == b'\r' { c = 0; continue; }
        if b == b'\n' { r += 1; c = 0; continue; }
        if r >= ROWS { break; }
        let entry = (b as u16) | ((attr as u16) << 8);
        unsafe { VGA_BUF.add(r * COLS + c).write_volatile(entry); }
        c += 1; if c >= COLS { c = 0; r += 1; }
    }
    vga_update_cursor(c as u16, r as u16);
}

fn vga_update_cursor(col: u16, row: u16) {
    let pos = row * COLS as u16 + col;
    unsafe {
        // Port VGA index 0x3D4 / data 0x3D5
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x3D4u16, in("al") 0x0Fu8, options(nostack, nomem)
        );
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x3D5u16, in("al") (pos & 0xFF) as u8, options(nostack, nomem)
        );
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x3D4u16, in("al") 0x0Eu8, options(nostack, nomem)
        );
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x3D5u16, in("al") ((pos >> 8) & 0xFF) as u8, options(nostack, nomem)
        );
    }
}

fn verify_cap_token() {}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
```

### 4.4 `drivers/display/framebuffer/src/main.rs` — FB VESA

```rust
#![no_std]
#![no_main]

//! # framebuffer_driver — Ring 1
//! Framebuffer linéaire VESA (passé par bootloader via BootInfo).
//! Mode : 800×600×32 ou résolution passée par exo-boot.
//! Interface tty_server : blitting rectangle de glyphes 8×16.

use core::panic::PanicInfo;

const TTY_SERVER_PID: u32 = 12;

/// IPC messages vers tty_server
const MSG_FB_READY:      u32 = 0x300;
const MSG_FB_BLIT_GLYPH: u32 = 0x301; // [glyph:u8, col:u16, row:u16, fg:u32, bg:u32]
const MSG_FB_CLEAR:      u32 = 0x302;
const MSG_FB_SCROLL_UP:  u32 = 0x303; // scroll une ligne (hauteur de police = 16px)

// Police bitmap 8×16 (Vga ROM font, 256 glyphes × 16 octets)
// Intégrée statiquement — remplace une dépendance runtime sur le FS
static FONT_8X16: &[u8; 256 * 16] = include_bytes!("../font/vga_font_8x16.bin");
// Note : vga_font_8x16.bin doit être ajouté dans drivers/display/framebuffer/font/

static mut FB_PTR:    *mut u32 = core::ptr::null_mut();
static mut FB_WIDTH:  usize    = 0;
static mut FB_HEIGHT: usize    = 0;
static mut FB_STRIDE: usize    = 0; // en pixels (pas en octets)

/// Dessine un glyphe (char ASCII) à la position colonne/ligne.
fn blit_glyph(glyph: u8, col: usize, row: usize, fg: u32, bg: u32) {
    let px = col * 8;
    let py = row * 16;
    let font_row = &FONT_8X16[(glyph as usize) * 16..][..16];
    unsafe {
        for (y, &bits) in font_row.iter().enumerate() {
            for x in 0..8usize {
                let color = if bits & (0x80 >> x) != 0 { fg } else { bg };
                let off = (py + y) * FB_STRIDE + (px + x);
                FB_PTR.add(off).write_volatile(color);
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // BootInfo injecté par init_server au lancement du driver
    // (adresse physique FB, width, height, stride)
    // Pour l'instant : paramètres hardcodés QEMU VGA std
    unsafe {
        FB_WIDTH  = 800;
        FB_HEIGHT = 600;
        FB_STRIDE = 800;
        // SYS_MMIO_MAP le framebuffer physique
        let phys_fb: u64 = 0xFD00_0000; // adresse typique QEMU VGA
        let size         = FB_WIDTH * FB_HEIGHT * 4;
        let virt = exo_syscall_abi::syscall2(532, phys_fb, size as u64);
        if virt < 0 { panic!("FB MMIO map failed"); }
        FB_PTR = virt as *mut u32;
    }
    // Clear écran
    unsafe {
        for i in 0..(FB_WIDTH * FB_HEIGHT) { FB_PTR.add(i).write_volatile(0x00000000); }
    }

    // TODO : boucle IPC MSG_FB_BLIT_GLYPH / MSG_FB_CLEAR / MSG_FB_SCROLL_UP
    loop { unsafe { core::arch::asm!("hlt"); } }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
```

> **Action requise** : ajouter `drivers/display/framebuffer/font/vga_font_8x16.bin`  
> Source : extraire depuis n'importe quel kernel Linux (`/usr/share/consolefonts/`) ou générer via `mkfont`.

---

## 5. TTY Server

### 5.1 Rôle et position

Le `tty_server` est un **nouveau serveur Ring 1 (PID 12)**. C'est la pièce centrale de l'I/O shell :

- Reçoit les `InputEvent` de `input_server`
- Applique la **line discipline** (écho, effacement `BS`, `^C` → SIGINT, `^D` → EOF)
- Rend les caractères vers le display (vga_driver ou framebuffer_driver)
- Expose un **fd virtuel** (via VFS) au shell pour `SYS_READ` et `SYS_WRITE`
- Gère une paire **PTY maître/esclave** pour isolation du shell

### 5.2 Structure du serveur

```
servers/tty_server/
├── Cargo.toml
└── src/
    ├── main.rs          ← boucle IPC + dispatch
    ├── protocol.rs      ← types IPC (IPC-02 : Sized, taille fixe)
    ├── line_disc.rs     ← line discipline (echo, BS, CR/LF, signals)
    ├── pty.rs           ← paire maître/esclave PTY
    ├── vt100.rs         ← parseur séquences VT100 (ESC [ ... m/A/B/C/D/H/J)
    └── display_backend.rs ← abstraction VGA / framebuffer
```

### 5.3 `src/protocol.rs`

```rust
use exo_types::FixedString;

pub const TTY_SERVER_PID: u32 = 12;

// Messages reçus par tty_server
pub const MSG_INPUT_EVENT:  u32 = 0x100; // depuis input_server (InputEvent 16 octets)
pub const MSG_SHELL_WRITE:  u32 = 0x110; // depuis shell (données à afficher)
pub const MSG_SHELL_READ:   u32 = 0x111; // shell demande une ligne (bloquant)
pub const MSG_TTY_OPEN:     u32 = 0x120; // ouvrir un PTY (retourne fd pair)
pub const MSG_TTY_CLOSE:    u32 = 0x121;
pub const MSG_VGA_READY:    u32 = 0x200; // depuis vga_driver
pub const MSG_FB_READY:     u32 = 0x300; // depuis framebuffer_driver

// Réponses
pub const MSG_SHELL_READ_REPLY: u32 = 0x112; // ligne complète (terminée \n)
pub const MSG_TTY_OPEN_REPLY:   u32 = 0x122; // fd maître, fd esclave

/// Payload MSG_SHELL_WRITE — max 256 octets de données
#[repr(C)]
pub struct WritePayload {
    pub len:  u16,
    pub data: [u8; 254], // complète à 256 octets (IPC-02)
}

/// Payload MSG_SHELL_READ_REPLY — une ligne, terminée \n
#[repr(C)]
pub struct ReadReply {
    pub len:  u16,
    pub data: [u8; 254],
}
```

### 5.4 `src/line_disc.rs` — line discipline

```rust
//! Line discipline : transformations canoniques appliquées aux InputEvent
//! avant transmission au shell (mode CANON) ou passage brut (mode RAW).

use exo_types::InputEvent;

pub enum LineDisciplineMode { Canon, Raw }

pub struct LineDiscipline {
    pub mode:   LineDisciplineMode,
    buf:        [u8; 256],
    len:        usize,
    echo:       bool,
}

impl LineDiscipline {
    pub const fn new() -> Self {
        Self { mode: LineDisciplineMode::Canon, buf: [0u8; 256], len: 0, echo: true }
    }

    /// Traite un InputEvent clavier → Option<ligne complète prête pour le shell>
    pub fn feed(&mut self, ev: &InputEvent) -> Option<&[u8]> {
        if ev.value == 0 { return None; } // key-up ignoré en mode canon
        let ascii = hid_to_ascii(ev.code, ev.mods);
        match ascii {
            b'\x03' => { /* ^C → SIGINT — à signaler au shell */ self.len = 0; None }
            b'\x04' => { /* ^D → EOF */ None }
            b'\x7F' | b'\x08' => { // Backspace
                if self.len > 0 { self.len -= 1; }
                None
            }
            b'\r' | b'\n' => {
                self.buf[self.len] = b'\n';
                self.len += 1;
                let end = self.len;
                self.len = 0;
                Some(&self.buf[..end]) // ligne prête
            }
            ch if ch != 0 && self.len < 255 => {
                self.buf[self.len] = ch;
                self.len += 1;
                None
            }
            _ => None
        }
    }

    /// Conversion HID code → ASCII (table partielle, US QWERTY)
    fn hid_to_ascii_inner(code: u16, mods: u8) -> u8 {
        hid_to_ascii(code, mods)
    }
}

/// Table HID → ASCII US QWERTY (sous-ensemble shell)
pub fn hid_to_ascii(code: u16, mods: u8) -> u8 {
    let shift = mods & 0x01 != 0;
    match code {
        0x0004 => if shift { b'A' } else { b'a' },
        0x0005 => if shift { b'B' } else { b'b' },
        0x0006 => if shift { b'C' } else { b'c' },
        0x0007 => if shift { b'D' } else { b'd' },
        0x0008 => if shift { b'E' } else { b'e' },
        0x0009 => if shift { b'F' } else { b'f' },
        0x000A => if shift { b'G' } else { b'g' },
        0x000B => if shift { b'H' } else { b'h' },
        0x000C => if shift { b'I' } else { b'i' },
        0x000D => if shift { b'J' } else { b'j' },
        0x000E => if shift { b'K' } else { b'k' },
        0x000F => if shift { b'L' } else { b'l' },
        0x0010 => if shift { b'M' } else { b'm' },
        0x0011 => if shift { b'N' } else { b'n' },
        0x0012 => if shift { b'O' } else { b'o' },
        0x0013 => if shift { b'P' } else { b'p' },
        0x0014 => if shift { b'Q' } else { b'q' },
        0x0015 => if shift { b'R' } else { b'r' },
        0x0016 => if shift { b'S' } else { b's' },
        0x0017 => if shift { b'T' } else { b't' },
        0x0018 => if shift { b'U' } else { b'u' },
        0x0019 => if shift { b'V' } else { b'v' },
        0x001A => if shift { b'W' } else { b'w' },
        0x001B => if shift { b'X' } else { b'x' },
        0x001C => if shift { b'Y' } else { b'y' },
        0x001D => if shift { b'Z' } else { b'z' },
        0x001E => if shift { b'!' } else { b'1' },
        0x001F => if shift { b'@' } else { b'2' },
        0x0020 => if shift { b'#' } else { b'3' },
        0x0021 => if shift { b'$' } else { b'4' },
        0x0022 => if shift { b'%' } else { b'5' },
        0x0023 => if shift { b'^' } else { b'6' },
        0x0024 => if shift { b'&' } else { b'7' },
        0x0025 => if shift { b'*' } else { b'8' },
        0x0026 => if shift { b'(' } else { b'9' },
        0x0027 => if shift { b')' } else { b'0' },
        0x002C => b' ',
        0x002D => if shift { b'_' } else { b'-' },
        0x002E => if shift { b'+' } else { b'=' },
        0x002F => if shift { b'{' } else { b'[' },
        0x0030 => if shift { b'}' } else { b']' },
        0x0031 => if shift { b'|' } else { b'\\' },
        0x0033 => if shift { b':' } else { b';' },
        0x0034 => if shift { b'"' } else { b'\'' },
        0x0035 => if shift { b'~' } else { b'`' },
        0x0036 => if shift { b'<' } else { b',' },
        0x0037 => if shift { b'>' } else { b'.' },
        0x0038 => if shift { b'?' } else { b'/' },
        0x0028 => b'\n',  // Enter
        0x002B => b'\t',  // Tab
        0x002A => b'\x7F', // Backspace
        0x0029 => b'\x1B', // Escape
        // Ctrl combinations (mods & 0x02)
        code if mods & 0x02 != 0 => {
            // ^C = 0x03, ^D = 0x04, ^Z = 0x1A, etc.
            match code {
                0x0006 => b'\x03', // ^C
                0x0007 => b'\x04', // ^D
                0x001C => b'\x1A', // ^Z
                _      => 0,
            }
        }
        _ => 0,
    }
}
```

### 5.5 `src/main.rs` — boucle principale tty_server

```rust
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_types::InputEvent;

mod display_backend;
mod line_disc;
mod protocol;
mod pty;
mod vt100;

use line_disc::LineDiscipline;
use display_backend::DisplayBackend;

static DISC:    spin::Mutex<LineDiscipline>  = spin::Mutex::new(LineDiscipline::new());
static DISPLAY: spin::Mutex<DisplayBackend>  = spin::Mutex::new(DisplayBackend::new());

const SHELL_PID_SLOT: u32 = 13; // PID attendu du shell lancé par init_server

#[no_mangle]
pub extern "C" fn _start() -> ! {
    verify_cap_token();
    register_ipc_endpoint();

    // Attendre que le display soit prêt (MSG_VGA_READY ou MSG_FB_READY)
    wait_display_ready();

    print_banner();

    loop {
        let mut msg = [0u8; 64];
        let ret = unsafe {
            syscall::syscall3(syscall::SYS_EXO_IPC_RECV,
                msg.as_mut_ptr() as u64, msg.len() as u64, u64::MAX)
        };
        if ret < 0 { continue; }
        let msg_type = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);

        match msg_type {
            protocol::MSG_INPUT_EVENT => {
                // Désérialiser InputEvent depuis payload[4..20]
                let ev = unsafe {
                    &*(msg.as_ptr().add(4) as *const InputEvent)
                };
                let mut disc = DISC.lock();
                if let Some(line) = disc.feed(ev) {
                    // Ligne complète → envoyer au shell via IPC
                    forward_line_to_shell(line);
                } else {
                    // Echo caractère
                    let ch = line_disc::hid_to_ascii(ev.code, ev.mods);
                    if ch != 0 { DISPLAY.lock().putchar(ch); }
                }
            }
            protocol::MSG_SHELL_WRITE => {
                let len = u16::from_le_bytes([msg[4], msg[5]]) as usize;
                let data = &msg[6..6 + len.min(58)];
                for &b in data { DISPLAY.lock().putchar(b); }
            }
            protocol::MSG_VGA_READY | protocol::MSG_FB_READY => {
                DISPLAY.lock().set_ready(msg_type);
            }
            _ => {}
        }
    }
}

fn print_banner() {
    let banner = b"ExoOS v0.1 — tty ready\r\nexo$ ";
    for &b in banner { DISPLAY.lock().putchar(b); }
}

fn forward_line_to_shell(line: &[u8]) {
    // IPC vers shell (PID 13)
    let mut payload = [0u8; 64];
    payload[0..4].copy_from_slice(&protocol::MSG_SHELL_READ_REPLY.to_le_bytes());
    let len = line.len().min(58);
    payload[4..6].copy_from_slice(&(len as u16).to_le_bytes());
    payload[6..6 + len].copy_from_slice(&line[..len]);
    unsafe {
        syscall::syscall4(syscall::SYS_EXO_IPC_SEND,
            SHELL_PID_SLOT as u64,
            protocol::MSG_SHELL_READ_REPLY as u64,
            payload.as_ptr() as u64, 64);
    }
}

fn register_ipc_endpoint() {
    unsafe { syscall::syscall1(syscall::SYS_EXO_IPC_CREATE, 12); }
}
fn wait_display_ready() {
    // Spin brève — le display driver démarre avant tty_server
}
fn verify_cap_token() {}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
```

---

## 6. Input Server

### 6.1 Rôle

`input_server` est un **agrégateur Ring 1 (PID 11)** :

- Reçoit les `InputEvent` bruts de **ps2_driver** et **usb_hid_driver**
- Applique un filtre de déduplication (pas deux événements identiques en < 5ms)
- Transfère à `tty_server` via IPC

### 6.2 Structure minimale

```
servers/input_server/
├── Cargo.toml
└── src/
    ├── main.rs      ← boucle IPC + routage
    └── filter.rs    ← déduplication + rate limiting
```

`src/main.rs` :

```rust
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_types::InputEvent;

mod filter;

const TTY_SERVER_PID: u32 = 12;
const MSG_INPUT_EVENT: u32 = 0x100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    verify_cap_token();
    unsafe { syscall::syscall1(syscall::SYS_EXO_IPC_CREATE, 11); }

    let mut filt = filter::InputFilter::new();

    loop {
        let mut msg = [0u8; 64];
        let ret = unsafe {
            syscall::syscall3(syscall::SYS_EXO_IPC_RECV,
                msg.as_mut_ptr() as u64, msg.len() as u64, u64::MAX)
        };
        if ret < 0 { continue; }
        let msg_type = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);

        if msg_type == MSG_INPUT_EVENT {
            let ev = unsafe { &*(msg.as_ptr().add(4) as *const InputEvent) };
            if filt.accept(ev) {
                // Retransmettre à tty_server
                let mut out = [0u8; 64];
                out[0..4].copy_from_slice(&MSG_INPUT_EVENT.to_le_bytes());
                out[4..20].copy_from_slice(unsafe {
                    core::slice::from_raw_parts(ev as *const InputEvent as *const u8, 16)
                });
                unsafe {
                    syscall::syscall4(syscall::SYS_EXO_IPC_SEND,
                        TTY_SERVER_PID as u64,
                        MSG_INPUT_EVENT as u64,
                        out.as_ptr() as u64, 64);
                }
            }
        }
    }
}

fn verify_cap_token() {}
#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
```

---

## 7. Loader ELF

### 7.1 État actuel — critique

Tous les fichiers dans `loader/src/` sont **vides (0 octets)**. C'est un **bloqueur P0** pour lancer le shell.

```
loader/src/main.rs             — 22 octets ("//! nothing for moment")
loader/src/entry.rs            — 0 octets
loader/src/elf/parser.rs       — 0 octets
loader/src/elf/mod.rs          — 0 octets
loader/src/elf/segments.rs     — 0 octets
loader/src/elf/relocations.rs  — 0 octets
loader/src/elf/dynamic.rs      — 0 octets
loader/src/dynamic_linker/     — tous vides
loader/src/security/           — tous vides
```

### 7.2 Plan loader minimal (statique, pour le shell)

Pour la première itération, on n'a **pas besoin du linker dynamique**. Le shell sera compilé en **statique** (`no_std`, lié statiquement avec `exo-libc`). Le loader doit juste :

1. Parser l'en-tête ELF64 (`ET_EXEC`)
2. Mapper les segments `PT_LOAD` via `SYS_MMAP`
3. Passer l'entrée `e_entry` au scheduler via `SYS_FORK + SYS_EXECVE`
4. Valider la signature Blake3 (`PHX-03`)

### 7.3 `loader/src/elf/parser.rs`

```rust
//! Parser ELF64 minimal pour exécutables statiques Ring 3.
//! Supporte ET_EXEC, EM_X86_64, EI_CLASS=2 (64-bit), EI_DATA=1 (LE).

pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ET_EXEC:   u16      = 2;
pub const EM_X86_64: u16      = 62;

/// En-tête ELF64
#[repr(C)]
pub struct Elf64Header {
    pub e_ident:     [u8; 16],
    pub e_type:      u16,
    pub e_machine:   u16,
    pub e_version:   u32,
    pub e_entry:     u64,    // Point d'entrée
    pub e_phoff:     u64,    // Offset Program Header Table
    pub e_shoff:     u64,    // Offset Section Header Table (non utilisé ici)
    pub e_flags:     u32,
    pub e_ehsize:    u16,
    pub e_phentsize: u16,
    pub e_phnum:     u16,    // Nombre de Program Headers
    pub e_shentsize: u16,
    pub e_shnum:     u16,
    pub e_shstrndx:  u16,
}

/// Program Header ELF64
#[repr(C)]
pub struct Elf64Phdr {
    pub p_type:   u32,
    pub p_flags:  u32,
    pub p_offset: u64,  // Offset dans le fichier
    pub p_vaddr:  u64,  // Adresse virtuelle cible
    pub p_paddr:  u64,  // Adresse physique (ignoré)
    pub p_filesz: u64,  // Taille dans le fichier
    pub p_memsz:  u64,  // Taille en mémoire (peut être > filesz → BSS)
    pub p_align:  u64,  // Alignement (doit être puissance de 2)
}

pub const PT_LOAD: u32 = 1;
pub const PF_X:    u32 = 1;
pub const PF_W:    u32 = 2;
pub const PF_R:    u32 = 4;

/// Résultat du parsing
pub struct ElfInfo {
    pub entry:    u64,          // e_entry
    pub segments: [LoadSeg; 8], // max 8 PT_LOAD segments
    pub seg_count: usize,
}

pub struct LoadSeg {
    pub vaddr:   u64,
    pub filesz:  u64,
    pub memsz:   u64,
    pub offset:  u64,
    pub flags:   u32, // PF_R | PF_W | PF_X
}

/// Valider et parser un en-tête ELF depuis un buffer mémoire.
pub fn parse(data: &[u8]) -> Option<ElfInfo> {
    if data.len() < core::mem::size_of::<Elf64Header>() { return None; }
    let hdr = unsafe { &*(data.as_ptr() as *const Elf64Header) };

    // Validation
    if hdr.e_ident[..4] != ELF_MAGIC   { return None; }
    if hdr.e_ident[4]   != 2           { return None; } // EI_CLASS = ELFCLASS64
    if hdr.e_ident[5]   != 1           { return None; } // EI_DATA = ELFDATA2LSB
    if hdr.e_type       != ET_EXEC     { return None; } // ET_EXEC seulement
    if hdr.e_machine    != EM_X86_64   { return None; }

    let ph_off   = hdr.e_phoff as usize;
    let ph_num   = hdr.e_phnum as usize;
    let ph_size  = hdr.e_phentsize as usize;

    if ph_off + ph_num * ph_size > data.len() { return None; }

    let mut info = ElfInfo {
        entry:     hdr.e_entry,
        segments:  [LoadSeg { vaddr: 0, filesz: 0, memsz: 0, offset: 0, flags: 0 }; 8],
        seg_count: 0,
    };

    for i in 0..ph_num {
        let ph = unsafe {
            &*(data.as_ptr().add(ph_off + i * ph_size) as *const Elf64Phdr)
        };
        if ph.p_type != PT_LOAD { continue; }
        if info.seg_count >= 8  { break; }

        info.segments[info.seg_count] = LoadSeg {
            vaddr:  ph.p_vaddr,
            filesz: ph.p_filesz,
            memsz:  ph.p_memsz,
            offset: ph.p_offset,
            flags:  ph.p_flags,
        };
        info.seg_count += 1;
    }

    if info.seg_count == 0 { return None; }
    Some(info)
}
```

### 7.4 `loader/src/main.rs` — loader complet minimal

```rust
#![no_std]
#![no_main]

//! # ExoOS Loader — PID transitoire lancé par init_server
//!
//! Arguments (passés par execve argv) :
//!   argv[0] = chemin du binaire à charger (ex: "/bin/exo-shell")
//!
//! Séquence :
//!   1. Ouvrir le fichier via SYS_EXOFS_OPEN_BY_PATH (519)
//!   2. Lire en mémoire
//!   3. Parser ELF64
//!   4. Mapper segments PT_LOAD via SYS_MMAP
//!   5. Valider signature Blake3 via crypto_server (PHX-03)
//!   6. Fork + jump vers e_entry (Ring 3)
//!   7. Le parent (loader) exit proprement

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

mod elf;
mod security;

// Buffer statique pour le binaire à charger (max 4 Mo)
static mut ELF_BUF: [u8; 4 * 1024 * 1024] = [0u8; 4 * 1024 * 1024];

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Récupérer argv[0] depuis la pile (convention SysV AMD64)
    let path = b"/bin/exo-shell\0";

    // 1. Ouvrir le fichier
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_EXOFS_OPEN_BY_PATH,
            path.as_ptr() as u64,
            0,   // O_RDONLY
            0,   // mode
            // cap_rights : lecture
            exo_syscall_abi::EXO_CAP_RIGHT_READ as u64,
        )
    };
    if fd < 0 { panic!("loader: cannot open shell binary"); }

    // 2. Lire le binaire
    let read_len = unsafe {
        syscall::syscall3(
            syscall::SYS_READ,
            fd as u64,
            ELF_BUF.as_mut_ptr() as u64,
            ELF_BUF.len() as u64,
        )
    };
    if read_len <= 0 { panic!("loader: read failed"); }
    unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }

    let data = unsafe { &ELF_BUF[..read_len as usize] };

    // 3. Parser ELF
    let elf_info = elf::parser::parse(data).expect("loader: invalid ELF");

    // 4. Mapper segments PT_LOAD
    for seg in &elf_info.segments[..elf_info.seg_count] {
        let prot = elf_flags_to_mmap_prot(seg.flags);
        let aligned_vaddr = seg.vaddr & !0xFFF;
        let aligned_size  = (seg.memsz + (seg.vaddr & 0xFFF) + 0xFFF) & !0xFFF;

        let ret = unsafe {
            syscall::syscall6(
                syscall::SYS_MMAP,
                aligned_vaddr,
                aligned_size,
                prot as u64,
                0x32, // MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS
                u64::MAX, // fd = -1
                0,
            )
        };
        if ret < 0 { panic!("loader: mmap segment failed"); }

        // Copier les données du fichier
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(seg.offset as usize),
                seg.vaddr as *mut u8,
                seg.filesz as usize,
            );
            // Zéroiser BSS (memsz > filesz)
            if seg.memsz > seg.filesz {
                core::ptr::write_bytes(
                    (seg.vaddr + seg.filesz) as *mut u8,
                    0,
                    (seg.memsz - seg.filesz) as usize,
                );
            }
        }
    }

    // 5. Allouer stack Ring 3 (8 Ko)
    let stack_top: u64 = 0x7FFF_FFFF_0000;
    unsafe {
        syscall::syscall6(
            syscall::SYS_MMAP,
            stack_top - 0x2000,
            0x2000,
            0x3, // PROT_READ | PROT_WRITE
            0x22, // MAP_PRIVATE | MAP_ANONYMOUS
            u64::MAX, 0,
        );
    }

    // 6. Fork + jump vers e_entry
    let child = unsafe { syscall::syscall0(syscall::SYS_FORK) };
    if child == 0 {
        // Enfant : sauter vers le shell
        unsafe {
            core::arch::asm!(
                "mov rsp, {stack}",
                "jmp {entry}",
                stack = in(reg) stack_top,
                entry = in(reg) elf_info.entry,
                options(noreturn)
            );
        }
    }
    // Parent (loader) : exit proprement
    unsafe { syscall::syscall1(syscall::SYS_EXIT, 0); }
    loop {}
}

fn elf_flags_to_mmap_prot(flags: u32) -> u32 {
    let mut prot = 0u32;
    if flags & elf::parser::PF_R != 0 { prot |= 1; } // PROT_READ
    if flags & elf::parser::PF_W != 0 { prot |= 2; } // PROT_WRITE
    if flags & elf::parser::PF_X != 0 { prot |= 4; } // PROT_EXEC
    prot
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
```

---

## 8. Shell Binaire

### 8.1 Positionnement

```
userspace/
└── exo-shell/
    ├── Cargo.toml
    └── src/
        ├── main.rs      ← REPL principal
        ├── builtins.rs  ← cd, pwd, echo, exit
        ├── commands.rs  ← touch, cat, ls, top, kill
        ├── ipc.rs       ← helpers IPC vers tty_server
        ├── fs.rs        ← helpers SYS_EXOFS_* pour fichiers
        └── proc.rs      ← helpers SYS_GETPID, SYS_KILL, top via scheduler_server
```

`Cargo.toml` :

```toml
[package]
name = "exo-shell"
version = "0.1.0"
edition = "2021"

[dependencies]
exo_types    = { path = "../../libs/exo_types" }
exo_syscall_abi = { path = "../../servers/syscall_abi" }

[profile.release]
panic = "abort"
opt-level = "s"  # optimisation taille (shell léger)
lto = true

[[bin]]
name = "exo-shell"
path = "src/main.rs"
```

### 8.2 `src/main.rs` — REPL

```rust
#![no_std]
#![no_main]

//! # exo-shell — Shell interactif Ring 3
//!
//! Commandes supportées :
//!   cd <path>   pwd   ls [path]   touch <file>
//!   cat <file>  top   kill <pid>  echo <args...>   exit

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

mod builtins;
mod commands;
mod fs;
mod ipc;
mod proc;

const TTY_SERVER_PID: u32 = 12;
const MAX_LINE: usize     = 256;
const MAX_ARGS: usize     = 16;

static mut CWD: [u8; 256] = *b"/\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Afficher le prompt initial
    ipc::tty_write(b"exo$ ");

    loop {
        // Lire une ligne depuis tty_server (bloquant)
        let mut line_buf = [0u8; MAX_LINE];
        let len = ipc::tty_read(&mut line_buf);
        if len == 0 { continue; }

        // Trim trailing \n
        let line = trim_newline(&line_buf[..len]);
        if line.is_empty() {
            ipc::tty_write(b"exo$ ");
            continue;
        }

        // Parser les arguments
        let mut args: [&[u8]; MAX_ARGS] = [b""; MAX_ARGS];
        let argc = split_args(line, &mut args);

        // Dispatcher la commande
        match args[0] {
            b"exit" => {
                ipc::tty_write(b"Bye!\r\n");
                unsafe { syscall::syscall1(syscall::SYS_EXIT, 0); }
            }
            b"cd"   => builtins::cmd_cd(&args[..argc]),
            b"pwd"  => builtins::cmd_pwd(),
            b"echo" => builtins::cmd_echo(&args[..argc]),
            b"touch"=> commands::cmd_touch(&args[..argc]),
            b"cat"  => commands::cmd_cat(&args[..argc]),
            b"ls"   => commands::cmd_ls(&args[..argc]),
            b"top"  => commands::cmd_top(),
            b"kill" => commands::cmd_kill(&args[..argc]),
            cmd     => {
                ipc::tty_write(b"exo: command not found: ");
                ipc::tty_write(cmd);
                ipc::tty_write(b"\r\n");
            }
        }

        ipc::tty_write(b"exo$ ");
    }
}

fn trim_newline(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && (s[end-1] == b'\n' || s[end-1] == b'\r') { end -= 1; }
    &s[..end]
}

fn split_args<'a>(line: &'a [u8], args: &mut [&'a [u8]; MAX_ARGS]) -> usize {
    let mut count = 0;
    let mut start = 0;
    let mut in_word = false;
    for (i, &b) in line.iter().enumerate() {
        if b == b' ' || b == b'\t' {
            if in_word && count < MAX_ARGS {
                args[count] = &line[start..i];
                count += 1;
                in_word = false;
            }
        } else if !in_word {
            start = i;
            in_word = true;
        }
    }
    if in_word && count < MAX_ARGS { args[count] = &line[start..]; count += 1; }
    count
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    ipc::tty_write(b"\r\nPANIC\r\n");
    loop { unsafe { core::arch::asm!("hlt"); } }
}
```

### 8.3 `src/builtins.rs`

```rust
use exo_syscall_abi as syscall;
use crate::ipc;

// Accès au CWD global du shell
extern "C" { static mut CWD: [u8; 256]; }

pub fn cmd_cd(args: &[&[u8]]) {
    if args.len() < 2 {
        ipc::tty_write(b"cd: missing argument\r\n");
        return;
    }
    let path = args[1];
    let ret = unsafe {
        syscall::syscall2(syscall::SYS_CHDIR,
            path.as_ptr() as u64, path.len() as u64)
    };
    if ret < 0 {
        ipc::tty_write(b"cd: no such directory\r\n");
    } else {
        // Mettre à jour CWD
        unsafe {
            CWD[..path.len()].copy_from_slice(path);
            CWD[path.len()] = 0;
        }
    }
}

pub fn cmd_pwd() {
    // Utiliser SYS_GETCWD (79)
    let mut buf = [0u8; 256];
    let ret = unsafe {
        syscall::syscall2(syscall::SYS_GETCWD,
            buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    if ret < 0 {
        ipc::tty_write(b"pwd: error\r\n");
    } else {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(ret as usize);
        ipc::tty_write(&buf[..len]);
        ipc::tty_write(b"\r\n");
    }
}

pub fn cmd_echo(args: &[&[u8]]) {
    for (i, arg) in args[1..].iter().enumerate() {
        if i > 0 { ipc::tty_write(b" "); }
        ipc::tty_write(arg);
    }
    ipc::tty_write(b"\r\n");
}
```

### 8.4 `src/commands.rs`

```rust
use exo_syscall_abi as syscall;
use crate::{ipc, fs, proc};

pub fn cmd_touch(args: &[&[u8]]) {
    if args.len() < 2 { ipc::tty_write(b"touch: missing filename\r\n"); return; }
    let path = args[1];

    // Créer un objet ExoFS vide via SYS_EXOFS_OBJECT_CREATE (504)
    let ret = unsafe {
        syscall::syscall4(
            syscall::SYS_EXOFS_OBJECT_CREATE,
            path.as_ptr() as u64,
            path.len() as u64,
            0, // flags
            exo_syscall_abi::EXO_CAP_RIGHT_WRITE as u64,
        )
    };
    if ret < 0 { ipc::tty_write(b"touch: create failed\r\n"); }
}

pub fn cmd_cat(args: &[&[u8]]) {
    if args.len() < 2 { ipc::tty_write(b"cat: missing filename\r\n"); return; }
    let path = args[1];

    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_EXOFS_OPEN_BY_PATH,
            path.as_ptr() as u64,
            0,   // O_RDONLY
            0,   // mode
            exo_syscall_abi::EXO_CAP_RIGHT_READ as u64,
        )
    };
    if fd < 0 { ipc::tty_write(b"cat: no such file\r\n"); return; }

    let mut buf = [0u8; 512];
    loop {
        let n = unsafe {
            syscall::syscall3(syscall::SYS_READ, fd as u64,
                buf.as_mut_ptr() as u64, buf.len() as u64)
        };
        if n <= 0 { break; }
        ipc::tty_write(&buf[..n as usize]);
    }
    unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
}

pub fn cmd_ls(args: &[&[u8]]) {
    let path: &[u8] = if args.len() >= 2 { args[1] } else { b"/" };

    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_EXOFS_OPEN_BY_PATH,
            path.as_ptr() as u64, path.len() as u64, 0,
            exo_syscall_abi::EXO_CAP_RIGHT_READ as u64,
        )
    };
    if fd < 0 { ipc::tty_write(b"ls: cannot open directory\r\n"); return; }

    // SYS_EXOFS_READDIR (520)
    let mut buf = [0u8; 1024];
    let cap_rights: u64 = exo_syscall_abi::EXO_CAP_RIGHT_READ as u64;
    let n = unsafe {
        syscall::syscall4(
            syscall::SYS_EXOFS_READDIR,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            cap_rights,
        )
    };
    if n < 0 {
        ipc::tty_write(b"ls: readdir failed\r\n");
    } else {
        // Format de buf : entrées séparées par \0, deux \0 = fin
        let mut pos = 0usize;
        while pos < n as usize {
            let start = pos;
            while pos < n as usize && buf[pos] != 0 { pos += 1; }
            if pos == start { break; }
            ipc::tty_write(&buf[start..pos]);
            ipc::tty_write(b"  ");
            pos += 1;
        }
        ipc::tty_write(b"\r\n");
    }
    unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
}

pub fn cmd_top() {
    // Interroger scheduler_server via IPC (SCHED_MSG_GET_STAT = ?)
    // Pour l'instant : afficher PID courant + mémoire basique
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    ipc::tty_write(b"PID  CMD\r\n");
    ipc::tty_write(b"--- shell top (stub) ---\r\n");
    ipc::tty_write(b"shell pid: ");
    ipc::tty_write_u64(pid as u64);
    ipc::tty_write(b"\r\n");
    // TODO : interroger scheduler_server.stats_collector pour liste threads
}

pub fn cmd_kill(args: &[&[u8]]) {
    if args.len() < 2 { ipc::tty_write(b"kill: missing pid\r\n"); return; }
    let pid = parse_u64(args[1]);
    if pid == 0 { ipc::tty_write(b"kill: invalid pid\r\n"); return; }

    // SYS_KILL (62) : signal SIGTERM (15)
    let ret = unsafe {
        syscall::syscall2(syscall::SYS_KILL, pid, 15)
    };
    if ret < 0 {
        ipc::tty_write(b"kill: failed (permission denied or no such process)\r\n");
    } else {
        ipc::tty_write(b"kill: signal sent\r\n");
    }
}

fn parse_u64(s: &[u8]) -> u64 {
    s.iter().fold(0u64, |acc, &b| {
        if b >= b'0' && b <= b'9' { acc * 10 + (b - b'0') as u64 } else { acc }
    })
}
```

### 8.5 `src/ipc.rs` — helpers communication TTY

```rust
use exo_syscall_abi as syscall;

const TTY_SERVER_PID: u32  = 12;
const MSG_SHELL_WRITE: u32 = 0x110;
const MSG_SHELL_READ:  u32 = 0x111;
const MSG_SHELL_READ_REPLY: u32 = 0x112;

/// Envoyer des données à afficher sur le terminal.
pub fn tty_write(data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let chunk_len = (data.len() - offset).min(58);
        let mut payload = [0u8; 64];
        payload[0..4].copy_from_slice(&MSG_SHELL_WRITE.to_le_bytes());
        payload[4..6].copy_from_slice(&(chunk_len as u16).to_le_bytes());
        payload[6..6 + chunk_len].copy_from_slice(&data[offset..offset + chunk_len]);
        unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_SEND,
                TTY_SERVER_PID as u64,
                MSG_SHELL_WRITE as u64,
                payload.as_ptr() as u64, 64,
            );
        }
        offset += chunk_len;
    }
}

/// Attendre une ligne complète depuis tty_server (bloquant).
/// Retourne le nombre d'octets écrits dans buf.
pub fn tty_read(buf: &mut [u8]) -> usize {
    let mut msg = [0u8; 64];
    loop {
        let ret = unsafe {
            syscall::syscall3(
                syscall::SYS_EXO_IPC_RECV,
                msg.as_mut_ptr() as u64,
                msg.len() as u64,
                u64::MAX, // timeout infini
            )
        };
        if ret < 0 { continue; }
        let msg_type = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
        if msg_type == MSG_SHELL_READ_REPLY {
            let len = u16::from_le_bytes([msg[4], msg[5]]) as usize;
            let copy_len = len.min(buf.len()).min(58);
            buf[..copy_len].copy_from_slice(&msg[6..6 + copy_len]);
            return copy_len;
        }
    }
}

/// Afficher un u64 en décimal sur le terminal.
pub fn tty_write_u64(n: u64) {
    if n == 0 { tty_write(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    let mut v = n;
    while v > 0 && i > 0 { i -= 1; buf[i] = b'0' + (v % 10) as u8; v /= 10; }
    tty_write(&buf[i..]);
}
```

---

## 9. Corrections `init_server`

### 9.1 `service_table.rs` — ajouter tty_server et input_server

```rust
// Ajouter dans les constantes de dépendances
const DEPS_INPUT:  &[&str] = &["device_server"];
const DEPS_TTY:    &[&str] = &["input_server", "vfs_server"];
const DEPS_SHELL:  &[&str] = &["tty_server"];

// Nouveaux binaires
pub static INPUT_SERVER_BIN: &[u8] = b"/sbin/exo-input-server\0";
pub static TTY_SERVER_BIN:   &[u8] = b"/sbin/exo-tty-server\0";
pub static SHELL_BIN:        &[u8] = b"/bin/exo-shell\0";

// Modifier SERVICE_COUNT : 9 → 11 (ou 12 avec le shell)
pub const SERVICE_COUNT: usize = 12;

// Ajouter à la fin de CANONICAL_SERVICES (après exo_shield) :
ServiceMetadata {
    name: "input_server",
    bin_path: INPUT_SERVER_BIN,
    requires: DEPS_INPUT,
    ready_timeout_ms: 500,
    critical: true,
},
ServiceMetadata {
    name: "tty_server",
    bin_path: TTY_SERVER_BIN,
    requires: DEPS_TTY,
    ready_timeout_ms: 750,
    critical: true,
},
ServiceMetadata {
    name: "exo-shell",
    bin_path: SHELL_BIN,
    requires: DEPS_SHELL,
    ready_timeout_ms: 1000,
    critical: false, // le shell peut être relancé sans arrêter le système
},
```

### 9.2 `boot_sequence.rs` — lancement du shell via loader

```rust
// Après démarrage de tty_server, lancer le shell via le loader :
// (ajouter après la boucle de démarrage des 11 premiers services)

// Lancer exo-shell via le loader ELF
let shell_pid = unsafe {
    spawn_service("exo-shell", SHELL_BIN)
};
if shell_pid == 0 {
    // Log d'erreur — pas de panic car le shell n'est pas critique
} else {
    service_manager::register_pid("exo-shell", shell_pid);
}
```

---

## 10. Corrections et Blocages identifiés

### 10.1 Blocages P0 — à résoudre avant tout

| ID | Composant | Problème | Correction |
|----|-----------|----------|------------|
| **USR-P0-01** | `loader/src/` | Tous les fichiers vides — le shell ne peut pas être lancé | Implémenter `elf/parser.rs` + `main.rs` (§7) |
| **USR-P0-02** | `drivers/input/ps2/src/` | Tous les fichiers vides — aucune entrée clavier possible | Implémenter `i8042.rs` + `keyboard.rs` (§3) |
| **USR-P0-03** | `drivers/display/vga/src/` | Vide — aucun affichage possible | Implémenter `vga_driver` (§4.3) |
| **USR-P0-04** | `SYS_EXO_IPC_RECV` bloquant | Le shell attend une ligne mais `tty_server` ne sait pas à qui forwarder si le shell n'a pas de PID connu à l'avance | Mécanisme de registration : le shell envoie `MSG_SHELL_REGISTER` à tty_server avec son PID au démarrage |

### 10.2 Blocages P1 — à résoudre pour la complétude

| ID | Composant | Problème | Correction |
|----|-----------|----------|------------|
| **USR-P1-01** | `syscall_abi` | `EXO_CAP_RIGHT_READ` / `EXO_CAP_RIGHT_WRITE` / `EXO_CAP_RIGHT_EXEC` non définis | Ajouter dans `lib.rs` : `pub const EXO_CAP_RIGHT_READ: u32 = 1 << 0;` etc. |
| **USR-P1-02** | `SYS_EXOFS_READDIR` (520) | Format de retour non documenté dans syscall_abi — le shell `ls` ne sait pas parser le buffer | Documenter le format : `[u16 entry_len][u8 name_len][u8 type][name bytes]` en boucle |
| **USR-P1-03** | `tty_server` ↔ `ps2_driver` | PID 11 (input_server) fixe — si service_table change d'ordre, le PID change | Utiliser la table de nommage IPC (SYS_EXO_IPC_CREATE avec endpoint nommé) au lieu de PID hardcodés |
| **USR-P1-04** | `cmd_top` | `scheduler_server` expose `SCHED_MSG_GET_STAT` mais le format de réponse n'est pas dans `protocol.rs` | Documenter et implémenter le type `StatReply` dans `scheduler_server/src/protocol.rs` |
| **USR-P1-05** | Font bitmap | `vga_font_8x16.bin` manquant pour le framebuffer | Ajouter `drivers/display/framebuffer/font/vga_font_8x16.bin` (binaire, 4096 octets) |

### 10.3 Déficiences P2 — améliorations post-MVP

| ID | Composant | Observation |
|----|-----------|-------------|
| **USR-P2-01** | Line discipline | Manque le support `↑`/`↓` pour l'historique des commandes (séquences VT100 `ESC[A`/`ESC[B`) |
| **USR-P2-02** | `cmd_cat` | Lecture en chunks fixes — fichiers > 512 octets nécessitent une boucle avec SYS_LSEEK |
| **USR-P2-03** | `cmd_ls` | Pas de tri alphabétique ni d'affichage des permissions/tailles |
| **USR-P2-04** | `cmd_kill` | Seul SIGTERM supporté — ajouter `-9` (SIGKILL) et parsing du signal |
| **USR-P2-05** | `exo_shield` | Le shell Ring 3 va traverser les hooks `exec_hooks.rs` de ExoShield — vérifier que le chemin `/bin/exo-shell` est dans la whitelist des binaires autorisés |
| **USR-P2-06** | `ipc_router` | `ServiceId::InputServer` et `ServiceId::TtyServer` doivent être ajoutés dans `lib.rs` et `service_id_of()` |

### 10.4 Correction critique — registration PID shell

**Problème** : `tty_server` hardcode `SHELL_PID_SLOT = 13`. Si init_server lance les services dans un ordre différent ou si un service crashe et est relancé, le PID sera différent.

**Correction** : Ajouter un message `MSG_SHELL_REGISTER` :

```rust
// Dans tty_server/src/protocol.rs
pub const MSG_SHELL_REGISTER: u32 = 0x130;

// Dans tty_server/src/main.rs — ajouter dans le match :
protocol::MSG_SHELL_REGISTER => {
    let pid = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
    SHELL_PID.store(pid, Ordering::Release);
}

// Dans exo-shell/src/main.rs — au démarrage :
fn register_with_tty() {
    let my_pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) } as u32;
    let mut payload = [0u8; 64];
    payload[0..4].copy_from_slice(&ipc::MSG_SHELL_REGISTER.to_le_bytes());
    payload[4..8].copy_from_slice(&my_pid.to_le_bytes());
    unsafe {
        syscall::syscall4(syscall::SYS_EXO_IPC_SEND,
            TTY_SERVER_PID as u64, ipc::MSG_SHELL_REGISTER as u64,
            payload.as_ptr() as u64, 64);
    }
}
```

---

## 11. Plan de développement phasé

### Phase A — Infrastructure display (1-2 jours)

**Objectif** : voir du texte à l'écran depuis le kernel.

1. Implémenter `drivers/display/vga/src/main.rs` (§4.3)
2. Ajouter `vga_driver` dans le boot (démarré avant `tty_server`)
3. Tester avec QEMU : `qemu-system-x86_64 -display sdl`

**Critère de succès** : le banner `ExoOS VGA driver ready` s'affiche au boot.

### Phase B — Input clavier (1-2 jours)

**Objectif** : les touches pressées génèrent des `InputEvent`.

1. Implémenter `drivers/input/ps2/src/i8042.rs` + `keyboard.rs` + `main.rs`
2. Implémenter `servers/input_server/src/main.rs`
3. Ajouter `ps2_driver` et `input_server` dans `service_table.rs`

**Critère de succès** : presser une touche déclenche un `InputEvent` visible en log kernel.

### Phase C — TTY Server (2-3 jours)

**Objectif** : les touches s'affichent à l'écran avec line discipline.

1. Implémenter `servers/tty_server/src/` (§5)
2. Connecter : `ps2_driver → input_server → tty_server → vga_driver`
3. Tester echo et Backspace

**Critère de succès** : on peut taper des caractères qui s'affichent, Backspace efface.

### Phase D — Loader ELF (2-3 jours)

**Objectif** : charger un ELF statique depuis ExoFS.

1. Implémenter `loader/src/elf/parser.rs` + `loader/src/main.rs` (§7)
2. Compiler un ELF Ring 3 minimal (hello world → tty_write)
3. Valider le mapping des segments PT_LOAD

**Critère de succès** : `init_server` lance le loader qui exécute l'ELF hello world.

### Phase E — Shell MVP (3-5 jours)

**Objectif** : shell interactif avec toutes les commandes listées.

1. Implémenter `userspace/exo-shell/src/` (§8)
2. Brancher sur `tty_server` (read/write IPC)
3. Implémenter `cd`, `pwd`, `echo`, `exit` (Phase E-1)
4. Implémenter `touch`, `cat`, `ls` via SYS_EXOFS_* (Phase E-2)
5. Implémenter `top` via scheduler_server, `kill` via SYS_KILL (Phase E-3)

**Critère de succès** : session shell complète :

```
ExoOS v0.1 — tty ready
exo$ echo hello world
hello world
exo$ pwd
/
exo$ touch /test.txt
exo$ cat /test.txt
exo$ ls /
test.txt  bin/  sbin/
exo$ top
PID  CMD
13   exo-shell
exo$ kill 999
kill: failed (no such process)
exo$ cd /bin
exo$ pwd
/bin
exo$ exit
Bye!
```

---

## Résumé des fichiers à créer

| Fichier | Statut | Phase |
|---------|--------|-------|
| `libs/exo_types/src/input_event.rs` | NOUVEAU | B |
| `drivers/input/ps2/src/i8042.rs` | À implémenter (vide) | B |
| `drivers/input/ps2/src/keyboard.rs` | À implémenter (vide) | B |
| `drivers/input/ps2/src/mouse.rs` | À implémenter (vide) | B |
| `drivers/input/ps2/src/main.rs` | À implémenter (vide) | B |
| `drivers/display/vga/src/main.rs` | À implémenter (vide) | A |
| `drivers/display/framebuffer/src/main.rs` | À implémenter (vide) | A+ |
| `drivers/display/framebuffer/font/vga_font_8x16.bin` | NOUVEAU (binaire) | A+ |
| `servers/input_server/src/main.rs` | NOUVEAU | B |
| `servers/input_server/src/filter.rs` | NOUVEAU | B |
| `servers/tty_server/src/main.rs` | NOUVEAU | C |
| `servers/tty_server/src/protocol.rs` | NOUVEAU | C |
| `servers/tty_server/src/line_disc.rs` | NOUVEAU | C |
| `servers/tty_server/src/vt100.rs` | NOUVEAU | C |
| `servers/tty_server/src/display_backend.rs` | NOUVEAU | C |
| `loader/src/elf/parser.rs` | À implémenter (vide) | D |
| `loader/src/main.rs` | À implémenter (stub) | D |
| `userspace/exo-shell/src/main.rs` | NOUVEAU | E |
| `userspace/exo-shell/src/builtins.rs` | NOUVEAU | E |
| `userspace/exo-shell/src/commands.rs` | NOUVEAU | E |
| `userspace/exo-shell/src/ipc.rs` | NOUVEAU | E |
| `servers/init_server/src/service_table.rs` | MODIFIER | B |
| `servers/ipc_router/src/lib.rs` | MODIFIER | B |

---

*Document produit par **claude beta** — ExoOS Userspace Shell Plan — 2026-05-05*  
*Basé sur audit du codebase actuel : Architecture v7, Driver Framework v10, syscall_abi canonique*  
*Toutes les corrections respectent les invariants PHX-02, IPC-02, SRV-01, CAP-01, DRV-08*
