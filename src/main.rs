use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Insert,
    Visual,
    Shell,
    Ai,
    Browser,
}

struct Shell {
    output: Arc<Mutex<Vec<u8>>>,
    writer: Box<dyn Write + Send>,
    // se conservan a propósito: mantienen el PTY abierto y al hijo vivo
    _master: Box<dyn MasterPty + Send>,
    _child: Box<dyn Child + Send>,
}

impl Shell {
    fn resize(&mut self, rows: u16, cols: u16) {
        let _ = self._master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

struct Browser {
    active: bool,
    cwd: PathBuf,
    entries: Vec<PathBuf>,
    cursor: usize,
    scroll: usize,
}

impl Browser {
    fn refresh(&mut self) {
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

    fn cd(&mut self, dir: PathBuf) {
        self.cwd = dir;
        self.cursor = 0;
        self.scroll = 0;
        self.refresh();
    }

    fn cursor_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn cursor_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }

    fn current(&self) -> Option<PathBuf> {
        self.entries.get(self.cursor).cloned()
    }

    fn go_top(&mut self) {
        self.cursor = 0;
    }

    fn go_bottom(&mut self) {
        self.cursor = self.entries.len().saturating_sub(1);
    }
}

struct App {
    path: PathBuf,
    lines: Vec<String>,
    row: usize,
    col: usize,
    top: usize,
    left: usize,
    anchor: Option<(usize, usize)>,
    mode: Mode,
    top_mode: Mode, // editor (Normal) o Ai: a dónde vuelve Tab desde el shell
    dirty: bool,
    shell: Shell,
    status: String,
    pending: Option<char>,
    search_query: String,
    searching: bool,
    matches: Vec<(usize, usize)>,
    match_idx: usize,
    // org
    folded: HashSet<usize>,
    // marcas de vim: m<x> fija · '<x> salta
    marks: HashMap<char, (usize, usize)>,
    // navegador
    browser: Browser,
    // ia
    ai_lines: Vec<String>,
    ai_input: String,
    ai_history: Vec<String>,
    ai_hist_idx: usize,
    ai_running: bool,
    ai_out: Arc<Mutex<String>>,
    ai_flag: Arc<AtomicBool>,
    ai_child: Arc<Mutex<Option<std::process::Child>>>,
    ai_session_n: usize, // nº de conversación pi (Ctrl+N la reinicia)
    ai_selecting: bool,
    ai_sel_anchor: Option<usize>,
    ai_sel_cursor: usize,
    shell_rows: u16,
    work_dir: PathBuf,
    redraw: bool, // si es false, se salta el redibujado (anti-parpadeo)
    prev: Frame, // último frame pintado (para el diff por celdas)
    editor_rows: u16, // altura actual del panel de edición (scroll al 75%)
    read_only: bool, // fichero sin permisos de escritura: Ctrl+S pedirá sudo
    sudo_prompt: bool, // pidiendo contraseña sudo
    sudo_pass: String, // contraseña (se limpia tras usarla)
    sudo_text: String, // contenido a escribir vía sudo
}

impl App {
    // ---------- texto ----------
    fn selected(&self) -> String {
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

    fn line_len(&self) -> usize {
        self.lines.get(self.row).map(|s| s.chars().count()).unwrap_or(0)
    }

    fn clamp_col(&mut self) {
        let max = self.line_len().saturating_sub(1);
        if self.col > max && self.line_len() > 0 {
            self.col = max;
        } else if self.line_len() == 0 {
            self.col = 0;
        }
    }

    fn move_cursor(&mut self, dr: isize, dc: isize) {
        if dr != 0 {
            if let Some(nr) = self.next_visible_row(self.row, dr) {
                self.row = nr;
            }
        }
        let nc = (self.col as isize + dc).max(0) as usize;
        self.col = nc.min(self.line_len().saturating_sub(1).max(0));
        self.ensure_visible();
    }

    fn ensure_hscroll(&mut self, width: usize) {
        let editor_w = width.saturating_sub(7);
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

    fn ensure_visible(&mut self) {
        // margen de scroll: el cursor llega hasta el 75% de la altura del editor
        let visible = (self.editor_rows as usize) * 3 / 4;
        if visible == 0 {
            return;
        }
        if self.row < self.top {
            self.top = self.row;
        }
        if self.row >= self.top + visible {
            self.top = self.row + 1 - visible;
        }
    }

    fn insert_char(&mut self, ch: char) {
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

    fn newline(&mut self) {
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

    fn backspace(&mut self) {
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

    fn delete_char_at_cursor(&mut self) {
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

    fn delete_line(&mut self) {
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

    fn insert_line_below(&mut self) {
        self.lines.insert(self.row + 1, String::new());
        self.shift_folds_insert(self.row + 1);
        self.row += 1;
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    fn insert_line_above(&mut self) {
        self.lines.insert(self.row, String::new());
        self.shift_folds_insert(self.row);
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    fn delete_selection(&mut self) {
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

    fn save(&mut self) -> Result<()> {
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
    fn begin_sudo_save(&mut self, text: String) {
        self.sudo_text = text;
        self.sudo_pass.clear();
        self.sudo_prompt = true;
        self.status = format!(
            "Solo lectura: introduce contraseña sudo para {}",
            self.path.display()
        );
    }

    /// escribe el fichero vía `sudo -S sh -c 'cat > "$1"'` con la contraseña por stdin
    fn sudo_save_now(&mut self) {
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

    fn send_selection(&mut self) -> Result<()> {
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

    // ---------- ia: selección del panel ----------
    /// líneas visibles del panel IA (historial + salida en curso)
    fn ai_pane_lines(&self, max_lines: usize) -> Vec<String> {
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
    fn send_ai_selection(&mut self) -> Result<()> {
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
    fn open_file(&mut self, path: PathBuf) {
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

    fn browser_enter(&mut self) {
        let Some(p) = self.browser.current() else { return };
        if p.is_dir() {
            self.browser.cd(p);
        } else {
            self.open_file(p);
        }
    }

    fn browser_up(&mut self) {
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
            }
        }
    }

    // ---------- org: cabeceras y plegado ----------
    fn header_level(&self, row: usize) -> Option<u8> {
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
    fn fold_end(&self, h: usize) -> usize {
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

    fn is_hidden(&self, row: usize) -> bool {
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

    fn next_visible_row(&self, from: usize, dir: isize) -> Option<usize> {
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

    fn first_visible(&self) -> usize {
        (0..self.lines.len())
            .find(|&r| !self.is_hidden(r))
            .unwrap_or(0)
    }

    fn last_visible(&self) -> usize {
        (0..self.lines.len())
            .rev()
            .find(|&r| !self.is_hidden(r))
            .unwrap_or(0)
    }

    fn reveal_row(&mut self, row: usize) {
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

    fn toggle_fold(&mut self, row: usize) {
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

    fn fold_all(&mut self) {
        self.folded.clear();
        for r in 0..self.lines.len() {
            if self.header_level(r).is_some() {
                self.folded.insert(r);
            }
        }
        self.reveal_row(self.row);
        self.status = "Todas plegadas".into();
    }

    fn unfold_all(&mut self) {
        self.folded.clear();
        self.status = "Todas abiertas".into();
    }

    fn shift_folds_insert(&mut self, at: usize) {
        let next: HashSet<usize> = self
            .folded
            .iter()
            .map(|&f| if f >= at { f + 1 } else { f })
            .collect();
        self.folded = next;
    }

    fn shift_folds_remove(&mut self, at: usize) {
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

    fn shift_folds_range(&mut self, r1: usize, r2: usize) {
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

    fn sanitize_folds(&mut self) {
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
    fn screen_row_of(&self, row: usize) -> usize {
        let mut sy = 0;
        for r in self.top..row {
            if !self.is_hidden(r) {
                sy += 1;
            }
        }
        sy
    }

    // ---------- ia ----------
    fn ai_start(&mut self) {
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

    fn ai_flush(&mut self) {
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
        self.redraw = true;
    }

    fn ai_cancel(&mut self) {
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

fn main() -> Result<()> {
    let path = PathBuf::from(env::args().nth(1).unwrap_or_else(|| "untitled.org".into()));
    let text = fs::read_to_string(&path).unwrap_or_default();
    let lines = if text.is_empty() {
        vec![String::new()]
    } else {
        text.lines().map(str::to_owned).collect()
    };

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let shell_rows = 9.min(rows.saturating_sub(5));
    let editor_rows = rows.saturating_sub(shell_rows + 2).max(1);
    // directorio fijo para pi: la conversación vive en una sesión por lanzamiento
    let work_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let read_only = !file_writable(&path);

    let pty = native_pty_system().openpty(PtySize {
        rows: shell_rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell_name = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut cmd = CommandBuilder::new(shell_name);
    cmd.env("TERM", env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()));

    let child = pty.slave.spawn_command(cmd)?;
    let mut reader = pty.master.try_clone_reader()?;
    let output = Arc::new(Mutex::new(Vec::new()));
    let output_reader = Arc::clone(&output);

    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => output_reader.lock().unwrap().extend_from_slice(&buf[..n]),
            }
        }
    });

    let shell = Shell {
        output,
        writer: pty.master.take_writer()?,
        _master: pty.master,
        _child: child,
    };

    let cwd = path
        .parent()
        .map(|p| p.to_path_buf())
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| work_dir.clone());

    let mut browser = Browser {
        active: true,
        cwd,
        entries: Vec::new(),
        cursor: 0,
        scroll: 0,
    };
    browser.refresh();

    let mut app = App {
        path,
        lines,
        row: 0,
        col: 0,
        top: 0,
        left: 0,
        anchor: None,
        mode: Mode::Normal,
        top_mode: Mode::Normal,
        dirty: false,
        shell,
        status: "Ctrl+B tree · i escribir · Tab plegar · Ctrl+A IA · Ctrl+E enviar · Ctrl+Shift+↑↓ borde · Ctrl+Q salir".into(),
        pending: None,
        search_query: String::new(),
        searching: false,
        matches: Vec::new(),
        match_idx: 0,
        folded: HashSet::new(),
        marks: HashMap::new(),
        browser,
        ai_lines: Vec::new(),
        ai_input: String::new(),
        ai_history: Vec::new(),
        ai_hist_idx: 0,
        ai_running: false,
        ai_out: Arc::new(Mutex::new(String::new())),
        ai_flag: Arc::new(AtomicBool::new(false)),
        ai_child: Arc::new(Mutex::new(None)),
        ai_session_n: 0,
        ai_selecting: false,
        ai_sel_anchor: None,
        ai_sel_cursor: 0,
        shell_rows,
        work_dir,
        redraw: true,
        prev: Frame::new(cols, rows),
        editor_rows,
        read_only,
        sudo_prompt: false,
        sudo_pass: String::new(),
        sudo_text: String::new(),
    };

    // abrir el fichero inicial con todos los headers plegados
    for r in 0..app.lines.len() {
        if app.header_level(r).is_some() {
            app.folded.insert(r);
        }
    }

    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let result = run(&mut app);
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    // matar la IA si quedó corriendo
    if let Some(mut c) = app.ai_child.lock().unwrap().take() {
        let _ = c.kill();
    }

    result
}

fn browser_width(w: u16) -> u16 {
    (w / 4).clamp(24, 40)
}

/// ¿se puede escribir en esta ruta? (para detectar ficheros de root)
fn file_writable(p: &std::path::Path) -> bool {
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
fn layout(h: u16, shell_rows: u16) -> (u16, u16) {
    let shell_rows = shell_rows.clamp(3, h.saturating_sub(5).max(3));
    let editor_rows = h.saturating_sub(shell_rows + 2).max(1);
    (editor_rows, shell_rows)
}

fn run(app: &mut App) -> Result<()> {
    app.redraw = true;
    let mut last_shell_len = 0usize;
    let mut last_ai_len = 0usize;
    loop {
        app.ai_flush();
        // redibujar solo si algo cambió (teclas, salida del shell, IA, resize):
        // reescribir 50 veces/segundo es lo que hacía parpadear la pantalla
        let shell_len = app.shell.output.lock().unwrap().len();
        let ai_len = if app.ai_running {
            app.ai_out.lock().unwrap().len()
        } else {
            0
        };
        if app.redraw || shell_len != last_shell_len || ai_len != last_ai_len {
            draw(app)?;
            last_shell_len = shell_len;
            last_ai_len = ai_len;
            app.redraw = false;
        }

        if event::poll(Duration::from_millis(20))? {
            let ev = event::read()?;
            app.redraw = true;
            if let Event::Resize(c, r) = ev {
                let (_, sr) = layout(r, app.shell_rows);
                app.shell_rows = sr;
                app.shell.resize(sr, c);
                continue;
            }
            if let Event::Key(k) = ev {
                // prompt de contraseña sudo: captura todas las teclas
                if app.sudo_prompt {
                    handle_sudo_key(app, k);
                    continue;
                }
                // mover el borde editor/shell: Ctrl+Shift+↑ reduce el shell (más edición)
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && k.modifiers.contains(KeyModifiers::SHIFT)
                {
                    match k.code {
                        KeyCode::Up => app.shell_rows = app.shell_rows.saturating_sub(1),
                        KeyCode::Down => app.shell_rows = app.shell_rows.saturating_add(1),
                        _ => {}
                    }
                    if matches!(k.code, KeyCode::Up | KeyCode::Down) {
                        let (c, r) = terminal::size()?;
                        app.shell_rows = app.shell_rows.clamp(3, r.saturating_sub(5).max(3));
                        app.shell.resize(app.shell_rows, c);
                        app.status = format!("Borde movido · shell {} líneas", app.shell_rows);
                        continue;
                    }
                }
                // navegación entre paneles con Shift+flechas: ← tree · →/↑ editor (o IA) · ↓ shell
                if k.modifiers.contains(KeyModifiers::SHIFT)
                    && matches!(
                        k.code,
                        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
                    )
                {
                    app.searching = false;
                    match k.code {
                        KeyCode::Left => {
                            if app.mode != Mode::Browser {
                                if app.mode != Mode::Shell {
                                    app.top_mode = app.mode;
                                }
                                app.browser.active = true;
                                app.browser.refresh();
                                app.mode = Mode::Browser;
                            }
                            continue;
                        }
                        KeyCode::Right | KeyCode::Up => {
                            app.mode = match app.top_mode {
                                Mode::Browser | Mode::Shell => Mode::Normal,
                                m => m,
                            };
                            continue;
                        }
                        KeyCode::Down => {
                            if app.mode != Mode::Shell {
                                app.top_mode = app.mode;
                                app.mode = Mode::Shell;
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                if app.searching {
                    handle_search_key(app, k);
                    continue;
                }
                // Ctrl+Tab lo traga la terminal; Shift+Tab (CSI Z, KeyCode::BackTab) llega siempre
                let is_backtab = k.code == KeyCode::BackTab;
                let toggle_tab = (k.code == KeyCode::Tab
                    && (k.modifiers.contains(KeyModifiers::CONTROL)
                        || k.modifiers.contains(KeyModifiers::SHIFT)))
                    || is_backtab;
                if k.modifiers.contains(KeyModifiers::CONTROL) || toggle_tab {
                    match k.code {
                        KeyCode::Char('q') => {
                            if app.dirty {
                                app.status = "Cambios sin guardar: Ctrl+S para guardar".into();
                                continue;
                            }
                            break;
                        }
                        KeyCode::Char('s') => {
                            app.save()?;
                            continue;
                        }
                        KeyCode::Char('a') => {
                            // alternar editor <-> IA
                            if app.mode != Mode::Shell {
                                app.top_mode = app.mode;
                            }
                            app.mode = match app.mode {
                                Mode::Ai => Mode::Normal,
                                _ => Mode::Ai,
                            };
                            continue;
                        }
                        KeyCode::Char('b') => {
                            // alternar navegador de ficheros:
                            // 1ª pulsación: visible + foco · 2ª: ocultar · Esc: foco al editor (sigue visible)
                            if app.mode == Mode::Browser {
                                app.browser.active = false;
                                app.mode = app.top_mode;
                            } else {
                                if app.mode != Mode::Shell {
                                    app.top_mode = app.mode;
                                }
                                app.browser.active = true;
                                app.browser.refresh();
                                app.mode = Mode::Browser;
                            }
                            continue;
                        }
                        KeyCode::Enter if matches!(app.mode, Mode::Normal | Mode::Visual) => {
                            app.send_selection()?;
                            continue;
                        }
                        // Ctrl+Enter es indistinguible de Enter en la mayoría de terminales;
                        // Ctrl+E (0x05) es fiable. En modo Shell pasa al shell (readline).
                        KeyCode::Char('e') if matches!(app.mode, Mode::Normal | Mode::Visual) => {
                            app.send_selection()?;
                            continue;
                        }
                        KeyCode::Char('e') if app.mode == Mode::Ai => {
                            app.send_ai_selection()?;
                            continue;
                        }
                        KeyCode::Char('n') if app.mode == Mode::Ai => {
                            app.ai_session_n += 1;
                            app.ai_lines.clear();
                            app.ai_input.clear();
                            app.ai_selecting = false;
                            app.ai_sel_anchor = None;
                            app.status = "Nueva conversación IA (nueva sesión pi)".into();
                            continue;
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            app.mode = match app.mode {
                                Mode::Shell => app.top_mode,
                                _ => {
                                    app.top_mode = app.mode;
                                    Mode::Shell
                                }
                            };
                            continue;
                        }
                        KeyCode::Char('k') if app.mode != Mode::Shell => {
                            app.shell.output.lock().unwrap().clear();
                            continue;
                        }
                        _ => {}
                    }
                }

                if app.mode == Mode::Shell {
                    send_key_to_shell(&mut app.shell.writer, k)?;
                    continue;
                }
                if app.mode == Mode::Ai {
                    handle_ai_key(app, k);
                    continue;
                }
                if app.mode == Mode::Browser {
                    handle_browser_key(app, k);
                    continue;
                }

                handle_editor_key(app, k);
            }
        }
    }
    Ok(())
}

fn handle_sudo_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            app.sudo_prompt = false;
            app.sudo_pass.clear();
            app.sudo_text.clear();
            app.status = "Guardado con sudo cancelado".into();
        }
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sudo_prompt = false;
            app.sudo_pass.clear();
            app.sudo_text.clear();
            app.status = "Guardado con sudo cancelado".into();
        }
        KeyCode::Enter => {
            app.sudo_save_now();
        }
        KeyCode::Backspace => {
            app.sudo_pass.pop();
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sudo_pass.push(c);
        }
        _ => {}
    }
}

fn handle_browser_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => app.browser.cursor_down(),
        KeyCode::Char('k') | KeyCode::Up => app.browser.cursor_up(),
        KeyCode::Char('l') | KeyCode::Enter | KeyCode::Right => app.browser_enter(),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => app.browser_up(),
        KeyCode::Char('g') => app.browser.go_top(),
        KeyCode::Char('G') => app.browser.go_bottom(),
        KeyCode::Esc => {
            app.mode = app.top_mode;
        }
        _ => {}
    }
}

fn handle_ai_key(app: &mut App, k: KeyEvent) {
    // modo selección dentro del panel IA: v entrar · j/k mover · Ctrl+E enviar · Esc salir
    if app.ai_selecting {
        match k.code {
            KeyCode::Esc | KeyCode::Char('v') => {
                app.ai_selecting = false;
                app.ai_sel_anchor = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.ai_sel_cursor = app.ai_sel_cursor.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.ai_sel_cursor = app.ai_sel_cursor.saturating_sub(1)
            }
            KeyCode::Char('g') => app.ai_sel_cursor = 0,
            KeyCode::Char('G') => app.ai_sel_cursor = usize::MAX,
            _ => {}
        }
        return;
    }
    match k.code {
        KeyCode::Esc => {
            if app.ai_running {
                app.ai_cancel();
            }
        }
        KeyCode::Enter => {
            app.ai_start();
        }
        KeyCode::Char('v') => {
            app.ai_selecting = true;
            app.ai_sel_anchor = Some(0);
            app.ai_sel_cursor = 0;
            app.status = "Selecciona con j/k, envía con Ctrl+E, sal con Esc".into();
        }
        KeyCode::Backspace => {
            app.ai_input.pop();
        }
        KeyCode::Up => {
            if app.ai_hist_idx > 0 {
                app.ai_hist_idx -= 1;
                app.ai_input = app
                    .ai_history
                    .get(app.ai_hist_idx)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        KeyCode::Down => {
            if app.ai_hist_idx + 1 < app.ai_history.len() {
                app.ai_hist_idx += 1;
                app.ai_input = app
                    .ai_history
                    .get(app.ai_hist_idx)
                    .cloned()
                    .unwrap_or_default();
            } else {
                app.ai_hist_idx = app.ai_history.len();
                app.ai_input.clear();
            }
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.ai_input.push(c);
        }
        _ => {}
    }
}

fn handle_editor_key(app: &mut App, k: KeyEvent) {
    match app.mode {
        Mode::Normal => {
            // secuencias de dos teclas (gg, dd, z + a/o/c/r/m, m<x> marca, '<x> salto)
            let fold_seq = app.pending == Some('z')
                && matches!(
                    k.code,
                    KeyCode::Char('a')
                        | KeyCode::Char('o')
                        | KeyCode::Char('c')
                        | KeyCode::Char('r')
                        | KeyCode::Char('m')
                );
            let mark_seq = matches!(app.pending, Some('m') | Some('\''))
                && matches!(k.code, KeyCode::Char(c) if c.is_ascii_lowercase());
            let repeatable = matches!(
                k.code,
                KeyCode::Char('g')
                    | KeyCode::Char('d')
                    | KeyCode::Char('z')
                    | KeyCode::Char('m')
                    | KeyCode::Char('\'')
            ) || fold_seq
                || mark_seq;
            if !repeatable {
                app.pending = None;
            }
            match k.code {
                // marcas de vim: m<x> fija la posición · '<x> salta a ella
                KeyCode::Char(c) if app.pending == Some('m') && c.is_ascii_lowercase() => {
                    app.marks.insert(c, (app.row, app.col));
                    app.pending = None;
                    app.status = format!("Marca '{}' establecida", c);
                }
                KeyCode::Char(c) if app.pending == Some('\'') && c.is_ascii_lowercase() => {
                    if let Some(&(r, cc)) = app.marks.get(&c) {
                        app.row = r;
                        app.col = cc;
                        app.reveal_row(app.row);
                        app.ensure_visible();
                        app.status = format!("Saltando a la marca '{}'", c);
                    } else {
                        app.status = format!("La marca '{}' no existe", c);
                    }
                    app.pending = None;
                }
                KeyCode::Char('h') | KeyCode::Left => app.move_cursor(0, -1),
                KeyCode::Char('j') | KeyCode::Down => app.move_cursor(1, 0),
                KeyCode::Char('k') | KeyCode::Up => app.move_cursor(-1, 0),
                KeyCode::Char('l') | KeyCode::Right => app.move_cursor(0, 1),
                KeyCode::Tab => {
                    let r = app.row;
                    app.toggle_fold(r);
                    app.pending = None;
                }
                KeyCode::Char('a')
                | KeyCode::Char('o')
                | KeyCode::Char('c')
                | KeyCode::Char('r')
                | KeyCode::Char('m')
                    if app.pending == Some('z') =>
                {
                    // comandos de plegado: za alternar · zc cerrar · zo abrir · zr todo abierto · zm todo cerrado
                    let r = app.row;
                    match k.code {
                        KeyCode::Char('a') => app.toggle_fold(r),
                        KeyCode::Char('c') => {
                            if app.header_level(r).is_some() && !app.folded.contains(&r) {
                                app.folded.insert(r);
                                app.status = "Cabecera plegada".into();
                            }
                        }
                        KeyCode::Char('o') => {
                            if app.folded.contains(&r) {
                                app.folded.remove(&r);
                                app.status = "Cabecera abierta".into();
                            }
                        }
                        KeyCode::Char('r') => app.unfold_all(),
                        KeyCode::Char('m') => app.fold_all(),
                        _ => {}
                    }
                    app.pending = None;
                }
                KeyCode::Char('g') if app.pending == Some('g') => {
                    app.row = app.first_visible();
                    app.col = 0;
                    app.pending = None;
                    app.ensure_visible();
                }
                KeyCode::Char('g') => app.pending = Some('g'),
                KeyCode::Char('G') => {
                    app.row = app.last_visible();
                    app.col = 0;
                    app.pending = None;
                    app.ensure_visible();
                }
                KeyCode::Char('0') | KeyCode::Home => {
                    app.col = 0;
                    app.ensure_visible();
                }
                KeyCode::Char('$') | KeyCode::End => {
                    app.col = app.line_len().saturating_sub(1);
                    app.ensure_visible();
                }
                KeyCode::Char('z') => app.pending = Some('z'),
                KeyCode::Char('m') => app.pending = Some('m'),
                KeyCode::Char('\'') => app.pending = Some('\''),
                KeyCode::Char('d') if app.pending == Some('d') => {
                    app.delete_line();
                    app.pending = None;
                }
                KeyCode::Char('d') => app.pending = Some('d'),
                KeyCode::Char('x') => app.delete_char_at_cursor(),
                KeyCode::Char('i') => app.mode = Mode::Insert,
                KeyCode::Char('a') => {
                    if app.col < app.line_len() {
                        app.col += 1;
                    }
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('A') => {
                    app.col = app.line_len();
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('o') => {
                    app.insert_line_below();
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('O') => {
                    app.insert_line_above();
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('n') if !app.search_query.is_empty() && !app.matches.is_empty() => {
                    app.match_idx = (app.match_idx + 1) % app.matches.len();
                    let (r, c) = app.matches[app.match_idx];
                    app.row = r;
                    app.col = c;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('N') if !app.search_query.is_empty() && !app.matches.is_empty() => {
                    app.match_idx = (app.match_idx + app.matches.len() - 1) % app.matches.len();
                    let (r, c) = app.matches[app.match_idx];
                    app.row = r;
                    app.col = c;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('/') => {
                    app.searching = true;
                    app.search_query.clear();
                    app.pending = None;
                }
                KeyCode::Char('v') => {
                    app.mode = Mode::Visual;
                    app.anchor = Some((app.row, app.col));
                    app.pending = None;
                }
                _ => {}
            }
        }
        Mode::Insert => match k.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                // al salir de insertar, el cursor vuelve al último carácter (como vim)
                if app.col > 0 {
                    app.col -= 1;
                }
            }
            KeyCode::Left => app.move_cursor(0, -1),
            KeyCode::Right => app.move_cursor(0, 1),
            KeyCode::Up => app.move_cursor(-1, 0),
            KeyCode::Down => app.move_cursor(1, 0),
            KeyCode::Home => {
                app.col = 0;
                app.ensure_visible();
            }
            KeyCode::End => {
                app.col = app.line_len();
                app.ensure_visible();
            }
            KeyCode::Enter => app.newline(),
            KeyCode::Backspace => app.backspace(),
            // Tab en inserción: plegar/desplegar la cabecera bajo el cursor (como en Normal)
            KeyCode::Tab => {
                let r = app.row;
                app.toggle_fold(r);
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                app.insert_char(c)
            }
            _ => {}
        },
        Mode::Visual => match k.code {
            KeyCode::Esc | KeyCode::Char('v') => {
                app.mode = Mode::Normal;
                app.anchor = None;
            }
            KeyCode::Char('h') | KeyCode::Left => app.move_cursor(0, -1),
            KeyCode::Char('j') | KeyCode::Down => app.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => app.move_cursor(-1, 0),
            KeyCode::Char('l') | KeyCode::Right => app.move_cursor(0, 1),
            KeyCode::Home => {
                app.col = 0;
                app.ensure_visible();
            }
            KeyCode::End => {
                app.col = app.line_len().saturating_sub(1);
                app.ensure_visible();
            }
            KeyCode::Char('x') | KeyCode::Delete => app.delete_selection(),
            _ => {}
        },
        Mode::Shell | Mode::Ai | Mode::Browser => {}
    }
}

fn handle_search_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            app.searching = false;
            app.search_query.clear();
            app.matches.clear();
        }
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.searching = false;
            app.search_query.clear();
            app.matches.clear();
        }
        KeyCode::Enter => {
            app.searching = false;
            if app.search_query.is_empty() {
                return;
            }
            app.matches = compute_matches(&app.lines, &app.search_query);
            app.match_idx = app
                .matches
                .iter()
                .position(|&(r, c)| (r, c) >= (app.row, app.col))
                .unwrap_or(0);
            if let Some(&(r, c)) = app.matches.get(app.match_idx) {
                app.row = r;
                app.col = c;
                app.reveal_row(app.row);
                app.ensure_visible();
            }
            app.status = format!("{} coincidencias", app.matches.len());
        }
        KeyCode::Backspace => {
            app.search_query.pop();
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.search_query.push(c);
        }
        _ => {}
    }
}

fn compute_matches(lines: &[String], query: &str) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    if query.is_empty() {
        return v;
    }
    for (r, line) in lines.iter().enumerate() {
        for (pos, _) in line.match_indices(query) {
            v.push((r, line[..pos].chars().count()));
        }
    }
    v
}

fn send_key_to_shell(writer: &mut Box<dyn Write + Send>, k: KeyEvent) -> Result<()> {
    let mut bytes = Vec::new();

    if k.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = k.code {
            let c = c.to_ascii_lowercase() as u8;
            if (b'a'..=b'z').contains(&c) {
                bytes.push(c - b'a' + 1);
            }
        }
    } else {
        match k.code {
            KeyCode::Enter => bytes.push(b'\r'),
            KeyCode::Backspace => bytes.push(127),
            KeyCode::Tab => bytes.push(b'\t'),
            KeyCode::Esc => bytes.push(27),
            KeyCode::Char(c) => bytes.extend_from_slice(c.to_string().as_bytes()),
            KeyCode::Up => bytes.extend_from_slice(b"\x1b[A"),
            KeyCode::Down => bytes.extend_from_slice(b"\x1b[B"),
            KeyCode::Right => bytes.extend_from_slice(b"\x1b[C"),
            KeyCode::Left => bytes.extend_from_slice(b"\x1b[D"),
            _ => {}
        }
    }

    if !bytes.is_empty() {
        writer.write_all(&bytes)?;
        writer.flush()?;
    }
    Ok(())
}

/// color de una línea org (None = por defecto)
fn org_line_style(app: &App, idx: usize) -> Option<(Color, bool)> {
    let line = app.lines.get(idx)?;
    let lvl = app.header_level(idx);
    if let Some(l) = lvl {
        let rest: String = line.chars().skip(l as usize + 1).collect();
        if rest.starts_with("TODO") {
            return Some((Color::Red, false));
        }
        if rest.starts_with("DONE") {
            return Some((Color::Green, false));
        }
        let c = match l {
            1 => Color::Cyan,
            2 => Color::Green,
            3 => Color::Yellow,
            _ => Color::Magenta,
        };
        return Some((c, l == 1));
    }
    if line.starts_with("#+BEGIN_SRC") || line.starts_with("#+END_SRC") {
        return Some((Color::Magenta, false));
    }
    if line.starts_with("#+") {
        return Some((Color::DarkYellow, false));
    }
    None
}

// ---------- frame en memoria + diff por celdas (elimina el parpadeo) ----------
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    reverse: bool,
    bold: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    st: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            st: Style::default(),
        }
    }
}

#[derive(Clone)]
struct Frame {
    w: u16,
    h: u16,
    cells: Vec<Cell>,
}

impl Frame {
    fn new(w: u16, h: u16) -> Self {
        Frame {
            w,
            h,
            cells: vec![Cell::default(); (w as usize) * (h as usize)],
        }
    }

    fn put(&mut self, x: u16, y: u16, ch: char, st: Style) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = y as usize * self.w as usize + x as usize;
        self.cells[i] = Cell { ch, st };
    }

    fn text(&mut self, x: u16, y: u16, s: &str, st: Style) {
        for (i, c) in s.chars().enumerate() {
            self.put(x + i as u16, y, c, st);
        }
    }
}

/// escribe en la terminal únicamente las celdas que cambiaron respecto al frame anterior
fn render_frame(prev: &mut Frame, cur: &Frame, out: &mut io::Stdout) -> io::Result<()> {
    if prev.w != cur.w || prev.h != cur.h {
        // cambio de tamaño: limpiar y repintar todo
        *prev = Frame::new(cur.w, cur.h);
        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    }
    let w = cur.w as usize;
    for y in 0..cur.h as usize {
        let row = y * w;
        let mut x = 0usize;
        while x < w {
            let i = row + x;
            if prev.cells[i] == cur.cells[i] {
                x += 1;
                continue;
            }
            // tramo de celdas cambiadas: un solo MoveTo y cada celda con su estilo
            queue!(out, MoveTo(x as u16, y as u16))?;
            while x < w && prev.cells[row + x] != cur.cells[row + x] {
                let c = cur.cells[row + x];
                let st = c.st;
                if let Some(fg) = st.fg {
                    queue!(out, SetForegroundColor(fg))?;
                }
                if let Some(bg) = st.bg {
                    queue!(out, SetBackgroundColor(bg))?;
                }
                if st.reverse {
                    queue!(out, SetAttribute(Attribute::Reverse))?;
                }
                if st.bold {
                    queue!(out, SetAttribute(Attribute::Bold))?;
                }
                queue!(out, Print(c.ch))?;
                if st.fg.is_some() || st.bg.is_some() || st.reverse || st.bold {
                    queue!(out, SetAttribute(Attribute::Reset))?;
                }
                x += 1;
            }
        }
    }
    *prev = cur.clone();
    Ok(())
}

/// aplica una secuencia SGR (parámetros de ESC[...m) sobre un estilo
fn parse_sgr(params: &str, mut st: Style) -> Style {
    const FD: [Color; 8] = [
        Color::Black,
        Color::DarkRed,
        Color::DarkGreen,
        Color::DarkYellow,
        Color::DarkBlue,
        Color::DarkMagenta,
        Color::DarkCyan,
        Color::Grey,
    ];
    const FB: [Color; 8] = [
        Color::Grey,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::White,
    ];
    let ps: Vec<&str> = params.split(';').collect();
    let mut k = 0usize;
    while k < ps.len() {
        let n = ps[k].parse::<u16>().unwrap_or(999);
        match n {
            0 => st = Style::default(),
            1 => st.bold = true,
            7 => st.reverse = true,
            22 => st.bold = false,
            27 => st.reverse = false,
            39 => st.fg = None,
            49 => st.bg = None,
            30..=37 => st.fg = Some(FD[(n - 30) as usize]),
            40..=47 => st.bg = Some(FD[(n - 40) as usize]),
            90..=97 => st.fg = Some(FB[(n - 90) as usize]),
            100..=107 => st.bg = Some(FB[(n - 100) as usize]),
            38 | 48 => {
                if k + 2 < ps.len() && ps[k + 1] == "5" {
                    let v = ps[k + 2].parse::<u8>().unwrap_or(7);
                    if n == 38 {
                        st.fg = Some(Color::AnsiValue(v));
                    } else {
                        st.bg = Some(Color::AnsiValue(v));
                    }
                    k += 2;
                } else if k + 4 < ps.len() && ps[k + 1] == "2" {
                    let r = ps[k + 2].parse::<u8>().unwrap_or(0);
                    let g = ps[k + 3].parse::<u8>().unwrap_or(0);
                    let b = ps[k + 4].parse::<u8>().unwrap_or(0);
                    if n == 38 {
                        st.fg = Some(Color::Rgb { r, g, b });
                    } else {
                        st.bg = Some(Color::Rgb { r, g, b });
                    }
                    k += 4;
                }
            }
            _ => {}
        }
        k += 1;
    }
    st
}

/// pinta la salida del shell en el frame interpretando \r, \b, \n y colores ANSI.
/// Un mini-emulador de terminal: sin él, los \r de readline (historial con ↑)
/// concatenaban dos comandos seguidos.
fn draw_shell_output(app: &App, fr: &mut Frame, bx: u16, y0: u16, rows: u16, area_w: u16) {
    let shell = app.shell.output.lock().unwrap();
    let raw = String::from_utf8_lossy(&shell);
    // limitar a la cola reciente (el buffer crece sin límite)
    let mut tail: &str = &raw;
    if raw.len() > 8192 {
        let start = raw.len() - 8192;
        if raw.is_char_boundary(start) {
            tail = &raw[start..];
        }
    }

    // modelo de línea virtual: \r sobrescribe desde el inicio, \b retrocede
    let mut lines: Vec<Vec<(char, Style)>> = vec![Vec::new()];
    let mut x = 0usize;
    let mut st = Style::default();
    let chars: Vec<char> = tail.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                lines.push(Vec::new());
                x = 0;
                i += 1;
            }
            '\r' => {
                x = 0;
                i += 1;
            }
            '\x08' | '\x7f' => {
                x = x.saturating_sub(1);
                i += 1;
            }
            '\x1b' => {
                if i + 1 < chars.len() && chars[i + 1] == '[' {
                    // CSI: hasta el byte final 0x40-0x7e
                    let mut j = i + 2;
                    while j < chars.len() && !('@'..='~').contains(&chars[j]) {
                        j += 1;
                    }
                    if j < chars.len() && chars[j] == 'm' {
                        let params: String = chars[i + 2..j].iter().collect();
                        st = parse_sgr(&params, st);
                    }
                    i = j + 1;
                } else if i + 1 < chars.len() && chars[i + 1] == ']' {
                    // OSC: hasta BEL o ESC \
                    let mut j = i + 2;
                    while j < chars.len() {
                        if chars[j] == '\x07' {
                            j += 1;
                            break;
                        }
                        if chars[j] == '\x1b' && j + 1 < chars.len() && chars[j + 1] == '\\' {
                            j += 2;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                } else {
                    i += 2;
                }
            }
            c if (c as u32) < 32 => i += 1,
            c => {
                let line = lines.last_mut().unwrap();
                while line.len() <= x {
                    line.push((' ', Style::default()));
                }
                line[x] = (c, st);
                x += 1;
                i += 1;
            }
        }
        if lines.len() > 2048 {
            let drop = lines.len() - 1024;
            lines.drain(..drop);
        }
    }

    let start = lines.len().saturating_sub(rows as usize);
    for (r, line) in lines[start..].iter().enumerate() {
        let y = y0 + r as u16;
        if y >= fr.h {
            break;
        }
        for (col, &(ch, st)) in line.iter().enumerate() {
            if col as u16 >= area_w {
                break;
            }
            fr.put(bx + col as u16, y, ch, st);
        }
    }
}

fn draw(app: &mut App) -> Result<()> {
    let (w, h) = terminal::size()?;
    let mut out = io::stdout();

    let (editor_rows, shell_rows) = layout(h, app.shell_rows);
    app.editor_rows = editor_rows;
    let shell_top = editor_rows + 1;
    let status_y = h.saturating_sub(1);

    app.sanitize_folds();
    if app.is_hidden(app.top) {
        while app.top < app.lines.len() && app.is_hidden(app.top) {
            app.top += 1;
        }
    }
    if app.is_hidden(app.row) {
        app.reveal_row(app.row);
    }
    app.ensure_visible();

    let bx: u16 = if app.browser.active { browser_width(w) } else { 0 };
    let area_w = w.saturating_sub(bx);
    app.ensure_hscroll(area_w as usize);

    if !app.search_query.is_empty() {
        app.matches = compute_matches(&app.lines, &app.search_query);
        if app.match_idx >= app.matches.len() {
            app.match_idx = 0;
        }
    }

    // pintar el frame en memoria (sin tocar la terminal todavía)
    let mut fr = Frame::new(w, h);

    if app.browser.active {
        draw_browser(app, bx, h, &mut fr);
    }

    if app.mode == Mode::Ai {
        draw_ai_pane(app, editor_rows, bx, area_w, &mut fr);
    } else if app.mode != Mode::Browser {
        draw_editor_lines(app, editor_rows, bx, area_w, &mut fr);
    }

    // separador del shell (solo a la derecha del navegador)
    fr.text(bx, shell_top, &"─".repeat(area_w as usize), Style::default());

    // salida del shell: últimas líneas con colores ANSI y \r/\b interpretados
    let max_rows = h.saturating_sub(4).saturating_sub(shell_top) as usize;
    let available = (shell_rows.saturating_sub(2) as usize).min(max_rows);
    draw_shell_output(app, &mut fr, bx, shell_top + 1, available as u16, area_w);

    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Visual => "VISUAL",
        Mode::Shell => "SHELL ",
        Mode::Ai => "AI    ",
        Mode::Browser => "BROWSE",
    };

    let dirty = if app.dirty { " [+]".to_string() } else { String::new() };
    // truncar a ancho de pantalla: si la barra de estado desborda, el terminal
    // hace wrap, scrollea el frame una fila y el cursor queda desplazado
    let status_text: String = if app.sudo_prompt {
        format!(
            "Contraseña sudo ({}): {}",
            app.path.display(),
            "*".repeat(app.sudo_pass.chars().count())
        )
    } else if app.searching {
        format!("/{}", app.search_query)
    } else {
        format!(" {}{}  {}", mode, dirty, app.status)
    }
    .chars()
    .take(w as usize)
    .collect();
    fr.text(0, status_y, &status_text, Style::default());

    // barra de ayuda permanente: atajos del modo activo, siempre visibles
    let help = match app.mode {
        Mode::Normal | Mode::Insert | Mode::Visual => {
            "Ctrl+B tree · i · Esc · Tab/za plegar · m marca · 'a saltar · Ctrl+A IA · Ctrl+E enviar · Ctrl+S · Ctrl+Q salir"
        }
        Mode::Browser => {
            "j/k mover · l entrar · h subir · Enter abrir · g/G extremos · Esc editor · Ctrl+B ocultar"
        }
        Mode::Ai => {
            "Enter enviar · ↑/↓ historial · v seleccionar · Ctrl+E enviar · Ctrl+N nueva · Ctrl+B tree · Esc cancelar · Tab shell"
        }
        Mode::Shell => {
            "escribe comandos · Shift+Tab editor · Ctrl+B tree · Ctrl+E enviar · Ctrl+Shift+↑↓ borde · Ctrl+Q salir"
        },
    };
    let help_text: String = help.chars().take(w as usize).collect();
    fr.text(
        0,
        status_y - 1,
        &help_text,
        Style {
            fg: Some(Color::DarkGrey),
            ..Default::default()
        },
    );

    // cursor bloque del editor: celda en vídeo inverso dentro del frame
    if !app.sudo_prompt
        && !app.searching
        && !matches!(app.mode, Mode::Ai | Mode::Browser | Mode::Shell)
    {
        let y = app.screen_row_of(app.row).min(editor_rows.saturating_sub(1) as usize) as u16;
        let x = (bx as usize + 6 + app.col.saturating_sub(app.left))
            .min(w.saturating_sub(1) as usize) as u16;
        let ch = app
            .lines
            .get(app.row)
            .and_then(|l| l.chars().nth(app.col))
            .unwrap_or(' ');
        let (fg, bold) = match org_line_style(app, app.row) {
            Some((c, b)) => (Some(c), b),
            None => (None, false),
        };
        fr.put(
            x,
            y,
            ch,
            Style {
                fg,
                bold,
                reverse: true,
                ..Default::default()
            },
        );
    }

    // diff con el frame anterior: solo se reescriben las celdas que cambiaron
    render_frame(&mut app.prev, &fr, &mut out)?;

    // cursor real del terminal según el modo
    if app.sudo_prompt || app.searching {
        let prompt_len = status_text.chars().count() as u16;
        queue!(out, Show, MoveTo(prompt_len, status_y))?;
    } else if app.mode == Mode::Ai {
        let input_row = editor_rows.saturating_sub(1);
        let x = (bx + (2 + app.ai_input.chars().count()) as u16).min(area_w.saturating_sub(1));
        queue!(out, Show, MoveTo(x, input_row))?;
    } else if app.mode == Mode::Shell {
        let cy = (shell_top + shell_rows.saturating_sub(2)).min(h.saturating_sub(3));
        queue!(out, Show, MoveTo(bx, cy))?;
    } else {
        // editor / navegador: el cursor se pinta en el frame; ocultar el real
        queue!(out, Hide, MoveTo(0, 0))?;
    }

    out.flush()?;
    Ok(())
}

fn draw_browser(app: &mut App, bx: u16, h: u16, fr: &mut Frame) {
    // cabecera con el directorio actual; pista de cómo enfocarlo si no lo está
    let dirname = app
        .browser
        .cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.browser.cwd.display().to_string());
    let title: String = if app.mode == Mode::Browser {
        format!("┌ {} ", dirname)
    } else {
        format!("┌ {} · Ctrl+B ", dirname)
    }
    .chars()
    .take(bx.saturating_sub(1) as usize)
    .collect();
    fr.text(0, 0, &title, Style { reverse: true, ..Default::default() });

    let rows = h.saturating_sub(3) as usize; // filas 1..h-3 (deja la ayuda y el estado)
    let start = app.browser.scroll.min(app.browser.entries.len());
    let end = (start + rows).min(app.browser.entries.len());
    let name_w = bx.saturating_sub(3) as usize;

    for (i, e) in app.browser.entries[start..end].iter().enumerate() {
        let y = 1 + i as u16;
        let sel = start + i == app.browser.cursor;
        let name = e
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let icon = if e.is_dir() { "▸" } else { "·" };
        let text: String = format!(" {} {}", icon, name);
        let text: String = text.chars().take(name_w).collect();
        let hidden = name.starts_with('.');
        let st = if sel {
            Style { reverse: true, ..Default::default() }
        } else if hidden {
            // ocultos: atenuados para distinguirlos
            Style { fg: Some(Color::DarkGrey), ..Default::default() }
        } else if e.is_dir() {
            Style { fg: Some(Color::DarkYellow), ..Default::default() }
        } else {
            Style { fg: Some(Color::DarkGrey), ..Default::default() }
        };
        fr.text(0, y, &text, st);
        // separador vertical
        fr.put(bx - 1, y, '│', Style::default());
    }

    // rellenar el resto de filas con el separador
    for y in (1 + (end - start) as u16)..h.saturating_sub(2) {
        fr.put(bx - 1, y, '│', Style::default());
    }

    // si el cursor se sale de la ventana, ajustar scroll (se hace en cada frame)
    if app.browser.cursor < app.browser.scroll {
        app.browser.scroll = app.browser.cursor;
    }
    if app.browser.cursor >= app.browser.scroll + rows {
        app.browser.scroll = app.browser.cursor + 1 - rows;
    }
}

fn draw_editor_lines(app: &mut App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
    let mut real = app.top;
    let mut screen_y = 0u16;
    let max_content = area_w.saturating_sub(6) as usize;
    let mut in_src = false;

    while screen_y < editor_rows && real < app.lines.len() {
        if app.is_hidden(real) {
            real += 1;
            continue;
        }
        let idx = real;
        let folded_here = app.header_level(idx).is_some() && app.folded.contains(&idx);
        let line = &app.lines[idx];

        // estado de bloque src (recorrido en orden)
        if !in_src && line.starts_with("#+BEGIN_SRC") {
            in_src = true;
        } else if in_src && line.starts_with("#+END_SRC") {
            in_src = false;
        }

        let (fg, bold) = if let Some((c, b)) = org_line_style(app, idx) {
            (Some(c), b)
        } else if in_src {
            (Some(Color::DarkGrey), false)
        } else {
            (None, false)
        };

        fr.text(bx, screen_y, &format!("{:>5} ", idx + 1), Style::default());

        let mut printed = 0usize;
        for (x, ch) in line.chars().skip(app.left).enumerate() {
            if x + 6 >= area_w as usize {
                break;
            }
            printed += 1;
            let col_abs = app.left + x;
            let selected = app.mode == Mode::Visual && is_selected(app, idx, col_abs);
            // resaltar el rango completo de cada coincidencia, no solo su inicio
            let hit = if app.search_query.is_empty() {
                false
            } else {
                let qlen = app.search_query.chars().count();
                app.matches
                    .iter()
                    .any(|&(r, c)| r == idx && col_abs >= c && col_abs < c + qlen)
            };
            let st = if hit && !selected {
                Style {
                    fg: Some(Color::Black),
                    bg: Some(Color::DarkYellow),
                    ..Default::default()
                }
            } else if selected {
                Style { fg, bold, reverse: true, ..Default::default() }
            } else {
                Style { fg, bold, ..Default::default() }
            };
            fr.put(bx + 6 + x as u16, screen_y, ch, st);
        }

        if folded_here {
            // marcador de plegado al final del texto visible
            if printed + 2 < max_content {
                fr.text(
                    bx + 6 + printed as u16,
                    screen_y,
                    " [+]",
                    Style {
                        fg: Some(Color::DarkYellow),
                        ..Default::default()
                    },
                );
            }
            real = app.fold_end(idx) + 1;
        } else {
            real += 1;
        }
        screen_y += 1;
    }
}

fn draw_ai_pane(app: &App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
    let title = if app.ai_running {
        " IA · pi · procesando…"
    } else {
        " IA · pi · escribe tu pregunta y pulsa Enter (Esc cancela)"
    };
    fr.text(
        bx,
        0,
        &title.chars().take(area_w as usize).collect::<String>(),
        Style { reverse: true, ..Default::default() },
    );

    let mut all: Vec<String> = app.ai_lines.clone();
    if app.ai_running {
        let cur = app.ai_out.lock().unwrap();
        for l in cur.split('\n') {
            if !l.is_empty() {
                all.push(l.to_string());
            }
        }
    }

    let max_lines = editor_rows.saturating_sub(3) as usize;
    let start = all.len().saturating_sub(max_lines);
    let cur = app.ai_sel_cursor.min(max_lines.saturating_sub(1));
    for (i, l) in all[start..].iter().enumerate() {
        let clean: String = strip_ansi(l).chars().take(area_w as usize).collect();
        let selected = app.ai_selecting
            && app.ai_sel_anchor.map_or(false, |a| {
                let (r1, r2) = if a <= cur { (a, cur) } else { (cur, a) };
                i >= r1 && i <= r2
            });
        let st = if selected {
            Style { reverse: true, ..Default::default() }
        } else if app.ai_selecting && i == cur {
            Style { fg: Some(Color::DarkYellow), ..Default::default() }
        } else {
            Style::default()
        };
        fr.text(bx, 1 + i as u16, &clean, st);
    }

    let input_row = editor_rows.saturating_sub(1);
    let input_text: String = format!("> {}", app.ai_input)
        .chars()
        .take(area_w.saturating_sub(1) as usize)
        .collect();
    fr.text(bx, input_row, &input_text, Style::default());
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ ... byte final 0x40-0x7e
            Some('[') => {
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... hasta BEL o ESC \\
            Some(']') => {
                let mut prev_esc = false;
                for c2 in chars.by_ref() {
                    if c2 == '\u{7}' {
                        break;
                    }
                    if prev_esc && c2 == '\\' {
                        break;
                    }
                    prev_esc = c2 == '\x1b';
                }
            }
            Some(_) | None => {}
        }
    }
    out
}

fn is_selected(app: &App, row: usize, col: usize) -> bool {
    let Some((ar, ac)) = app.anchor else { return false };
    let (br, bc) = (app.row, app.col);
    let ((r1, c1), (r2, c2)) = if (ar, ac) <= (br, bc) {
        ((ar, ac), (br, bc))
    } else {
        ((br, bc), (ar, ac))
    };

    if row < r1 || row > r2 {
        return false;
    }
    if r1 == r2 {
        return col >= c1 && col <= c2;
    }
    if row == r1 {
        return col >= c1;
    }
    if row == r2 {
        return col <= c2;
    }
    true
}
