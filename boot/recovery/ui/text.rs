use alloc::string::String;

pub struct TextUI {
    width: u32,
    height: u32,
}

impl TextUI {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn draw_menu(&self, items: &[MenuItem]) {
        // Affichage du menu en mode texte
    }

    pub fn get_input(&self) -> String {
        // Lecture de l'entrée utilisateur
        String::new()
    }

    pub fn display_error(&self, error: &str) {
        // Affichage d'un message d'erreur
    }
}

#[derive(Debug)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
}

#[derive(Debug)]
pub enum MenuAction {
    Backup,
    Restore,
    Repair,
    Exit,
}
