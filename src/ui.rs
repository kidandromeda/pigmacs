// ————— render: dibujo del frame completo —————
use crate::{
    ai::draw_ai_pane,
    app::{layout, App, Mode},
    browser::{browser_width, draw_browser},
    editor::{compute_matches, draw_editor_lines, org_line_style},
    frame::{render_frame, Frame, Style},
    program::draw_menu,
    shell::draw_shell_output,
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
  en modo PROG (programa lanzado con Ctrl+P) sólo Ctrl/Alt+Q, S, P, G y
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

TREE DE FICHEROS
  j k                 mover · l/Enter abrir · h subir
  g / G               principio / fin
  b                   marcar/desmarcar el directorio actual
  B                   saltar al siguiente marcador (★ en el título)
  Esc                 volver al editor (el tree sigue visible)
  el cwd del proceso sigue al tree: gdu, ncdu… arrancan en el dir mirado

PROGRAMAS EN LA VENTANA PRINCIPAL (Ctrl+P)
  j k / ↑↓            mover por el menú · Enter lanzar · Esc/q cerrar
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
  Ctrl+F / Ctrl+T     tree de ficheros / terminal (T: ir y salir)
  Ctrl+P / Ctrl+G     menú de programas / cerrar programa en marcha
"#;

pub(crate) fn draw_help(app: &mut App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
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
