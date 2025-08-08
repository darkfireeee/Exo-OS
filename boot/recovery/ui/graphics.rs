use alloc::vec::Vec;

pub struct GraphicalUI {
    framebuffer: Vec<u32>,
    width: u32,
    height: u32,
}

impl GraphicalUI {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            framebuffer: vec![0; (width * height) as usize],
            width,
            height,
        }
    }

    pub fn init_graphics(&mut self) -> Result<(), &'static str> {
        // Initialisation du mode graphique
        Ok(())
    }

    pub fn draw_window(&mut self, window: &Window) {
        // Dessin d'une fenêtre graphique
    }

    pub fn handle_input(&mut self, event: InputEvent) {
        // Gestion des événements d'entrée
    }
}

#[derive(Debug)]
pub struct Window {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub title: String,
}

#[derive(Debug)]
pub enum InputEvent {
    MouseMove(u32, u32),
    MouseClick(u32, u32),
    KeyPress(char),
}
