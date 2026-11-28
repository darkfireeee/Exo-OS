//! ExoFS file operations, read-only system zone guards, real Trash, clipboard & undo.

use crate::apps::files::{
    default_writable_dir, is_read_only_zone, join_path, FileUndoOp, FilesApp, FsEntry, SortBy,
    TRASH_DIR, USER_DOCS,
};
use exo_ui::{Clipboard, TextField};

impl FilesApp {
    pub fn reload(&mut self) {
        self.entries.clear();
        let target = self.cwd.clone();
        match std::fs::read_dir(&target) {
            Ok(rd) => {
                for ent in rd.flatten() {
                    let name = ent.file_name().to_string_lossy().into_owned();
                    let full = join_path(&target, &name);
                    let md = ent.metadata().ok();
                    let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
                    let is_exec = !is_dir && (target == "/bin" || target == "/sbin");
                    self.entries.push(FsEntry { name, path: full, is_dir, size, is_exec });
                }
                self.sort_entries();
                self.status_line = format!("{} items", self.entries.len());
            }
            Err(_) => {
                self.status_line = format!("Inaccessible path: {}", target);
            }
        }
        self.selected.retain(|&i| i < self.entries.len());
        self.refresh_inspector();
    }

    pub fn sort_entries(&mut self) {
        let by = self.sort_by;
        self.entries.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| match by {
                SortBy::Name => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
                SortBy::Size => b.size.cmp(&a.size),
                SortBy::Kind => file_ext(&a.name).cmp(file_ext(&b.name)).then_with(|| a.name.cmp(&b.name)),
            })
        });
    }

    pub fn cycle_sort(&mut self) {
        self.sort_by = match self.sort_by {
            SortBy::Name => SortBy::Size,
            SortBy::Size => SortBy::Kind,
            SortBy::Kind => SortBy::Name,
        };
        self.sort_entries();
        self.status_line = match self.sort_by {
            SortBy::Name => String::from("Trié par nom"),
            SortBy::Size => String::from("Trié par taille"),
            SortBy::Kind => String::from("Trié par type"),
        };
    }

    pub fn select_all(&mut self) {
        if self.search_focused {
            self.search.select_all();
        } else if self.renaming_idx.is_some() {
            self.rename_field.select_all();
        } else {
            self.selected = self.filtered_indices();
            self.refresh_inspector();
        }
    }

    pub fn create_new_folder(&mut self) {
        if is_read_only_zone(&self.cwd) {
            self.status_line = String::from("Refusé : zone système en lecture seule");
            return;
        }
        let redirect_home = self.cwd == "/";
        let base_dir = if redirect_home { default_writable_dir(USER_DOCS) } else { self.cwd.clone() };
        let _ = std::fs::create_dir_all(&base_dir);
        for n in 1..=99 {
            let name = if n == 1 { String::from("Nouveau dossier") } else { format!("Nouveau dossier {}", n) };
            let path = join_path(&base_dir, &name);
            if std::fs::metadata(&path).is_err() {
                match std::fs::create_dir(&path) {
                    Ok(()) => {
                        self.undo_stack.push(FileUndoOp::CreatedDir { path: path.clone() });
                        if redirect_home {
                            self.navigate(&base_dir);
                            self.status_line = format!("Dossier créé dans {} : {}", base_dir, name);
                        } else {
                            self.status_line = format!("Dossier créé : {}", name);
                            self.reload();
                        }
                    }
                    Err(_) => self.status_line = String::from("Erreur : permission refusée ou volume en lecture seule"),
                }
                return;
            }
        }
    }

    pub fn create_new_file(&mut self) {
        if is_read_only_zone(&self.cwd) {
            self.status_line = String::from("Refusé : zone système en lecture seule");
            return;
        }
        let redirect_home = self.cwd == "/";
        let base_dir = if redirect_home { default_writable_dir(USER_DOCS) } else { self.cwd.clone() };
        let _ = std::fs::create_dir_all(&base_dir);
        for n in 1..=99 {
            let name = if n == 1 { String::from("note.txt") } else { format!("note_{}.txt", n) };
            let path = join_path(&base_dir, &name);
            if std::fs::metadata(&path).is_err() {
                match std::fs::write(&path, b"# Document ExoOS\n") {
                    Ok(()) => {
                        self.undo_stack.push(FileUndoOp::CreatedFile { path: path.clone() });
                        if redirect_home {
                            self.navigate(&base_dir);
                            self.status_line = format!("Fichier créé dans {} : {}", base_dir, name);
                        } else {
                            self.status_line = format!("Fichier créé : {}", name);
                            self.reload();
                        }
                    }
                    Err(_) => self.status_line = String::from("Erreur : permission refusée ou volume en lecture seule"),
                }
                return;
            }
        }
    }

    pub fn begin_rename(&mut self) {
        if is_read_only_zone(&self.cwd) {
            self.status_line = String::from("Refusé : zone système en lecture seule");
            return;
        }
        if let Some(&idx) = self.selected.first() {
            if let Some(ent) = self.entries.get(idx) {
                self.renaming_idx = Some(idx);
                self.rename_field = TextField::with_text(&ent.name);
                self.rename_field.select_all();
            }
        }
    }

    pub fn commit_rename(&mut self) {
        let Some(idx) = self.renaming_idx.take() else { return; };
        let new_name = self.rename_field.text.trim().to_string();
        if new_name.is_empty() || new_name.contains('/') {
            self.status_line = String::from("Nom invalide");
            return;
        }
        if let Some(ent) = self.entries.get(idx) {
            let dst = join_path(&self.cwd, &new_name);
            if dst != ent.path {
                match std::fs::rename(&ent.path, &dst) {
                    Ok(()) => {
                        self.undo_stack.push(FileUndoOp::Renamed {
                            old_path: ent.path.clone(),
                            new_path: dst,
                        });
                        self.status_line = format!("Renommé en « {} »", new_name);
                        self.reload();
                    }
                    Err(_) => self.status_line = String::from("Renommage refusé par le système"),
                }
            }
        }
    }

    pub fn copy_selection(&mut self, clip: &mut Clipboard, cut: bool) {
        if self.search_focused {
            if cut { self.search.cut(clip); } else { self.search.copy(clip); }
            return;
        }
        if self.renaming_idx.is_some() {
            if cut { self.rename_field.cut(clip); } else { self.rename_field.copy(clip); }
            return;
        }
        let paths: Vec<String> = self.selected.iter().filter_map(|&i| self.entries.get(i).map(|e| e.path.clone())).collect();
        if !paths.is_empty() {
            let n = paths.len();
            clip.set_text(&paths.join("\n"));
            clip.set_files(paths, cut);
            self.status_line = if cut { format!("{} élément(s) coupé(s)", n) } else { format!("{} élément(s) copié(s)", n) };
        }
    }

    pub fn paste_into_cwd(&mut self, clip: &mut Clipboard) {
        if self.search_focused { self.search.paste(clip); return; }
        if self.renaming_idx.is_some() { self.rename_field.paste(clip); return; }
        if is_read_only_zone(&self.cwd) {
            self.status_line = String::from("Collage refusé : zone système en lecture seule");
            return;
        }
        if clip.file_paths.is_empty() { return; }
        let mut count = 0usize;
        for src in clip.file_paths.clone() {
            let fname = src.rsplit('/').next().unwrap_or("copie");
            let mut dst = join_path(&self.cwd, fname);
            if dst == src {
                dst = join_path(&self.cwd, &format!("copie_{}", fname));
            }
            let ok = if clip.file_cut {
                std::fs::rename(&src, &dst).is_ok()
            } else if let Ok(data) = std::fs::read(&src) {
                std::fs::write(&dst, &data).is_ok()
            } else {
                false
            };
            if ok { count += 1; }
        }
        if clip.file_cut { clip.clear_files(); }
        self.status_line = format!("{} élément(s) collé(s)", count);
        self.reload();
    }

    /// Move selected items to real Trash (`/tmp/.exo_trash`) per Spec §2.3 & §2.6.
    pub fn move_selected_to_trash(&mut self) {
        if is_read_only_zone(&self.cwd) {
            self.status_line = String::from("Suppression refusée : zone système protégée");
            return;
        }
        let _ = std::fs::create_dir_all(TRASH_DIR);
        let mut moved = 0usize;
        for &idx in &self.selected {
            if let Some(ent) = self.entries.get(idx) {
                let dst = join_path(TRASH_DIR, &ent.name);
                if std::fs::rename(&ent.path, &dst).is_ok() {
                    self.undo_stack.push(FileUndoOp::Trashed {
                        orig_path: ent.path.clone(),
                        trash_path: dst,
                    });
                    moved += 1;
                }
            }
        }
        self.status_line = if moved > 0 {
            format!("{} élément(s) déplacé(s) vers la corbeille", moved)
        } else {
            String::from("Impossible de déplacer vers la corbeille")
        };
        self.reload();
    }

    pub fn restore_from_trash(&mut self) {
        let target_dir = default_writable_dir(USER_DOCS);
        let mut restored = 0usize;
        for &idx in &self.selected {
            if let Some(ent) = self.entries.get(idx) {
                let dst = join_path(&target_dir, &ent.name);
                if std::fs::rename(&ent.path, &dst).is_ok() {
                    restored += 1;
                }
            }
        }
        self.status_line = format!("{} élément(s) restauré(s) dans {}", restored, target_dir);
        self.reload();
    }

    pub fn delete_selected_permanently(&mut self) {
        let mut removed = 0usize;
        for &idx in &self.selected {
            if let Some(ent) = self.entries.get(idx) {
                let ok = if ent.is_dir { std::fs::remove_dir(&ent.path).is_ok() } else { std::fs::remove_file(&ent.path).is_ok() };
                if ok { removed += 1; }
            }
        }
        self.status_line = format!("{} élément(s) supprimé(s) définitivement", removed);
        self.reload();
    }

    pub fn empty_trash(&mut self) {
        if let Ok(rd) = std::fs::read_dir(TRASH_DIR) {
            for ent in rd.flatten() {
                let p = ent.path();
                let _ = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p));
            }
        }
        self.status_line = String::from("Corbeille vidée");
        self.reload();
    }

    pub fn undo(&mut self) {
        if self.search_focused { self.search.undo(); return; }
        if self.renaming_idx.is_some() { self.rename_field.undo(); return; }
        if let Some(op) = self.undo_stack.pop() {
            match op {
                FileUndoOp::Renamed { old_path, new_path } => { let _ = std::fs::rename(&new_path, &old_path); }
                FileUndoOp::CreatedFile { path } => { let _ = std::fs::remove_file(&path); }
                FileUndoOp::CreatedDir { path } => { let _ = std::fs::remove_dir(&path); }
                FileUndoOp::Trashed { orig_path, trash_path } => { let _ = std::fs::rename(&trash_path, &orig_path); }
            }
            self.status_line = String::from("Action annulée");
            self.reload();
        }
    }
}

fn file_ext(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or("")
}
