//! Modular native graphical applications for ExoOS.

pub mod editor;
pub mod files;
pub mod games;
pub mod monitor;
pub mod settings;
pub mod terminal;
mod types;

pub use types::{ctx_cmd, draw_textfield, AppAction, AppKind};
