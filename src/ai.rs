// ————— pigmacs · panel de IA —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::{
    app::App,
    frame::{Frame, Style, strip_ansi},
    theme::*,
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
    // cabecera con banda de color según el estado: amarillo mientras
    // consulta, verde si hay respuesta, acento si está en reposo
    let (band_bg, band_fg) = if app.ai_running {
        (WARN, Color::Black)
    } else if !app.ai_lines.is_empty() {
        (OK, Color::White)
    } else {
        (ACCENT, Color::White)
    };
    let title = if app.ai_running {
        " IA · pi · procesando…"
    } else if app.ai_lines.is_empty() {
        " IA · pi · escribe tu pregunta y pulsa Enter (Esc cancela)"
    } else {
        " IA · pi · Enter nueva pregunta (↑ historial · v seleccionar)"
    };
    let band_st = band(band_bg, band_fg);
    for x in bx..bx + area_w {
        fr.put(x, 0, ' ', band_st);
    }
    fr.text(
        bx,
        0,
        &title.chars().take(area_w as usize).collect::<String>(),
        pill(band_bg, band_fg),
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
            band(ACCENT_HI, Color::Black)
        } else if app.ai_selecting && i == cur {
            fg(WARN)
        } else {
            fg(FG)
        };
        fr.text(bx, 1 + i as u16, &clean, st);
    }

    // campo de entrada: prompt en acento y texto en blanco
    let input_row = editor_rows.saturating_sub(1);
    for c in 0..area_w.saturating_sub(1) {
        fr.put(bx + c, input_row, ' ', Style::default());
    }
    fr.text(bx, input_row, "> ", bold(ACCENT_HI));
    let input_text: String = app
        .ai_input
        .chars()
        .take(area_w.saturating_sub(3) as usize)
        .collect();
    fr.text(bx + 2, input_row, &input_text, fg(FG));
}
