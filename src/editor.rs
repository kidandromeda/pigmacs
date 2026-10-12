// ————— editor de texto/org —————
use crate::{
    app::{App, Mode},
    frame::{Frame, Style},
};
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};

pub(crate) fn handle_editor_key(app: &mut App, k: KeyEvent) {
    // los Ctrl/Alt que no son atajos no deben ejecutar comandos del editor
    // (p. ej. Ctrl+X no puede borrar como si fuese la tecla x)
    if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return;
    }
    match app.mode {
        Mode::Normal => {
            // secuencias de dos teclas (gg, dd, z + a/o/c/r/m, m<x> marca, '<x> salto)
            let fold_seq = app.pending == Some('z')
                && matches!(
                    k.code,
                    KeyCode::Char('a')
                        | KeyCode::Char('o')
                        | KeyCode::Char('c')
                        | KeyCode::Char('r')
                        | KeyCode::Char('m')
                        | KeyCode::Char('n')
                );
            let mark_seq = matches!(app.pending, Some('m') | Some('\''))
                && matches!(k.code, KeyCode::Char(c) if c.is_ascii_lowercase());
            let repeatable = matches!(
                k.code,
                KeyCode::Char('g')
                    | KeyCode::Char('d')
                    | KeyCode::Char('z')
                    | KeyCode::Char('m')
                    | KeyCode::Char('\'')
            ) || fold_seq
                || mark_seq;
            if !repeatable {
                app.pending = None;
            }
            // el contador numérico se mantiene con dígitos y se consume en G/gg
            if !matches!(k.code, KeyCode::Char(c) if c.is_ascii_digit())
                && !matches!(k.code, KeyCode::Char('g') | KeyCode::Char('G'))
            {
                app.count = None;
            }
            match k.code {
                // marcas de vim: m<x> fija la posición · '<x> salta a ella
                KeyCode::Char(c) if app.pending == Some('m') && c.is_ascii_lowercase() => {
                    app.marks.insert(c, (app.row, app.col));
                    app.pending = None;
                    app.status = format!("Marca '{}' establecida", c);
                }
                KeyCode::Char(c) if app.pending == Some('\'') && c.is_ascii_lowercase() => {
                    if let Some(&(r, cc)) = app.marks.get(&c) {
                        app.row = r;
                        app.col = cc;
                        app.reveal_row(app.row);
                        app.ensure_visible();
                        app.status = format!("Saltando a la marca '{}'", c);
                    } else {
                        app.status = format!("La marca '{}' no existe", c);
                    }
                    app.pending = None;
                }
                // contador numérico: 12G → línea 12 · 5gg → línea 5
                KeyCode::Char(d) if d.is_ascii_digit() && (app.count.is_some() || d != '0') => {
                    let n = (d as u8 - b'0') as usize;
                    app.count = Some(app.count.unwrap_or(0) * 10 + n);
                }
                KeyCode::Char('h') | KeyCode::Left => app.move_cursor(0, -1),
                KeyCode::Char('j') | KeyCode::Down => app.move_cursor(1, 0),
                KeyCode::Char('k') | KeyCode::Up => app.move_cursor(-1, 0),
                KeyCode::Char('l') | KeyCode::Right => app.move_cursor(0, 1),
                KeyCode::Tab => {
                    let r = app.row;
                    app.toggle_fold(r);
                    app.pending = None;
                }
                KeyCode::Char('a')
                | KeyCode::Char('o')
                | KeyCode::Char('c')
                | KeyCode::Char('r')
                | KeyCode::Char('m')
                | KeyCode::Char('n')
                    if app.pending == Some('z') =>
                {
                    // plegado: za alternar · zc cerrar · zo abrir · zr todo abierto · zm todo cerrado · zn números
                    let r = app.row;
                    match k.code {
                        KeyCode::Char('a') => app.toggle_fold(r),
                        KeyCode::Char('c') => {
                            if app.header_level(r).is_some() && !app.folded.contains(&r) {
                                app.folded.insert(r);
                                app.status = "Cabecera plegada".into();
                            }
                        }
                        KeyCode::Char('o') => {
                            if app.folded.contains(&r) {
                                app.folded.remove(&r);
                                app.status = "Cabecera abierta".into();
                            }
                        }
                        KeyCode::Char('r') => app.unfold_all(),
                        KeyCode::Char('m') => app.fold_all(),
                        KeyCode::Char('n') => {
                            app.show_numbers = !app.show_numbers;
                            app.status = if app.show_numbers {
                                "Números de línea: sí".into()
                            } else {
                                "Números de línea: no".into()
                            };
                        }
                        _ => {}
                    }
                    app.pending = None;
                }
                KeyCode::Char('g') if app.pending == Some('g') => {
                    let target = app
                        .count
                        .map(|c| c.saturating_sub(1))
                        .unwrap_or_else(|| app.first_visible());
                    app.row = target.min(app.last_visible());
                    app.col = 0;
                    app.count = None;
                    app.pending = None;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('g') => app.pending = Some('g'),
                KeyCode::Char('G') => {
                    let last = app.last_visible();
                    app.row = app
                        .count
                        .map(|c| c.saturating_sub(1).min(last))
                        .unwrap_or(last);
                    app.col = 0;
                    app.count = None;
                    app.pending = None;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('0') | KeyCode::Home => {
                    app.col = 0;
                    app.ensure_visible();
                }
                KeyCode::Char('$') | KeyCode::End => {
                    app.col = app.line_len().saturating_sub(1);
                    app.ensure_visible();
                }
                KeyCode::Char('z') => app.pending = Some('z'),
                KeyCode::Char('m') => app.pending = Some('m'),
                KeyCode::Char('\'') => app.pending = Some('\''),
                KeyCode::Char('d') if app.pending == Some('d') => {
                    app.delete_line();
                    app.pending = None;
                }
                KeyCode::Char('d') => app.pending = Some('d'),
                KeyCode::Char('x') => app.delete_char_at_cursor(),
                KeyCode::Char('u') => app.undo_edit(),
                KeyCode::Char('i') => app.mode = Mode::Insert,
                KeyCode::Char('a') => {
                    if app.col < app.line_len() {
                        app.col += 1;
                    }
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('A') => {
                    app.col = app.line_len();
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('o') => {
                    app.insert_line_below();
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('O') => {
                    app.insert_line_above();
                    app.mode = Mode::Insert;
                }
                KeyCode::Char('n') if !app.search_query.is_empty() && !app.matches.is_empty() => {
                    app.match_idx = (app.match_idx + 1) % app.matches.len();
                    let (r, c) = app.matches[app.match_idx];
                    app.row = r;
                    app.col = c;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('N') if !app.search_query.is_empty() && !app.matches.is_empty() => {
                    app.match_idx = (app.match_idx + app.matches.len() - 1) % app.matches.len();
                    let (r, c) = app.matches[app.match_idx];
                    app.row = r;
                    app.col = c;
                    app.reveal_row(app.row);
                    app.ensure_visible();
                }
                KeyCode::Char('/') => {
                    app.searching = true;
                    app.search_query.clear();
                    app.pending = None;
                }
                KeyCode::Char('?') => {
                    app.help_open = true;
                    app.help_scroll = 0;
                    app.pending = None;
                }
                KeyCode::Char('v') => {
                    app.mode = Mode::Visual;
                    app.anchor = Some((app.row, app.col));
                    app.pending = None;
                }
                _ => {}
            }
        }
        Mode::Insert => match k.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                // al salir de insertar, el cursor vuelve al último carácter (como vim)
                if app.col > 0 {
                    app.col -= 1;
                }
            }
            KeyCode::Left => app.move_cursor(0, -1),
            KeyCode::Right => app.move_cursor(0, 1),
            KeyCode::Up => app.move_cursor(-1, 0),
            KeyCode::Down => app.move_cursor(1, 0),
            KeyCode::Home => {
                app.col = 0;
                app.ensure_visible();
            }
            KeyCode::End => {
                app.col = app.line_len();
                app.ensure_visible();
            }
            KeyCode::Enter => app.newline(),
            KeyCode::Backspace => app.backspace(),
            // Tab en inserción: plegar/desplegar la cabecera bajo el cursor (como en Normal)
            KeyCode::Tab => {
                let r = app.row;
                app.toggle_fold(r);
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                app.insert_char(c)
            }
            _ => {}
        },
        Mode::Visual => match k.code {
            KeyCode::Esc | KeyCode::Char('v') => {
                app.mode = Mode::Normal;
                app.anchor = None;
            }
            KeyCode::Char('h') | KeyCode::Left => app.move_cursor(0, -1),
            KeyCode::Char('j') | KeyCode::Down => app.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => app.move_cursor(-1, 0),
            KeyCode::Char('l') | KeyCode::Right => app.move_cursor(0, 1),
            KeyCode::Home => {
                app.col = 0;
                app.ensure_visible();
            }
            KeyCode::End => {
                app.col = app.line_len().saturating_sub(1);
                app.ensure_visible();
            }
            KeyCode::Char('x') | KeyCode::Delete => app.delete_selection(),
            _ => {}
        },
        Mode::Shell | Mode::Ai | Mode::Browser | Mode::Program => {}
    }
}

pub(crate) fn handle_search_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            app.searching = false;
            app.search_query.clear();
            app.matches.clear();
        }
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.searching = false;
            app.search_query.clear();
            app.matches.clear();
        }
        KeyCode::Enter => {
            app.searching = false;
            if app.search_query.is_empty() {
                return;
            }
            app.matches = compute_matches(&app.lines, &app.search_query);
            app.match_idx = app
                .matches
                .iter()
                .position(|&(r, c)| (r, c) >= (app.row, app.col))
                .unwrap_or(0);
            if let Some(&(r, c)) = app.matches.get(app.match_idx) {
                app.row = r;
                app.col = c;
                app.reveal_row(app.row);
                app.ensure_visible();
            }
            app.status = format!("{} coincidencias", app.matches.len());
        }
        KeyCode::Backspace => {
            app.search_query.pop();
        }
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.search_query.push(c);
        }
        _ => {}
    }
}

pub(crate) fn compute_matches(lines: &[String], query: &str) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    if query.is_empty() {
        return v;
    }
    for (r, line) in lines.iter().enumerate() {
        for (pos, _) in line.match_indices(query) {
            v.push((r, line[..pos].chars().count()));
        }
    }
    v
}

/// color de una línea org (None = por defecto)
pub(crate) fn org_line_style(app: &App, idx: usize) -> Option<(Color, bool)> {
    let line = app.lines.get(idx)?;
    let lvl = app.header_level(idx);
    if let Some(l) = lvl {
        let rest: String = line.chars().skip(l as usize + 1).collect();
        if rest.starts_with("TODO") {
            return Some((Color::Red, false));
        }
        if rest.starts_with("DONE") {
            return Some((Color::Green, false));
        }
        let c = match l {
            1 => Color::Cyan,
            2 => Color::Green,
            3 => Color::Yellow,
            _ => Color::Magenta,
        };
        return Some((c, l == 1));
    }
    if line.starts_with("#+BEGIN_SRC") || line.starts_with("#+END_SRC") {
        return Some((Color::Magenta, false));
    }
    if line.starts_with("#+") {
        return Some((Color::DarkYellow, false));
    }
    None
}

pub(crate) fn draw_editor_lines(app: &mut App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
    let mut real = app.top;
    let mut screen_y = 0u16;
    let gutter = app.gutter();
    let max_content = area_w.saturating_sub(gutter as u16) as usize;
    let mut in_src = false;

    while screen_y < editor_rows && real < app.lines.len() {
        if app.is_hidden(real) {
            real += 1;
            continue;
        }
        let idx = real;
        let folded_here = app.header_level(idx).is_some() && app.folded.contains(&idx);
        let line = &app.lines[idx];

        // estado de bloque src (recorrido en orden)
        if !in_src && line.starts_with("#+BEGIN_SRC") {
            in_src = true;
        } else if in_src && line.starts_with("#+END_SRC") {
            in_src = false;
        }

        let (fg, bold) = if let Some((c, b)) = org_line_style(app, idx) {
            (Some(c), b)
        } else if in_src {
            (Some(Color::DarkGrey), false)
        } else {
            (None, false)
        };

        if app.show_numbers {
            fr.text(bx, screen_y, &format!("{:>5} ", idx + 1), Style::default());
        } else {
            fr.put(bx, screen_y, ' ', Style::default());
        }

        let mut printed = 0usize;
        for (x, ch) in line.chars().skip(app.left).enumerate() {
            if x + gutter >= area_w as usize {
                break;
            }
            printed += 1;
            let col_abs = app.left + x;
            let selected = app.mode == Mode::Visual && is_selected(app, idx, col_abs);
            // resaltar el rango completo de cada coincidencia, no solo su inicio
            let hit = if app.search_query.is_empty() {
                false
            } else {
                let qlen = app.search_query.chars().count();
                app.matches
                    .iter()
                    .any(|&(r, c)| r == idx && col_abs >= c && col_abs < c + qlen)
            };
            let st = if hit && !selected {
                Style {
                    fg: Some(Color::Black),
                    bg: Some(Color::DarkYellow),
                    ..Default::default()
                }
            } else if selected {
                Style { fg, bold, reverse: true, ..Default::default() }
            } else {
                Style { fg, bold, ..Default::default() }
            };
            fr.put(bx + gutter as u16 + x as u16, screen_y, ch, st);
        }

        if folded_here {
            // marcador de plegado al final del texto visible
            if printed + 2 < max_content {
                fr.text(
                    bx + gutter as u16 + printed as u16,
                    screen_y,
                    " [+]",
                    Style {
                        fg: Some(Color::DarkYellow),
                        ..Default::default()
                    },
                );
            }
            real = app.fold_end(idx) + 1;
        } else {
            real += 1;
        }
        screen_y += 1;
    }
}

pub(crate) fn is_selected(app: &App, row: usize, col: usize) -> bool {
    let Some((ar, ac)) = app.anchor else { return false };
    let (br, bc) = (app.row, app.col);
    let ((r1, c1), (r2, c2)) = if (ar, ac) <= (br, bc) {
        ((ar, ac), (br, bc))
    } else {
        ((br, bc), (ar, ac))
    };

    if row < r1 || row > r2 {
        return false;
    }
    if r1 == r2 {
        return col >= c1 && col <= c2;
    }
    if row == r1 {
        return col >= c1;
    }
    if row == r2 {
        return col <= c2;
    }
    true
}
