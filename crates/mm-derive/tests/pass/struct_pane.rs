// Test: struct with pane mode
use mm_derive::WizardItem;
use ratatui::text::Line;

#[derive(Debug, Clone)]
pub enum WizardOffer {
    Popup(Vec<Line<'static>>),
    Pane { title: String, lines: Vec<Line<'static>> },
    Both { popup: Vec<Line<'static>>, pane_title: String, pane_lines: Vec<Line<'static>> },
}

pub trait WizardItem {
    fn wizard(&self, width: u16) -> Option<WizardOffer>;
}

#[derive(WizardItem)]
#[wizard(pane)]
struct DetailInfo {
    name: String,
    #[wizard(pane_title)]
    title: String,
    #[wizard(pane_content)]
    lines: Vec<Line<'static>>,
}

fn main() {
    let d = DetailInfo {
        name: "x".into(),
        title: "Title".into(),
        lines: vec![Line::raw("content")],
    };
    assert!(matches!(d.wizard(80), Some(WizardOffer::Pane { .. })));
}
