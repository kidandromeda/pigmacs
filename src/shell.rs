// ————— pigmacs · terminal (shell PTY) —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.0.1
// Fecha:      2026-10-13
use crate::{
    app::App,
    frame::{Frame, Style},
    term::parse_sgr,
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, MasterPty, PtySize};
use std::io::Write;
use std::sync::{Arc, Mutex};

pub(crate) struct Shell {
    pub(crate) output: Arc<Mutex<Vec<u8>>>,
    pub(crate) writer: Box<dyn Write + Send>,
    // se conservan a propósito: mantienen el PTY abierto y al hijo vivo
    pub(crate) _master: Box<dyn MasterPty + Send>,
    pub(crate) _child: Box<dyn Child + Send>,
}

impl Shell {
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let _ = self._master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

/// una tecla pulsada → bytes que espera un programa sobre un PTY
/// (readline, tmux y cualquier TUI: htop, ranger…)
pub(crate) fn key_bytes(k: KeyEvent) -> Vec<u8> {
    key_bytes_mode(k, false)
}

/// igual que `key_bytes`, pero `app_cursor` (DECCKM) hace que las flechas
/// viajen como SS3 (ESC O A…) como pide un programa en modo aplicación
pub(crate) fn key_bytes_mode(k: KeyEvent, app_cursor: bool) -> Vec<u8> {
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

pub(crate) fn send_key_to_shell(writer: &mut Box<dyn Write + Send>, k: KeyEvent) -> Result<()> {
    let bytes = key_bytes(k);
    if !bytes.is_empty() {
        writer.write_all(&bytes)?;
        writer.flush()?;
    }
    Ok(())
}

/// secuencias CSI del mini-emulador del panel shell: readline las usa al
/// editar (Delete → DCH, Ctrl+U → EL, flechas → CUF/CUB…); sin
/// interpretarlas, los caracteres que se borraban se quedaban pintados
pub(crate) fn csi_shell(
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
pub(crate) fn draw_shell_output(app: &App, fr: &mut Frame, bx: u16, y0: u16, rows: u16, area_w: u16) {
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
