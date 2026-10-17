// ————— pigmacs · lanzador de aplicaciones gráficas (Ctrl+X) —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-17
//
// Lee los .desktop del sistema (XDG) y los ofrece en un popup difuso igual
// que el menú Ctrl+P. Al elegir uno:
//
//   · con sesión gráfica (DISPLAY / WAYLAND_DISPLAY) el comando va tal cual
//     al shell embebido;
//   · sin ella (tty real) el comando se lanza con `startx <binario>`, que
//     arranca X en la tty actual con la app como cliente: lo mismo que
//     teclear `startx /usr/bin/brave-browser` a mano.
//
// Sin dependencias nuevas: el parser de .desktop es nuestro.
use crate::{
    app::{App, Mode},
    frame::Frame,
    program::{draw_popup, launch_program, subseq, which},
    shell::run_in_shell,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// una aplicación del sistema, sacada de su fichero .desktop
#[derive(Clone)]
pub(crate) struct AppEntry {
    pub(crate) name: String,     // Nombre (lo que se muestra y se filtra)
    pub(crate) exec: String,     // Exec crudo (con códigos de campo)
    pub(crate) generic: String,  // GenericName
    pub(crate) keywords: String, // Keywords (para el filtro difuso)
    pub(crate) icon: Option<String>,
    pub(crate) terminal: bool, // Terminal=true: correr en la ventana principal
    pub(crate) file: PathBuf,  // ruta del .desktop (%k y depuración)
}

// ————— escritorio: lectura de los .desktop —————

/// directorios XDG de datos (usuario primero: sus .desktop pisan los del sistema)
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let home = env::var_os("HOME").map(PathBuf::from);
    let local = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(l) = local {
        dirs.push(l.join("applications"));
    }
    let sys = env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in sys.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs
}

/// lista las aplicaciones del sistema, ordenadas por nombre y sin duplicados
/// (un .desktop del usuario con el mismo nombre tapa al del sistema)
pub(crate) fn desktop_apps() -> Vec<AppEntry> {
    let mut por_nombre: HashMap<String, AppEntry> = HashMap::new();
    for dir in data_dirs() {
        for path in desktop_files(&dir, 0) {
            if let Some(e) = parse_desktop(&path) {
                por_nombre.insert(
                    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    e,
                );
            }
        }
    }
    let mut v: Vec<AppEntry> = por_nombre.into_values().collect();
    v.sort_by_key(|e| e.name.to_lowercase());
    v
}

/// los .desktop de un directorio (y de sus subdirectorios, p. ej.
/// kde/, gnome/): el spec permite organizarlos así
fn desktop_files(dir: &Path, nivel: u8) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if nivel > 2 {
        return out;
    }
    let Ok(leidos) = fs::read_dir(dir) else { return out };
    for entrada in leidos.flatten() {
        let p = entrada.path();
        if p.is_dir() {
            out.extend(desktop_files(&p, nivel + 1));
        } else if p.extension().map(|e| e == "desktop").unwrap_or(false) {
            out.push(p);
        }
    }
    out
}

/// parsea un .desktop: sólo el grupo [Desktop Entry] y sólo Type=Application
fn parse_desktop(path: &Path) -> Option<AppEntry> {
    let texto = fs::read_to_string(path).ok()?;
    let mut en_grupo = false;
    let mut campos: HashMap<String, String> = HashMap::new();
    for linea in texto.lines() {
        let linea = linea.trim_end();
        if linea.starts_with('[') {
            en_grupo = linea == "[Desktop Entry]";
            continue;
        }
        if !en_grupo || linea.is_empty() || linea.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = linea.split_once('=') {
            campos.insert(k.trim().to_string(), desescapar(v));
        }
    }
    let tipo = campos.get("Type").map(|s| s.as_str()).unwrap_or("Application");
    if tipo != "Application" {
        return None;
    }
    if es_si(campos.get("Hidden")) || es_si(campos.get("NoDisplay")) {
        return None;
    }
    // OnlyShowIn / NotShowIn: sólo se lista en los escritorios compatibles
    // (en una tty no hay XDG_CURRENT_DESKTOP, así que OnlyShowIn se oculta)
    let escritorio = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    if let Some(o) = campos.get("OnlyShowIn") {
        if !en_lista(o, &escritorio) {
            return None;
        }
    }
    if let Some(n) = campos.get("NotShowIn") {
        if en_lista(n, &escritorio) {
            return None;
        }
    }
    // TryExec: si el binario no está instalado, la entrada no sirve
    if let Some(t) = campos.get("TryExec") {
        if resolve_exe(t.trim()).is_none() {
            return None;
        }
    }
    let exec = campos.get("Exec")?.trim().to_string();
    if exec.is_empty() {
        return None;
    }
    let name = campos
        .get("Name")
        .cloned()
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    Some(AppEntry {
        name,
        exec,
        generic: campos.get("GenericName").cloned().unwrap_or_default(),
        keywords: campos.get("Keywords").cloned().unwrap_or_default(),
        icon: campos.get("Icon").cloned(),
        terminal: es_si(campos.get("Terminal")),
        file: path.to_path_buf(),
    })
}

/// los escapes del spec: \s \n \t \r y \\
fn desescapar(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn es_si(v: Option<&String>) -> bool {
    matches!(v.map(|s| s.trim()), Some("true") | Some("True") | Some("1"))
}

/// ¿el escritorio actual está en la lista `;`-separada del .desktop?
fn en_lista(lista: &str, escritorio: &str) -> bool {
    if escritorio.is_empty() {
        return false;
    }
    // XDG_CURRENT_DESKTOP puede ser "ubuntu:GNOME"
    let mios: Vec<&str> = escritorio.split(':').filter(|s| !s.is_empty()).collect();
    lista
        .split(';')
        .filter(|s| !s.is_empty())
        .any(|d| mios.contains(&d.trim().to_lowercase().as_str()))
}

// ————— filtro difuso —————

/// texto contra el que se filtra: nombre, genérico, palabras clave y binario
fn pajar(e: &AppEntry) -> String {
    let binario = e.exec.split_whitespace().next().unwrap_or("");
    format!("{} {} {} {}", e.name, e.generic, e.keywords, binario).to_lowercase()
}

/// entradas que coinciden con lo tecleado (vacío = todas)
pub(crate) fn filtered<'a>(apps: &'a [AppEntry], query: &str) -> Vec<&'a AppEntry> {
    let q: String = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if q.is_empty() {
        return apps.iter().collect();
    }
    apps.iter().filter(|e| subseq(&pajar(e), &q)).collect()
}

// ————— comando: .desktop → argv —————

/// parte la línea Exec en argumentos (comillas del spec: "a b" es uno solo)
fn tokenizar_exec(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut empezado = false;
    let mut entre_comillas = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                entre_comillas = !entre_comillas;
                empezado = true; // "" es un argumento vacío, no un separador
            }
            '\\' => {
                if let Some(o) = chars.next() {
                    cur.push(o);
                    empezado = true;
                }
            }
            c if c.is_whitespace() && !entre_comillas => {
                if empezado {
                    out.push(std::mem::take(&mut cur));
                    empezado = false;
                }
            }
            c => {
                cur.push(c);
                empezado = true;
            }
        }
    }
    if empezado {
        out.push(cur);
    }
    out
}

/// códigos de campo del spec: %f/%u… se descartan (no tenemos fichero que
/// abrir), %c es el nombre, %k el .desktop, %i pasa a --icon y %% es literal
fn expandir_campo(t: &str, e: &AppEntry) -> Vec<String> {
    match t {
        "%i" => match &e.icon {
            Some(i) if !i.is_empty() => vec!["--icon".into(), i.clone()],
            _ => vec![],
        },
        "%c" => vec![e.name.clone()],
        "%k" => vec![e.file.to_string_lossy().into_owned()],
        "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%m" | "%v" => vec![],
        _ => {
            let mut s = String::new();
            let mut chars = t.chars();
            while let Some(c) = chars.next() {
                if c != '%' {
                    s.push(c);
                    continue;
                }
                match chars.next() {
                    Some('%') => s.push('%'),
                    Some(_) => {} // código de campo suelto: se ignora
                    None => s.push('%'),
                }
            }
            if s.is_empty() { vec![] } else { vec![s] }
        }
    }
}

/// la lista de argumentos lista para ejecutar de una entrada del launcher
pub(crate) fn argv_of(e: &AppEntry) -> Vec<String> {
    tokenizar_exec(&e.exec)
        .into_iter()
        .flat_map(|t| expandir_campo(&t, e))
        .collect()
}

/// resuelve el binario: ruta absoluta tal cual, nombre vía PATH
fn resolve_exe(nombre: &str) -> Option<PathBuf> {
    if nombre.contains('/') {
        let p = PathBuf::from(nombre);
        return if p.is_file() { Some(p) } else { None };
    }
    which(nombre)
}

// ————— cita para el shell —————

/// cita un token: simples comillas, y las comillas simples del valor se
/// escapan cerrando y reabriendo la comilla ('don\'t' → 'don'\''t')
fn comilla(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn comando(argv: &[String]) -> String {
    argv.iter().map(|a| comilla(a)).collect::<Vec<_>>().join(" ")
}

// ————— apertura y teclado —————

/// Ctrl+X: abre el lanzador con la lista fresca de .desktop
pub(crate) fn open_launcher(app: &mut App) {
    app.launcher_apps = desktop_apps();
    app.launcher_query.clear();
    app.launcher_cursor = 0;
    app.launcher_open = true;
    app.status = format!(
        "{} aplicaciones gráficas · escribe para filtrar · Esc cierra",
        app.launcher_apps.len()
    );
}

/// teclas del lanzador: escribir filtra (difuso) · j/k o flechas mueven ·
/// Enter lanza la elegida · Esc limpia el filtro y Esc cierra
///
/// Igual que el menú Ctrl+P: con el filtro vacío j/k navegan; una vez
/// empiezas a filtrar, j/k son letras y la navegación pasa a ↑↓.
pub(crate) fn handle_launcher_key(app: &mut App, k: KeyEvent) {
    let n = filtered(&app.launcher_apps, &app.launcher_query).len();
    let filtro_activo = !app.launcher_query.is_empty();
    let max = n.saturating_sub(1);
    match k.code {
        KeyCode::Esc => {
            if filtro_activo {
                app.launcher_query.clear();
                app.launcher_cursor = 0;
            } else {
                app.launcher_open = false;
                app.status = "Lanzador cerrado".into();
            }
        }
        KeyCode::Char('q') if !filtro_activo => {
            app.launcher_open = false;
            app.status = "Lanzador cerrado".into();
        }
        KeyCode::Down => app.launcher_cursor = (app.launcher_cursor + 1).min(max),
        KeyCode::Up => app.launcher_cursor = app.launcher_cursor.saturating_sub(1),
        KeyCode::Char('j') if !filtro_activo => app.launcher_cursor = (app.launcher_cursor + 1).min(max),
        KeyCode::Char('k') if !filtro_activo => app.launcher_cursor = app.launcher_cursor.saturating_sub(1),
        KeyCode::Home => app.launcher_cursor = 0,
        KeyCode::End => app.launcher_cursor = max,
        KeyCode::Backspace => {
            app.launcher_query.pop();
            app.launcher_cursor = 0;
        }
        KeyCode::Enter => {
            let elegida = {
                let entradas = filtered(&app.launcher_apps, &app.launcher_query);
                entradas.get(app.launcher_cursor).map(|e| (*e).clone())
            };
            if let Some(e) = elegida {
                app.launcher_open = false;
                app.launcher_query.clear();
                launch_app(app, &e);
            } else {
                app.launcher_open = false;
            }
        }
        KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            app.launcher_query.push(c);
            app.launcher_cursor = 0;
        }
        _ => {}
    }
}

// ————— lanzamiento —————

/// lanza la app elegida: las de terminal van a la ventana principal (como
/// Ctrl+P); las gráficas arrancan X con startx si no hay sesión gráfica
pub(crate) fn launch_app(app: &mut App, e: &AppEntry) {
    let argv = argv_of(e);
    if argv.is_empty() {
        app.status = format!("«{}»: el .desktop no tiene comando", e.name);
        return;
    }
    if e.terminal {
        let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        if let Err(err) = launch_program(app, &e.name, &refs) {
            app.status = format!("No se pudo lanzar «{}»: {err}", e.name);
        }
        return;
    }
    launch_gui(app, e, argv);
}

/// app gráfica: con sesión gráfica el comando va al shell embebido; sin
/// ella, `startx <binario>` (startx sólo acepta clientes con ruta absoluta
/// y sin argumentos, así que si los hay se genera un wrapper temporal)
fn launch_gui(app: &mut App, e: &AppEntry, argv: Vec<String>) {
    let Some(exe) = resolve_exe(&argv[0]) else {
        app.status = format!("«{}»: «{}» no está en el PATH", e.name, argv[0]);
        return;
    };
    let mut argv = argv;
    argv[0] = exe.to_string_lossy().into_owned();

    let en_x = !env::var("DISPLAY").unwrap_or_default().is_empty()
        || !env::var("WAYLAND_DISPLAY").unwrap_or_default().is_empty();

    let linea = if en_x {
        comando(&argv)
    } else if let Some(startx) = which("startx") {
        if argv.len() == 1 {
            format!("{} {}", comilla(&startx.to_string_lossy()), comilla(&argv[0]))
        } else {
            match wrapper(&argv) {
                Some(w) => format!("{} {}", comilla(&startx.to_string_lossy()), comilla(&w.to_string_lossy())),
                None => {
                    app.status = format!("«{}»: no se pudo escribir el wrapper temporal", e.name);
                    return;
                }
            }
        }
    } else {
        app.status = format!("«{}»: no hay startx (xinit) ni sesión gráfica", e.name);
        return;
    };

    if let Err(err) = run_in_shell(app, &linea) {
        app.status = format!("No se pudo lanzar «{}»: {err}", e.name);
        return;
    }
    // el comando se ve correr en el panel shell: Ctrl+T vuelve al modo anterior
    if app.mode != Mode::Shell {
        app.top_mode = app.mode;
        app.mode = Mode::Shell;
    }
    app.status = if en_x {
        format!("«{}» lanzado en la sesión gráfica", e.name)
    } else {
        format!("«{}» lanzado con startx · Ctrl+T vuelve al editor", e.name)
    };
}

/// script temporal ejecutable con el comando ya citado: startx lo usa como
/// cliente en lugar del binario cuando la app pide argumentos
fn wrapper(argv: &[String]) -> Option<PathBuf> {
    let ruta = env::temp_dir().join(format!("pigmacs-launch-{}.sh", std::process::id()));
    fs::write(&ruta, format!("#!/bin/sh\nexec {}\n", comando(argv))).ok()?;
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(&ruta, fs::Permissions::from_mode(0o755)).ok()?;
    Some(ruta)
}

// ————— dibujo —————

/// popup difuso con las aplicaciones: la ventana sigue al cursor (scroll),
/// se filtra al escribir y el pie recuerda las teclas
pub(crate) fn draw_launcher(app: &mut App, bx: u16, area_w: u16, rows: u16, fr: &mut Frame) {
    let entries = filtered(&app.launcher_apps, &app.launcher_query);
    let cur = app.launcher_cursor.min(entries.len().saturating_sub(1));

    // el popup no puede pasar de `rows - 1` filas: dentro caben la línea del
    // filtro, el pie, hasta dos indicadores de scroll (↑/↓) y las entradas
    let hueco = (rows as usize).saturating_sub(5); // filas del popup - bordes - filtro - pie
    let visibles = hueco.saturating_sub(2).max(1);
    // el cursor siempre a la vista: la ventana sigue al cursor (scroll)
    let primero = if cur >= visibles { cur + 1 - visibles } else { 0 };
    let ultimo = (primero + visibles).min(entries.len());

    let mut lines: Vec<(String, bool)> = Vec::new();
    lines.push((format!("> {}_", app.launcher_query), false));
    if primero > 0 {
        lines.push((format!("  ↑ {} antes", primero), false));
    }
    for (i, e) in entries[primero..ultimo].iter().enumerate() {
        let idx = primero + i;
        lines.push((
            format!("{}{}", if idx == cur { "▸ " } else { "  " }, e.name),
            idx == cur,
        ));
    }
    if ultimo < entries.len() {
        lines.push((format!("  ↓ {} más", entries.len() - ultimo), false));
    }
    if entries.is_empty() {
        lines.push((
            if app.launcher_query.is_empty() {
                "sin aplicaciones .desktop en el sistema".to_string()
            } else {
                format!("sin coincidencias para «{}»", app.launcher_query)
            },
            false,
        ));
    }
    lines.push(("Enter lanzar · Esc cierra".into(), false));

    let mut ancho: u16 = lines
        .iter()
        .map(|(s, _)| s.chars().count() as u16)
        .max()
        .unwrap_or(24)
        .saturating_add(4)
        .clamp(28, area_w.saturating_sub(2).max(28));
    // el título no puede salirse del popup
    ancho = ancho.max(" pigmacs · Aplicaciones gráficas ".chars().count() as u16 + 2);
    draw_popup(
        fr,
        bx,
        area_w,
        rows,
        " pigmacs · Aplicaciones gráficas ",
        ancho,
        &lines,
    );
}
