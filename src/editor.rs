// ————— pigmacs · editor de texto/org —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
use crate::{
    app::{App, Mode},
    frame::{Frame, Style},
    theme::*,
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

/// ---------- resaltado de sintaxis de los bloques #+BEGIN_SRC ----------
/// Sólo para las líneas visibles de cada frame (coste despreciable) y sin
/// parser externo: un autómata por línea con estado de cadenas multilínea.

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Lang {
    Python,
    Bash,
    Rust,
    C,
    Js,
    Other,
}

/// lenguaje a partir de la cabecera `#+BEGIN_SRC <lenguaje>`
pub(crate) fn lang_of(header: &str) -> Lang {
    let l = header
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_ascii_lowercase();
    match l.as_str() {
        "python" | "py" => Lang::Python,
        "bash" | "sh" | "shell" | "zsh" => Lang::Bash,
        "rust" | "rs" => Lang::Rust,
        "c" | "cpp" | "c++" | "h" | "hpp" => Lang::C,
        "js" | "javascript" | "ts" | "typescript" => Lang::Js,
        _ => Lang::Other,
    }
}

const PY_KW: &[&str] = &[
    "def", "class", "if", "elif", "else", "for", "while", "return", "import", "from", "as",
    "with", "try", "except", "finally", "raise", "lambda", "yield", "pass", "break",
    "continue", "and", "or", "not", "in", "is", "global", "nonlocal", "assert", "del",
    "async", "await",
];
const PY_BI: &[&str] = &[
    "self", "None", "True", "False", "print", "len", "range", "str", "int", "float", "list",
    "dict", "set", "tuple", "open", "type", "super", "isinstance", "enumerate", "zip", "map",
    "filter", "input", "repr", "sorted", "sum", "min", "max", "abs",
];
const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "select", "time", "coproc",
];
const SH_BI: &[&str] = &[
    "echo", "cd", "export", "local", "return", "read", "source", "set", "unset", "alias",
    "trap", "exit", "test", "printf", "shift", "eval", "exec", "pwd", "mkdir", "rm", "cp",
    "mv", "ls", "cat", "grep", "sed", "awk", "cargo", "sudo",
];
const RS_KW: &[&str] = &[
    "fn", "let", "mut", "if", "else", "match", "loop", "for", "while", "struct", "enum",
    "impl", "trait", "use", "mod", "pub", "ref", "move", "where", "in", "as", "const",
    "static", "crate", "self", "super", "return", "break", "continue", "unsafe", "extern",
    "dyn", "type", "await", "async",
];
const RS_TY: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128", "usize", "isize",
    "f32", "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box", "HashMap",
    "HashSet", "Some", "None", "Ok", "Err", "Self",
];
const C_KW: &[&str] = &[
    "if", "else", "for", "while", "do", "switch", "case", "default", "break", "continue",
    "return", "goto", "struct", "union", "enum", "typedef", "sizeof", "static", "extern",
    "const", "volatile", "inline", "register", "auto", "signed", "unsigned",
];
const C_TY: &[&str] = &[
    "int", "char", "float", "double", "void", "long", "short", "size_t", "FILE", "bool",
    "uint32_t", "int32_t", "uint8_t", "uint64_t", "true", "false", "NULL",
];
const JS_KW: &[&str] = &[
    "function", "return", "if", "else", "for", "while", "do", "switch", "case", "break",
    "continue", "const", "let", "var", "new", "class", "extends", "this", "typeof",
    "instanceof", "try", "catch", "finally", "throw", "async", "await", "import", "export",
    "from", "default", "delete", "in", "of", "void", "yield", "static",
];
const JS_TY: &[&str] = &[
    "String", "Number", "Boolean", "Object", "Array", "Promise", "undefined", "null", "true",
    "false", "console", "Math", "JSON", "document", "window",
];

fn keyword_st(lang: Lang, w: &str) -> Style {
    let (kw, ty, bi): (&[&str], &[&str], &[&str]) = match lang {
        Lang::Python => (PY_KW, &[], PY_BI),
        Lang::Bash => (SH_KW, &[], SH_BI),
        Lang::Rust => (RS_KW, RS_TY, &[]),
        Lang::C => (C_KW, C_TY, &[]),
        Lang::Js => (JS_KW, JS_TY, &[]),
        Lang::Other => (&[], &[], &[]),
    };
    if kw.contains(&w) {
        bold(SYNTAX_KW)
    } else if ty.contains(&w) {
        bold(SYNTAX_TYPE)
    } else if bi.contains(&w) {
        if matches!(
            w,
            "self" | "None" | "True" | "False" | "null" | "undefined" | "this"
        ) {
            bold(SYNTAX_TYPE)
        } else {
            fg(SYNTAX_FN)
        }
    } else {
        fg(FG)
    }
}

/// estilo por carácter de una línea dentro de un bloque SRC.
/// `triple` arrastra el estado de las cadenas multilínea (python """…""")
pub(crate) fn src_styles(line: &str, lang: Lang, triple: &mut Option<char>) -> Vec<Style> {
    let ch: Vec<char> = line.chars().collect();
    let mut st = vec![fg(FG); ch.len()];
    if ch.is_empty() {
        return st;
    }
    let mut i = 0usize;

    // comentario de línea según lenguaje
    let com: fn(&[char], usize) -> bool = match lang {
        Lang::Rust | Lang::C | Lang::Js => |c, i| c[i] == '/' && c.get(i + 1) == Some(&'/'),
        _ => |c, i| c[i] == '#',
    };

    // seguir una cadena triple abierta en una línea anterior
    if let Some(d) = *triple {
        let mut j = 0usize;
        let mut cerrado = None;
        while j + 2 < ch.len() {
            if ch[j] == d && ch[j + 1] == d && ch[j + 2] == d {
                cerrado = Some(j + 2);
                break;
            }
            j += 1;
        }
        let fin = cerrado.unwrap_or(ch.len() - 1);
        for s in st.iter_mut().take(fin + 1) {
            *s = fg(SYNTAX_STR);
        }
        if let Some(f) = cerrado {
            *triple = None;
            i = f + 1;
        } else {
            return st;
        }
    }

    // preprocesador en C (línea que empieza por #)
    if lang == Lang::C && line.trim_start().starts_with('#') {
        for s in st.iter_mut() {
            *s = bold(SYNTAX_TYPE);
        }
        return st;
    }

    while i < ch.len() {
        let c = ch[i];
        if com(&ch, i) {
            for s in st[i..].iter_mut() {
                *s = fg(SYNTAX_COM);
            }
            break;
        }
        // cadenas
        if c == '"' || c == '\'' || c == '`' {
            // ¿apertura de cadena triple (python)?
            if lang == Lang::Python && i + 2 < ch.len() && ch[i + 1] == c && ch[i + 2] == c {
                let mut j = i + 3;
                let mut cerrado = None;
                while j + 2 < ch.len() {
                    if ch[j] == c && ch[j + 1] == c && ch[j + 2] == c {
                        cerrado = Some(j + 2);
                        break;
                    }
                    j += 1;
                }
                if let Some(f) = cerrado {
                    for s in st[i..=f].iter_mut() {
                        *s = fg(SYNTAX_STR);
                    }
                    i = f + 1;
                } else {
                    for s in st[i..].iter_mut() {
                        *s = fg(SYNTAX_STR);
                    }
                    *triple = Some(c);
                    break;
                }
                continue;
            }
            // cadena normal (por línea: suficiente para el resaltado)
            let mut j = i + 1;
            while j < ch.len() && ch[j] != c {
                if ch[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            let fin = if j < ch.len() { j } else { ch.len() - 1 };
            for s in st[i..=fin].iter_mut() {
                *s = fg(SYNTAX_STR);
            }
            i = fin + 1;
            continue;
        }
        // números (0x, decimales, sufijos, _)
        if c.is_ascii_digit() && (i == 0 || !(ch[i - 1].is_alphanumeric() || ch[i - 1] == '_'))
        {
            let mut j = i;
            while j < ch.len() && (ch[j].is_ascii_alphanumeric() || ch[j] == '.' || ch[j] == '_')
            {
                j += 1;
            }
            for s in st[i..j].iter_mut() {
                *s = fg(SYNTAX_NUM);
            }
            i = j;
            continue;
        }
        // lifetime de rust: 'a
        if lang == Lang::Rust
            && c == '\''
            && i + 1 < ch.len()
            && ch[i + 1].is_alphabetic()
            && ch.get(i + 2) != Some(&'\'')
        {
            let mut j = i + 1;
            while j < ch.len() && (ch[j].is_alphanumeric() || ch[j] == '_') {
                j += 1;
            }
            for s in st[i..j].iter_mut() {
                *s = bold(SYNTAX_TYPE);
            }
            i = j;
            continue;
        }
        // palabra (keyword / builtin / tipo / función)
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < ch.len() && (ch[j].is_alphanumeric() || ch[j] == '_') {
                j += 1;
            }
            let w: String = ch[i..j].iter().collect();
            let mut estilo = keyword_st(lang, &w);
            if lang == Lang::Python && i > 0 && ch[i - 1] == '@' {
                estilo = bold(SYNTAX_TYPE);
            } else {
                let es_kw = matches!(
                    w.as_str(),
                    "if" | "for" | "while" | "return" | "match" | "switch" | "catch"
                );
                let tras = ch[j..].iter().find(|&&c| c != ' ').copied();
                if !es_kw && tras == Some('(') && estilo.fg != Some(SYNTAX_TYPE) {
                    estilo = fg(SYNTAX_FN);
                }
            }
            for s in st[i..j].iter_mut() {
                *s = estilo;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    st
}

/// estado del bloque SRC de la línea `top` (rastreado hacia atrás hasta la
/// cabecera del bloque): un frame que empieza a mitad de un bloque pinta bien
pub(crate) fn src_state_at(lines: &[String], top: usize) -> (bool, Lang, Option<char>) {
    let mut i = top.min(lines.len());
    while i > 0 {
        i -= 1;
        let l = &lines[i];
        if l.starts_with("#+END_SRC") {
            return (false, Lang::Other, None);
        }
        if l.starts_with("#+BEGIN_SRC") {
            let lang = lang_of(l);
            let mut triple: Option<char> = None;
            for x in (i + 1)..top {
                src_styles(&lines[x], lang, &mut triple);
            }
            return (true, lang, triple);
        }
        if top - i > 500 {
            break; // bloque gigante: no rastrear más (sólo afecta a estilos)
        }
    }
    (false, Lang::Other, None)
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
    // el frame puede empezar a mitad de un bloque SRC: rastrear su estado
    let (mut in_src, mut lang, mut triple) = src_state_at(&app.lines, app.top);

    while screen_y < editor_rows && real < app.lines.len() {
        if app.is_hidden(real) {
            real += 1;
            continue;
        }
        let idx = real;
        let folded_here = app.header_level(idx).is_some() && app.folded.contains(&idx);
        let line = &app.lines[idx];
        let es_delim = line.starts_with("#+BEGIN_SRC") || line.starts_with("#+END_SRC");

        // estado de bloque src (recorrido en orden)
        if !in_src && line.starts_with("#+BEGIN_SRC") {
            in_src = true;
            lang = lang_of(line);
            triple = None;
        } else if in_src && line.starts_with("#+END_SRC") {
            in_src = false;
            triple = None;
        }

        // estilo base: resaltado de código dentro del bloque, color org fuera
        let src_st: Option<Vec<Style>> = if in_src && !es_delim {
            Some(src_styles(line, lang, &mut triple))
        } else {
            None
        };
        let (lfg, lbold) = if let Some((c, b)) = org_line_style(app, idx) {
            (Some(c), b)
        } else {
            (None, false)
        };
        let line_st = Style {
            fg: lfg,
            bold: lbold,
            ..Default::default()
        };

        if app.show_numbers {
            fr.text(bx, screen_y, &format!("{:>5} ", idx + 1), fg(DIM));
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
            let base = match &src_st {
                Some(v) => v.get(col_abs).copied().unwrap_or(line_st),
                None => line_st,
            };
            let st = if hit {
                // búsqueda: negro sobre amarillo
                Style {
                    fg: Some(Color::Black),
                    bg: Some(WARN),
                    ..Default::default()
                }
            } else if selected {
                // selección visual con fondo de acento
                band(ACCENT_HI, Color::Black)
            } else {
                base
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
                    fg(WARN),
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
