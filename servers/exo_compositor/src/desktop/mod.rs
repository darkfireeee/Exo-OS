//! ExoOS Desktop Shell — modularized into focused <= 260-line sub-modules.

pub mod dialogs;
pub mod dock;
pub mod input;
pub mod launcher;
pub mod render;
pub mod state;
pub mod topbar;
pub mod window;

pub use state::{DesktopShell, PowerRequest};
