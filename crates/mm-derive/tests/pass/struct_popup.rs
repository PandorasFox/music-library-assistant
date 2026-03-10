// Test: struct with popup mode
use mm_derive::WizardItem;
use ratatui::text::Line;

#[derive(Debug, Clone)]
pub enum WizardOffer {
    Popup(Vec<Line<'static>>),
    Pane { title: String, content: Vec<String> },
    Both { popup: Vec<Line<'static>>, pane_title: String, pane_content: Vec<String> },
}

pub trait WizardItem {
    fn wizard(&self, width: u16) -> Option<WizardOffer>;
}

#[derive(WizardItem)]
#[wizard(popup)]
struct TrackInfo {
    name: String,
    #[wizard(popup_content)]
    info_lines: Vec<Line<'static>>,
}

fn main() {
    let t = TrackInfo {
        name: "track".into(),
        info_lines: vec![Line::raw("info")],
    };
    assert!(matches!(t.wizard(80), Some(WizardOffer::Popup(_))));
}
