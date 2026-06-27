use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
use wordcartel_core::theme::{quantize, Color, Depth, Face};

fn to_rcolor(c: Color, depth: Depth) -> RColor {
    match quantize(c, depth) {
        Color::Rgb { r, g, b } => RColor::Rgb(r, g, b),
        Color::Indexed(i) => RColor::Indexed(i),
        Color::Default => RColor::Reset,
        // named → ratatui named (1:1, so the Default theme reproduces today's Color::Cyan etc.)
        Color::Black => RColor::Black,
        Color::Red => RColor::Red,
        Color::Green => RColor::Green,
        Color::Yellow => RColor::Yellow,
        Color::Blue => RColor::Blue,
        Color::Magenta => RColor::Magenta,
        Color::Cyan => RColor::Cyan,
        Color::Gray => RColor::Gray,
        Color::DarkGray => RColor::DarkGray,
        Color::LightRed => RColor::LightRed,
        Color::LightGreen => RColor::LightGreen,
        Color::LightYellow => RColor::LightYellow,
        Color::LightBlue => RColor::LightBlue,
        Color::LightMagenta => RColor::LightMagenta,
        Color::LightCyan => RColor::LightCyan,
        Color::White => RColor::White,
    }
}

pub fn face_to_ratatui(face: &Face, depth: Depth) -> RStyle {
    let mut s = RStyle::default();
    if let Some(c) = face.fg { s = s.fg(to_rcolor(c, depth)); }
    if let Some(c) = face.bg { s = s.bg(to_rcolor(c, depth)); }
    if let Some(c) = face.underline_color { s = s.underline_color(to_rcolor(c, depth)); }
    let add = |on: Option<bool>, m: Modifier, s: RStyle| if on == Some(true) { s.add_modifier(m) } else { s };
    s = add(face.bold, Modifier::BOLD, s);
    s = add(face.italic, Modifier::ITALIC, s);
    s = add(face.underline, Modifier::UNDERLINED, s);
    s = add(face.strike, Modifier::CROSSED_OUT, s);
    s = add(face.reverse, Modifier::REVERSED, s);
    s = add(face.dim, Modifier::DIM, s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::theme::{Color, Face, Depth};
    use ratatui::style::{Color as RColor, Modifier};
    #[test]
    fn maps_rgb_and_modifiers_at_truecolor() {
        let f = Face { fg: Some(Color::Rgb{r:1,g:2,b:3}), bold: Some(true), underline: Some(true),
                       underline_color: Some(Color::Red), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::Truecolor);
        assert_eq!(s.fg, Some(RColor::Rgb(1,2,3)));
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert!(s.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(s.underline_color, Some(RColor::Red));
    }
    #[test]
    fn default_color_is_reset_not_a_color() {
        let f = Face { fg: Some(Color::Default), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::Truecolor);
        assert_eq!(s.fg, Some(RColor::Reset));
    }
    #[test]
    fn quantizes_at_ansi16() {
        let f = Face { fg: Some(Color::Rgb{r:255,g:0,b:0}), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::Ansi16);
        // Rgb red → named Red/LightRed → ratatui named (NOT Indexed)
        assert!(matches!(s.fg, Some(RColor::Red) | Some(RColor::LightRed)));
    }
}
