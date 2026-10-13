// ————— pigmacs · emulador VT100 —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::frame::{Cell, Frame, Style};
use crossterm::style::Color;

/// estado del parser del emulador VT100
#[derive(PartialEq)]
pub(crate) enum PState {
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
pub(crate) struct Term {
    pub(crate) w: u16,
    pub(crate) h: u16,
    pub(crate) grid: Vec<Vec<Cell>>, // todas las filas con el ancho completo
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) top: usize, // región de scroll (inclusive)
    pub(crate) bot: usize,
    pub(crate) st: Style,         // estilo SGR activo
    pub(crate) wrap: bool,        // auto-wrap (DEC, activo por defecto)
    pub(crate) pending_wrap: bool, // wrap diferido como en xterm (xenl)
    pub(crate) app_cursor: bool,  // DECCKM: flechas en modo aplicación (SS3)
    pub(crate) cursor_visible: bool,
    pub(crate) saved: Option<(usize, usize, Style)>,
    pub(crate) alt: Option<(Vec<Vec<Cell>>, usize, usize, Style)>,
    pub(crate) last_ch: char,    // para REP (ESC[b)
    pub(crate) reply: Vec<u8>,   // respuestas (DSR/DA) pendientes de escribir al PTY
    pub(crate) buf: Vec<u8>,     // bytes recibidos aún sin procesar (secuencia a medias)
    pub(crate) state: PState,
    pub(crate) csi: String,
}

impl Term {
    pub(crate) fn new(w: u16, h: u16) -> Self {
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

    pub(crate) fn resize(&mut self, w: u16, h: u16) {
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
    pub(crate) fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        self.parse();
    }

    pub(crate) fn parse(&mut self) {
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
    pub(crate) fn ctrl(&mut self, b: u8) {
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

    pub(crate) fn line_feed(&mut self) {
        self.pending_wrap = false;
        if self.y == self.bot {
            self.scroll_up(1);
        } else if self.y + 1 < self.h as usize {
            self.y += 1;
        }
    }

    pub(crate) fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.y == self.top {
            self.scroll_down(1);
        } else {
            self.y = self.y.saturating_sub(1);
        }
    }

    pub(crate) fn scroll_up(&mut self, n: usize) {
        let region = (self.bot - self.top + 1).min(self.h as usize);
        for _ in 0..n.min(region) {
            self.grid.remove(self.top);
            self.grid
                .insert(self.bot, vec![Cell::default(); self.w as usize]);
        }
    }

    pub(crate) fn scroll_down(&mut self, n: usize) {
        let region = (self.bot - self.top + 1).min(self.h as usize);
        for _ in 0..n.min(region) {
            self.grid.remove(self.bot);
            self.grid
                .insert(self.top, vec![Cell::default(); self.w as usize]);
        }
    }

    pub(crate) fn put_char(&mut self, c: char) {
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

    pub(crate) fn save_cursor(&mut self) {
        self.saved = Some((self.x, self.y, self.st));
    }

    pub(crate) fn restore_cursor(&mut self) {
        if let Some((x, y, st)) = self.saved {
            self.x = x.min(self.w.saturating_sub(1) as usize);
            self.y = y.min(self.h.saturating_sub(1) as usize);
            self.st = st;
        }
        self.pending_wrap = false;
    }

    pub(crate) fn reset(&mut self) {
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

    pub(crate) fn alt_screen(&mut self, on: bool) {
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

    pub(crate) fn set_mode(&mut self, n: usize, on: bool) {
        match n {
            1 => self.app_cursor = on,
            7 => self.wrap = on,
            25 => self.cursor_visible = on,
            47 | 1047 | 1049 => self.alt_screen(on),
            _ => {} // bracketed paste y demás: se ignoran
        }
    }

    pub(crate) fn erase_display(&mut self, n: usize) {
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

    pub(crate) fn erase_line(&mut self, n: usize) {
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

    pub(crate) fn insert_lines(&mut self, n: usize) {
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

    pub(crate) fn delete_lines(&mut self, n: usize) {
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

    pub(crate) fn delete_chars(&mut self, n: usize) {
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

    pub(crate) fn insert_chars(&mut self, n: usize) {
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

    pub(crate) fn erase_chars(&mut self, n: usize) {
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
    pub(crate) fn csi_final(&mut self, params: &str, fin: char) {
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
    pub(crate) fn draw(&self, fr: &mut Frame, bx: u16, rows: u16) {
        for y in 0..(self.h as usize).min(rows as usize) {
            for x in 0..self.w as usize {
                let c = self.grid[y][x];
                fr.put(bx + x as u16, y as u16, c.ch, c.st);
            }
        }
    }
}

/// aplica una secuencia SGR (parámetros de ESC[...m) sobre un estilo
pub(crate) fn parse_sgr(params: &str, mut st: Style) -> Style {
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
