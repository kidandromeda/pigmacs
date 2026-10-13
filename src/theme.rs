// ————— pigmacs · tema: paleta y estilos comunes —————
// Autor:      kidandromeda
// Licencia:   MIT
// Versión:    3.1.0
// Fecha:      2026-10-14
//
// Paleta semántica única de la aplicación. pigmacs es un programa para
// *tty real*: sólo se usan los 16 colores ANSI (nada de 256/truecolor) y
// glifos que existan en la fuente de la consola (─│┌┐└┘├┤▸★●·⋯).
// Todos los módulos pintan con estos nombres; no hay colores sueltos.
use crate::frame::Style;
use crossterm::style::Color;

/// acento principal: bordes, separadores, títulos de panel
pub(crate) const ACCENT: Color = Color::DarkCyan;
/// acento brillante: píldoras, teclas de la ayuda, selección
pub(crate) const ACCENT_HI: Color = Color::Cyan;
/// estados
pub(crate) const OK: Color = Color::DarkGreen;
pub(crate) const WARN: Color = Color::DarkYellow;
pub(crate) const ERR: Color = Color::DarkRed;
/// texto
pub(crate) const FG: Color = Color::White;
pub(crate) const DIM: Color = Color::DarkGrey;
/// tree
pub(crate) const DIR: Color = Color::DarkYellow;
pub(crate) const FILE: Color = Color::White;
/// resaltado de código
pub(crate) const SYNTAX_KW: Color = Color::Cyan;
pub(crate) const SYNTAX_STR: Color = Color::Green;
pub(crate) const SYNTAX_NUM: Color = Color::DarkYellow;
pub(crate) const SYNTAX_FN: Color = Color::Yellow;
pub(crate) const SYNTAX_TYPE: Color = Color::Magenta;
pub(crate) const SYNTAX_COM: Color = Color::DarkGrey;

/// texto plano en un color
pub(crate) fn fg(c: Color) -> Style {
    Style {
        fg: Some(c),
        ..Default::default()
    }
}

/// texto en negrita
pub(crate) fn bold(c: Color) -> Style {
    Style {
        fg: Some(c),
        bold: true,
        ..Default::default()
    }
}

/// píldora: texto con fondo de color (p. ej. el indicador de modo)
pub(crate) fn pill(bg: Color, fg: Color) -> Style {
    Style {
        fg: Some(fg),
        bg: Some(bg),
        bold: true,
        ..Default::default()
    }
}

/// banda: fila entera con fondo (cabeceras de panel, selección ancha)
pub(crate) fn band(bg: Color, fg: Color) -> Style {
    Style {
        fg: Some(fg),
        bg: Some(bg),
        ..Default::default()
    }
}
