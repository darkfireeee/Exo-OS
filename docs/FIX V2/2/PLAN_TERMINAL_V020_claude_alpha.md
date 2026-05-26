# PLAN-TERM-01 — Migration Terminal vers architecture microkernel correcte
## ExoOS v0.2.0 — Terminal, Affichage, et Extension Shell

**Auteur :** claude-alpha  
**Scope :** `kernel/src/arch/x86_64/terminal.rs`, `framebuffer_early.rs`, `boot_display.rs`,
`drivers/display/framebuffer/`, `drivers/input/ps2/`, `servers/tty_server/`,
`servers/input_server/`, `servers/exosh/`

---

## 1. Problème actuel — Violation microkernel confirmée dans le code

### Ce qui existe aujourd'hui

```
SYS_READ  fd=0  →  fs_bridge::fs_read  →  arch::x86_64::terminal::poll_byte_for_process
SYS_WRITE fd=1  →  fs_bridge::fs_write →  arch::x86_64::terminal::write_from_process
                                               │
                              ┌────────────────┴──────────────────┐
                              │  boot_display::terminal_write_bytes │
                              │  framebuffer_early.rs               │  ← Ring0
                              │  VT100, glyphs, cursor, ANSI escapes│  ← Ring0
                              └────────────────┬──────────────────-┘
                                               │
                              ┌────────────────┴──────────────────┐
                              │  inb(0x64) / inb(0x60)            │  ← Ring0, accès direct
                              │  keyboard_irq_drain()             │  ← Ring0, IRQ PS/2
                              │  KEYBOARD: Mutex<KeyboardState>   │  ← Ring0
                              └───────────────────────────────────┘
```

**Violations identifiées :**

| Composant | Fichier | Violation |
|-----------|---------|-----------|
| Rendu VT100 + glyphes + curseur | `framebuffer_early.rs` | Logique display dans Ring0 |
| Accès direct port PS/2 `inb(0x60/0x64)` | `terminal.rs:194–230` | I/O hardware dans Ring0 |
| Buffer clavier `KEYBOARD: Mutex<KeyboardState>` | `terminal.rs` | État input dans Ring0 |
| `BOOT_TTY_HANDLE` court-circuite tty_server | `fs_bridge.rs:254` | fd 0/1/2 bypassent Ring1 |
| `drivers/input/ps2/src/main.rs` | stub vide `fn main() {}` | Driver déclaré, non implémenté |

**Règle microkernel violée (Architecture v7) :** *"Zero Ring0 driver logic — DRV-ARCH-01"*.  
Ring0 doit se limiter à : context switch, page tables, interruptions, IPC dispatch.  
Toute logique de périphérique appartient à Ring1.

---

## 2. Architecture cible v0.2.0

```
┌─────────────────────────────────────────────────────────────────────┐
│  exosh (Ring3, PID=16)                                               │
│  SYS_READ(fd=0) / SYS_WRITE(fd=1)                                   │
└──────────────────────────────┬──────────────────────────────────────┘
                               │ IPC RPC
┌──────────────────────────────▼──────────────────────────────────────┐
│  tty_server (Ring1, PID=14)                                          │
│  • Line discipline (echo, line-edit, Ctrl+C→SIGINT, Ctrl+Z→SIGTSTP) │
│  • VT100/ANSI state machine                                          │
│  • PTY master/slave                                                  │
│  CapToken[DEV_MMIO | DEV_IRQ] → fb_server                           │
└──────────┬──────────────────────────────────┬───────────────────────┘
           │ IPC                              │ IPC
┌──────────▼──────────┐           ┌───────────▼────────────────────────┐
│  fb_server (Ring1)  │           │  input_server (Ring1, PID=13)       │
│  • Reçoit streams   │           │  • Reçoit InputEvent de ps2_driver  │
│  • Rendu glyphes    │           │  • Queue InputEventWire[128]         │
│  • Curseur, scroll  │           │  • Dispatch vers tty_server abonné  │
│  CapToken[DEV_MMIO] │           │  CapToken[DEV_IRQ] → ps2_driver     │
└──────────┬──────────┘           └───────────┬────────────────────────┘
           │ SYS_EXO_MMAP (CAP_PHYSMAP)       │ IPC INPUT_MSG_PUSH
           │ + CapToken[DEV_MMIO]             │
┌──────────▼──────────┐           ┌───────────▼────────────────────────┐
│  framebuffer physique│           │  ps2_driver (Ring1)                 │
│  (UEFI, FIXMAP)     │           │  • SYS_IRQ_REGISTER(IRQ1)           │
│  mappage MMIO Ring1 │           │  • inb(0x60/0x64) via SYS_IOPORT    │
└─────────────────────┘           │  • Décode scancodes set1/set2        │
                                  │  CapToken[DEV_IRQ | DEV_MMIO]        │
                                  └────────────────────────────────────────┘
```

**Principe CapToken :**  
Aucun driver Ring1 n'accède à un port I/O ou MMIO sans présenter un `CapToken` avec les droits
`DEV_MMIO` ou `DEV_IRQ` correspondant à cet objet. Le kernel valide le token à chaque appel.  
**Pas de modèle rwx** — les droits sont portés par le token, pas par l'inode.

---

## 3. Plan d'action — 5 étapes ordonnées

### Étape 1 — Câbler `ps2_driver` comme vrai driver Ring1
**Fichiers :** `drivers/input/ps2/src/main.rs`  
**Prérequis CapToken :** `CapToken[DEV_IRQ | DEV_MMIO]` pour IRQ1 (clavier) et ports 0x60/0x64

**Actions :**
1. Enregistrer IRQ1 via `SYS_IRQ_REGISTER(irq=1, ep=SERVER_ENDPOINT_ID, bdf=0)`
2. Accéder aux ports I/O via `SYS_IOPORT_READ(port=0x60)` / `SYS_IOPORT_READ(port=0x64)` — **jamais `inb` direct**
3. Porter le décodeur scancode set1/set2 depuis `arch/x86_64/terminal.rs` vers `ps2_driver`
4. Sur chaque touche décodée : envoyer `INPUT_MSG_PUSH(InputEventWire)` à `input_server`
5. Répondre aux IRQ : signaler EOI via `SYS_IRQ_EOI(irq_reg_id)`

**Suppression :** `keyboard_irq_drain()`, `KEYBOARD: Mutex<KeyboardState>`, `poll_byte()`, `read_byte_for_process()` dans `arch/x86_64/terminal.rs`

**CapToken requis :**
```
ps2_driver reçoit au boot via init_server :
  CapToken { type: Device, rights: DEV_IRQ | DEV_MMIO, object_id: <ps2_device_id> }
```

---

### Étape 2 — Créer `fb_server` comme serveur Ring1 d'affichage
**Fichiers :** nouveau `servers/fb_server/src/main.rs`  
**Prérequis CapToken :** `CapToken[DEV_MMIO | MEM_MAP]` pour la région MMIO framebuffer

**Actions :**
1. Recevoir de `device_server` la `FramebufferInfo` (adresse physique, stride, format)
2. Mapper la région framebuffer en espace virtuel Ring1 via :
   ```
   SYS_EXO_MMAP(phys_addr, len, prot=RW, flags=MAP_PHYSMAP, cap_token=fb_cap)
   ```
   Le kernel valide `DEV_MMIO` dans le token avant d'établir le mapping — **pas de `CAP_PHYSMAP` global**
3. Porter le rendu de `framebuffer_early.rs` : glyphes, `draw_glyph_scaled`, scroll, curseur, ANSI
4. Exposer un endpoint IPC avec opcodes :
   - `FB_OP_WRITE_TEXT(x, y, bytes, fg, bg)` — rendu texte
   - `FB_OP_CLEAR(color)` — effacement
   - `FB_OP_SCROLL(lines)` — défilement
   - `FB_OP_SET_CURSOR(x, y, visible)` — curseur
5. Enregistrer auprès de `device_server` comme `fb_server`

**CapToken requis :**
```
fb_server reçoit au boot via init_server :
  CapToken { type: Device, rights: DEV_MMIO | MEM_MAP, object_id: <framebuffer_device_id> }
```

---

### Étape 3 — Étendre `tty_server` pour câbler fb_server et input_server
**Fichiers :** `servers/tty_server/src/main.rs`  
**État actuel :** `tty_server` est un simple buffer de line discipline IPC — il n'a ni VT100 ni rendu

**Actions :**
1. Ajouter la machine d'état VT100/ANSI (ESC sequences : `\x1b[H`, `\x1b[2J`, `\x1b[Xm`, etc.)
2. À chaque caractère à afficher : IPC vers `fb_server` `FB_OP_WRITE_TEXT`
3. S'abonner à `input_server` : envoyer `INPUT_MSG_PUSH` pour recevoir les InputEventWire
4. Line discipline : echo local, line-edit (backspace, DEL), Ctrl+C → `SIGINT` → pgid foreground,
   Ctrl+Z → `SIGTSTP`, Ctrl+D → EOF
5. PTY : exposer `/dev/pts/0` via ExoFS pour les futurs sous-shells

**CapToken requis :**
```
tty_server reçoit délégation via device_server :
  CapToken { type: IpcEndpoint, rights: IPC_CONNECT | IPC_SEND, object_id: <fb_server_ep> }
  CapToken { type: IpcEndpoint, rights: IPC_CONNECT | IPC_SEND, object_id: <input_server_ep> }
```

---

### Étape 4 — Supprimer le court-circuit `BOOT_TTY_HANDLE` dans `fs_bridge.rs`
**Fichiers :** `kernel/src/syscall/fs_bridge.rs`

**Actions :**
1. Supprimer la constante `BOOT_TTY_HANDLE = 1` et le test `if fd <= 2`
2. `sys_read(fd=0)` → résolution normale via `OpenFileTable` → handle ExoFS `/dev/pts/0`
3. `sys_write(fd=1)` → résolution normale → handle ExoFS → tty_server via VFS
4. `open("/dev/pts/0", O_RDWR)` au démarrage d'exosh → reçoit fd 0/1/2 normaux
5. Le kernel ne touche plus jamais à `arch::x86_64::terminal` depuis les syscalls I/O

**Précaution de transition :**  
Garder `framebuffer_early` uniquement pour les panic kernel et les phases de boot pré-Ring1
(avant que fb_server soit `READY`). Le flag `framebuffer_early::is_active()` devient
`boot_phase_pre_userspace()`.

---

### Étape 5 — Nettoyer `arch/x86_64/terminal.rs`
**Fichiers :** `kernel/src/arch/x86_64/terminal.rs`, `kernel/src/arch/x86_64/framebuffer_early.rs`

**Ce qui reste dans `arch/` (légitimement) :**
- `debug_write()` → port 0xE9 QEMU debugcon — **outil de debug kernel, jamais userspace**
- `stage_ok()`, `boot_complete()` → affichage phases de boot pré-Ring1
- `panic_screen()` → rendu kernel panic avec registres — Ring0 uniquement

**Ce qui doit disparaître de `arch/` :**
- `write_from_process()` — appartient à fb_server
- `poll_byte_for_process()` / `read_byte_for_process()` — appartient à ps2_driver/input_server
- `KEYBOARD: Mutex<KeyboardState>` — appartient à ps2_driver
- `keyboard_irq_drain()` — appartient à ps2_driver
- `terminal_clear()`, `terminal_write_bytes()` depuis userspace paths — appartient à fb_server

---

## 4. Séquence de boot modifiée (après migration)

```
Étape 1 – 8   : inchangées (ARCH → MEMORY → TIME → DRIVERS → SCHEDULER → PROCESS → SECURITY → IPC)
Étape 9        : ExoFS mount — framebuffer_early reste actif (boot display)
Étape 10       : Ring1 boot — init_server PID1
Étape 11       : ps2_driver (PID=?) — enregistre IRQ1, CapToken[DEV_IRQ|DEV_MMIO]
Étape 12       : fb_server  (PID=?) — mappe framebuffer, CapToken[DEV_MMIO|MEM_MAP]
Étape 13       : input_server ready — ps2_driver lui envoie les InputEvent
Étape 14       : tty_server ready — câblé fb_server + input_server
Étape 15       : framebuffer_early désactivé — fb_server prend le relais
Étape 16       : exosh — open("/dev/pts/0") → fd 0/1/2 normaux via tty_server
```

---

## 5. Modèle CapToken pour le terminal (récapitulatif)

```
CapToken utilisés dans la chaîne terminal :

ps2_driver :
  { type: Device, rights: DEV_IRQ | DEV_MMIO, object_id: ps2_device }
  → permet SYS_IRQ_REGISTER(1) + SYS_IOPORT_READ(0x60/0x64)
  → accordé par device_server au boot, validé par kernel à chaque syscall

fb_server :
  { type: Device, rights: DEV_MMIO | MEM_MAP, object_id: framebuffer_device }
  → permet SYS_EXO_MMAP(phys_base_fb, len, MAP_PHYSMAP)
  → révocable par device_server si fb_server crashe (ExoPhoenix-safe)

tty_server :
  { type: IpcEndpoint, rights: IPC_CONNECT | IPC_SEND, object_id: fb_server_ep }
  { type: IpcEndpoint, rights: IPC_CONNECT | IPC_SEND, object_id: input_server_ep }
  → délégués par device_server au READY de fb_server et input_server

exosh :
  { type: FileInode, rights: READ | WRITE, object_id: /dev/pts/0 }
  → fd 0/1/2 installés par open() normal, pas de cap spéciale
  → aucun accès hardware direct — conforme Ring3
```

**Aucun de ces tokens ne porte de droits rwx sur les fichiers.**  
Les droits `READ` et `WRITE` dans le token désignent des droits d'accès à l'objet référencé
(ici un inode de device), pas des permissions POSIX. Le modèle CapToken remplace entièrement
`chmod`/`chown`/`rwx`.

---

## 6. Extension shell — 30 commandes exosh v0.2.0

Les commandes existantes (10 environ au boot actuel) plus 20 nouvelles.  
Toutes sont des built-ins exosh ou binaires `/bin/` — **aucune n'accède au hardware directement**.  
Tous les accès fichiers passent par ExoFS via CapToken[FileInode, READ|WRITE].

### Commandes existantes (conservées)
1. `help` — liste les commandes disponibles
2. `ping <ip> <count>` — ICMP echo via network_server
3. `tcping <ip> <port>` — TCP connect via network_server
4. `top` — liste les processus et leur état
5. `clear` — efface l'écran (FB_OP_CLEAR via tty_server)
6. `exit` — quitte exosh

### Nouvelles commandes — système de fichiers
7. `ls [path]` — liste le contenu d'un répertoire via SYS_GETDENTS64
8. `cat <file>` — affiche le contenu d'un fichier via SYS_READ
9. `echo <text>` — écrit sur stdout, supporte redirection `>`
10. `pwd` — affiche le répertoire courant via SYS_GETCWD
11. `cd <path>` — change de répertoire via SYS_CHDIR
12. `mkdir <path>` — crée un répertoire via SYS_MKDIR
13. `rm <file>` — supprime un fichier via SYS_UNLINK
14. `rmdir <dir>` — supprime un répertoire vide via SYS_RMDIR
15. `touch <file>` — crée un fichier vide via SYS_OPEN + SYS_CLOSE
16. `mv <src> <dst>` — renomme via SYS_RENAME
17. `cp <src> <dst>` — copie via SYS_READ + SYS_WRITE
18. `stat <path>` — infos d'un fichier via SYS_STAT (inode, taille, captoken type)

### Nouvelles commandes — processus
19. `ps` — liste détaillée PID/PPID/PGID/SID/état (extension de top)
20. `kill <pid> [signal]` — envoie un signal via SYS_KILL
21. `sleep <ms>` — attend N millisecondes via SYS_NANOSLEEP

### Nouvelles commandes — mémoire et kernel
22. `meminfo` — affiche l'état mémoire : free/used/cached/swap via SYS_SYSINFO
23. `capinfo <pid>` — liste les CapTokens détenus par un processus (ExoOS-specific)
24. `dmesg` — affiche le ring buffer de log kernel via SYS_SYSLOG

### Nouvelles commandes — réseau
25. `ifconfig` — affiche l'adresse MAC, IP, MTU via network_server IPC
26. `netstat` — liste les sockets TCP/UDP ouverts via network_server IPC
27. `wget <url>` — télécharge une URL via TCP → réseau → ExoFS (HTTP simple, pas HTTPS)

### Nouvelles commandes — debug kernel
28. `syscall-stat` — affiche les compteurs par numéro de syscall (stat_inc counters)
29. `ipc-stat` — affiche les stats IPC par endpoint (messages envoyés/reçus/timeout)
30. `reboot` — shutdown propre via SYS_REBOOT avec arrêt ordonné des serveurs Ring1

---

## 7. Ordre d'implémentation recommandé

```
Priorité 1 (déblocage terminal microkernel-correct) :
  Étape 1 — ps2_driver           ← débloque input réel Ring1
  Étape 2 — fb_server            ← débloque affichage Ring1
  Étape 3 — tty_server extension ← câble les deux
  Étape 4 — suppression BOOT_TTY_HANDLE
  Étape 5 — nettoyage arch/

Priorité 2 (extension shell — après terminal migré) :
  Groupe FS : ls, cat, echo, pwd, cd, mkdir, rm, rmdir, touch, mv, cp, stat
  Groupe process : ps, kill, sleep
  Groupe kernel : meminfo, capinfo, dmesg
  Groupe réseau : ifconfig, netstat, wget
  Groupe debug : syscall-stat, ipc-stat, reboot
```

---

## 8. Ce qui NE change PAS

- `framebuffer_early` reste pour les panics kernel et les phases pré-Ring1 — **légitime**
- `debug_write()` / port 0xE9 — debug kernel, jamais userspace — **légitime**
- Le modèle `CapToken` est déjà en place (`kernel/src/security/capability/`) — **aucune refonte**
- `input_server` reste tel quel — c'est déjà un relais IPC pur — **correct**
- `tty_server` existant (line buffer) reste, on l'étend — **pas de réécriture**
