//! Tests unitaires ExoFS — un test file par sous-module.
//!
//! Chaque test vérifie les invariants du sous-module sans dépendance disque.

#[cfg(test)]
pub mod test_core;

#[cfg(test)]
pub mod test_epoch_record;

#[cfg(test)]
pub mod test_blob_id;

#[cfg(test)]
pub mod test_xchacha20;
