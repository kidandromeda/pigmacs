// pigmacs — editor multimodal org con shell PTY, panel IA y programa integrado
mod ai;
mod app;
mod browser;
mod editor;
mod frame;
mod program;
mod shell;
mod term;
mod ui;

use ai::*;
use app::*;
use browser::*;
use editor::*;
use frame::*;
use program::*;
use shell::*;
use ui::*;

use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        atomic::AtomicBool,
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};


pub(crate) fn main() -> Result<()> {
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

/// ¿estamos en una consola de texto real (/dev/tty1…) y no en pts (tmux,
/// xterm, ssh)? El printk del kernel sólo se pinta en la consola de texto.
pub(crate) fn running_on_vt() -> bool {
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
pub(crate) fn lower_kernel_printk() -> Option<String> {
    let orig = fs::read_to_string("/proc/sys/kernel/printk").ok()?;
    let first = orig.split_whitespace().next()?.to_string();
    fs::write("/proc/sys/kernel/printk", "1\n").ok()?;
    Some(first)
}

pub(crate) fn run(app: &mut App) -> Result<()> {
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
pub(crate) enum Shortcut {
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
pub(crate) fn shortcut_of(k: KeyEvent) -> Option<Shortcut> {
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
pub(crate) fn shortcut(app: &mut App, s: Shortcut) -> Result<Option<bool>> {
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
pub(crate) fn resize_border(app: &mut App, delta: isize) -> Result<()> {
    let (c, r) = terminal::size()?;
    let min = 3isize;
    let max = (r as isize).saturating_sub(5).max(3);
    app.shell_rows = (app.shell_rows as isize + delta).clamp(min, max) as u16;
    app.shell.resize(app.shell_rows, c);
    app.status = format!("Borde movido · shell {} líneas", app.shell_rows);
    Ok(())
}

pub(crate) fn handle_help_key(app: &mut App, k: KeyEvent) {
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

pub(crate) fn handle_sudo_key(app: &mut App, k: KeyEvent) {
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
