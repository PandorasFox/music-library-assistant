// Test: unit variant with skip
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
enum Simple {
    #[wizard(skip)]
    Separator,

    #[wizard(popup)]
    Item {
        #[wizard(popup_content)]
        lines: Vec<Line<'static>>,
    },
}

fn main() {
    let s = Simple::Separator;
    assert!(s.wizard(80).is_none());
}
