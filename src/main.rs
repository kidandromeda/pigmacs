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
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Insert,
    Visual,
    Shell,
    Ai,
    Browser,
    Program, // programa lanzado desde el menú (Ctrl+P), con su propio PTY
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

// ————— programa lanzado en la ventana principal (menú Ctrl+P) —————

/// programas disponibles en el menú (se comprueba el PATH al dibujarlo)
const MENU: [&str; 8] = [
    "htop", "btop", "glances", "ncdu", "ranger", "gitui", "tmux", "less",
];

/// ¿está el programa en el PATH y es ejecutable?
fn in_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(path) = env::var("PATH") else { return false };
    path.split(':').any(|d| {
        !d.is_empty()
            && fs::metadata(std::path::Path::new(d).join(name))
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    })
}

/// estado del parser del emulador VT100
#[derive(PartialEq)]
enum PState {
    Normal,
    Esc,
    Csi,
    Osc,
    OscEsc,
    Dcs,
    DcsEsc,
    Skip, // ESC ( ) * + # o ESC SP: se traga el byte siguiente
}

/// emulador VT100 para el programa de la ventana principal: suficiente para
/// TUIs (htop, ranger…) — CSI/OSC/SGR, región de scroll, pantalla alterna,
/// wrap diferido y respuestas DSR/DA a lo que el programa pregunte
struct Term {
    w: u16,
    h: u16,
    grid: Vec<Vec<Cell>>, // todas las filas con el ancho completo
    x: usize,
    y: usize,
    top: usize, // región de scroll (inclusive)
    bot: usize,
    st: Style,         // estilo SGR activo
    wrap: bool,        // auto-wrap (DEC, activo por defecto)
    pending_wrap: bool, // wrap diferido como en xterm (xenl)
    app_cursor: bool,  // DECCKM: flechas en modo aplicación (SS3)
    cursor_visible: bool,
    saved: Option<(usize, usize, Style)>,
    alt: Option<(Vec<Vec<Cell>>, usize, usize, Style)>,
    last_ch: char,    // para REP (ESC[b)
    reply: Vec<u8>,   // respuestas (DSR/DA) pendientes de escribir al PTY
    buf: Vec<u8>,     // bytes recibidos aún sin procesar (secuencia a medias)
    state: PState,
    csi: String,
}

impl Term {
    fn new(w: u16, h: u16) -> Self {
        Term {
            w,
            h,
            grid: vec![vec![Cell::default(); w as usize]; h as usize],
            x: 0,
            y: 0,
            top: 0,
            bot: h.saturating_sub(1) as usize,
            st: Style::default(),
            wrap: true,
            pending_wrap: false,
            app_cursor: false,
            cursor_visible: true,
            saved: None,
            alt: None,
            last_ch: ' ',
            reply: Vec::new(),
            buf: Vec::new(),
            state: PState::Normal,
            csi: String::new(),
        }
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.w = w;
        self.h = h;
        self.grid = vec![vec![Cell::default(); w as usize]; h as usize];
        self.x = self.x.min(w.saturating_sub(1) as usize);
        self.y = self.y.min(h.saturating_sub(1) as usize);
        self.top = 0;
        self.bot = h.saturating_sub(1) as usize;
        self.pending_wrap = false;
        self.alt = None; // el programa repinta solo tras el SIGWINCH
    }

    /// añade bytes al búfer interno y procesa todo lo que se pueda
    fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        self.parse();
    }

    fn parse(&mut self) {
        let len = self.buf.len();
        let mut i = 0usize;
        while i < len {
            let b = self.buf[i];
            match self.state {
                PState::Normal => {
                    if b == 0x1b {
                        self.state = PState::Esc;
                        i += 1;
                    } else if b < 0x20 {
                        self.ctrl(b);
                        i += 1;
                    } else if b < 0x80 {
                        self.put_char(b as char);
                        i += 1;
                    } else {
                        // UTF-8 multibyte (si el trozo está corto, se espera)
                        let need = if b >= 0xf0 {
                            4
                        } else if b >= 0xe0 {
                            3
                        } else if b >= 0xc2 {
                            2
                        } else {
                            1
                        };
                        if i + need > len {
                            break;
                        }
                        let ch = std::str::from_utf8(&self.buf[i..i + need])
                            .ok()
                            .and_then(|s| s.chars().next());
                        match ch {
                            Some(c) => {
                                self.put_char(c);
                                i += need;
                            }
                            None => {
                                self.put_char('\u{fffd}');
                                i += 1;
                            }
                        }
                    }
                }
                PState::Esc => {
                    self.pending_wrap = false;
                    match b {
                        b'[' => {
                            self.state = PState::Csi;
                            self.csi.clear();
                        }
                        b']' => self.state = PState::Osc,
                        b'P' => self.state = PState::Dcs,
                        b'7' => {
                            self.save_cursor();
                            self.state = PState::Normal;
                        }
                        b'8' => {
                            self.restore_cursor();
                            self.state = PState::Normal;
                        }
                        b'D' => {
                            self.line_feed();
                            self.state = PState::Normal;
                        }
                        b'E' => {
                            self.x = 0;
                            self.line_feed();
                            self.state = PState::Normal;
                        }
                        b'M' => {
                            self.reverse_index();
                            self.state = PState::Normal;
                        }
                        b'c' => {
                            self.reset();
                            self.state = PState::Normal;
                        }
                        b'(' | b')' | b'*' | b'+' | b'#' | b' ' => {
                            self.state = PState::Skip;
                        }
                        0x1b => {} // ESC ESC: mantenerse en Esc
                        _ => self.state = PState::Normal,
                    }
                    i += 1;
                }
                PState::Skip => {
                    self.state = PState::Normal;
                    i += 1;
                }
                PState::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        let params = std::mem::take(&mut self.csi);
                        self.state = PState::Normal;
                        self.csi_final(&params, b as char);
                    } else if self.csi.len() < 64 {
                        self.csi.push(b as char);
                    } else {
                        // secuencia de basura: abandonar
                        self.state = PState::Normal;
                        self.csi.clear();
                    }
                    i += 1;
                }
                PState::Osc => {
                    match b {
                        0x07 => self.state = PState::Normal,
                        0x1b => self.state = PState::OscEsc,
                        _ => {} // título de ventana: se ignora
                    }
                    i += 1;
                }
                PState::OscEsc => {
                    self.state = PState::Normal;
                    i += 1;
                }
                PState::Dcs => {
                    if b == 0x1b {
                        self.state = PState::DcsEsc;
                    }
                    i += 1;
                }
                PState::DcsEsc => {
                    self.state = if b == b'\\' {
                        PState::Normal
                    } else if b == 0x1b {
                        PState::DcsEsc
                    } else {
                        PState::Dcs
                    };
                    i += 1;
                }
            }
        }
        // queda un remanente sin terminar: conservarlo para el próximo feed
        self.buf.drain(..i);
    }

    /// bytes de control imprimibles-directos
    fn ctrl(&mut self, b: u8) {
        match b {
            b'\r' => {
                self.x = 0;
                self.pending_wrap = false;
            }
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            0x08 => {
                self.x = self.x.saturating_sub(1);
                self.pending_wrap = false;
            }
            b'\t' => {
                let nx = ((self.x / 8) + 1) * 8;
                self.x = nx.min(self.w.saturating_sub(1) as usize);
                self.pending_wrap = false;
            }
            _ => {} // BEL y demás: sin efecto
        }
    }

    fn line_feed(&mut self) {
        self.pending_wrap = false;
        if self.y == self.bot {
            self.scroll_up(1);
        } else if self.y + 1 < self.h as usize {
            self.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.y == self.top {
            self.scroll_down(1);
        } else {
            self.y = self.y.saturating_sub(1);
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let region = (self.bot - self.top + 1).min(self.h as usize);
        for _ in 0..n.min(region) {
            self.grid.remove(self.top);
            self.grid
                .insert(self.bot, vec![Cell::default(); self.w as usize]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let region = (self.bot - self.top + 1).min(self.h as usize);
        for _ in 0..n.min(region) {
            self.grid.remove(self.bot);
            self.grid
                .insert(self.top, vec![Cell::default(); self.w as usize]);
        }
    }

    fn put_char(&mut self, c: char) {
        if self.pending_wrap {
            // wrap diferido: el salto ocurre antes del siguiente carácter
            self.pending_wrap = false;
            self.x = 0;
            self.line_feed();
        }
        if self.y >= self.grid.len() || self.x >= self.w as usize {
            return;
        }
        self.grid[self.y][self.x] = Cell { ch: c, st: self.st };
        self.last_ch = c;
        if self.x + 1 < self.w as usize {
            self.x += 1;
        } else if self.wrap {
            self.pending_wrap = true;
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Some((self.x, self.y, self.st));
    }

    fn restore_cursor(&mut self) {
        if let Some((x, y, st)) = self.saved {
            self.x = x.min(self.w.saturating_sub(1) as usize);
            self.y = y.min(self.h.saturating_sub(1) as usize);
            self.st = st;
        }
        self.pending_wrap = false;
    }

    fn reset(&mut self) {
        self.grid = vec![vec![Cell::default(); self.w as usize]; self.h as usize];
        self.x = 0;
        self.y = 0;
        self.top = 0;
        self.bot = self.h.saturating_sub(1) as usize;
        self.st = Style::default();
        self.wrap = true;
        self.pending_wrap = false;
        self.app_cursor = false;
        self.cursor_visible = true;
        self.saved = None;
        self.alt = None;
    }

    fn alt_screen(&mut self, on: bool) {
        if on && self.alt.is_none() {
            let grid = std::mem::replace(
                &mut self.grid,
                vec![vec![Cell::default(); self.w as usize]; self.h as usize],
            );
            self.alt = Some((grid, self.x, self.y, self.st));
            self.x = 0;
            self.y = 0;
        } else if !on {
            if let Some((grid, x, y, st)) = self.alt.take() {
                self.grid = grid;
                self.x = x.min(self.w.saturating_sub(1) as usize);
                self.y = y.min(self.h.saturating_sub(1) as usize);
                self.st = st;
            }
        }
        self.pending_wrap = false;
    }

    fn set_mode(&mut self, n: usize, on: bool) {
        match n {
            1 => self.app_cursor = on,
            7 => self.wrap = on,
            25 => self.cursor_visible = on,
            47 | 1047 | 1049 => self.alt_screen(on),
            _ => {} // bracketed paste y demás: se ignoran
        }
    }

    fn erase_display(&mut self, n: usize) {
        match n {
            0 => {
                if let Some(l) = self.grid.get_mut(self.y) {
                    if self.x < l.len() {
                        for c in &mut l[self.x..] {
                            *c = Cell::default();
                        }
                    }
                }
                for l in self.grid.iter_mut().skip(self.y + 1) {
                    for c in l.iter_mut() {
                        *c = Cell::default();
                    }
                }
            }
            1 => {
                for l in self.grid.iter_mut().take(self.y) {
                    for c in l.iter_mut() {
                        *c = Cell::default();
                    }
                }
                if let Some(l) = self.grid.get_mut(self.y) {
                    for c in l.iter_mut().take(self.x + 1) {
                        *c = Cell::default();
                    }
                }
            }
            _ => {
                for l in &mut self.grid {
                    for c in l.iter_mut() {
                        *c = Cell::default();
                    }
                }
            }
        }
        self.pending_wrap = false;
    }

    fn erase_line(&mut self, n: usize) {
        if let Some(l) = self.grid.get_mut(self.y) {
            match n {
                0 => {
                    if self.x < l.len() {
                        for c in &mut l[self.x..] {
                            *c = Cell::default();
                        }
                    }
                }
                1 => {
                    for c in l.iter_mut().take(self.x + 1) {
                        *c = Cell::default();
                    }
                }
                _ => {
                    for c in l.iter_mut() {
                        *c = Cell::default();
                    }
                }
            }
        }
        self.pending_wrap = false;
    }

    fn insert_lines(&mut self, n: usize) {
        if self.y < self.top || self.y > self.bot {
            return;
        }
        let n = n.min(self.bot - self.top + 1);
        for _ in 0..n {
            self.grid.remove(self.bot);
            self.grid
                .insert(self.y, vec![Cell::default(); self.w as usize]);
        }
        self.pending_wrap = false;
    }

    fn delete_lines(&mut self, n: usize) {
        if self.y < self.top || self.y > self.bot {
            return;
        }
        let n = n.min(self.bot - self.top + 1);
        for _ in 0..n {
            self.grid.remove(self.y);
            self.grid
                .insert(self.bot, vec![Cell::default(); self.w as usize]);
        }
        self.pending_wrap = false;
    }

    fn delete_chars(&mut self, n: usize) {
        let w = self.w as usize;
        let n = n.min(w.saturating_sub(self.x));
        let end = self.x + n;
        if let Some(l) = self.grid.get_mut(self.y) {
            if self.x < l.len() {
                l.drain(self.x..end.min(l.len()));
            }
            while l.len() < w {
                l.push(Cell::default());
            }
        }
        self.pending_wrap = false;
    }

    fn insert_chars(&mut self, n: usize) {
        let w = self.w as usize;
        let n = n.min(w.saturating_sub(self.x));
        if let Some(l) = self.grid.get_mut(self.y) {
            for _ in 0..n {
                l.insert(self.x, Cell::default());
                l.pop();
            }
        }
        self.pending_wrap = false;
    }

    fn erase_chars(&mut self, n: usize) {
        let end = self.x + n;
        if let Some(l) = self.grid.get_mut(self.y) {
            let lim = end.min(l.len());
            if self.x < lim {
                for c in &mut l[self.x..lim] {
                    *c = Cell::default();
                }
            }
        }
        self.pending_wrap = false;
    }

    /// dispatch de la secuencia CSI final (params = texto entre ESC[ y el final)
    fn csi_final(&mut self, params: &str, fin: char) {
        // una secuencia de posición/obborrado cancela el wrap pendiente
        if !matches!(fin, 'm' | 'n' | 'c' | 't' | 'b' | 'h' | 'l' | 'q' | 'g') {
            self.pending_wrap = false;
        }
        let is_priv = params.starts_with('?');
        let ps: Vec<usize> = params
            .trim_start_matches(['?', '>', '<', '!', ' '])
            .split(';')
            .filter_map(|s| s.parse().ok())
            .collect();
        // por defecto: 1 para A/B/C… («uno»), 0 para J/K («desde aquí»)
        let n1 = ps.first().copied().unwrap_or(1);
        let n0 = ps.first().copied().unwrap_or(0);
        let max_x = self.w.saturating_sub(1) as usize;
        let max_y = self.h.saturating_sub(1) as usize;
        match fin {
            'm' => {
                self.st = if params.is_empty() {
                    Style::default()
                } else {
                    parse_sgr(params, self.st)
                };
            }
            'A' => self.y = self.y.saturating_sub(n1).max(self.top),
            'B' => self.y = (self.y + n1).min(self.bot),
            'C' => self.x = (self.x + n1).min(max_x),
            'D' => self.x = self.x.saturating_sub(n1),
            'E' => {
                self.x = 0;
                self.y = (self.y + n1).min(self.bot);
            }
            'F' => {
                self.x = 0;
                self.y = self.y.saturating_sub(n1).max(self.top);
            }
            'G' | '`' => self.x = (n1.max(1) - 1).min(max_x),
            'H' | 'f' => {
                let row = ps.first().copied().unwrap_or(1).max(1) - 1;
                let col = ps.get(1).copied().unwrap_or(1).max(1) - 1;
                self.y = row.min(max_y);
                self.x = col.min(max_x);
            }
            'd' => self.y = (n1.max(1) - 1).min(max_y),
            'J' => self.erase_display(n0),
            'K' => self.erase_line(n0),
            'L' => self.insert_lines(n1),
            'M' => self.delete_lines(n1),
            'P' => self.delete_chars(n1),
            '@' => self.insert_chars(n1),
            'X' => self.erase_chars(n1),
            'S' => self.scroll_up(n1),
            'T' => self.scroll_down(n1),
            'r' => {
                let top = ps.first().copied().unwrap_or(1).max(1) - 1;
                let bot = ps
                    .get(1)
                    .copied()
                    .unwrap_or(self.h as usize)
                    .max(1)
                    .min(self.h as usize)
                    - 1;
                self.top = top.min(bot);
                self.bot = bot;
                self.x = 0;
                self.y = 0;
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'h' if is_priv => self.set_mode(n0, true),
            'l' if is_priv => self.set_mode(n0, false),
            'n' if !is_priv => {
                if n0 == 6 {
                    let x = self.x + 1;
                    let y = self.y + 1;
                    self.reply.extend_from_slice(format!("\x1b[{y};{x}R").as_bytes());
                } else if n0 == 5 {
                    self.reply.extend_from_slice(b"\x1b[0n");
                }
            }
            'c' => {
                if params.starts_with('>') {
                    self.reply.extend_from_slice(b"\x1b[>0;10;1c");
                } else {
                    self.reply.extend_from_slice(b"\x1b[?6c");
                }
            }
            't' if !is_priv && n0 == 18 => {
                let w = self.w;
                let h = self.h;
                self.reply.extend_from_slice(format!("\x1b[8;{h};{w}t").as_bytes());
            }
            'b' => {
                // REP: repetir el último carácter
                let c = self.last_ch;
                for _ in 0..n1 {
                    self.put_char(c);
                }
            }
            'Z' => {
                self.x = if self.x % 8 == 0 {
                    self.x.saturating_sub(8)
                } else {
                    (self.x / 8) * 8
                };
            }
            _ => {} // cursor style, tabs…: sin efecto
        }
    }

    /// pinta la rejilla en el frame
    fn draw(&self, fr: &mut Frame, bx: u16, rows: u16) {
        for y in 0..(self.h as usize).min(rows as usize) {
            for x in 0..self.w as usize {
                let c = self.grid[y][x];
                fr.put(bx + x as u16, y as u16, c.ch, c.st);
            }
        }
    }
}

/// programa en ejecución sobre su propio PTY, dibujado en el área del editor
struct Program {
    name: String,
    writer: Box<dyn Write + Send>,
    // se conservan a propósito: mantienen el PTY abierto y al hijo vivo
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    term: Term,
}

impl Program {
    fn resize(&mut self, rows: u16, cols: u16) {
        if self.term.w == cols && self.term.h == rows {
            return;
        }
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.term.resize(cols, rows);
    }

    /// tecla pulsada → bytes hacia el programa (respeta DECCKM/modo app)
    fn send_key(&mut self, k: KeyEvent) -> Result<()> {
        let b = key_bytes_mode(k, self.term.app_cursor);
        if !b.is_empty() {
            self.writer.write_all(&b)?;
            self.writer.flush()?;
        }
        Ok(())
    }

    /// `Some(descripción)` si el programa ya terminó
    fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(st)) => Some(format!("{st:?}")),
            _ => None,
        }
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
    // deshacer/rehacer
    undo: Vec<(Vec<String>, usize, usize)>,
    redo: Vec<(Vec<String>, usize, usize)>,
    undo_kind: Option<&'static str>,
    // contador numérico (12G) y números de línea
    count: Option<usize>,
    show_numbers: bool,
    // marcadores de directorio del tree
    bookmarks: Vec<PathBuf>,
    // ventana de ayuda
    help_open: bool,
    help_scroll: usize,
    // programas lanzados desde el menú (Ctrl+P)
    program: Option<Program>,
    menu_open: bool,
    menu_cursor: usize,
    // consola de texto real (/dev/ttyN): saneado de avisos del kernel
    vt_mode: bool,
    force_full: bool, // repintar todas las celdas en el próximo draw
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

    fn newline(&mut self) {
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

    fn backspace(&mut self) {
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

    fn delete_char_at_cursor(&mut self) {
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

    fn delete_line(&mut self) {
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

    fn insert_line_below(&mut self) {
        self.push_undo("line");
        self.lines.insert(self.row + 1, String::new());
        self.shift_folds_insert(self.row + 1);
        self.row += 1;
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    fn insert_line_above(&mut self) {
        self.push_undo("line");
        self.lines.insert(self.row, String::new());
        self.shift_folds_insert(self.row);
        self.col = 0;
        self.ensure_visible();
        self.dirty = true;
    }

    fn delete_selection(&mut self) {
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

    // ---------- deshacer / rehacer ----------
    fn push_undo(&mut self, kind: &'static str) {
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

    fn undo_edit(&mut self) {
        if let Some((lines, row, col)) = self.undo.pop() {
            self.redo.push((self.lines.clone(), self.row, self.col));
            self.restore_state(lines, row, col);
            self.status = "Deshecho".into();
        } else {
            self.status = "Nada que deshacer".into();
        }
        self.undo_kind = None;
    }

    fn redo_edit(&mut self) {
        if let Some((lines, row, col)) = self.redo.pop() {
            self.undo.push((self.lines.clone(), self.row, self.col));
            self.restore_state(lines, row, col);
            self.status = "Rehecho".into();
        } else {
            self.status = "Nada que rehacer".into();
        }
        self.undo_kind = None;
    }

    fn restore_state(&mut self, lines: Vec<String>, row: usize, col: usize) {
        self.lines = lines;
        self.row = row.min(self.lines.len().saturating_sub(1));
        self.col = col;
        self.clamp_col();
        self.ensure_visible();
        self.dirty = true;
    }

    /// ancho del margen izquierdo del editor (números de línea o un espacio)
    fn gutter(&self) -> usize {
        if self.show_numbers {
            6
        } else {
            1
        }
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

    let vt_mode = running_on_vt();
    let printk_restore = if vt_mode { lower_kernel_printk() } else { None };

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
        status: "Ctrl+F tree · Ctrl+T terminal · i escribir · Ctrl+A IA · Ctrl+E enviar · F1 ayuda · Ctrl+Q salir".into(),
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
        undo: Vec::new(),
        redo: Vec::new(),
        undo_kind: None,
        count: None,
        show_numbers: true,
        bookmarks: Vec::new(),
        help_open: false,
        help_scroll: 0,
        program: None,
        menu_open: false,
        menu_cursor: 0,
        vt_mode,
        force_full: false,
    };

    // abrir el fichero inicial con todos los headers plegados
    for r in 0..app.lines.len() {
        if app.header_level(r).is_some() {
            app.folded.insert(r);
        }
    }

    // aviso del kernel en consolas de texto: si no se puede bajar el nivel
    // de consola, se repinta la pantalla cada 2 s (run()) para borrarlos
    if vt_mode && printk_restore.is_none() {
        app.status =
            "kernel.printk sin permisos · «sudo dmesg -n 1» evita los avisos del kernel".into();
    }

    terminal::enable_raw_mode()?;
    // limpiar la terminal antes de arrancar: en una consola de texto (ttyN)
    // lo que hubiera en pantalla se quedaba encima de pigmacs (en las
    // terminales con pantalla alterna no se nota, pero tampoco hace daño)
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        Hide,
        MoveTo(0, 0),
        Clear(ClearType::All)
    )?;
    let result = run(&mut app);
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    // restaurar el nivel de consola del kernel que había antes
    if let Some(orig) = printk_restore {
        let _ = fs::write("/proc/sys/kernel/printk", format!("{orig}\n"));
    }

    // matar la IA si quedó corriendo
    if let Some(mut c) = app.ai_child.lock().unwrap().take() {
        let _ = c.kill();
    }

    result
}

fn browser_width(w: u16) -> u16 {
    (w / 4).clamp(24, 40)
}

/// ¿estamos en una consola de texto real (/dev/tty1…) y no en pts (tmux,
/// xterm, ssh)? El printk del kernel sólo se pinta en la consola de texto.
fn running_on_vt() -> bool {
    for fd in ["0", "1", "2"] {
        if let Ok(t) = fs::read_link(format!("/proc/self/fd/{fd}")) {
            let s = t.to_string_lossy();
            if let Some(rest) = s.strip_prefix("/dev/tty") {
                if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                    return true;
                }
            }
        }
    }
    false
}

/// baja el nivel de mensajes que el kernel pinta en la consola de texto
/// (sólo hace falta permiso; como usuario normal falla y se ignora).
/// Devuelve el valor original para restaurarlo al salir. Los mensajes
/// siguen quedando en el búfer: sólo dejan de pintarse en pantalla.
fn lower_kernel_printk() -> Option<String> {
    let orig = fs::read_to_string("/proc/sys/kernel/printk").ok()?;
    let first = orig.split_whitespace().next()?.to_string();
    fs::write("/proc/sys/kernel/printk", "1\n").ok()?;
    Some(first)
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
    let mut last_heal = Instant::now();
    loop {
        app.ai_flush();
        // programa de la ventana principal: volcar su salida al emulador,
        // responder a lo que pregunte (DSR/DA) y detectar si ha terminado
        if let Some(p) = app.program.as_mut() {
            let chunk = {
                let mut o = p.output.lock().unwrap();
                if o.is_empty() {
                    None
                } else {
                    Some(std::mem::take(&mut *o))
                }
            };
            if let Some(v) = chunk {
                p.term.feed(&v);
                app.redraw = true;
            }
            if !p.term.reply.is_empty() {
                let r = std::mem::take(&mut p.term.reply);
                let _ = p.writer.write_all(&r);
                let _ = p.writer.flush();
            }
            if let Some(st) = p.exited() {
                let name = p.name.clone();
                app.program = None;
                if app.mode == Mode::Program {
                    app.mode = Mode::Normal;
                }
                if app.top_mode == Mode::Program {
                    app.top_mode = Mode::Normal;
                }
                app.status = format!("«{name}» terminó ({st})");
                app.redraw = true;
            }
        }
        // consola de texto real: el printk del kernel se pinta encima del
        // programa (no se puede evitar sin permisos); cada 2 s se reescriben
        // todas las celdas para borrar lo que haya puesto
        if app.vt_mode && last_heal.elapsed() >= Duration::from_secs(2) {
            last_heal = Instant::now();
            app.force_full = true;
            app.redraw = true;
        }
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
                // ventana de ayuda: captura todas las teclas
                if app.help_open {
                    handle_help_key(app, k);
                    continue;
                }
                // menú de programas (Ctrl+P): captura todas las teclas
                if app.menu_open {
                    handle_menu_key(app, k);
                    continue;
                }
                // mover el borde editor/shell: Ctrl+Shift+↑ reduce el shell (más edición)
                // (grupo gui; el grupo tty hace lo mismo con Ctrl+U / Ctrl+D)
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && k.modifiers.contains(KeyModifiers::SHIFT)
                {
                    match k.code {
                        KeyCode::Up => {
                            resize_border(app, 1)?;
                            continue;
                        }
                        KeyCode::Down => {
                            resize_border(app, -1)?;
                            continue;
                        }
                        _ => {}
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
                // ——— programa en la ventana principal ———
                // sólo Q/S/P/G, Ctrl+J (volver al editor) y F1 siguen siendo
                // de pigmacs; el resto de teclas ceden al programa (Ctrl+A,
                // Ctrl+F, flechas…)
                if app.mode == Mode::Program {
                    if k.code == KeyCode::F(1) {
                        app.help_open = true;
                        app.help_scroll = 0;
                        continue;
                    }
                    if let Some(sc) = shortcut_of(k) {
                        match sc {
                            Shortcut::Quit
                            | Shortcut::Save
                            | Shortcut::Menu
                            | Shortcut::CloseProgram
                            | Shortcut::ToEditor => {
                                if let Some(quit) = shortcut(app, sc)? {
                                    if quit {
                                        break;
                                    }
                                }
                                continue;
                            }
                            _ => {} // cede al programa
                        }
                    }
                    if k.code == KeyCode::BackTab {
                        // Shift+Tab siempre es de pigmacs: salta al shell
                        app.top_mode = Mode::Program;
                        app.mode = Mode::Shell;
                        continue;
                    }
                    if let Some(p) = app.program.as_mut() {
                        p.send_key(k)?;
                    }
                    continue;
                }
                // ——— atajos globales: dos grupos ———
                // grupo tty: Ctrl+<letra> · grupo gui: Alt+<letra> (misma letra, misma acción)
                if let Some(sc) = shortcut_of(k) {
                    if let Some(quit) = shortcut(app, sc)? {
                        if quit {
                            break;
                        }
                        continue;
                    }
                }
                // cualquier Alt+letra sin asignar se ignora: no debe escribirse en el editor
                if k.modifiers.contains(KeyModifiers::ALT)
                    && !k.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char(_))
                {
                    continue;
                }
                // F1: ayuda (ambos grupos; en el shell se queda para el programa de dentro)
                if k.code == KeyCode::F(1) && app.mode != Mode::Shell {
                    app.help_open = true;
                    app.help_scroll = 0;
                    continue;
                }
                // Ctrl+Tab lo traga la terminal; Shift+Tab (CSI Z, KeyCode::BackTab) llega siempre
                let is_backtab = k.code == KeyCode::BackTab;
                let toggle_tab = (k.code == KeyCode::Tab
                    && (k.modifiers.contains(KeyModifiers::CONTROL)
                        || k.modifiers.contains(KeyModifiers::SHIFT)))
                    || is_backtab;
                if toggle_tab {
                    app.mode = match app.mode {
                        Mode::Shell => app.top_mode,
                        _ => {
                            if app.mode != Mode::Shell {
                                app.top_mode = app.mode;
                            }
                            Mode::Shell
                        }
                    };
                    continue;
                }
                // Ctrl+Enter: envía la selección (en la mayoría de terminales es indistinguible
                // de Enter; Ctrl+E es el atajos fiable de los dos grupos)
                if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Enter {
                    if matches!(app.mode, Mode::Normal | Mode::Visual) {
                        app.send_selection()?;
                        continue;
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

// ————— atajos de teclado —————
// Cada acción del programa tiene DOS atajos:
//   grupo tty: Ctrl+<letra>  — llegan siempre: son bytes de control ASCII,
//                              garantizados incluso en una consola de texto
//   grupo gui: Alt+<letra>   — para terminales dentro de entornos gráficos
//                              (ESC+letra; en el shell ceden el paso al readline)
// Reservadas para el programa de dentro (tmux, readline…): Ctrl+B es el prefijo
// de tmux y Ctrl+A/E/K/R/C/D/L/U/W/Y… son de readline: en modo SHELL no se
// interceptan ninguno de los dos grupos (solo Ctrl+Q, Ctrl+S, Ctrl+Enter,
// Ctrl+J y Shift+Tab). Ctrl+J existe porque en las tty reales el kernel no
// genera Shift+Tab (CSI Z), pero el byte 0x0A de Ctrl+J sí llega siempre.
enum Shortcut {
    Quit,
    Save,
    Ai,
    Tree,
    Terminal,
    Editor,
    Send,
    Redo,
    ClearShell,
    NewAi,
    BorderUp,
    BorderDown,
    Menu,         // Ctrl/Alt+P: lanzar un programa en la ventana principal
    CloseProgram, // Ctrl/Alt+G: cerrar el programa en marcha
    ToEditor,     // Ctrl+J: volver al editor (escape seguro en tty reales)
}

/// resuelve un atajo de cualquiera de los dos grupos (Ctrl+letra o Alt+letra)
fn shortcut_of(k: KeyEvent) -> Option<Shortcut> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    if ctrl == alt {
        // ni teclas normales ni combinaciones Ctrl+Alt (AltGr)
        return None;
    }
    let KeyCode::Char(c) = k.code else { return None };
    Some(match c.to_ascii_lowercase() {
        'q' => Shortcut::Quit,
        's' => Shortcut::Save,
        'a' => Shortcut::Ai,
        'f' => Shortcut::Tree,
        't' => Shortcut::Terminal,
        'v' => Shortcut::Editor,
        'e' => Shortcut::Send,
        'r' => Shortcut::Redo,
        'k' => Shortcut::ClearShell,
        'n' => Shortcut::NewAi,
        'u' => Shortcut::BorderUp,
        'd' => Shortcut::BorderDown,
        'p' => Shortcut::Menu,
        'g' => Shortcut::CloseProgram,
        // Ctrl+J = volver al editor. Reservado incluso en el shell: en las tty
        // reales Shift+Tab no se genera, pero el kernel sí entrega siempre el
        // byte 0x0A de Ctrl+J (Alt+J queda sin asignar)
        'j' => {
            if ctrl {
                Shortcut::ToEditor
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

/// aplica un atajo. `Ok(None)` = no aplica aquí y el evento sigue su camino
/// (p. ej. en modo SHELL la tecla va al programa de dentro);
/// `Ok(Some(false))` = gestionada · `Ok(Some(true))` = salir del programa
fn shortcut(app: &mut App, s: Shortcut) -> Result<Option<bool>> {
    let in_shell = app.mode == Mode::Shell;
    match s {
        Shortcut::Quit => {
            if app.dirty {
                app.status = "Cambios sin guardar: Ctrl+S para guardar".into();
            }
            return Ok(Some(true));
        }
        Shortcut::Save => app.save()?,
        // en el panel shell ceden el paso al programa de dentro (readline, tmux…)
        Shortcut::Ai if in_shell => return Ok(None),
        Shortcut::Ai => {
            app.top_mode = app.mode;
            app.mode = if app.mode == Mode::Ai { Mode::Normal } else { Mode::Ai };
        }
        Shortcut::Tree if in_shell => return Ok(None),
        Shortcut::Tree => {
            // 1ª pulsación: visible + foco · 2ª: ocultar · Esc: foco al editor
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
        }
        Shortcut::Terminal if in_shell => return Ok(None),
        Shortcut::Terminal => {
            app.top_mode = app.mode;
            app.mode = Mode::Shell;
        }
        Shortcut::Editor if in_shell => return Ok(None),
        Shortcut::Editor => {
            app.mode = match app.top_mode {
                Mode::Browser | Mode::Shell => Mode::Normal,
                m => m,
            };
        }
        // Ctrl+J: volver al editor. Reservado también en el shell y en el modo
        // programa: en las tty reales Shift+Tab no llega, pero Ctrl+J sí
        Shortcut::ToEditor => app.mode = Mode::Normal,
        Shortcut::Send => match app.mode {
            Mode::Normal | Mode::Visual => app.send_selection()?,
            Mode::Ai => app.send_ai_selection()?,
            _ => return Ok(None),
        },
        Shortcut::Redo => match app.mode {
            Mode::Normal | Mode::Insert | Mode::Visual => app.redo_edit(),
            _ => return Ok(None),
        },
        Shortcut::ClearShell if in_shell => return Ok(None),
        Shortcut::ClearShell => {
            app.shell.output.lock().unwrap().clear();
            app.status = "Terminal limpiada".into();
        }
        Shortcut::NewAi => {
            if app.mode != Mode::Ai {
                return Ok(None);
            }
            app.ai_session_n += 1;
            app.ai_lines.clear();
            app.ai_input.clear();
            app.ai_selecting = false;
            app.ai_sel_anchor = None;
            app.status = "Nueva conversación IA (nueva sesión)".into();
        }
        Shortcut::BorderUp if in_shell => return Ok(None),
        Shortcut::BorderUp => resize_border(app, 1)?,
        Shortcut::BorderDown if in_shell => return Ok(None),
        Shortcut::BorderDown => resize_border(app, -1)?,
        // en el shell ceden a readline (nunca interrumpir un comando)
        Shortcut::Menu if in_shell => return Ok(None),
        Shortcut::Menu => app.menu_open = true,
        Shortcut::CloseProgram if in_shell => return Ok(None),
        Shortcut::CloseProgram => {
            if app.program.is_some() {
                close_program(app);
            } else {
                app.status = "No hay programa en ejecución".into();
            }
        }
    }
    Ok(Some(false))
}

/// mueve el borde editor/terminal (positivo = más terminal)
fn resize_border(app: &mut App, delta: isize) -> Result<()> {
    let (c, r) = terminal::size()?;
    let min = 3isize;
    let max = (r as isize).saturating_sub(5).max(3);
    app.shell_rows = (app.shell_rows as isize + delta).clamp(min, max) as u16;
    app.shell.resize(app.shell_rows, c);
    app.status = format!("Borde movido · shell {} líneas", app.shell_rows);
    Ok(())
}

/// una tecla pulsada → bytes que espera un programa sobre un PTY
/// (readline, tmux y cualquier TUI: htop, ranger…)
fn key_bytes(k: KeyEvent) -> Vec<u8> {
    key_bytes_mode(k, false)
}

/// igual que `key_bytes`, pero `app_cursor` (DECCKM) hace que las flechas
/// viajen como SS3 (ESC O A…) como pide un programa en modo aplicación
fn key_bytes_mode(k: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let arrow = |csi: &[u8], ss3: &[u8]| -> Vec<u8> {
        if app_cursor {
            ss3.to_vec()
        } else {
            csi.to_vec()
        }
    };
    let mut b: Vec<u8> = match k.code {
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![127],
        KeyCode::Esc => vec![27],
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Up => arrow(b"\x1b[A", b"\x1bOA"),
        KeyCode::Down => arrow(b"\x1b[B", b"\x1bOB"),
        KeyCode::Right => arrow(b"\x1b[C", b"\x1bOC"),
        KeyCode::Left => arrow(b"\x1b[D", b"\x1bOD"),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },
        KeyCode::Char(c) => {
            if ctrl {
                let l = c.to_ascii_lowercase() as u8;
                if (32..127).contains(&l) {
                    vec![l & 0x1f] // Ctrl+A = 0x01 … Ctrl+Z = 0x1a
                } else {
                    vec![]
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        _ => vec![],
    };
    if alt {
        b.insert(0, 27); // Alt+letra = ESC+letra (meta de readline)
    }
    b
}

// ————— menú de programas (Ctrl/Alt+P) —————

/// programas del menú que están instalados en el PATH
fn menu_entries() -> Vec<&'static str> {
    MENU.iter().copied().filter(|m| in_path(m)).collect()
}

/// lanza `name` sobre su propio PTY ocupando el área del editor
fn launch_program(app: &mut App, name: &str) -> Result<()> {
    close_program(app); // si ya había uno, se cierra antes de lanzar otro
    let (cols, rows) = terminal::size()?;
    let (editor_rows, _) = layout(rows, app.shell_rows);
    let bx = if app.browser.active { browser_width(cols) } else { 0 };
    let cols = cols.saturating_sub(bx).max(2);
    let rows = editor_rows.max(2);

    let pty = native_pty_system().openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = CommandBuilder::new(name);
    cmd.env("TERM", env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()));
    cmd.cwd(&app.work_dir);
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

    app.program = Some(Program {
        name: name.into(),
        writer: pty.master.take_writer()?,
        master: pty.master,
        child,
        output,
        term: Term::new(cols, rows),
    });
    app.mode = Mode::Program;
    app.status = format!("«{name}» en marcha · Ctrl+G lo cierra");
    Ok(())
}

/// cierra el programa en marcha (si lo hay) y vuelve al editor
fn close_program(app: &mut App) {
    if let Some(mut p) = app.program.take() {
        let _ = p.child.kill();
        let _ = p.child.try_wait(); // no dejar zombies
    }
    if app.mode == Mode::Program {
        app.mode = Mode::Normal;
    }
    if app.top_mode == Mode::Program {
        app.top_mode = Mode::Normal;
    }
    app.status = "Programa cerrado".into();
}

/// teclas del menú: j/k o flechas mueven · Enter lanza · Esc/q cierra
fn handle_menu_key(app: &mut App, k: KeyEvent) {
    let entries = menu_entries();
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => app.menu_open = false,
        KeyCode::Down | KeyCode::Char('j') => {
            app.menu_cursor = (app.menu_cursor + 1).min(entries.len().saturating_sub(1));
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_cursor = app.menu_cursor.saturating_sub(1);
        }
        KeyCode::Enter => {
            if let Some(&name) = entries.get(app.menu_cursor) {
                app.menu_open = false;
                // si ese programa ya está en marcha, sólo se recupera el foco
                // (así Ctrl+J al editor no lo pierde: Ctrl+P + Enter lo recupera)
                if app.program.as_ref().map(|p| p.name.as_str()) == Some(name) {
                    app.mode = Mode::Program;
                    app.status = format!("«{name}» en marcha · Ctrl+G lo cierra");
                } else if let Err(e) = launch_program(app, name) {
                    app.status = format!("No se pudo lanzar {name}: {e}");
                }
            }
        }
        _ => {}
    }
}

/// menú flotante en el centro del área principal
fn draw_menu(app: &mut App, bx: u16, area_w: u16, rows: u16, fr: &mut Frame) {
    let entries = menu_entries();
    let cur = app.menu_cursor.min(entries.len().saturating_sub(1));
    let mut lines: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{}{n}", if i == cur { "▸ " } else { "  " }))
        .collect();
    if lines.is_empty() {
        lines.push("sin programas en el PATH".into());
    }
    lines.push("Enter lanzar · Esc cierra".into());

    let width = 30u16.min(area_w.saturating_sub(2)).max(10);
    let height = (lines.len() as u16 + 2).min(rows.saturating_sub(1));
    let x = bx + area_w.saturating_sub(width) / 2;
    let y = rows.saturating_sub(height) / 2;

    let bd = Style {
        fg: Some(Color::DarkGrey),
        ..Default::default()
    };
    let title = " Lanzar programa ";
    let dashes = width.saturating_sub(2 + title.len() as u16);
    fr.text(x, y, &format!("┌{title}{}┐", "─".repeat(dashes as usize)), bd);
    for (i, line) in lines.iter().take(height.saturating_sub(2) as usize).enumerate() {
        let content: String = line.chars().take(width.saturating_sub(2) as usize).collect();
        let pad = width.saturating_sub(2).saturating_sub(content.chars().count() as u16);
        let st = if i == cur && i < entries.len() {
            Style {
                reverse: true,
                ..Default::default()
            }
        } else {
            Style::default()
        };
        fr.text(x + 1, y + 1 + i as u16, &format!("{content}{}", " ".repeat(pad as usize)), st);
    }
    fr.text(
        x,
        y + height.saturating_sub(1),
        &format!("└{}┘", "─".repeat(width.saturating_sub(2) as usize)),
        bd,
    );
}

const HELP_TEXT: &str = r#"PIGMACS · MANUAL COMPLETO
═══════════════════════════
ATAJOS EN DOS GRUPOS
  Ctrl+<letra>  grupo TTY — funciona en cualquier consola de texto
  Alt+<letra>   grupo GUI — para terminales en entornos gráficos
  (misma letra = misma acción; Ctrl+B queda libre para tmux)
  en modo SHELL las teclas van al programa de dentro: Ctrl+B (prefijo
  tmux), Ctrl+A/E/K/R (readline) se respetan; solo Ctrl+Q, Ctrl+S,
  Ctrl+Enter, Ctrl+J y Shift+Tab siguen siendo de pigmacs
  en modo PROG (programa lanzado con Ctrl+P) sólo Ctrl/Alt+Q, S, P, G y
  Ctrl+J son de pigmacs; el resto de teclas ceden al programa en marcha

ACCIONES GLOBALES
  Ctrl+Q / Alt+Q     salir (avisa si hay cambios sin guardar)
  Ctrl+S / Alt+S     guardar (pide sudo si el fichero es de root)
  Ctrl+A / Alt+A     alternar editor <-> panel IA
  Ctrl+F / Alt+F     mostrar/ocultar y enfocar el tree
  Ctrl+T / Alt+T     ir al terminal (shell)
  Ctrl+V / Alt+V     volver al editor
  Ctrl+J             volver al editor desde el shell/programa
                     (escape para tty reales: Shift+Tab no llega allí)
  Ctrl+E / Ctrl+Enter enviar selección (o línea entera) al shell
  Ctrl+R / Alt+R     rehacer
  Ctrl+K / Alt+K     limpiar la pantalla de la terminal
  Ctrl+U / Ctrl+D    subir/bajar el borde (más terminal / más editor)
  Ctrl+N / Alt+N     nueva conversación IA (dentro del panel IA)
  Ctrl+P / Alt+P     menú para lanzar programas en la ventana principal
  Ctrl+G / Alt+G     cerrar el programa en marcha
  F1 / ?             esta ayuda
  Shift+←/→/↑/↓      árbol · editor · terminal (grupo gui)
  Ctrl+Shift+↑/↓     mover el borde editor/terminal (grupo gui)
  Tab / Shift+Tab    alternar terminal <-> ventana anterior
                     (en tty reales usa Ctrl+J para volver al editor)

EDITOR · MODO NORMAL
  h j k l / ←↓↑→      mover el cursor
  i / a / A           insertar (antes, después, fin de línea)
  o / O               nueva línea debajo / encima
  x                   borrar carácter · dd borrar línea
  u                   deshacer · Ctrl+R rehacer
  v                   selección visual
  /                   buscar · n/N siguiente/anterior
  m + letra           fijar marca · ' + letra saltar a ella
  G / gg              final / principio · 12G ir a la línea 12
  zn                  mostrar/ocultar números de línea
  ?                   esta ayuda

ORG MODE · PLIEGUES
  Tab / za            plegar/desplegar cabecera
  zc / zo             cerrar / abrir cabecera
  zr / zm             abrir todo / plegar todo
  (al abrir un fichero todo queda plegado)

MODO INSERCIÓN
  Esc                 volver a Normal
  Enter / ←→↑↓        edición normal
  Tab                 pliega la cabecera bajo el cursor

MODO VISUAL
  h j k l / ←↓↑→      extender la selección
  x                   borrar la selección
  Esc / v             salir

TERMINAL
  ↑↓                  historial de comandos (colores ANSI incluidos)
  Home/Fin/Pág↑/Pág↓  navegación del propio readline (nuevo)
  Ctrl+B              prefijo de tmux: llega al programa de dentro
  Ctrl+A/E/K/R        reservados para readline (emacs) dentro del shell
  Ctrl+K              limpiar la pantalla (desde el editor/IA/tree)

TREE DE FICHEROS
  j k                 mover · l/Enter abrir · h subir
  g / G               principio / fin
  b                   marcar/desmarcar el directorio actual
  B                   saltar al siguiente marcador (★ en el título)
  Esc                 volver al editor (el tree sigue visible)

PROGRAMAS EN LA VENTANA PRINCIPAL (Ctrl+P)
  j k / ↑↓            mover por el menú · Enter lanzar · Esc/q cerrar
  (sólo aparecen los instalados en el PATH: htop, btop, glances, ncdu,
   ranger, gitui, tmux, less)
  el programa corre sobre su propio PTY ocupando el área del editor y
  se dibuja con un emulador VT100 (colores 256, pantalla alterna…)
  mientras está activo, sus teclas le ceden salvo Ctrl/Alt+Q (salir),
  Ctrl/Alt+S (guardar el fichero), Ctrl/Alt+P (menú), Ctrl/Alt+G (cerrar
  el programa), Ctrl+J (volver al editor dejándolo en segundo plano) y
  Shift+Tab (saltar al shell y volver)
  si el programa termina solo, pigmacs vuelve al editor y lo indica
  en el status; el menú también abre sobre un programa en marcha
  (se lanza el nuevo y se cierra el anterior; si eliges el que ya
  está en marcha sólo se recupera su foco con Ctrl+P + Enter)

PANEL IA (pi coding agent)
  Enter               enviar la pregunta
  ↑↓                  historial de preguntas
  v                   seleccionar texto de la respuesta
  Ctrl+E              enviar la selección al shell
  Ctrl+N              nueva conversación
  Esc                 cancelar la consulta en curso

GUARDAR · SUDO
  Ctrl+S              guardar; si el fichero es de root pide contraseña
  (en el prompt sudo: Enter acepta, Esc/Ctrl+C cancela)

GENERAL
  Ctrl+Q              salir (avisa si hay cambios sin guardar)
  Ctrl+S              guardar
  Ctrl+K              limpiar la terminal
  Ctrl+F / Ctrl+T     tree de ficheros / terminal
  Ctrl+P / Ctrl+G     menú de programas / cerrar programa en marcha
"#;

fn handle_help_key(app: &mut App, k: KeyEvent) {
    if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return;
    }
    match k.code {
        // la misma tecla que abre la ayuda la cierra (F1 / ?)
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(1) | KeyCode::Char('?') => {
            app.help_open = false;
            app.help_scroll = 0;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.help_scroll = app.help_scroll.saturating_add(1)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.help_scroll = app.help_scroll.saturating_sub(1)
        }
        KeyCode::Char('g') => app.help_scroll = 0,
        KeyCode::Char('G') => app.help_scroll = usize::MAX,
        _ => {}
    }
}

fn draw_help(app: &mut App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
    let lines: Vec<&str> = HELP_TEXT.lines().collect();
    let max = editor_rows.saturating_sub(1) as usize;
    let start = app.help_scroll.min(lines.len().saturating_sub(max.max(1)));
    let end = (start + max).min(lines.len());
    for (i, l) in lines[start..end].iter().enumerate() {
        let clean: String = l.chars().take(area_w as usize).collect();
        let st = if i == 0 {
            Style {
                bold: true,
                ..Default::default()
            }
        } else {
            Style::default()
        };
        fr.text(bx, i as u16, &clean, st);
    }
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
                app.status = "Directorio desmarcado".into();
            } else {
                app.bookmarks.push(app.browser.cwd.clone());
                app.status = format!("Marcado: {}", app.browser.cwd.display());
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
                app.status = format!("Marcador: {}", app.browser.cwd.display());
            }
        }
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
        KeyCode::Char(c)
            if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.ai_input.push(c);
        }
        _ => {}
    }
}

fn handle_editor_key(app: &mut App, k: KeyEvent) {
    // los Ctrl/Alt que no son atajos no deben ejecutar comandos del editor
    // (p. ej. Ctrl+X no puede borrar como si fuese la tecla x)
    if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return;
    }
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
                        | KeyCode::Char('n')
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
            // el contador numérico se mantiene con dígitos y se consume en G/gg
            if !matches!(k.code, KeyCode::Char(c) if c.is_ascii_digit())
                && !matches!(k.code, KeyCode::Char('g') | KeyCode::Char('G'))
            {
                app.count = None;
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
                // contador numérico: 12G → línea 12 · 5gg → línea 5
                KeyCode::Char(d) if d.is_ascii_digit() && (app.count.is_some() || d != '0') => {
                    let n = (d as u8 - b'0') as usize;
                    app.count = Some(app.count.unwrap_or(0) * 10 + n);
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
                | KeyCode::Char('n')
                    if app.pending == Some('z') =>
                {
                    // plegado: za alternar · zc cerrar · zo abrir · zr todo abierto · zm todo cerrado · zn números
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
                        KeyCode::Char('n') => {
                            app.show_numbers = !app.show_numbers;
                            app.status = if app.show_numbers {
                                "Números de línea: sí".into()
                            } else {
                                "Números de línea: no".into()
                            };
                        }
                        _ => {}
                    }
                    app.pending = None;
                }
                KeyCode::Char('g') if app.pending == Some('g') => {
                    let target = app
                        .count
                        .map(|c| c.saturating_sub(1))
                        .unwrap_or_else(|| app.first_visible());
                    app.row = target.min(app.last_visible());
                    app.col = 0;
                    app.count = None;
                    app.pending = None;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('g') => app.pending = Some('g'),
                KeyCode::Char('G') => {
                    let last = app.last_visible();
                    app.row = app
                        .count
                        .map(|c| c.saturating_sub(1).min(last))
                        .unwrap_or(last);
                    app.col = 0;
                    app.count = None;
                    app.pending = None;
                    app.reveal_row(app.row);
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
                KeyCode::Char('u') => app.undo_edit(),
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
                KeyCode::Char('?') => {
                    app.help_open = true;
                    app.help_scroll = 0;
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
        Mode::Shell | Mode::Ai | Mode::Browser | Mode::Program => {}
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
    let bytes = key_bytes(k);
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

/// escribe en la terminal únicamente las celdas que cambiaron respecto al
/// frame anterior; con `force` se reescriben todas (borra lo que un aviso
/// del kernel haya pintado encima en las consolas de texto)
fn render_frame(prev: &mut Frame, cur: &Frame, out: &mut io::Stdout, force: bool) -> io::Result<()> {
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
            if !force && prev.cells[i] == cur.cells[i] {
                x += 1;
                continue;
            }
            // tramo de celdas cambiadas: un solo MoveTo y cada celda con su estilo
            queue!(out, MoveTo(x as u16, y as u16))?;
            while x < w && (force || prev.cells[row + x] != cur.cells[row + x]) {
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
    // ESC[m sin parámetros = restaurar el estilo por defecto
    if params.trim().is_empty() {
        return Style::default();
    }
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

/// secuencias CSI del mini-emulador del panel shell: readline las usa al
/// editar (Delete → DCH, Ctrl+U → EL, flechas → CUF/CUB…); sin
/// interpretarlas, los caracteres que se borraban se quedaban pintados
fn csi_shell(
    lines: &mut Vec<Vec<(char, Style)>>,
    x: &mut usize,
    st: &mut Style,
    params: &str,
    fin: char,
) {
    let mut ps = params.split(';');
    let n1: usize = ps.next().and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let n0: usize = params
        .split(';')
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    match fin {
        'm' => {
            *st = if params.is_empty() {
                Style::default()
            } else {
                parse_sgr(params, *st)
            };
        }
        // CUF / CUB: cursor a la derecha / izquierda dentro de la línea
        'C' => {
            if let Some(line) = lines.last() {
                *x = (*x + n1).min(line.len());
            }
        }
        'D' => *x = x.saturating_sub(n1),
        // CHA: columna absoluta (1-based)
        'G' => {
            if let Some(line) = lines.last() {
                *x = (n1 - 1).min(line.len());
            }
        }
        // CUP: sólo se usa la columna — la edición ocurre en la última línea
        'H' | 'f' => {
            let col: usize = params
                .split(';')
                .nth(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            if let Some(line) = lines.last() {
                *x = col.saturating_sub(1).min(line.len());
            }
        }
        // DCH: borrar n caracteres bajo el cursor (tecla Delete)
        'P' => {
            if let Some(line) = lines.last_mut() {
                if *x < line.len() {
                    let end = (*x + n1).min(line.len());
                    line.drain(*x..end);
                }
            }
        }
        // ICH: insertar n espacios; el cursor no se mueve
        '@' => {
            if let Some(line) = lines.last_mut() {
                for _ in 0..n1 {
                    let at = (*x).min(line.len());
                    line.insert(at, (' ', Style::default()));
                }
            }
        }
        // EL: desde el cursor (0) · hasta el cursor (1) · toda la línea (2)
        'K' => {
            if let Some(line) = lines.last_mut() {
                match n0 {
                    1 => {
                        for c in line.iter_mut().take(*x + 1) {
                            *c = (' ', Style::default());
                        }
                    }
                    2 => {
                        for c in line.iter_mut() {
                            *c = (' ', Style::default());
                        }
                    }
                    _ => line.truncate(*x),
                }
            }
        }
        // ED: 2/3 limpia todo (clear) · 0 desde el cursor · 1 hasta el cursor
        'J' => match n0 {
            2 | 3 => {
                lines.clear();
                lines.push(Vec::new());
                *x = 0;
            }
            1 => {
                if let Some(line) = lines.last_mut() {
                    for c in line.iter_mut().take(*x + 1) {
                        *c = (' ', Style::default());
                    }
                }
            }
            _ => {
                if let Some(line) = lines.last_mut() {
                    line.truncate(*x);
                }
            }
        },
        _ => {}
    }
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
                    if j < chars.len() {
                        let params: String = chars[i + 2..j].iter().collect();
                        csi_shell(&mut lines, &mut x, &mut st, &params, chars[j]);
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

    // el programa ocupa el área del editor: su PTY sigue la geometría actual
    // (resize de terminal, borde editor/shell, abrir/cerrar el tree)
    if let Some(p) = app.program.as_mut() {
        p.resize(editor_rows.max(1), area_w.max(1));
    }

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

    if app.help_open {
        draw_help(app, editor_rows, bx, area_w, &mut fr);
    } else if app.mode == Mode::Program {
        if let Some(p) = &app.program {
            p.term.draw(&mut fr, bx, editor_rows);
        }
    } else if app.mode == Mode::Ai {
        draw_ai_pane(app, editor_rows, bx, area_w, &mut fr);
    } else if app.mode != Mode::Browser {
        draw_editor_lines(app, editor_rows, bx, area_w, &mut fr);
    }

    // menú de programas: siempre encima del área principal
    if app.menu_open {
        draw_menu(app, bx, area_w, editor_rows, &mut fr);
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
        Mode::Program => "PROG  ",
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
            "Ctrl+F tree · Ctrl+T terminal · i · u deshacer · Ctrl+R rehacer · Tab/za plegar · m marca · F1/? ayuda · Ctrl+E enviar · Ctrl+S · Ctrl+Q"
        }
        Mode::Browser => {
            "j/k mover · l/h entrar/subir · b marca · B saltar · g/G extremos · Esc editor · Ctrl+F ocultar"
        }
        Mode::Ai => {
            "Enter enviar · ↑/↓ historial · v seleccionar · Ctrl+E enviar · Ctrl+N nueva · Esc cancelar · Tab shell"
        }
        Mode::Shell => {
            "escribe comandos · Ctrl+B tmux · Ctrl+A/E/K readline · Ctrl+J editor · Ctrl+Q salir"
        },
        Mode::Program => {
            "teclas al programa · Ctrl+P menú · Ctrl+G cerrar · Ctrl+J editor · Ctrl+Q salir"
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
        && !matches!(app.mode, Mode::Ai | Mode::Browser | Mode::Shell | Mode::Program)
    {
        let y = app.screen_row_of(app.row).min(editor_rows.saturating_sub(1) as usize) as u16;
        let x = (bx as usize + app.gutter() + app.col.saturating_sub(app.left))
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
    let force = std::mem::take(&mut app.force_full);
    render_frame(&mut app.prev, &fr, &mut out, force)?;

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
    } else if app.mode == Mode::Program {
        // el cursor del programa (htop, less…) donde él lo haya puesto
        let vis = app
            .program
            .as_ref()
            .map(|p| p.term.cursor_visible)
            .unwrap_or(false);
        if vis {
            let p = app.program.as_ref().unwrap();
            let cx = bx + (p.term.x as u16).min(area_w.saturating_sub(1));
            let cy = (p.term.y as u16).min(editor_rows.saturating_sub(1));
            queue!(out, Show, MoveTo(cx, cy))?;
        } else {
            queue!(out, Hide, MoveTo(0, 0))?;
        }
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
    let starred = app.bookmarks.iter().any(|p| p == &app.browser.cwd);
    let title: String = if app.mode == Mode::Browser {
        format!("┌ {} {} ", dirname, if starred { "★" } else { "" })
    } else {
        format!("┌ {} {} · Ctrl+F ", dirname, if starred { "★" } else { "" })
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
    let gutter = app.gutter();
    let max_content = area_w.saturating_sub(gutter as u16) as usize;
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

        if app.show_numbers {
            fr.text(bx, screen_y, &format!("{:>5} ", idx + 1), Style::default());
        } else {
            fr.put(bx, screen_y, ' ', Style::default());
        }

        let mut printed = 0usize;
        for (x, ch) in line.chars().skip(app.left).enumerate() {
            if x + gutter >= area_w as usize {
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
            fr.put(bx + gutter as u16 + x as u16, screen_y, ch, st);
        }

        if folded_here {
            // marcador de plegado al final del texto visible
            if printed + 2 < max_content {
                fr.text(
                    bx + gutter as u16 + printed as u16,
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
