//! Reusable Slate widgets. Screens compose these; no game rules live here.

pub mod chrome;
pub mod panels;
pub mod spans;
pub mod table;
pub mod theme;

pub use theme::{Depth, Intent, Surface, Theme};

#[cfg(test)]
pub(crate) mod test_util {
    use ratatui::buffer::Buffer;

    pub fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.x..area.x + area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }
}
