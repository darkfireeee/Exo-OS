#![no_std]
//! # `exo-ui` — Moteur d'interface graphique 2D d'ExoOS
//!
//! Pure-Rust (`#![no_std]` + `alloc`), zéro dépendance externe, conçu pour le
//! repli logiciel comme pour les backends matériels (virtio-gpu / GPU natif) :
//! - [`color`] : palettes neutres Clair/Sombre, accents harmonisés et transitions fluides.
//! - [`canvas`] : primitives vectorielles anti-aliasées, flou, ombres et fonds harmonieux.
//! - [`font`] : polices anti-aliasées 8 bits (`.exofont`) UTF-8.
//! - [`charts`] : jauges radiales, courbes temporelles, barres SLA et radar 6 axes.
//! - [`text_edit`] : presse-papiers global, sélection souris/clavier, `Ctrl+A/C/X/V/Z/Y`.
//! - [`widgets`] : géométrie, contrôles natifs, états d'option et menus contextuels.

extern crate alloc;

pub mod canvas;
pub mod charts;
pub mod color;
pub mod font;
pub mod text_edit;
pub mod widgets;

pub use canvas::Canvas;
pub use color::{AccentPreset, AnimationMode, Color, Palette, Theme, ThemeMode};
pub use font::ExoFont;
pub use text_edit::{Clipboard, TextField};
pub use widgets::{ContextMenu, ContextMenuItem, OptionState, Rect};
