// ————— pigmacs · estado central —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::{
    browser::Browser,
    frame::{strip_ansi, Frame},
    launcher::AppEntry,
    launcher::XSesion,
    program::Program,
    shell::Shell,
};
use anyhow::Result;
use crossterm::terminal;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Normal,
    Insert,
    Visual,
    Shell,
    Ai,
    Browser,
    Program, // programa lanzado desde el menú (Ctrl+P), con su propio PTY
}

pub(crate) struct App {
    pub(crate) path: PathBuf,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) col: usize,
    pub(crate) top: usize,
    pub(crate) left: usize,
    pub(crate) anchor: Option<(usize, usize)>,
    pub(crate) mode: Mode,
    pub(crate) top_mode: Mode, // editor (Normal) o Ai: a dónde vuelve Tab desde el shell
    pub(crate) dirty: bool,
    pub(crate) shell: Shell,
    pub(crate) status: String,
    pub(crate) pending: Option<char>,
    pub(crate) search_query: String,
    pub(crate) searching: bool,
    pub(crate) matches: Vec<(usize, usize)>,
    pub(crate) match_idx: usize,
    // org
    pub(crate) folded: HashSet<usize>,
    // marcas de vim: m<x> fija · '<x> salta
    pub(crate) marks: HashMap<char, (usize, usize)>,
    // navegador
    pub(crate) browser: Browser,
    // ia
    pub(crate) ai_lines: Vec<String>,
    pub(crate) ai_input: String,
    pub(crate) ai_history: Vec<String>,
    pub(crate) ai_hist_idx: usize,
    pub(crate) ai_running: bool,
    pub(crate) ai_out: Arc<Mutex<String>>,
    pub(crate) ai_flag: Arc<AtomicBool>,
    pub(crate) ai_child: Arc<Mutex<Option<std::process::Child>>>,
    pub(crate) ai_session_n: usize, // nº de conversación pi (Ctrl+N la reinicia)
    pub(crate) ai_selecting: bool,
    pub(crate) ai_sel_anchor: Option<usize>,
    pub(crate) ai_sel_cursor: usize,
    pub(crate) shell_rows: u16,
    pub(crate) work_dir: PathBuf,
    pub(crate) redraw: bool, // si es false, se salta el redibujado (anti-parpadeo)
    pub(crate) prev: Frame, // último frame pintado (para el diff por celdas)
    pub(crate) editor_rows: u16, // altura actual del panel de edición (scroll al 75%)
    pub(crate) read_only: bool, // fichero sin permisos de escritura: Ctrl+S pedirá sudo
    pub(crate) sudo_prompt: bool, // pidiendo contraseña sudo
    pub(crate) sudo_pass: String, // contraseña (se limpia tras usarla)
    pub(crate) sudo_text: String, // contenido a escribir vía sudo
    // deshacer/rehacer
    pub(crate) undo: Vec<(Vec<String>, usize, usize)>,
    pub(crate) redo: Vec<(Vec<String>, usize, usize)>,
    pub(crate) undo_kind: Option<&'static str>,
    // contador numérico (12G) y números de línea
    pub(crate) count: Option<usize>,
    pub(crate) show_numbers: bool,
    // marcadores de directorio del tree
    pub(crate) bookmarks: Vec<PathBuf>,
    // ventana de ayuda
    pub(crate) help_open: bool,
    pub(crate) help_scroll: usize,
    // programas lanzados desde el menú (Ctrl+P)
    pub(crate) program: Option<Program>,
    pub(crate) menu_open: bool,
    pub(crate) menu_cursor: usize,
    // filtro del menú Ctrl+P: escribir filtra (difuso), Enter lanza
    // lo tecleado si no hay coincidencias
    pub(crate) menu_query: String,
    // lanzador de aplicaciones gráficas (Ctrl+X): .desktop del sistema
    pub(crate) launcher_open: bool,
    pub(crate) launcher_query: String,
    pub(crate) launcher_cursor: usize,
    pub(crate) launcher_apps: Vec<AppEntry>, // caché al abrir (se relee cada vez)
    // app gráfica lanzada con startx sobre la tty real: mientras vive,
    // pigmacs se aparta y no pinta nada (x_poll en el bucle de eventos)
    pub(crate) x: Option<XSesion>,
    // consola de texto real (/dev/ttyN): saneado de avisos del kernel
    pub(crate) vt_mode: bool,
    pub(crate) force_full: bool, // repintar todas las celdas en el próximo draw
    // bell pendiente de emitir (fin de la consulta IA): lo escribe draw()
    pub(crate) bell: bool,
}

impl App {
    // ---------- texto ----------
    pub(crate) fn selected(&self) -> String {
        let Some((ar, ac)) = self.anchor else { return String::new() };
        let (br, bc) = (self.row, self.col);
        let ((r1, c1), (r2, c2)) = if (ar, ac) <= (br, bc) {
            ((ar, ac), (br, bc))
        } else {
            ((br, bc), (ar, ac))
        };

        if r1 == r2 {
            return self.lines[r1]
                .chars()
                .skip(c1)
                .take(c2.saturating_sub(c1) + 1)
                .collect();
        }

        let mut s = String::new();
        s.push_str(&self.lines[r1].chars().skip(c1).collect::<String>());
        s.push('\n');
        for r in r1 + 1..r2 {
            s.push_str(&self.lines[r]);
            s.push('\n');
        }
        s.push_str(&self.lines[r2].chars().take(c2 + 1).collect::<String>());
        s
    }

    pub(crate) fn line_len(&self) -> usize {
        self.lines.get(self.row).map(|s| s.chars().count()).unwrap_or(0)
    }

    pub(crate) fn clamp_col(&mut self) {
        let max = self.line_len().saturating_sub(1);
        if self.col > max && self.line_len() > 0 {
            self.col = max;
        } else if self.line_len() == 0 {
            self.col = 0;
        }
    }

    pub(crate) fn move_cursor(&mut self, dr: isize, dc: isize) {
        if dr != 0 {
            if let Some(nr) = self.next_visible_row(self.row, dr) {
                self.row = nr;
            }
        }
        let nc = (self.col as isize + dc).max(0) as usize;
        self.col = nc.min(self.line_len().saturating_sub(1).max(0));
        self.ensure_visible();
    }

    pub(crate) fn ensure_hscroll(&mut self, width: usize) {
        let editor_w = width.saturating_sub(self.gutter() + 1);
        if editor_w == 0 {
            return;
        }
        if self.col < self.left {
            self.left = self.col;
        }
        if self.col >= self.left + editor_w {
            self.left = self.col + 1 - editor_w;
        }
    }

    pub(crate) fn ensure_visible(&mut self) {
        // margen de scroll: el cursor llega hasta el 75% de la altura del editor
        let visible = (self.editor_rows as usize) * 3 / 4;
        if visible == 0 {
            return;
        }
        if self.row < self.top {
            self.top = self.row;
            return;
        }
        // se cuentan filas *visibles*: las plegadas no ocupan pantalla. Con
        // filas lógicas, pasar de una cabecera plegada a otra daba un `top`
        // menor que la fila real, el texto saltaba hacia arriba y el cursor
        // aparecía al principio de la ventana
        let mut r = self.row;
        let mut contadas = 0usize;
        while contadas < visible && r >= self.top {
            if !self.is_hidden(r) {
                contadas += 1;
                if contadas == visible && r > self.top {
                    // `visible` filas visibles entre `r` y el cursor: el
                    // cursor queda justo en la última del margen
                    self.top = r;
                }
            }
            if r == 0 {
                break;
            }
            r -= 1;
        }
    }

    pub(crate) fn insert_char(&mut self, ch: char) {
        self.push_undo("insert");
        let line = &mut self.lines[self.row];
        let byte = line
            .char_indices()
            .nth(self.col)
            .map(|x| x.0)
            .unwrap_or(line.len());
        line.insert(byte, ch);
        self.col += 1;
        self.dirty = true;
    }

    pub(crate) fn newline(&mut self) {
        self.push_undo("insert");
        let byte = self.lines[self.row]
            .char_indices()
            .nth(self.col)
            .map(|x| x.0)
            .unwrap_or(self.lines[self.row].len());
        let rest = self.lines[self.row].split_off(byte);
        self.lines.insert(self.row + 1, rest);
        self.shift_folds_insert(self.row + 1);
        self.row += 1;
        self.col = 0;
        self.dirty = true;
        self.ensure_visible();
    }

    pub(crate) fn backspace(&mut self) {
        self.push_undo("delete");
        if self.col > 0 {
            let byte = self.lines[self.row]
                .char_indices()
                .nth(self.col - 1)
                .map(|x| x.0)
                .unwrap_or(0);
            self.lines[self.row].remove(byte);
            self.col -= 1;
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.shift_folds_remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&current);
        }
        self.dirty = true;
        self.clamp_col();
    }

    pub(crate) fn delete_char_at_cursor(&mut self) {
        self.push_undo("delete");
        let len = self.line_len();
        if len == 0 {
            return;
        }
        let idx = self.col.min(len.saturating_sub(1));
        let line = &mut self.lines[self.row];
        let byte = line
            .char_indices()
            .nth(idx)
            .map(|x| x.0)
            .unwrap_or(line.len());
        if byte < line.len() {
            line.remove(byte);
        }
        self.clamp_col();
        self.dirty = true;
    }

    pub(crate) fn delete_line(&mut self) {
        self.push_undo("line");
        if self.lines.len() <= 1 {
            self.lines[0].clear();
        } else {
            self.lines.remove(self.row);
            self.shift_folds_remove(self.row);
            if self.row >= self.lines.len() {
                self.row = self.lines.len() - 1;
            }
        }
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    pub(crate) fn insert_line_below(&mut self) {
        self.push_undo("line");
        self.lines.insert(self.row + 1, String::new());
        self.shift_folds_insert(self.row + 1);
        self.row += 1;
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    pub(crate) fn insert_line_above(&mut self) {
        self.push_undo("line");
        self.lines.insert(self.row, String::new());
        self.shift_folds_insert(self.row);
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    pub(crate) fn delete_selection(&mut self) {
        self.push_undo("sel");
        let Some((ar, ac)) = self.anchor else { return };
        let (br, bc) = (self.row, self.col);
        let ((r1, c1), (r2, c2)) = if (ar, ac) <= (br, bc) {
            ((ar, ac), (br, bc))
        } else {
            ((br, bc), (ar, ac))
        };

        if r1 == r2 {
            let line = &mut self.lines[r1];
            let b1 = line
                .char_indices()
                .nth(c1)
                .map(|x| x.0)
                .unwrap_or(line.len());
            let b2 = line
                .char_indices()
                .nth(c2 + 1)
                .map(|x| x.0)
                .unwrap_or(line.len());
            line.drain(b1..b2);
            self.row = r1;
            self.col = c1;
        } else {
            let l1 = &self.lines[r1];
            let b1 = l1.char_indices().nth(c1).map(|x| x.0).unwrap_or(l1.len());
            let tail1 = l1[b1..].to_string();
            let l2 = &self.lines[r2];
            let b2 = l2
                .char_indices()
                .nth(c2 + 1)
                .map(|x| x.0)
                .unwrap_or(l2.len());
            let tail2 = l2[b2..].to_string();
            let merged = format!("{}{}", tail1, tail2);
            self.lines.drain(r1..=r2);
            self.shift_folds_range(r1, r2);
            self.lines.insert(r1, merged);
            self.row = r1;
            self.col = c1;
        }
        self.anchor = None;
        self.mode = Mode::Normal;
        self.clamp_col();
        self.ensure_visible();
        self.dirty = true;
    }

    pub(crate) fn save(&mut self) -> Result<()> {
        let text = self.lines.join("\n");
        if self.read_only {
            self.begin_sudo_save(text);
            return Ok(());
        }
        match fs::write(&self.path, &text) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("Guardado: {}", self.path.display());
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                self.begin_sudo_save(text);
            }
            Err(e) => {
                self.status = format!("No se pudo guardar: {}", e);
            }
        }
        Ok(())
    }

    /// pide la contraseña sudo para escribir un fichero sin permisos
    pub(crate) fn begin_sudo_save(&mut self, text: String) {
        self.sudo_text = text;
        self.sudo_pass.clear();
        self.sudo_prompt = true;
        self.status = format!(
            "Solo lectura: introduce contraseña sudo para {}",
            self.path.display()
        );
    }

    /// escribe el fichero vía `sudo -S sh -c 'cat > "$1"'` con la contraseña por stdin
    pub(crate) fn sudo_save_now(&mut self) {
        let pass = self.sudo_pass.clone();
        let text = self.sudo_text.clone();
        let path = self.path.clone();
        self.sudo_prompt = false;
        self.sudo_pass.clear();
        self.sudo_text.clear();

        let mut cmd = Command::new("sudo");
        cmd.arg("-S");
        cmd.arg("sh").arg("-c").arg("cat > \"$1\"").arg("sh").arg(&path);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("sudo no disponible: {}", e);
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(format!("{}\n{}", pass, text).as_bytes());
        }
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if ok {
            self.dirty = false;
            self.status = format!("Guardado (sudo): {}", path.display());
        } else {
            let msg = err
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .last()
                .unwrap_or("contraseña incorrecta")
                .to_string();
            self.status = format!("sudo falló: {} (Ctrl+S para reintentar)", msg);
        }
    }

    pub(crate) fn send_selection(&mut self) -> Result<()> {
        // sin selección activa: enviar la línea entera bajo el cursor
        let s = if self.anchor.is_some() {
            self.selected()
        } else {
            self.lines.get(self.row).cloned().unwrap_or_default()
        };
        if s.trim().is_empty() {
            self.status = "Línea vacía".into();
            return Ok(());
        }

        self.shell.writer.write_all(s.as_bytes())?;
        self.shell.writer.write_all(b"\n")?;
        self.shell.writer.flush()?;

        self.mode = Mode::Normal;
        self.anchor = None;
        self.status = "Enviado al shell".into();
        Ok(())
    }

    // ---------- deshacer / rehacer ----------
    pub(crate) fn push_undo(&mut self, kind: &'static str) {
        if self.undo_kind == Some(kind) {
            return;
        }
        self.undo.push((self.lines.clone(), self.row, self.col));
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.undo_kind = Some(kind);
        self.redo.clear();
    }

    pub(crate) fn undo_edit(&mut self) {
        if let Some((lines, row, col)) = self.undo.pop() {
            self.redo.push((self.lines.clone(), self.row, self.col));
            self.restore_state(lines, row, col);
            self.status = "Deshecho".into();
        } else {
            self.status = "Nada que deshacer".into();
        }
        self.undo_kind = None;
    }

    pub(crate) fn redo_edit(&mut self) {
        if let Some((lines, row, col)) = self.redo.pop() {
            self.undo.push((self.lines.clone(), self.row, self.col));
            self.restore_state(lines, row, col);
            self.status = "Rehecho".into();
        } else {
            self.status = "Nada que rehacer".into();
        }
        self.undo_kind = None;
    }

    pub(crate) fn restore_state(&mut self, lines: Vec<String>, row: usize, col: usize) {
        self.lines = lines;
        self.row = row.min(self.lines.len().saturating_sub(1));
        self.col = col;
        self.clamp_col();
        self.ensure_visible();
        self.dirty = true;
    }

    /// ancho del margen izquierdo del editor (números de línea o un espacio)
    pub(crate) fn gutter(&self) -> usize {
        if self.show_numbers {
            6
        } else {
            1
        }
    }

    // ---------- ia: selección del panel ----------
    /// líneas visibles del panel IA (historial + salida en curso)
    pub(crate) fn ai_pane_lines(&self, max_lines: usize) -> Vec<String> {
        let mut all: Vec<String> = self.ai_lines.clone();
        if self.ai_running {
            let cur = self.ai_out.lock().unwrap();
            for l in cur.split('\n') {
                if !l.is_empty() {
                    all.push(l.to_string());
                }
            }
        }
        let start = all.len().saturating_sub(max_lines);
        all[start..].to_vec()
    }

    /// envía la selección hecha en el panel IA al shell
    pub(crate) fn send_ai_selection(&mut self) -> Result<()> {
        let Some(a) = self.ai_sel_anchor else {
            self.status = "Sin selección: pulsa v en el panel IA".into();
            return Ok(());
        };
        let (r1, r2) = if a <= self.ai_sel_cursor {
            (a, self.ai_sel_cursor)
        } else {
            (self.ai_sel_cursor, a)
        };
        let (_, h) = terminal::size()?;
        let (editor_rows, _) = layout(h, self.shell_rows);
        let max_lines = editor_rows.saturating_sub(3) as usize;
        let pane = self.ai_pane_lines(max_lines.max(1));
        let mut s = String::new();
        let last = r2.min(pane.len().saturating_sub(1));
        for i in r1..=last {
            if let Some(l) = pane.get(i) {
                let clean = strip_ansi(l).replace('\r', "");
                if !clean.trim().is_empty() {
                    s.push_str(&clean);
                    s.push('\n');
                }
            }
        }
        let s = s.trim_end().to_string();
        if s.is_empty() {
            self.status = "Selección IA vacía".into();
            return Ok(());
        }
        self.shell.writer.write_all(s.as_bytes())?;
        self.shell.writer.write_all(b"\n")?;
        self.shell.writer.flush()?;
        self.ai_selecting = false;
        self.ai_sel_anchor = None;
        self.status = "Enviado al shell desde IA".into();
        Ok(())
    }

    // ---------- navegador de ficheros ----------
    pub(crate) fn open_file(&mut self, path: PathBuf) {
        let text = fs::read_to_string(&path).unwrap_or_default();
        let lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.lines().map(str::to_owned).collect()
        };
        self.path = path.clone();
        self.lines = lines;
        self.row = 0;
        self.col = 0;
        self.top = 0;
        self.left = 0;
        self.anchor = None;
        // los headers se abren SIEMPRE plegados: el usuario los despliega con Tab/za
        self.folded.clear();
        for r in 0..self.lines.len() {
            if self.header_level(r).is_some() {
                self.folded.insert(r);
            }
        }
        self.search_query.clear();
        self.matches.clear();
        self.pending = None;
        self.undo.clear();
        self.redo.clear();
        self.undo_kind = None;
        self.dirty = false;
        self.read_only = !file_writable(&path);
        self.status = if self.read_only {
            format!(
                "Abierto (solo lectura): {} · Ctrl+S pedirá sudo",
                path.display()
            )
        } else {
            format!("Abierto: {}", path.display())
        };
        self.mode = self.top_mode;
    }

    pub(crate) fn browser_enter(&mut self) {
        let Some(p) = self.browser.current() else { return };
        if p.is_dir() {
            self.browser.cd(p);
            self.status = format!("Directorio: {}", self.browser.cwd.display());
        } else {
            self.open_file(p);
        }
    }

    pub(crate) fn browser_up(&mut self) {
        // anclar rutas relativas (p.ej. ".") a su absoluta para poder subir por todo el sistema
        if self.browser.cwd.is_relative() {
            if let Ok(abs) = fs::canonicalize(&self.browser.cwd) {
                self.browser.cwd = abs;
            }
        }
        if let Some(parent) = self.browser.cwd.parent() {
            let p = parent.to_path_buf();
            if !p.as_os_str().is_empty() {
                self.browser.cd(p);
                self.status = format!("Directorio: {}", self.browser.cwd.display());
            }
        }
    }

    // ---------- org: cabeceras y plegado ----------
    pub(crate) fn header_level(&self, row: usize) -> Option<u8> {
        let l = self.lines.get(row)?;
        let b = l.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i] == b'*' {
            i += 1;
        }
        if i > 0 && i < b.len() && b[i] == b' ' {
            Some(i as u8)
        } else {
            None
        }
    }

    /// última línea cubierta por la cabecera `h` (contenido + subcabeceras)
    pub(crate) fn fold_end(&self, h: usize) -> usize {
        let Some(lvl) = self.header_level(h) else { return h };
        let mut r = h + 1;
        while r < self.lines.len() {
            if let Some(l) = self.header_level(r) {
                if l <= lvl {
                    break;
                }
            }
            r += 1;
        }
        r - 1
    }

    pub(crate) fn is_hidden(&self, row: usize) -> bool {
        if row >= self.lines.len() {
            return false;
        }
        for &h in &self.folded {
            if h < row && row <= self.fold_end(h) {
                return true;
            }
        }
        false
    }

    pub(crate) fn next_visible_row(&self, from: usize, dir: isize) -> Option<usize> {
        let mut r = from as isize + dir;
        while r >= 0 && (r as usize) < self.lines.len() {
            let u = r as usize;
            if !self.is_hidden(u) {
                return Some(u);
            }
            r += dir;
        }
        None
    }

    pub(crate) fn first_visible(&self) -> usize {
        (0..self.lines.len())
            .find(|&r| !self.is_hidden(r))
            .unwrap_or(0)
    }

    pub(crate) fn last_visible(&self) -> usize {
        (0..self.lines.len())
            .rev()
            .find(|&r| !self.is_hidden(r))
            .unwrap_or(0)
    }

    pub(crate) fn reveal_row(&mut self, row: usize) {
        if row >= self.lines.len() {
            return;
        }
        let to_remove: Vec<usize> = self
            .folded
            .iter()
            .copied()
            .filter(|&h| h < row && row <= self.fold_end(h))
            .collect();
        for h in to_remove {
            self.folded.remove(&h);
        }
    }

    pub(crate) fn toggle_fold(&mut self, row: usize) {
        if self.header_level(row).is_none() {
            return;
        }
        if self.folded.contains(&row) {
            self.folded.remove(&row);
        } else {
            self.folded.insert(row);
            if self.is_hidden(self.row) {
                self.row = row;
                self.col = 0;
            }
        }
        self.status = if self.folded.contains(&row) {
            "Cabecera plegada".into()
        } else {
            "Cabecera abierta".into()
        };
    }

    pub(crate) fn fold_all(&mut self) {
        self.folded.clear();
        for r in 0..self.lines.len() {
            if self.header_level(r).is_some() {
                self.folded.insert(r);
            }
        }
        self.reveal_row(self.row);
        self.status = "Todas plegadas".into();
    }

    pub(crate) fn unfold_all(&mut self) {
        self.folded.clear();
        self.status = "Todas abiertas".into();
    }

    pub(crate) fn shift_folds_insert(&mut self, at: usize) {
        let next: HashSet<usize> = self
            .folded
            .iter()
            .map(|&f| if f >= at { f + 1 } else { f })
            .collect();
        self.folded = next;
    }

    pub(crate) fn shift_folds_remove(&mut self, at: usize) {
        let next: HashSet<usize> = self
            .folded
            .iter()
            .filter_map(|&f| {
                if f == at {
                    None
                } else if f > at {
                    Some(f - 1)
                } else {
                    Some(f)
                }
            })
            .collect();
        self.folded = next;
    }

    pub(crate) fn shift_folds_range(&mut self, r1: usize, r2: usize) {
        let span = r2 - r1 + 1;
        let next: HashSet<usize> = self
            .folded
            .iter()
            .filter_map(|&f| {
                if f >= r1 && f <= r2 {
                    None
                } else if f > r2 {
                    Some(f - span)
                } else {
                    Some(f)
                }
            })
            .collect();
        self.folded = next;
    }

    pub(crate) fn sanitize_folds(&mut self) {
        let valid: HashSet<usize> = self
            .folded
            .iter()
            .copied()
            .filter(|&r| r < self.lines.len() && self.header_level(r).is_some())
            .collect();
        if valid.len() != self.folded.len() {
            self.folded = valid;
        }
    }

    /// fila de pantalla de una línea real (contando solo visibles desde top)
    pub(crate) fn screen_row_of(&self, row: usize) -> usize {
        let mut sy = 0;
        for r in self.top..row {
            if !self.is_hidden(r) {
                sy += 1;
            }
        }
        sy
    }

    // ---------- ia ----------
    pub(crate) fn ai_start(&mut self) {
        let prompt = self.ai_input.trim().to_string();
        if prompt.is_empty() || self.ai_running {
            return;
        }
        self.ai_lines.push(format!(">>> {}", prompt));
        self.ai_lines.push(String::new());
        self.ai_history.push(prompt.clone());
        self.ai_hist_idx = self.ai_history.len();
        self.ai_input.clear();
        self.ai_running = true;
        self.ai_flag.store(true, Ordering::SeqCst);
        *self.ai_out.lock().unwrap() = String::new();

        let out = Arc::clone(&self.ai_out);
        let flag = Arc::clone(&self.ai_flag);
        let child_slot = Arc::clone(&self.ai_child);
        let dir = self.work_dir.clone();
        let session = format!("pigmacs_ai_{}", self.ai_session_n);

        thread::spawn(move || {
            // pi no interactivo con sesión fija: la conversación conserva el contexto
            let mut cmd = Command::new("pi");
            cmd.arg("-p");
            cmd.arg("--session-id").arg(&session);
            cmd.arg("--no-skills");
            cmd.arg("--no-extensions");
            if let Ok(m) = env::var("PIGMACS_AI_MODEL") {
                if !m.trim().is_empty() {
                    cmd.arg("--model").arg(m.trim());
                }
            }
            cmd.arg(&prompt);
            cmd.current_dir(dir);
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    out.lock().unwrap().push_str(&format!(
                        "No se pudo lanzar pi: {} (¿está en PATH?)",
                        e
                    ));
                    flag.store(false, Ordering::SeqCst);
                    return;
                }
            };
            let stdout = child.stdout.take();
            *child_slot.lock().unwrap() = Some(child);
            let mut tmp = String::new();
            if let Some(mut reader) = stdout {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            tmp.push_str(&String::from_utf8_lossy(&buf[..n]));
                            *out.lock().unwrap() = tmp.clone();
                        }
                    }
                }
            }
            // si pi falla sin salida, mostrar su stderr como diagnóstico
            let mut stderr_text = String::new();
            if let Some(mut er) = child_slot.lock().unwrap().as_mut().and_then(|c| c.stderr.take()) {
                let mut buf = [0u8; 4096];
                loop {
                    match er.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => stderr_text.push_str(&String::from_utf8_lossy(&buf[..n])),
                    }
                }
            }
            if let Some(mut c) = child_slot.lock().unwrap().take() {
                let _ = c.wait();
            }
            if tmp.is_empty() && !stderr_text.is_empty() {
                *out.lock().unwrap() = format!("pi: {}", strip_ansi(&stderr_text));
            }
            flag.store(false, Ordering::SeqCst);
        });
        self.status = "IA: consultando…".into();
    }

    pub(crate) fn ai_flush(&mut self) {
        if !self.ai_running {
            return;
        }
        if self.ai_flag.load(Ordering::SeqCst) {
            return;
        }
        // ha terminado: mover la salida al historial
        let out = self.ai_out.lock().unwrap().clone();
        *self.ai_out.lock().unwrap() = String::new();
        for l in out.split('\n') {
            let clean: String = strip_ansi(l).replace('\r', "").trim_end().to_string();
            if !clean.is_empty() {
                self.ai_lines.push(clean);
            }
        }
        self.ai_lines.push(String::new());
        self.ai_running = false;
        self.status = format!("IA: {} caracteres", out.len());
        // bell: avisa de que terminó sin tener que mirar el panel
        self.bell = true;
        self.redraw = true;
    }

    pub(crate) fn ai_cancel(&mut self) {
        if let Some(mut c) = self.ai_child.lock().unwrap().take() {
            let _ = c.kill();
        }
        // la salida parcial pasa al historial
        let out = self.ai_out.lock().unwrap().clone();
        *self.ai_out.lock().unwrap() = String::new();
        for l in out.split('\n') {
            let clean: String = strip_ansi(l).replace('\r', "").trim_end().to_string();
            if !clean.is_empty() {
                self.ai_lines.push(clean);
            }
        }
        self.ai_lines.push(String::new());
        self.ai_running = false;
        self.status = "IA: cancelado".into();
        self.redraw = true;
    }
}

/// ¿se puede escribir en esta ruta? (para detectar ficheros de root)
pub(crate) fn file_writable(p: &std::path::Path) -> bool {
    if p.exists() {
        fs::OpenOptions::new().write(true).open(p).is_ok()
    } else {
        p.parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(|d| fs::OpenOptions::new().write(true).open(d).is_ok())
            .unwrap_or(true)
    }
}

/// reparto de filas: editor arriba, shell abajo, 2 filas fijas (ayuda + estado)
pub(crate) fn layout(h: u16, shell_rows: u16) -> (u16, u16) {
    let shell_rows = shell_rows.clamp(3, h.saturating_sub(5).max(3));
    let editor_rows = h.saturating_sub(shell_rows + 2).max(1);
    (editor_rows, shell_rows)
}
