# SPEC-EXO-GRAPHICS — Pile Graphique ExoOS v0.2.0
## Framebuffer Ring1 · winit/wgpu/iced reportés v0.3.0

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0 — Périmètre limité (pas de Wayland, pas de wgpu/iced)

---

## 1. Périmètre v0.2.0

**Ce que v0.2.0 établit :**
- L'architecture complète de la pile graphique (design, interfaces, intégration Ring1)
- Le `fb_server` (Ring1) opérationnel avec rendu framebuffer
- Le routage d'événements clavier/souris via `input_server`
- Le shell texte/TTY et le rendu framebuffer direct

**Ce que v0.2.0 ne fait PAS :**
- Serveur de compositing Wayland (v0.3.0)
- Accélération GPU via DRM/KMS (v0.3.0)
- `winit`, `wgpu` et `iced` (v0.3.0, après userspace `std` complet)
- Multi-fenêtrage (v0.3.0)
- Applications graphiques POSIX (vlc, firefox) — nécessitent Wayland

**Résultat attendu à la fin de v0.2.0 :**
Un terminal/shell framebuffer fonctionnel avec rendu de texte propre et saisie clavier. Pas de bureau complet, pas de toolkit GUI, mais la fondation framebuffer est en place.

---

## 2. Architecture Globale

```
RING 3 — Applications texte
    │
    │  exo-graphics (client IPC Ring3)
    │    │  IPC: GraphicsRequest::{ Blit, EventPoll, ... }
    │    ▼
─────────────────────────────────── IPC SpscRing + SHM ───
    ▼
RING 1 — fb_server
    │
    │  Gestion du framebuffer physique
    │  Réception des blits depuis Ring3
    │  Redistribution des événements HID (depuis input_server)
    │  Composition simple (une seule surface en v0.2.0)
    │
    │  input_server (Ring1)
    │    │  PS/2 keyboard/mouse → events → fb_server → Ring3
    │    ▼
    │  tty_server (Ring1)
    │    │  TTY séquences d'échappement → fb_server
    │
    └── framebuffer physique (UEFI GOP ou VGA)
```

---

## 3. fb_server — Spécification Ring1

### 3.1 Responsabilités

`fb_server` est le seul processus Ring1 qui peut écrire dans le framebuffer physique. Aucun processus Ring3 ne touche directement le framebuffer.

```rust
// fb_server/src/main.rs (Ring1)

pub struct FbServer {
    fb:         FramebufferHandle,  // Buffer physique UEFI GOP
    back_buf:   Vec<u32>,           // Back buffer (composition)
    surfaces:   Vec<Surface>,       // Surfaces allouées aux clients
    event_queue: VecDeque<InputEvent>, // Events en attente
}

pub struct Surface {
    pub client_cap: CapToken,       // Qui possède cette surface
    pub region:     FbRegion,       // { x, y, width, height }
    pub shm_buf:    ShmHandle,      // Buffer partagé Ring3↔fb_server
    pub z_order:    u32,            // Profondeur de composition
}

impl FbServer {
    fn main_loop(&mut self) {
        loop {
            // 1. Traiter les événements input depuis input_server
            while let Ok(ev) = ipc_try_recv(InputEndpoint::ID) {
                self.route_input_event(ev);
            }

            // 2. Traiter les requêtes de blit depuis Ring3
            while let Ok(req) = ipc_try_recv(FbEndpoint::ID) {
                match req {
                    FbRequest::Blit { surface_id, dirty_rect } => {
                        self.composite_surface(surface_id, dirty_rect);
                    }
                    FbRequest::AllocSurface { cap, width, height } => {
                        let s = self.alloc_surface(cap, width, height);
                        ipc_reply(FbResponse::Surface(s));
                    }
                    FbRequest::PollEvents { surface_id } => {
                        let events = self.drain_events_for(surface_id);
                        ipc_reply(FbResponse::Events(events));
                    }
                }
            }

            // 3. Présenter le back buffer si dirty
            if self.dirty {
                self.present();
                self.dirty = false;
            }

            sys_sched_yield();
        }
    }

    fn composite_surface(&mut self, id: SurfaceId, rect: FbRegion) {
        let surf = &self.surfaces[id as usize];
        // Copie depuis le SHM vers le back buffer
        let src = surf.shm_buf.as_slice();
        // ... blit avec clipping ...
        self.dirty = true;
    }

    fn present(&mut self) {
        // Copie back_buf → fb physique (GOP ou linear framebuffer)
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.back_buf.as_ptr(),
                self.fb.base as *mut u32,
                self.back_buf.len(),
            );
        }
    }
}
```

### 3.2 Protocole IPC Ring3 → fb_server

```rust
pub enum FbRequest {
    // Allouer une surface (réservation d'une région du framebuffer)
    AllocSurface { cap: CapToken, width: u32, height: u32 },
    // Demander au fb_server de recomposer une zone modifiée du SHM
    Blit { surface_id: u32, dirty_rect: Option<FbRegion> },
    // Récupérer les événements clavier/souris pour cette surface
    PollEvents { surface_id: u32 },
    // Libérer une surface
    FreeSurface { surface_id: u32 },
}

pub enum FbResponse {
    Surface(SurfaceInfo),  // { surface_id, shm_handle, actual_region }
    Events(Vec<InputEvent>),
    Ok,
    Err(FbError),
}

pub struct InputEvent {
    pub kind: InputKind,
    pub timestamp_ns: u64,
}

pub enum InputKind {
    KeyDown  { scancode: u16, keycode: u32, modifiers: Modifiers },
    KeyUp    { scancode: u16, keycode: u32, modifiers: Modifiers },
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: u8, pressed: bool, x: u32, y: u32 },
    Scroll   { dx: f32, dy: f32 },
}
```

---

## 4. winit — Report v0.3.0

`winit` est hors périmètre v0.2.0. Le brouillon ci-dessous est conservé comme cible v0.3.0, après disponibilité d'un userspace `std` complet et d'un backend graphique ExoOS.

```rust
// exo-graphics/src/winit_backend.rs

use winit::platform::run_return::EventLoopExtRunReturn;

pub struct ExoFbBackend {
    fb_cap:     CapToken,      // Capability display
    surface:    SurfaceInfo,   // Surface allouée par fb_server
    shm:        ShmSlice,      // Buffer partagé
}

impl winit::platform::ExoPlatform for ExoFbBackend {
    fn create_window(&mut self, attrs: WindowAttributes) -> WindowHandle {
        // Requête allocation surface au fb_server
        let surface = ipc_send_recv(
            FbEndpoint::ID,
            FbRequest::AllocSurface {
                cap: self.fb_cap,
                width: attrs.inner_size.width,
                height: attrs.inner_size.height,
            }
        ).unwrap();
        self.surface = surface;
        WindowHandle { id: surface.surface_id }
    }

    fn poll_events(&mut self, event_loop: &mut EventLoop) {
        // Récupérer les InputEvent du fb_server → les convertir en winit::Event
        let raw = ipc_send_recv(FbEndpoint::ID, FbRequest::PollEvents {
            surface_id: self.surface.surface_id
        }).unwrap();

        for ev in raw.events {
            let winit_event = match ev.kind {
                InputKind::KeyDown { keycode, modifiers, .. } =>
                    Event::WindowEvent {
                        event: WindowEvent::KeyboardInput {
                            input: KeyboardInput {
                                state: ElementState::Pressed,
                                virtual_keycode: Some(keycode_to_winit(keycode)),
                                modifiers: modifiers.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                // ... autres événements ...
                _ => continue,
            };
            event_loop.push(winit_event);
        }
    }
}
```

---

## 5. wgpu — Report v0.3.0

`wgpu` est hors périmètre v0.2.0. Le brouillon ci-dessous est conservé comme cible v0.3.0, avec Wayland/DRM-KMS et les primitives `std` requises.

```rust
// exo-graphics/src/wgpu_init.rs

pub async fn create_wgpu_context(surface: &ExoSurface) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        // v0.3.0 : backend software ou DRM/KMS selon plateforme
        backends: wgpu::Backends::empty() | wgpu::Backends::GL,
        ..Default::default()
    });

    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        compatible_surface: Some(&surface.wgpu_surface),
        force_fallback_adapter: true,  // forcer le software fallback
    }).await.expect("Aucun adapter wgpu disponible");

    let (device, queue) = adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("ExoOS Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
        },
        None,
    ).await.unwrap();

    (device, queue)
}

/// Surface wgpu basée sur le SHM fb_server
pub struct ExoSurface {
    shm:        ShmSlice,
    surface_id: u32,
}

impl wgpu::SurfaceTarget for ExoSurface {
    fn present(&self, pixels: &[u8], width: u32, height: u32) {
        // Copier les pixels rendus dans le SHM
        self.shm.as_mut_slice()[..pixels.len()].copy_from_slice(pixels);
        // Notifier fb_server de recomposer
        let _ = ipc_send(FbEndpoint::ID, FbRequest::Blit {
            surface_id: self.surface_id,
            dirty_rect: None,  // toute la surface
        });
    }
}
```

---

## 6. iced — Report v0.3.0

`iced` est hors périmètre v0.2.0 parce qu'il dépend du chemin `winit/wgpu`. Le brouillon ci-dessous décrit le shell graphique v0.3.0.

```rust
// exosh/src/main.rs — Shell graphique ExoOS (iced)

use iced::{Application, Command, Element, Settings, Theme};
use iced::widget::{column, text, text_input, scrollable};

pub struct ExoShell {
    history:    Vec<String>,   // Historique des commandes
    input:      String,        // Input courant
    output:     Vec<String>,   // Sortie des commandes
}

#[derive(Debug, Clone)]
pub enum Msg {
    InputChanged(String),
    CommandSubmit,
    OutputReceived(String),
}

impl Application for ExoShell {
    type Message = Msg;
    type Theme   = Theme;
    type Executor = exo_runtime::IcedExecutor;  // Notre executor
    type Flags   = ();

    fn new(_: ()) -> (Self, Command<Msg>) {
        (ExoShell {
            history: Vec::new(),
            input:   String::new(),
            output:  vec!["ExoOS v0.3.0 — exosh graphique".into(),
                          "Type 'exo help' for commands.".into()],
        }, Command::none())
    }

    fn title(&self) -> String { "ExoOS Shell".into() }

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        match msg {
            Msg::InputChanged(s) => { self.input = s; Command::none() }
            Msg::CommandSubmit => {
                let cmd = self.input.clone();
                self.history.push(cmd.clone());
                self.input.clear();
                // Exécuter la commande via exo-runtime
                Command::perform(
                    execute_command(cmd),
                    Msg::OutputReceived
                )
            }
            Msg::OutputReceived(out) => {
                for line in out.lines() {
                    self.output.push(line.to_string());
                }
                Command::none()
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        // Affichage capability-native dans le terminal
        let output_area = scrollable(
            column(self.output.iter().map(|line| {
                text(line).size(14).font(MONO_FONT).into()
            }).collect())
        );

        let prompt = text_input("$ ", &self.input)
            .on_input(Msg::InputChanged)
            .on_submit(Msg::CommandSubmit)
            .font(MONO_FONT);

        column![
            output_area,
            prompt,
        ].into()
    }
}

async fn execute_command(cmd: String) -> String {
    // Parser la commande et dispatcher
    match cmd.trim() {
        "exo ls" | "exo ls ." => exo_ls(".").await,
        s if s.starts_with("exo ls ") => exo_ls(&s[7..]).await,
        s if s.starts_with("exo install ") => exo_install(&s[12..]).await,
        s if s.starts_with("exo compat install ") => exo_compat_install(&s[19..]).await,
        "exo phoenix status" => exo_phoenix_status().await,
        "exo doctor" => exo_doctor().await,
        "exo audit" => exo_audit().await,
        "clear" => String::from("\x1b[2J\x1b[H"),
        "exit" | "quit" => { sys_exit(0); unreachable!() }
        other => format!("Commande inconnue : '{}'. Tapez 'exo help'", other),
    }
}
```

---

## 7. ExoPhoenix-Safety de la Pile Graphique

Cette section cible v0.3.0. En v0.2.0, la contrainte ExoPhoenix porte sur `fb_server` et les buffers framebuffer directs. Pour v0.3.0, `wgpu` maintiendra des ressources GPU (buffers, textures, pipelines) dans le driver. Lors d'une bascule ExoPhoenix :

```rust
impl PhoenixSafe for ExoGraphicsContext {
    fn on_pre_switch(&self) -> Result<(), PhoenixError> {
        // 1. Flush tous les command buffers en vol
        self.queue.submit([]);
        self.device.poll(wgpu::Maintain::Wait);
        
        // 2. Libérer les ressources GPU lourdes (textures, buffers)
        self.drop_gpu_resources();
        
        // 3. Invalider la surface fb_server (sera recréée après)
        ipc_send(FbEndpoint::ID, FbRequest::FreeSurface {
            surface_id: self.surface.surface_id
        })?;
        
        Ok(())
    }

    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        // 1. Réallouer une surface fb_server
        let surface = ipc_send_recv(FbEndpoint::ID, FbRequest::AllocSurface {
            cap: self.fb_cap, width: self.width, height: self.height
        })?;
        
        // 2. Réinitialiser wgpu (le context GPU survit si pas de bascule de GPU)
        self.reinit_wgpu_surface(&surface)?;
        
        // 3. Redessiner l'UI complète
        self.force_full_redraw = true;
        
        Ok(())
    }
}
```

---

## 8. Checklist v0.2.0

- [ ] `fb_server` Ring1 fonctionnel (framebuffer GOP UEFI)
- [ ] `input_server` → `fb_server` : événements PS/2 routés correctement
- [-] winit backend ExoOS — reporté v0.3.0
- [-] wgpu — reporté v0.3.0
- [-] iced — reporté v0.3.0
- [ ] `exosh` texte compilé et exécutable avec prompt fonctionnel
- [ ] `exo ls` dans exosh affiche le format capability natif
- [ ] Bascule ExoPhoenix avec exosh texte actif → exosh redémarre proprement
- [ ] Pas de corruption framebuffer après bascule

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-GRAPHICS.md*
