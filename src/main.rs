use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::HashSet,
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
}

struct Shell {
    output: Arc<Mutex<Vec<u8>>>,
    writer: Box<dyn Write + Send>,
    // se conservan a propósito: mantienen el PTY abierto y al hijo vivo
    _master: Box<dyn MasterPty + Send>,
    _child: Box<dyn Child + Send>,
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
    // ia
    ai_lines: Vec<String>,
    ai_input: String,
    ai_running: bool,
    ai_out: Arc<Mutex<String>>,
    ai_flag: Arc<AtomicBool>,
    ai_child: Arc<Mutex<Option<std::process::Child>>>,
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
        self.ensure_visible(10);
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

    fn ensure_visible(&mut self, visible: usize) {
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
        self.ensure_visible(10);
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
        self.ensure_visible(10);
        self.dirty = true;
    }

    fn insert_line_below(&mut self) {
        self.lines.insert(self.row + 1, String::new());
        self.shift_folds_insert(self.row + 1);
        self.row += 1;
        self.col = 0;
        self.ensure_visible(10);
        self.dirty = true;
    }

    fn insert_line_above(&mut self) {
        self.lines.insert(self.row, String::new());
        self.shift_folds_insert(self.row);
        self.col = 0;
        self.ensure_visible(10);
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
        self.ensure_visible(10);
        self.dirty = true;
    }

    fn save(&mut self) -> Result<()> {
        let text = self.lines.join("\n");
        fs::write(&self.path, text)
            .with_context(|| format!("no se pudo guardar {}", self.path.display()))?;
        self.dirty = false;
        self.status = format!("Guardado: {}", self.path.display());
        Ok(())
    }

    fn send_selection(&mut self) -> Result<()> {
        let s = self.selected();
        if s.trim().is_empty() {
            self.status = "Selección vacía".into();
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
        self.ai_input.clear();
        self.ai_running = true;
        self.ai_flag.store(true, Ordering::SeqCst);
        *self.ai_out.lock().unwrap() = String::new();

        let out = Arc::clone(&self.ai_out);
        let flag = Arc::clone(&self.ai_flag);
        let child_slot = Arc::clone(&self.ai_child);
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        thread::spawn(move || {
            let mut cmd = Command::new("opencode");
            cmd.arg("run").arg("--pure").arg(&prompt);
            cmd.current_dir(dir);
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    out.lock().unwrap().push_str(&format!(
                        "No se pudo lanzar opencode: {} (¿está en PATH?)",
                        e
                    ));
                    flag.store(false, Ordering::SeqCst);
                    return;
                }
            };
            let stdout = child.stdout.take();
            *child_slot.lock().unwrap() = Some(child);
            if let Some(mut reader) = stdout {
                let mut buf = [0u8; 8192];
                let mut tmp = String::new();
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
            if let Some(mut c) = child_slot.lock().unwrap().take() {
                let _ = c.wait();
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
        status: "i escribir · hjkl mover · Tab plegar · z a/o/c/r/m · Ctrl+A IA · Tab shell · Ctrl+S guardar · Ctrl+Q salir".into(),
        pending: None,
        search_query: String::new(),
        searching: false,
        matches: Vec::new(),
        match_idx: 0,
        folded: HashSet::new(),
        ai_lines: Vec::new(),
        ai_input: String::new(),
        ai_running: false,
        ai_out: Arc::new(Mutex::new(String::new())),
        ai_flag: Arc::new(AtomicBool::new(false)),
        ai_child: Arc::new(Mutex::new(None)),
    };

    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let result = run(&mut app, editor_rows, shell_rows);
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    // matar la IA si quedó corriendo
    if let Some(mut c) = app.ai_child.lock().unwrap().take() {
        let _ = c.kill();
    }

    result
}

fn run(app: &mut App, editor_rows: u16, shell_rows: u16) -> Result<()> {
    loop {
        app.ai_flush();
        draw(app, editor_rows, shell_rows)?;

        if event::poll(Duration::from_millis(20))? {
            let ev = event::read()?;
            if let Event::Key(k) = ev {
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
                            app.mode = match app.mode {
                                Mode::Ai => Mode::Normal,
                                _ => Mode::Ai,
                            };
                            app.top_mode = app.mode;
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

                handle_editor_key(app, k);
            }
        }
    }
    Ok(())
}

fn handle_ai_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            if app.ai_running {
                app.ai_cancel();
            }
        }
        KeyCode::Enter => {
            app.ai_start();
        }
        KeyCode::Backspace => {
            app.ai_input.pop();
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
            // secuencias de dos teclas (gg, dd, z + a/o/c/r/m)
            let fold_seq = app.pending == Some('z')
                && matches!(
                    k.code,
                    KeyCode::Char('a')
                        | KeyCode::Char('o')
                        | KeyCode::Char('c')
                        | KeyCode::Char('r')
                        | KeyCode::Char('m')
                );
            let repeatable =
                matches!(k.code, KeyCode::Char('g') | KeyCode::Char('d') | KeyCode::Char('z'))
                    || fold_seq;
            if !repeatable {
                app.pending = None;
            }
            match k.code {
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
                    app.ensure_visible(10);
                }
                KeyCode::Char('g') => app.pending = Some('g'),
                KeyCode::Char('G') => {
                    app.row = app.last_visible();
                    app.col = 0;
                    app.pending = None;
                    app.ensure_visible(10);
                }
                KeyCode::Char('0') | KeyCode::Home => {
                    app.col = 0;
                    app.ensure_visible(10);
                }
                KeyCode::Char('$') | KeyCode::End => {
                    app.col = app.line_len().saturating_sub(1);
                    app.ensure_visible(10);
                }
                KeyCode::Char('z') => app.pending = Some('z'),
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
                    app.ensure_visible(10);
                }
                KeyCode::Char('N') if !app.search_query.is_empty() && !app.matches.is_empty() => {
                    app.match_idx = (app.match_idx + app.matches.len() - 1) % app.matches.len();
                    let (r, c) = app.matches[app.match_idx];
                    app.row = r;
                    app.col = c;
                    app.reveal_row(app.row);
                    app.ensure_visible(10);
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
                app.ensure_visible(10);
            }
            KeyCode::End => {
                app.col = app.line_len();
                app.ensure_visible(10);
            }
            KeyCode::Enter => app.newline(),
            KeyCode::Backspace => app.backspace(),
            KeyCode::Tab => app.insert_char('\t'),
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
                app.ensure_visible(10);
            }
            KeyCode::End => {
                app.col = app.line_len().saturating_sub(1);
                app.ensure_visible(10);
            }
            KeyCode::Char('x') | KeyCode::Delete => app.delete_selection(),
            _ => {}
        },
        Mode::Shell | Mode::Ai => {}
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
                app.ensure_visible(10);
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

fn draw(app: &mut App, editor_rows: u16, shell_rows: u16) -> Result<()> {
    let (w, h) = terminal::size()?;
    let mut out = io::stdout();

    let editor_rows = editor_rows.min(h.saturating_sub(3));
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
    app.ensure_visible(editor_rows.saturating_sub(1) as usize);
    app.ensure_hscroll(w as usize);

    if !app.search_query.is_empty() {
        app.matches = compute_matches(&app.lines, &app.search_query);
        if app.match_idx >= app.matches.len() {
            app.match_idx = 0;
        }
    }

    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;

    if app.mode == Mode::Ai {
        draw_ai_pane(app, editor_rows, w, &mut out)?;
    } else {
        draw_editor_lines(app, editor_rows, w, &mut out)?;
    }

    queue!(out, MoveTo(0, shell_top), Print("─".repeat(w as usize)))?;

    let shell = app.shell.output.lock().unwrap();
    let raw = String::from_utf8_lossy(&shell);
    let available = shell_rows.saturating_sub(1) as usize;
    let slines: Vec<&str> = raw.lines().collect();
    let start = slines.len().saturating_sub(available);

    for (i, line) in slines[start..].iter().enumerate() {
        let clean: String = strip_ansi(line)
            .replace('\r', "")
            .chars()
            .take(w as usize)
            .collect();
        queue!(
            out,
            MoveTo(0, shell_top + 1 + i as u16),
            Print(clean),
            Clear(ClearType::UntilNewLine)
        )?;
    }

    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Visual => "VISUAL",
        Mode::Shell => "SHELL ",
        Mode::Ai => "AI    ",
    };

    let dirty = if app.dirty { " [+]" } else { "" };
    // truncar a ancho de pantalla: si la barra de estado desborda, el terminal
    // hace wrap, scrollea el frame una fila y el cursor queda desplazado
    let status_text: String = format!(" {}{}  {}", mode, dirty, app.status)
        .chars()
        .take(w as usize)
        .collect();
    queue!(
        out,
        MoveTo(0, status_y),
        Clear(ClearType::CurrentLine),
        Print(status_text)
    )?;

    if app.searching {
        let prompt: String = format!("/{}", app.search_query)
            .chars()
            .take(w.saturating_sub(1) as usize)
            .collect();
        let prompt_len = prompt.chars().count() as u16;
        queue!(
            out,
            MoveTo(0, status_y),
            Clear(ClearType::CurrentLine),
            Print(prompt),
            Show,
            MoveTo(prompt_len, status_y)
        )?;
    } else if app.mode == Mode::Ai {
        // el cursor de la IA ya se dibujó en draw_ai_pane
    } else if app.mode != Mode::Shell {
        // cursor bloque estilo vim: el carácter bajo el cursor en vídeo inverso
        let y = app.screen_row_of(app.row).min(editor_rows.saturating_sub(1) as usize) as u16;
        let x = (6 + app.col.saturating_sub(app.left)).min(w.saturating_sub(1) as usize) as u16;
        let ch = app
            .lines
            .get(app.row)
            .and_then(|l| l.chars().nth(app.col))
            .unwrap_or(' ');
        queue!(
            out,
            MoveTo(x, y),
            Hide,
            SetAttribute(Attribute::Reverse),
            Print(ch),
            SetAttribute(Attribute::Reset),
            MoveTo(x, y)
        )?;
    } else {
        // en el shell se muestra el cursor real en la línea de entrada
        queue!(out, Show, MoveTo(0, shell_top + shell_rows.saturating_sub(1)))?;
    }

    out.flush()?;
    Ok(())
}

fn draw_editor_lines(app: &mut App, editor_rows: u16, w: u16, out: &mut io::Stdout) -> Result<()> {
    let mut real = app.top;
    let mut screen_y = 0u16;
    let max_content = w.saturating_sub(6) as usize;

    while screen_y < editor_rows && real < app.lines.len() {
        if app.is_hidden(real) {
            real += 1;
            continue;
        }
        let idx = real;
        let folded_here = app.header_level(idx).is_some() && app.folded.contains(&idx);
        let line = &app.lines[idx];
        queue!(out, MoveTo(0, screen_y), Print(format!("{:>5} ", idx + 1)))?;

        let mut printed = 0usize;
        for (x, ch) in line.chars().skip(app.left).enumerate() {
            if x + 6 >= w as usize {
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
            if hit && !selected {
                queue!(
                    out,
                    SetBackgroundColor(Color::DarkYellow),
                    SetForegroundColor(Color::Black)
                )?;
            }
            if selected {
                queue!(out, SetAttribute(Attribute::Reverse))?;
            }
            queue!(out, Print(ch))?;
            if selected || hit {
                queue!(out, SetAttribute(Attribute::Reset))?;
            }
        }

        if folded_here {
            // marcador de plegado al final del texto visible
            if printed + 2 < max_content {
                queue!(
                    out,
                    MoveTo((6 + printed) as u16, screen_y),
                    SetForegroundColor(Color::DarkYellow),
                    Print(" [+]"),
                    SetAttribute(Attribute::Reset)
                )?;
            }
            real = app.fold_end(idx) + 1;
        } else {
            real += 1;
        }
        screen_y += 1;
    }
    Ok(())
}

fn draw_ai_pane(app: &mut App, editor_rows: u16, w: u16, out: &mut io::Stdout) -> Result<()> {
    let title = if app.ai_running {
        " IA · opencode · procesando…"
    } else {
        " IA · opencode · escribe tu pregunta y pulsa Enter (Esc cancela)"
    };
    queue!(
        out,
        MoveTo(0, 0),
        Clear(ClearType::UntilNewLine),
        SetAttribute(Attribute::Reverse),
        Print(title.chars().take(w as usize).collect::<String>()),
        SetAttribute(Attribute::Reset)
    )?;

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
    for (i, l) in all[start..].iter().enumerate() {
        let clean: String = strip_ansi(l).chars().take(w as usize).collect();
        queue!(
            out,
            MoveTo(0, 1 + i as u16),
            Clear(ClearType::UntilNewLine),
            Print(clean)
        )?;
    }

    let input_row = editor_rows.saturating_sub(1);
    let input_text: String = format!("> {}", app.ai_input)
        .chars()
        .take(w.saturating_sub(1) as usize)
        .collect();
    queue!(
        out,
        MoveTo(0, input_row),
        Clear(ClearType::UntilNewLine),
        Print(input_text)
    )?;
    let x = ((2 + app.ai_input.chars().count()) as u16).min(w.saturating_sub(1));
    queue!(out, Show, MoveTo(x, input_row))?;
    Ok(())
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
