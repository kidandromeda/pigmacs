// ————— pigmacs · render: dibujo del frame completo —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::{
    ai::draw_ai_pane,
    app::{layout, App, Mode},
    browser::{browser_width, draw_browser},
    editor::{compute_matches, draw_editor_lines, org_line_style},
    frame::{render_frame, Frame, Style},
    launcher::draw_launcher,
    program::draw_menu,
    shell::draw_shell_output,
    theme::*,
};
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    queue,
    style::Color,
    terminal,
};
use std::io::{self, Write};

pub(crate) const HELP_TEXT: &str = r#"PIGMACS · MANUAL COMPLETO
═══════════════════════════
ATAJOS EN DOS GRUPOS
  Ctrl+<letra>  grupo TTY — funciona en cualquier consola de texto
  Alt+<letra>   grupo GUI — para terminales en entornos gráficos
  (misma letra = misma acción; Ctrl+B queda libre para tmux)
  en modo SHELL las teclas van al programa de dentro: Ctrl+B (prefijo
  tmux), Ctrl+A/E/K/R (readline) se respetan; solo Ctrl+Q, Ctrl+S,
  Ctrl+Enter, Ctrl+J y Shift+Tab siguen siendo de pigmacs
  en modo PROG (programa lanzado con Ctrl+P) sólo Ctrl/Alt+Q, S, P, G, X y
  Ctrl+J son de pigmacs; el resto de teclas ceden al programa en marcha

ACCIONES GLOBALES
  Ctrl+Q / Alt+Q     salir (avisa si hay cambios sin guardar)
  Ctrl+S / Alt+S     guardar (pide sudo si el fichero es de root)
  Ctrl+A / Alt+A     alternar editor <-> panel IA
  Ctrl+F / Alt+F     mostrar/ocultar y enfocar el tree
  Ctrl+T / Alt+T     terminal: ir y salir (vuelve al modo anterior)
  Ctrl+V / Alt+V     volver al editor
  Ctrl+J             volver al editor desde el shell/programa
                     (escape para tty reales: Shift+Tab no llega allí)
  Ctrl+E / Ctrl+Enter enviar selección (o línea entera) al shell
  Ctrl+R / Alt+R     rehacer
  Ctrl+K / Alt+K     limpiar la pantalla de la terminal
  Ctrl+U / Ctrl+D    subir/bajar el borde (más terminal / más editor)
  Ctrl+N / Alt+N     nueva conversación IA (dentro del panel IA)
  Ctrl+P / Alt+P     menú para lanzar programas en la ventana principal
  Ctrl+X / Alt+X     lanzador de aplicaciones gráficas (.desktop del sistema)
  Ctrl+G / Alt+G     cerrar el programa en marcha
  F1 / ?             esta ayuda (en cualquier panel, el shell incluido)
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
  el cursor se ve como un bloque de color en la línea del prompt

TREE DE FICHEROS
  j k                 mover · l/Enter abrir · h subir
  g / G               principio / fin
  b                   marcar/desmarcar el directorio actual (★)
  B                   saltar al siguiente marcador (★N en la cabecera)
  Esc                 volver al editor (el tree sigue visible)
  los directorios marcados se ven con ★ en el árbol y la cabecera
  cuenta cuántos hay (★3); sirven para saltar por tus sitios
  el cwd del proceso sigue al tree: gdu, ncdu… arrancan en el dir mirado

PROGRAMAS EN LA VENTANA PRINCIPAL (Ctrl+P)
  escribir            filtra el menú (difuso): "htp" encuentra htop;
                      Esc limpia el filtro y Esc de nuevo cierra
  j k / ↑↓            mover por el menú · Enter lanzar · Esc/q cerrar
  Enter sin matches   lanza lo tecleado tal cual (watch, yazi, iotop…)
  (sólo aparecen los instalados en el PATH: htop, btop, fastfetch,
   ncdu, gdu, nmtui, ranger, Midnight Commander, gitui, tmux, neovim,
   bluetui, jeeves, lynx, w3m)
   (neovim se lanza como nvim; Midnight Commander con mc; lynx y w3m
   abren www.duckduckgo.com)
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

LANZADOR DE APLICACIONES GRÁFICAS (Ctrl+X)
  lista los .desktop de /usr/share/applications, /usr/local/share y
  ~/.local/share/applications (los tuyos pisan los del sistema)
  escribir filtra (difuso): "brv" encuentra a Brave; j k / ↑↓ eligen;
  Enter lanza · Esc limpia el filtro y Esc de nuevo cierra
  sin sesión gráfica pigmacs lanza "startx /ruta/al/binario" él mismo,
  heredando la tty real (startx ve tty=/dev/ttyN y le pasa «-keeptty» a
  Xorg), igual que si lo teclearas a mano: X ocupa la consola y, al
  cerrar la app, vuelve pigmacs con la pantalla repintada; los errores
  de X se ven en la consola; si ya hay DISPLAY, el comando va al shell
  embebido
  con X en la consola, Ctrl+G (Alt+G) mata la app: SIGTERM a startx y a
  todo su árbol (Xorg incluido); si insiste, otra vez Ctrl+G = SIGKILL.
  Ojo: dentro de X el teclado es del servidor; si la consola no entrega
  la tecla a pigmacs, no llegará y habrá que cerrar la app desde ella
  los argumentos del .desktop y los códigos de campo (%f, %u, %c, %k,
  %i, %%) se resuelven (si la app pide argumentos se genera un wrapper
  temporal: startx sólo admite un cliente sin argumentos)
  las .desktop con Terminal=true se abren en la ventana principal,
  igual que las del menú Ctrl+P
  si no hay startx (xinit) ni DISPLAY, lo dice en el status

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

COLOR · ASPECTO (tty real, 16 colores)
  la barra inferior lleva el modo en una píldora de color y, a la
  derecha, el fichero y la posición; encima, la ayuda del modo activo
  cabeceras org: nivel 1 cyan · 2 verde · 3 amarillo · más magenta
  TODO rojo · DONE verde · bloques #+BEGIN_SRC con resaltado de
  sintaxis (python, bash, rust, c, js: keywords, cadenas, comentarios)
  la IA avisa con un bell al terminar la consulta y su cabecera
  cambia de color según el estado (amarillo = procesando, verde = ok)

GENERAL
  Ctrl+Q              salir (avisa si hay cambios sin guardar)
  Ctrl+S              guardar
  Ctrl+K              limpiar la terminal
  Ctrl+F / Ctrl+T     tree de ficheros / terminal (T: ir y salir)
  Ctrl+P / Ctrl+G     menú de programas / cerrar programa en marcha
  Ctrl+X              lanzador de aplicaciones gráficas (startx)
"#;

pub(crate) fn draw_help(app: &mut App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
    let lines: Vec<&str> = HELP_TEXT.lines().collect();
    let max = editor_rows.saturating_sub(1) as usize;
    let start = app.help_scroll.min(lines.len().saturating_sub(max.max(1)));
    let end = (start + max).min(lines.len());
    for (i, l) in lines[start..end].iter().enumerate() {
        let clean: String = l.chars().take(area_w as usize).collect();
        // título · separador decorativo · sección · tecla+descripción
        if i == 0 {
            fr.text(bx, i as u16, &clean, bold(ACCENT_HI));
            continue;
        }
        if clean.starts_with('═') {
            fr.text(bx, i as u16, &clean, fg(ACCENT));
            continue;
        }
        if !clean.starts_with(' ') {
            // cabecera de sección (sin sangría, en mayúsculas)
            fr.text(bx, i as u16, &clean, bold(ACCENT_HI));
            continue;
        }
        // línea con sangría: la tecla (hasta la primera doble separación)
        // en color y la descripción en gris, legible de un vistazo
        let bytes = clean.as_bytes();
        let indent = bytes.iter().take_while(|&&b| b == b' ').count();
        let mut key_end = bytes.len();
        if indent < bytes.len() {
            let mut seen_gap = false;
            let mut j = indent;
            while j < bytes.len() {
                if bytes[j] == b' ' {
                    if seen_gap {
                        key_end = j;
                        break;
                    }
                    // doble espacio = fin de la columna de teclas
                    if j + 1 < bytes.len() && bytes[j + 1] == b' ' {
                        key_end = j;
                        break;
                    }
                    seen_gap = true;
                } else {
                    seen_gap = false;
                }
                j += 1;
            }
        }
        // cortar siempre en frontera de char (HELP_TEXT tiene ←↓↑→, ★…)
        while key_end > 0 && !clean.is_char_boundary(key_end) {
            key_end -= 1;
        }
        let (key, desc) = clean.split_at(key_end);
        fr.text(bx, i as u16, key, fg(WARN));
        fr.text(
            bx + key.chars().count() as u16,
            i as u16,
            desc,
            fg(FG),
        );
    }
}

/// barra de ayuda inferior: la tecla de cada trozo en acento y la
/// descripción en gris — la ayuda del modo activo, siempre visible.
/// Todos los cálculos van con saturating_sub: en modo debug un
/// `limit - cx` negativo sería un pánico (rompió el smoke al hacer resize)
fn draw_keybar(fr: &mut Frame, x: u16, y: u16, s: &str, w: u16) {
    let mut cx = x;
    let limit = w;
    for (i, raw) in s.split('·').enumerate() {
        if cx >= limit {
            break;
        }
        if i > 0 {
            if limit.saturating_sub(cx) < 2 {
                break;
            }
            fr.text(cx, y, "· ", fg(DIM));
            cx += 2;
            if cx >= limit {
                break;
            }
        }
        let part = if i == 0 { raw } else { raw.trim_start() };
        let (head, rest) = match part.find(' ') {
            Some(p) => part.split_at(p),
            None => (part, ""),
        };
        // "escribe", "teclas", "historial"… son descripciones, no teclas
        let es_descripcion =
            head.chars().all(|c| c.is_alphabetic()) && head.chars().count() >= 4;
        let st = if es_descripcion { fg(DIM) } else { bold(ACCENT_HI) };
        let head: String = head
            .chars()
            .take(limit.saturating_sub(cx) as usize)
            .collect();
        fr.text(cx, y, &head, st);
        cx = cx.saturating_add(head.chars().count() as u16);
        if !rest.is_empty() && cx < limit {
            let rest: String = rest
                .chars()
                .take(limit.saturating_sub(cx) as usize)
                .collect();
            fr.text(cx, y, &rest, fg(DIM));
            cx = cx.saturating_add(rest.chars().count() as u16);
        }
    }
}

pub(crate) fn draw(app: &mut App) -> Result<()> {
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

    // lanzador de aplicaciones gráficas: el mismo popup, encima de todo
    if app.launcher_open {
        draw_launcher(app, bx, area_w, editor_rows, &mut fr);
    }

    // separador del shell (solo a la derecha del navegador)
    fr.text(
        bx,
        shell_top,
        &"─".repeat(area_w as usize),
        fg(ACCENT),
    );

    // salida del shell: últimas líneas con colores ANSI y \r/\b interpretados
    let max_rows = h.saturating_sub(4).saturating_sub(shell_top) as usize;
    let available = (shell_rows.saturating_sub(2) as usize).min(max_rows);
    draw_shell_output(app, &mut fr, bx, shell_top + 1, available as u16, area_w);

    // ---------- barra de estado: píldora del modo · aviso · identidad y
    // posición a la derecha (siempre visibles, estilo btop) ----------
    let (mode, pill_bg, pill_fg) = match app.mode {
        Mode::Normal => ("NORMAL", Color::Blue, Color::White),
        Mode::Insert => ("INSERT", Color::Green, Color::Black),
        Mode::Visual => ("VISUAL", Color::Magenta, Color::White),
        Mode::Shell => ("SHELL", Color::DarkCyan, Color::White),
        Mode::Ai => ("AI", Color::DarkMagenta, Color::White),
        Mode::Browser => ("BROWSE", Color::DarkBlue, Color::White),
        Mode::Program => ("PROG", Color::DarkRed, Color::White),
    };
    let mut x = 0u16;
    let pill_text = format!(" {mode} ");
    for ch in pill_text.chars() {
        fr.put(x, status_y, ch, pill(pill_bg, pill_fg));
        x += 1;
    }
    // cambios sin guardar: punto rojo (o espacio para el separador)
    fr.put(
        x,
        status_y,
        if app.dirty { '●' } else { ' ' },
        if app.dirty { bold(ERR) } else { Style::default() },
    );
    x += 1;

    // texto editable en status: contraseña sudo o búsqueda (con su cursor)
    // o el mensaje del programa
    let msg: String = if app.sudo_prompt {
        format!(
            "Contraseña sudo ({}): {}",
            app.path.display(),
            "*".repeat(app.sudo_pass.chars().count())
        )
    } else if app.searching {
        format!("/{}", app.search_query)
    } else {
        format!("{}", app.status)
    };

    // información de la derecha: identidad · fichero · posición
    let right: Vec<(String, Style)> = if app.sudo_prompt || app.searching {
        Vec::new()
    } else {
        let file = app
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pos = match app.mode {
            Mode::Normal | Mode::Insert | Mode::Visual => {
                format!("{}:{}", app.row + 1, app.col + 1)
            }
            Mode::Browser => format!(
                "{}/{}",
                app.browser.cursor + 1,
                app.browser.entries.len().max(1)
            ),
            _ => String::new(),
        };
        let mut v = vec![
            (
                format!("pigmacs v{}", env!("CARGO_PKG_VERSION")),
                bold(ACCENT_HI),
            ),
            ("│".to_string(), fg(ACCENT)),
            (file, fg(DIM)),
        ];
        if !pos.is_empty() {
            v.push(("│".to_string(), fg(ACCENT)));
            v.push((pos, fg(DIM)));
        }
        v
    };
    let right_w: u16 = 1
        + right
            .iter()
            .map(|(s, _)| s.chars().count() as u16 + 1)
            .sum::<u16>();

    // el aviso se recorta si hace falta: la identificación de la derecha
    // (fichero y posición) es información siempre útil, estilo btop
    let free = w.saturating_sub(right_w).saturating_sub(x);
    let shown: String = msg.chars().take(free as usize).collect();
    fr.text(x, status_y, &shown, fg(FG));
    let cur_x = x + shown.chars().count() as u16;
    let mut rx = w.saturating_sub(right_w).saturating_add(1);
    for (s, st) in &right {
        fr.text(rx, status_y, &s, *st);
        rx += s.chars().count() as u16 + 1;
    }

    // barra de ayuda permanente: atajos del modo activo, siempre visibles
    let help = match app.mode {
        Mode::Normal | Mode::Insert | Mode::Visual => {
            "i · u deshacer · Ctrl+R rehacer · Tab plegar · Ctrl+F tree · Ctrl+T terminal · Ctrl+E enviar · Ctrl+S guardar · F1 ayuda · Ctrl+Q salir"
        }
        Mode::Browser => {
            "j/k mover · l/h entrar/subir · b marca ★ · B saltar por marcados · g/G extremos · Esc editor · Ctrl+F ocultar"
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
    draw_keybar(
        &mut fr,
        0,
        status_y.saturating_sub(1),
        help,
        w,
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
        let bold = match org_line_style(app, app.row) {
            Some((_, b)) => b,
            None => false,
        };
        // cursor del modo: celda en color de acento dentro del frame
        fr.put(
            x,
            y,
            ch,
            Style {
                fg: Some(Color::Black),
                bg: Some(ACCENT_HI),
                bold,
                ..Default::default()
            },
        );
    }

    // diff con el frame anterior: solo se reescriben las celdas que cambiaron
    let force = std::mem::take(&mut app.force_full);
    render_frame(&mut app.prev, &fr, &mut out, force)?;

    // bell pendiente (fin de la consulta IA): suena en la tty sin mirar
    if std::mem::take(&mut app.bell) {
        queue!(out, crossterm::style::Print('\x07'))?;
    }

    // cursor real del terminal según el modo
    if app.sudo_prompt || app.searching {
        queue!(out, Show, MoveTo(cur_x.min(w.saturating_sub(1)), status_y))?;
    } else if app.mode == Mode::Ai {
        let input_row = editor_rows.saturating_sub(1);
        let x = (bx + (2 + app.ai_input.chars().count()) as u16).min(area_w.saturating_sub(1));
        queue!(out, Show, MoveTo(x, input_row))?;
    } else if app.mode == Mode::Shell {
        // el cursor del shell va pintado como bloque dentro del frame, en la
        // misma celda en la que escribe el prompt (si no, el real se quedaba
        // quieto en la esquina inferior izquierda y se escribía a ciegas)
        queue!(out, Hide, MoveTo(0, 0))?;
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
