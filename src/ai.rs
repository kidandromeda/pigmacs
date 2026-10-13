// ————— pigmacs · panel de IA —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.0.1
// Fecha:      2026-10-13
use crate::{
    app::App,
    frame::{Frame, Style, strip_ansi},
};
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};

pub(crate) fn handle_ai_key(app: &mut App, k: KeyEvent) {
    // modo selección dentro del panel IA: v entrar · j/k mover · Ctrl+E enviar · Esc salir
    if app.ai_selecting {
        match k.code {
            KeyCode::Esc | KeyCode::Char('v') => {
                app.ai_selecting = false;
                app.ai_sel_anchor = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.ai_sel_cursor = app.ai_sel_cursor.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.ai_sel_cursor = app.ai_sel_cursor.saturating_sub(1)
            }
            KeyCode::Char('g') => app.ai_sel_cursor = 0,
            KeyCode::Char('G') => app.ai_sel_cursor = usize::MAX,
            _ => {}
        }
        return;
    }
    match k.code {
        KeyCode::Esc => {
            if app.ai_running {
                app.ai_cancel();
            }
        }
        KeyCode::Enter => {
            app.ai_start();
        }
        KeyCode::Char('v') => {
            app.ai_selecting = true;
            app.ai_sel_anchor = Some(0);
            app.ai_sel_cursor = 0;
            app.status = "Selecciona con j/k, envía con Ctrl+E, sal con Esc".into();
        }
        KeyCode::Backspace => {
            app.ai_input.pop();
        }
        KeyCode::Up => {
            if app.ai_hist_idx > 0 {
                app.ai_hist_idx -= 1;
                app.ai_input = app
                    .ai_history
                    .get(app.ai_hist_idx)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        KeyCode::Down => {
            if app.ai_hist_idx + 1 < app.ai_history.len() {
                app.ai_hist_idx += 1;
                app.ai_input = app
                    .ai_history
                    .get(app.ai_hist_idx)
                    .cloned()
                    .unwrap_or_default();
            } else {
                app.ai_hist_idx = app.ai_history.len();
                app.ai_input.clear();
            }
        }
        KeyCode::Char(c)
            if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.ai_input.push(c);
        }
        _ => {}
    }
}

pub(crate) fn draw_ai_pane(app: &App, editor_rows: u16, bx: u16, area_w: u16, fr: &mut Frame) {
    let title = if app.ai_running {
        " IA · pi · procesando…"
    } else {
        " IA · pi · escribe tu pregunta y pulsa Enter (Esc cancela)"
    };
    fr.text(
        bx,
        0,
        &title.chars().take(area_w as usize).collect::<String>(),
        Style { reverse: true, ..Default::default() },
    );

    let mut all: Vec<String> = app.ai_lines.clone();
    if app.ai_running {
        let cur = app.ai_out.lock().unwrap();
        for l in cur.split('\n') {
            if !l.is_empty() {
                all.push(l.to_string());
            }
        }
    }

    let max_lines = editor_rows.saturating_sub(3) as usize;
    let start = all.len().saturating_sub(max_lines);
    let cur = app.ai_sel_cursor.min(max_lines.saturating_sub(1));
    for (i, l) in all[start..].iter().enumerate() {
        let clean: String = strip_ansi(l).chars().take(area_w as usize).collect();
        let selected = app.ai_selecting
            && app.ai_sel_anchor.map_or(false, |a| {
                let (r1, r2) = if a <= cur { (a, cur) } else { (cur, a) };
                i >= r1 && i <= r2
            });
        let st = if selected {
            Style { reverse: true, ..Default::default() }
        } else if app.ai_selecting && i == cur {
            Style { fg: Some(Color::DarkYellow), ..Default::default() }
        } else {
            Style::default()
        };
        fr.text(bx, 1 + i as u16, &clean, st);
    }

    let input_row = editor_rows.saturating_sub(1);
    let input_text: String = format!("> {}", app.ai_input)
        .chars()
        .take(area_w.saturating_sub(1) as usize)
        .collect();
    fr.text(bx, input_row, &input_text, Style::default());
}
