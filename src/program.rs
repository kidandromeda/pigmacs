// ————— programa lanzado en la ventana principal (menú Ctrl+P) —————
use crate::{
    app::{layout, App, Mode},
    browser::browser_width,
    frame::{Frame, Style},
    shell::key_bytes_mode,
    term::Term,
};
use anyhow::Result;
use crossterm::{
    event::{KeyCode, KeyEvent},
    style::Color,
    terminal,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

/// programas disponibles en el menú (se comprueba el PATH al dibujarlo)
pub(crate) const MENU: [&str; 14] = [
    "htop", "btop", "fastfetch", "ncdu", "gdu", "nmtui", "ranger", "gitui",
    "tmux", "neovim", "bluetui", "jeeves", "lynx", "w3m",
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

/// programas del menú que están instalados en el PATH
pub(crate) fn menu_entries() -> Vec<&'static str> {
    MENU.iter().copied().filter(|m| in_path(m)).collect()
}

/// lanza `name` sobre su propio PTY ocupando el área del editor
pub(crate) fn launch_program(app: &mut App, name: &str) -> Result<()> {
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

/// teclas del menú: j/k o flechas mueven · Enter lanza · Esc/q cierra
pub(crate) fn handle_menu_key(app: &mut App, k: KeyEvent) {
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
pub(crate) fn draw_menu(app: &mut App, bx: u16, area_w: u16, rows: u16, fr: &mut Frame) {
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
