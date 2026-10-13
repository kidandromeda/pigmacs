// ————— pigmacs · navegador de ficheros (tree) —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::{
    app::{App, Mode},
    frame::Frame,
    theme::*,
};
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use std::env;
use std::fs;
use std::path::PathBuf;

pub(crate) fn browser_width(w: u16) -> u16 {
    (w / 4).clamp(24, 40)
}

pub(crate) struct Browser {
    pub(crate) active: bool,
    pub(crate) cwd: PathBuf,
    pub(crate) entries: Vec<PathBuf>,
    pub(crate) cursor: usize,
    pub(crate) scroll: usize,
}

impl Browser {
    pub(crate) fn refresh(&mut self) {
        self.entries.clear();
        let Ok(rd) = fs::read_dir(&self.cwd) else { return };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                dirs.push(name);
            } else {
                files.push(name);
            }
        }
        dirs.sort();
        files.sort();
        for d in dirs {
            self.entries.push(self.cwd.join(d));
        }
        for f in files {
            self.entries.push(self.cwd.join(f));
        }
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
    }

    pub(crate) fn cd(&mut self, dir: PathBuf) {
        // el cwd del proceso sigue al tree: gdu, ncdu y cualquier programa
        // lanzado arrancan en el directorio que estás mirando
        if env::set_current_dir(&dir).is_ok() {
            // $PWD sincronizada: algunos programas (p. ej. ranger) confían
            // en la variable y la comparan con su cwd real
            env::set_var("PWD", &dir);
        }
        self.cwd = dir;
        self.cursor = 0;
        self.scroll = 0;
        self.refresh();
    }

    pub(crate) fn cursor_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(crate) fn cursor_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }

    pub(crate) fn current(&self) -> Option<PathBuf> {
        self.entries.get(self.cursor).cloned()
    }

    pub(crate) fn go_top(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn go_bottom(&mut self) {
        self.cursor = self.entries.len().saturating_sub(1);
    }
}

pub(crate) fn handle_browser_key(app: &mut App, k: KeyEvent) {
    // los Ctrl/Alt no son comandos del navegador (Ctrl+B es de tmux, va al programa de dentro)
    if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return;
    }
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => app.browser.cursor_down(),
        KeyCode::Char('k') | KeyCode::Up => app.browser.cursor_up(),
        KeyCode::Char('l') | KeyCode::Enter | KeyCode::Right => app.browser_enter(),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => app.browser_up(),
        KeyCode::Char('g') => app.browser.go_top(),
        KeyCode::Char('G') => app.browser.go_bottom(),
        KeyCode::Char('b') => {
            if let Some(pos) = app.bookmarks.iter().position(|p| p == &app.browser.cwd) {
                app.bookmarks.remove(pos);
                app.status = format!(
                    "☆ Directorio desmarcado · quedan {}",
                    app.bookmarks.len()
                );
            } else {
                app.bookmarks.push(app.browser.cwd.clone());
                app.status = format!(
                    "★ Marcado · {} marcadores en total",
                    app.bookmarks.len()
                );
            }
        }
        KeyCode::Char('B') => {
            if app.bookmarks.is_empty() {
                app.status = "Sin marcadores: b marca el directorio actual".into();
            } else {
                let cur = app.bookmarks.iter().position(|p| p == &app.browser.cwd);
                let next = match cur {
                    Some(i) => (i + 1) % app.bookmarks.len(),
                    None => 0,
                };
                let dir = app.bookmarks[next].clone();
                app.browser.cd(dir);
                app.status = format!(
                    "★ Marcador {}/{} · {}",
                    next + 1,
                    app.bookmarks.len(),
                    app.browser.cwd.display()
                );
            }
        }
        KeyCode::Esc => {
            app.mode = app.top_mode;
        }
        _ => {}
    }
}

pub(crate) fn draw_browser(app: &mut App, bx: u16, h: u16, fr: &mut Frame) {
    // cabecera con banda de color: directorio actual, ★ si está marcado y
    // contador de marcadores a la derecha (el objetivo de b/B, siempre visible)
    let dirname = app
        .browser
        .cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.browser.cwd.display().to_string());
    let starred = app.bookmarks.iter().any(|p| p == &app.browser.cwd);
    let marks_n = app.bookmarks.len();
    let band_st = band(ACCENT, FG);
    for x in 0..bx {
        fr.put(x, 0, ' ', band_st);
    }
    let mut title = format!("┌ {dirname}");
    if starred {
        title.push_str(" ★");
    }
    if app.mode != Mode::Browser {
        title.push_str(" · Ctrl+F ");
    }
    let title: String = title.chars().take(bx.saturating_sub(1) as usize).collect();
    fr.text(0, 0, &title, pill(ACCENT, FG));
    if marks_n > 0 {
        let right = format!("★{marks_n} ");
        let rw = right.chars().count() as u16;
        if bx > rw + 2 {
            fr.text(bx - rw, 0, &right, bold(WARN));
        }
    }

    let rows = h.saturating_sub(3) as usize; // filas 1..h-3 (deja la ayuda y el estado)
    let start = app.browser.scroll.min(app.browser.entries.len());
    let end = (start + rows).min(app.browser.entries.len());
    let name_w = bx.saturating_sub(3) as usize;
    let border = fg(ACCENT);

    for (i, e) in app.browser.entries[start..end].iter().enumerate() {
        let y = 1 + i as u16;
        let sel = start + i == app.browser.cursor;
        let marked = app.bookmarks.iter().any(|p| p == e);
        let name = e
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let icon = if e.is_dir() { "▸" } else { "·" };
        let mut text = format!(" {} {}", icon, name);
        if marked {
            text.push_str(" ★");
        }
        let star_pos = if marked { text.chars().count() - 1 } else { 0 };
        let text: String = text.chars().take(name_w).collect();
        let hidden = name.starts_with('.');
        let st = if sel {
            // selección con fondo de acento (no vídeo inverso genérico)
            band(ACCENT_HI, Color::Black)
        } else if hidden {
            fg(DIM)
        } else if e.is_dir() {
            if marked {
                bold(DIR)
            } else {
                fg(DIR)
            }
        } else {
            fg(FILE)
        };
        fr.text(0, y, &text, st);
        // la ★ del directorio marcado, en su propio color de acento
        if marked && !sel && star_pos < text.chars().count() {
            fr.put(star_pos as u16, y, '★', bold(ACCENT_HI));
        }
        // separador vertical
        fr.put(bx - 1, y, '│', border);
    }

    // rellenar el resto de filas con el separador
    for y in (1 + (end - start) as u16)..h.saturating_sub(2) {
        fr.put(bx - 1, y, '│', border);
    }

    // si el cursor se sale de la ventana, ajustar scroll (se hace en cada frame)
    if app.browser.cursor < app.browser.scroll {
        app.browser.scroll = app.browser.cursor;
    }
    if app.browser.cursor >= app.browser.scroll + rows {
        app.browser.scroll = app.browser.cursor + 1 - rows;
    }
}
