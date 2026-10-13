// ————— pigmacs · frame: celdas, estilos y pintado por diff —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.0.1
// Fecha:      2026-10-13
use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{Clear, ClearType},
};
use std::io;

// ---------- frame en memoria + diff por celdas (elimina el parpadeo) ----------
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Style {
    pub(crate) fg: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) reverse: bool,
    pub(crate) bold: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) st: Style,
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
pub(crate) struct Frame {
    pub(crate) w: u16,
    pub(crate) h: u16,
    pub(crate) cells: Vec<Cell>,
}

impl Frame {
    pub(crate) fn new(w: u16, h: u16) -> Self {
        Frame {
            w,
            h,
            cells: vec![Cell::default(); (w as usize) * (h as usize)],
        }
    }

    pub(crate) fn put(&mut self, x: u16, y: u16, ch: char, st: Style) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = y as usize * self.w as usize + x as usize;
        self.cells[i] = Cell { ch, st };
    }

    pub(crate) fn text(&mut self, x: u16, y: u16, s: &str, st: Style) {
        for (i, c) in s.chars().enumerate() {
            self.put(x + i as u16, y, c, st);
        }
    }
}

/// escribe en la terminal únicamente las celdas que cambiaron respecto al
/// frame anterior; con `force` se reescriben todas (borra lo que un aviso
/// del kernel haya pintado encima en las consolas de texto)
pub(crate) fn render_frame(prev: &mut Frame, cur: &Frame, out: &mut io::Stdout, force: bool) -> io::Result<()> {
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

pub(crate) fn strip_ansi(s: &str) -> String {
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
