// ————— pigmacs · portapapeles (anillo de muertes) —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    4.1.0
// Fecha:      2026-10-31
//
// Portapapeles propio, independiente de `Ctrl+E` (que envía al shell):
//
//   · anillo de muertes (kill ring) estilo emacs/vim: cada copia o corte
//     apila texto y `p`/`P` pega el último; `Ctrl+Y` abre el historial.
//   · registros con nombre: `"ay` guarda en el registro `a`, `"ap` lo pega;
//     `"0p`, `"1p`… eligen ranuras del anillo (0 = lo más reciente).
//   · sin entorno gráfico no hay portapapeles del sistema: `F2` captura lo
//     que se ve (editor, shell o programa) a un fichero temporal y lo abre
//     en el editor, donde la selección y `y` funcionan como siempre.
//   · al copiar se emite OSC 52 (best-effort): en una tty real no hace nada;
//     en tmux/xterm con integración copia también al portapapeles del sistema.
use crate::app::{App, Mode};
use crate::frame::Frame;
use crate::program::draw_popup;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::process::Command;

pub(crate) const ANILLO_MAX: usize = 50;

impl App {
    /// apila un texto en el anillo de muertes (sin duplicar el último)
    pub(crate) fn matar(&mut self, texto: String) {
        if texto.is_empty() {
            return;
        }
        if self.anillo.last().map(|t| t == &texto).unwrap_or(false) {
            return;
        }
        self.anillo.push(texto);
        if self.anillo.len() > ANILLO_MAX {
            self.anillo.remove(0);
        }
    }

    /// copia `texto` al registro activo (letra) o al anillo, y lo deja para
    /// OSC 52. `registro` se consume: vale para la operación siguiente.
    pub(crate) fn yank(&mut self, texto: String) {
        if texto.is_empty() {
            return;
        }
        let n = texto.chars().count();
        match self.registro {
            Some(c) if c.is_ascii_alphabetic() => {
                self.nombrados.insert(c, texto.clone());
                self.status = format!("Copiado al registro «{c}» ({n} car.) · \"{c}p pegar");
            }
            _ => {
                self.matar(texto.clone());
                self.status =
                    format!("Copiado al anillo ({n} car.) · p pegar · Ctrl+Y historial");
            }
        }
        self.registro = None;
        self.osc52 = Some(texto);
    }

    /// texto que pegará la siguiente operación, según el registro activo
    pub(crate) fn portapapeles(&self) -> Option<String> {
        match self.registro {
            Some(c) if c.is_ascii_digit() => {
                let n = (c as u8 - b'0') as usize;
                self.anillo.iter().rev().nth(n).cloned()
            }
            Some(c) => self.nombrados.get(&c).cloned(),
            None => self.anillo.last().cloned(),
        }
    }

    /// texto de la selección visual (extremos incluidos, con \n entre líneas)
    pub(crate) fn seleccion_texto(&self) -> Option<String> {
        let (ar, ac) = self.anchor?;
        let (br, bc) = (self.row, self.col);
        let ((r1, c1), (r2, c2)) = if (ar, ac) <= (br, bc) {
            ((ar, ac), (br, bc))
        } else {
            ((br, bc), (ar, ac))
        };
        if r1 == r2 {
            let l = self.lines.get(r1)?;
            Some(l.chars().skip(c1).take(c2 + 1 - c1).collect())
        } else {
            let mut out: String = self.lines.get(r1)?.chars().skip(c1).collect();
            for r in (r1 + 1)..r2 {
                out.push('\n');
                if let Some(l) = self.lines.get(r) {
                    out.push_str(l);
                }
            }
            out.push('\n');
            out.extend(self.lines.get(r2)?.chars().take(c2 + 1));
            Some(out)
        }
    }

    /// pega el registro activo en el cursor (`p`) o antes (`P`). Un texto
    /// terminado en \n viene de `yy` y se pega como línea completa.
    pub(crate) fn pegar(&mut self, antes: bool) {
        let Some(txt) = self.portapapeles() else {
            self.registro = None;
            self.status = "El anillo está vacío · y copia · Ctrl+Y historial".into();
            return;
        };
        let nombre = match self.registro {
            Some(c) => format!("registro «{c}»"),
            None => "anillo".to_string(),
        };
        self.registro = None;
        self.push_undo("pegado");
        // en INSERT se pega siempre "en el cursor" (aunque venga de yy)
        let linewise = self.mode != Mode::Insert
            && txt.ends_with('\n')
            && !txt.trim_end_matches('\n').contains('\n');
        if linewise {
            // yy: una línea nueva, encima (P) o debajo (p)
            let fila = if antes { self.row } else { self.row + 1 };
            self.lines
                .insert(fila, txt.trim_end_matches('\n').to_string());
            self.shift_folds_insert(fila);
            self.reconstruir_visibles();
            self.row = fila;
            self.col = 0;
        } else {
            let partes: Vec<String> = txt.split('\n').map(str::to_string).collect();
            self.insertar_texto(&partes, antes);
        }
        self.dirty = true;
        self.texto_ver += 1;
        self.clamp_col();
        self.ensure_visible();
        let n = txt.chars().count();
        self.status = format!("Pegado del {nombre} ({n} car.)");
    }

    /// inserta varias líneas en la posición del cursor (pegado no linewise)
    fn insertar_texto(&mut self, partes: &[String], antes: bool) {
        // en INSERT el cursor está ENTRE caracteres: se inserta ahí mismo
        let col = if antes || self.mode == Mode::Insert {
            self.col
        } else {
            self.col + 1
        };
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let col = col.min(chars.len());
        let prefijo: String = chars[..col].iter().collect();
        let sufijo: String = chars[col..].iter().collect();
        if partes.len() == 1 {
            self.lines[self.row] = format!("{prefijo}{}{sufijo}", partes[0]);
            self.col = col + partes[0].chars().count();
            return;
        }
        let ult = partes.len() - 1;
        let mut nuevas: Vec<String> = Vec::with_capacity(partes.len());
        nuevas.push(format!("{prefijo}{}", partes[0]));
        for p in &partes[1..ult] {
            nuevas.push(p.clone());
        }
        nuevas.push(format!("{}{sufijo}", partes[ult]));
        let n = nuevas.len();
        self.lines.splice(self.row..=self.row, nuevas);
        for _ in 1..n {
            self.shift_folds_insert(self.row + 1);
        }
        self.reconstruir_visibles();
        self.row += n - 1;
        self.col = partes[ult].chars().count();
    }
}

/// entradas del popup en orden de pantalla: primero los registros con
/// nombre (a-z, ordenados) y después el anillo del más antiguo al más nuevo.
/// Devuelve (etiqueta, texto) para pintar y pegar.
pub(crate) fn anillo_entradas(app: &App) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut nombres: Vec<char> = app.nombrados.keys().copied().collect();
    nombres.sort();
    for c in nombres {
        if let Some(t) = app.nombrados.get(&c) {
            out.push((format!("\"{c}"), t.clone()));
        }
    }
    let ultimo = app.anillo.len().saturating_sub(1);
    for (i, t) in app.anillo.iter().enumerate() {
        // ranura = distancia al más reciente (0 = último copiado)
        out.push((format!("#{}", ultimo - i), t.clone()));
    }
    out
}

/// teclas del popup del anillo (`Ctrl+Y`): elegir y pegar
pub(crate) fn handle_anillo_key(app: &mut App, k: KeyEvent) {
    if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return;
    }
    let n = anillo_entradas(app).len();
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('y') => app.anillo_open = false,
        KeyCode::Char('j') | KeyCode::Down => {
            if n > 0 {
                app.anillo_cursor = (app.anillo_cursor + 1).min(n - 1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.anillo_cursor = app.anillo_cursor.saturating_sub(1);
        }
        KeyCode::Char('g') => app.anillo_cursor = 0,
        KeyCode::Char('G') => app.anillo_cursor = n.saturating_sub(1),
        KeyCode::Enter => {
            if let Some((_, txt)) = anillo_entradas(app).get(app.anillo_cursor).cloned() {
                app.anillo_open = false;
                app.registro = None;
                pegar_texto(app, &txt);
            }
        }
        _ => {}
    }
}

/// pega un texto concreto según el panel activo: editor, shell o programa
pub(crate) fn pegar_texto(app: &mut App, txt: &str) {
    match app.mode {
        Mode::Shell => {
            let _ = app.shell.writer.write_all(txt.as_bytes());
            let _ = app.shell.writer.flush();
            app.status = format!("Pegado en el shell ({} car.)", txt.chars().count());
        }
        Mode::Program => {
            if let Some(p) = app.program.as_mut() {
                let _ = p.writer.write_all(txt.as_bytes());
                let _ = p.writer.flush();
            }
            app.status = format!("Pegado en el programa ({} car.)", txt.chars().count());
        }
        _ => {
            // editor: se apila en el anillo y se pega en el cursor
            app.matar(txt.to_string());
            app.registro = None;
            app.pegar(false);
        }
    }
}

/// popup con el historial del anillo: el más reciente abajo
pub(crate) fn draw_anillo(app: &mut App, bx: u16, area_w: u16, rows: u16, fr: &mut Frame) {
    let mut lines: Vec<(String, bool)> = Vec::new();
    let entradas = anillo_entradas(app);
    if entradas.is_empty() {
        lines.push(("el anillo está vacío".into(), false));
    }
    for (i, (etiqueta, txt)) in entradas.iter().enumerate() {
        // una línea por entrada: saltos y tabuladores a espacios
        let plano: String = txt
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .collect();
        let corto: String = plano.chars().take(42).collect();
        let n = txt.chars().count();
        lines.push((
            format!(
                "{}[{n:>4}] {etiqueta:<3} {corto}",
                if i == app.anillo_cursor { "▸ " } else { "  " }
            ),
            i == app.anillo_cursor,
        ));
    }
    lines.push(("Enter pega · j/k mover · Esc cierra".into(), false));
    draw_popup(fr, bx, area_w, rows, " portapapeles · anillo ", 54, &lines);
}

/// `F2`: captura lo que se ve en la ventana activa a un fichero temporal y
/// lo abre en el editor, donde la selección y `y` funcionan como siempre.
/// Con tmux se aprovecha `capture-pane` (incluye el scrollback).
pub(crate) fn capturar_ventana(app: &mut App) -> Option<String> {
    let (cols, filas) = crossterm::terminal::size().unwrap_or((80, 24));
    let (editor_rows, _) = crate::app::layout(filas, app.shell_rows);
    let cols = cols.saturating_sub(4);
    let (nombre, texto) = match app.mode {
        Mode::Shell => {
            if let Ok(salida) = Command::new("tmux")
                .args(["capture-pane", "-p", "-S", "-2000"])
                .output()
            {
                if salida.status.success() {
                    ("tmux", String::from_utf8_lossy(&salida.stdout).to_string())
                } else {
                    ("shell", crate::shell::shell_texto(app))
                }
            } else {
                ("shell", crate::shell::shell_texto(app))
            }
        }
        Mode::Program => match app.program.as_ref() {
            Some(p) => ("programa", p.term.texto_completo()),
            None => ("editor", editor_visible(app, editor_rows, cols)),
        },
        Mode::Ai => ("IA", app.ai_lines.join("\n")),
        Mode::Browser => ("editor", editor_visible(app, editor_rows, cols)),
        _ => ("editor", editor_visible(app, editor_rows, cols)),
    };
    if texto.trim().is_empty() {
        app.status = "No hay texto que capturar en esta ventana".into();
        return None;
    }
    let ruta = std::env::temp_dir().join(format!("pigmacs-captura-{}.txt", std::process::id()));
    std::fs::write(&ruta, &texto).ok()?;
    let lineas = texto.lines().count();
    app.open_file(ruta.clone());
    // el buffer de captura es read-only: evita guardar encima por accidente
    app.read_only = true;
    app.status = format!(
        "Captura de {nombre} → {} ({lineas} líneas) · y copiar · Ctrl+J volver",
        ruta.display()
    );
    Some(texto)
}

/// texto visible del editor (sin números de línea y sin filas plegadas)
fn editor_visible(app: &App, rows: u16, cols: u16) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut r = app.top;
    let mut pintadas = 0u16;
    while r < app.lines.len() && pintadas < rows {
        if !app.ocultas.get(r).copied().unwrap_or(false) {
            out.push(app.lines[r].chars().take(cols as usize).collect());
            pintadas += 1;
        }
        r += 1;
    }
    out.join("\n")
}

/// secuencia OSC 52 para copiar al portapapeles del sistema (best-effort)
pub(crate) fn osc52(texto: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(texto.as_bytes()))
}

/// base64 mínimo (sin dependencias) para OSC 52
fn base64(datos: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((datos.len() + 2) / 3 * 4);
    for trozo in datos.chunks(3) {
        let b = [
            trozo[0],
            *trozo.get(1).unwrap_or(&0),
            *trozo.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if trozo.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if trozo.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
