//! Images of the actual terminal buffer for visual inspection.

use ratatui::{buffer::Buffer, style::Color};

pub(super) fn save(buffer: &Buffer, name: &str) {
    let Some(root) = std::env::var_os("MAGI_LOGIN_PREVIEW") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).expect("preview directory");
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\"><rect width=\"100%\" height=\"100%\" fill=\"#17171d\"/>",
        buffer.area.width * 10,
        buffer.area.height * 20
    );
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let symbol = cell
                .symbol()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            if let Some(bg) = color(cell.bg) {
                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"10\" height=\"20\" fill=\"{bg}\"/>",
                    x * 10,
                    y * 20
                ));
            }
            let fg = color(cell.fg).unwrap_or_else(|| "#dedee8".into());
            svg.push_str(&format!("<text x=\"{}\" y=\"{}\" font-family=\"monospace\" font-size=\"16\" fill=\"{fg}\">{symbol}</text>",x*10,y*20+16));
        }
    }
    svg.push_str("</svg>");
    std::fs::write(root.join(format!("{name}.svg")), svg).expect("preview SVG");
}

fn color(color: Color) -> Option<String> {
    match color {
        Color::Reset => None,
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(i) if i >= 232 => {
            let v = 8 + (i - 232) * 10;
            Some(format!("#{v:02x}{v:02x}{v:02x}"))
        }
        Color::Black => Some("#17171d".into()),
        _ => Some("#9999aa".into()),
    }
}
