// ————— pigmacs · programa lanzado en la ventana principal (menú Ctrl+P) —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::{
    app::{layout, App, Mode},
    browser::browser_width,
    frame::Frame,
    shell::key_bytes_mode,
    term::Term,
    theme::*,
};
use anyhow::Result;
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
    terminal,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

/// programas del menú: (nombre visible, argv de lanzamiento).
/// El filtro de PATH se hace con `argv[0]` al dibujarlo.
pub(crate) const MENU: [(&str, &[&str]); 15] = [
    ("htop", &["htop"]),
    ("btop", &["btop"]),
    ("fastfetch", &["fastfetch"]),
    ("ncdu", &["ncdu"]),
    ("gdu", &["gdu"]),
    ("nmtui", &["nmtui"]),
    ("ranger", &["ranger"]),
    ("Midnight Commander", &["mc"]),
    ("gitui", &["gitui"]),
    ("tmux", &["tmux"]),
    ("neovim", &["nvim"]),
    ("bluetui", &["bluetui"]),
    ("jeeves", &["jeeves"]),
    ("lynx", &["lynx", "www.duckduckgo.com"]),
    ("w3m", &["w3m", "www.duckduckgo.com"]),
];

/// ¿está el programa en el PATH y es ejecutable?
pub(crate) fn in_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(path) = env::var("PATH") else { return false };
    path.split(':').any(|d| {
        !d.is_empty()
            && fs::metadata(std::path::Path::new(d).join(name))
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    })
}

/// programa en ejecución sobre su propio PTY, dibujado en el área del editor
pub(crate) struct Program {
    pub(crate) name: String,
    pub(crate) writer: Box<dyn Write + Send>,
    // se conservan a propósito: mantienen el PTY abierto y al hijo vivo
    pub(crate) master: Box<dyn MasterPty + Send>,
    pub(crate) child: Box<dyn Child + Send>,
    pub(crate) output: Arc<Mutex<Vec<u8>>>,
    pub(crate) term: Term,
}

impl Program {
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
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
    pub(crate) fn send_key(&mut self, k: KeyEvent) -> Result<()> {
        let b = key_bytes_mode(k, self.term.app_cursor);
        if !b.is_empty() {
            self.writer.write_all(&b)?;
            self.writer.flush()?;
        }
        Ok(())
    }

    /// `Some(descripción)` si el programa ya terminó
    pub(crate) fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(st)) => Some(format!("{st:?}")),
            _ => None,
        }
    }
}

/// programas del menú instalados en el PATH: (visible, argv)
pub(crate) fn menu_entries() -> Vec<(&'static str, &'static [&'static str])> {
    MENU.iter().copied().filter(|(_, argv)| in_path(argv[0])).collect()
}

/// ¿contiene `q` como subsecuencia? (filtro difuso: "htp" → "htop")
fn subseq(hay: &str, q: &str) -> bool {
    let mut it = hay.chars();
    q.chars().all(|c| it.any(|h| h == c))
}

/// entradas del menú filtradas por lo tecleado (vacío = todas)
pub(crate) fn filtered_menu(query: &str) -> Vec<(&'static str, &'static [&'static str])> {
    let all = menu_entries();
    let q: String = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if q.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|(name, _)| subseq(&name.to_lowercase(), &q))
        .collect()
}

/// lanza `name` (nombre visible) ejecutando `argv` sobre su propio PTY,
/// ocupando el área del editor
pub(crate) fn launch_program(app: &mut App, name: &str, argv: &[&str]) -> Result<()> {
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
    let mut cmd = CommandBuilder::new(argv[0]);
    cmd.args(&argv[1..]);
    cmd.env("TERM", env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()));
    // el programa arranca en el dir del tree (allí está el cwd del proceso)
    cmd.cwd(&app.browser.cwd);
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
pub(crate) fn close_program(app: &mut App) {
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

/// teclas del menú: escribir filtra (difuso) · j/k o flechas mueven ·
/// Enter lanza (o lo tecleado, si no hay coincidencias) · Esc limpia/cierra
///
/// Con el filtro vacío j/k navegan (compatibilidad con los atajos
/// habituales); una vez empiezas a filtrar, j/k son letras del filtro y
/// la navegación pasa a las flechas ↑↓.
pub(crate) fn handle_menu_key(app: &mut App, k: KeyEvent) {
    let entries = filtered_menu(&app.menu_query);
    let filtro_activo = !app.menu_query.is_empty();
    match k.code {
        KeyCode::Esc => {
            if filtro_activo {
                app.menu_query.clear();
                app.menu_cursor = 0;
            } else {
                app.menu_open = false;
            }
        }
        KeyCode::Char('q') if !filtro_activo => app.menu_open = false,
        KeyCode::Down => {
            app.menu_cursor = (app.menu_cursor + 1).min(entries.len().saturating_sub(1));
        }
        KeyCode::Up => {
            app.menu_cursor = app.menu_cursor.saturating_sub(1);
        }
        KeyCode::Char('j') if !filtro_activo => {
            app.menu_cursor = (app.menu_cursor + 1).min(entries.len().saturating_sub(1));
        }
        KeyCode::Char('k') if !filtro_activo => {
            app.menu_cursor = app.menu_cursor.saturating_sub(1);
        }
        KeyCode::Backspace => {
            app.menu_query.pop();
            app.menu_cursor = 0;
        }
        KeyCode::Enter => {
            if let Some((name, argv)) = entries.get(app.menu_cursor).copied() {
                app.menu_open = false;
                app.menu_query.clear();
                // si ese programa ya está en marcha, sólo se recupera el foco
                // (así Ctrl+J al editor no lo pierde: Ctrl+P + Enter lo recupera)
                if app.program.as_ref().map(|p| p.name.as_str()) == Some(name) {
                    app.mode = Mode::Program;
                    app.status = format!("«{name}» en marcha · Ctrl+G lo cierra");
                } else if let Err(e) = launch_program(app, name, argv) {
                    app.status = format!("No se pudo lanzar {name}: {e}");
                }
            } else if !app.menu_query.trim().is_empty() {
                // sin coincidencias: lanzar lo tecleado tal cual
                let cmd = app.menu_query.trim().to_string();
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                app.menu_open = false;
                app.menu_query.clear();
                if let Err(e) = launch_program(app, &cmd, &parts) {
                    app.status = format!("No se pudo lanzar {cmd}: {e}");
                }
            } else {
                app.menu_open = false;
            }
        }
        KeyCode::Char(c)
            if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.menu_query.push(c);
            app.menu_cursor = 0;
        }
        _ => {}
    }
}

/// menú flotante en el centro del área principal: escribe para filtrar
/// (difuso), Enter lanza, Esc limpia y cierra
pub(crate) fn draw_menu(app: &mut App, bx: u16, area_w: u16, rows: u16, fr: &mut Frame) {
    let entries = filtered_menu(&app.menu_query);
    let cur = app.menu_cursor.min(entries.len().saturating_sub(1));
    let mut lines: Vec<(String, bool)> = Vec::new();
    // línea del filtro (solo si hay algo tecleado)
    if !app.menu_query.is_empty() {
        lines.push((format!("> {}_", app.menu_query), false));
    }
    if entries.is_empty() {
        if app.menu_query.is_empty() {
            lines.push(("sin programas en el PATH".into(), false));
        } else {
            lines.push((format!("sin coincidencias para «{}»", app.menu_query), false));
        }
    }
    for (i, (name, _)) in entries.iter().enumerate() {
        lines.push((format!("{}{name}", if i == cur { "▸ " } else { "  " }), i == cur));
    }
    let pie = if entries.is_empty() && !app.menu_query.is_empty() {
        format!("Enter lanza «{}» · Esc cierra", app.menu_query)
    } else {
        "Enter lanzar · Esc cierra".to_string()
    };
    lines.push((pie, false));

    let width = if app.menu_query.is_empty() {
        30u16.min(area_w.saturating_sub(2)).max(10)
    } else {
        (app.menu_query.chars().count() as u16 + 14)
            .min(area_w.saturating_sub(2))
            .max(24)
    };
    let height = (lines.len() as u16 + 2).min(rows.saturating_sub(1));
    let x = bx + area_w.saturating_sub(width) / 2;
    let y = rows.saturating_sub(height) / 2;

    // sombra sutil detrás del popup (una columna/fila en oscuro)
    for sy in (y + 1)..=(y + height) {
        fr.put(x + width, sy, ' ', fg(DIM));
    }
    for sx in (x + 1)..=(x + width) {
        fr.put(sx, y + height, ' ', fg(DIM));
    }

    let bd = fg(ACCENT);
    let title = " pigmacs · Lanzar programa ";
    let dashes = width.saturating_sub(2 + title.len() as u16);
    fr.text(x, y, &format!("┌{title}{}┐", "─".repeat(dashes as usize)), bold(ACCENT_HI));
    for (i, (line, is_cur)) in lines.iter().take(height.saturating_sub(2) as usize).enumerate() {
        let content: String = line.chars().take(width.saturating_sub(2) as usize).collect();
        let pad = width.saturating_sub(2).saturating_sub(content.chars().count() as u16);
        let row = y + 1 + i as u16;
        // fondo de la fila: selección en acento, resto en oscuro
        let st = if *is_cur {
            band(ACCENT_HI, Color::Black)
        } else {
            fg(FG)
        };
        for c in 0..width.saturating_sub(2) {
            fr.put(x + 1 + c, row, ' ', fg(FG));
        }
        fr.text(x + 1, row, &format!("{content}{}", " ".repeat(pad as usize)), st);
        if *is_cur {
            for c in content.chars().count() as u16..width.saturating_sub(2) {
                fr.put(x + 1 + c, row, ' ', band(ACCENT_HI, Color::Black));
            }
        }
        // la línea de ayuda/pie en gris
        if i + 1 == lines.len() && !*is_cur {
            fr.text(
                x + 1,
                row,
                &content,
                fg(DIM),
            );
        }
    }
    fr.text(
        x,
        y + height.saturating_sub(1),
        &format!("└{}┘", "─".repeat(width.saturating_sub(2) as usize)),
        bd,
    );
}
