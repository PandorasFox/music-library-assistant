// Fail: pane variant missing pane_title field
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
enum Bad {
    #[wizard(pane)]
    Detail {
        // Missing #[wizard(pane_title)]!
        #[wizard(pane_content)]
        lines: Vec<Line<'static>>,
    },
}

fn main() {}
