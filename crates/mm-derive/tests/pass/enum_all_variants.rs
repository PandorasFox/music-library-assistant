// Test: enum with all variant types (skip, popup, pane, both)
use mm_derive::WizardItem;
use ratatui::text::Line;

// Minimal trait + types matching what the main crate provides.
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
enum TestItem {
    #[wizard(skip)]
    Header { title: String },

    #[wizard(popup)]
    Info {
        label: String,
        #[wizard(popup_content)]
        summary: Vec<Line<'static>>,
    },

    #[wizard(pane)]
    Detail {
        label: String,
        #[wizard(pane_title)]
        detail_title: String,
        #[wizard(pane_content)]
        detail_lines: Vec<Line<'static>>,
    },

    #[wizard(both)]
    Full {
        label: String,
        #[wizard(popup_content)]
        summary: Vec<Line<'static>>,
        #[wizard(pane_title)]
        detail_title: String,
        #[wizard(pane_content)]
        detail_lines: Vec<Line<'static>>,
    },
}

fn main() {
    // skip variant returns None
    let h = TestItem::Header { title: "test".into() };
    assert!(h.wizard(80).is_none());

    // popup variant returns Popup
    let info = TestItem::Info {
        label: "x".into(),
        summary: vec![Line::raw("hello")],
    };
    assert!(matches!(info.wizard(80), Some(WizardOffer::Popup(_))));

    // pane variant returns Pane
    let detail = TestItem::Detail {
        label: "x".into(),
        detail_title: "Title".into(),
        detail_lines: vec![Line::raw("line")],
    };
    assert!(matches!(detail.wizard(80), Some(WizardOffer::Pane { .. })));

    // both variant returns Both
    let full = TestItem::Full {
        label: "x".into(),
        summary: vec![Line::raw("sum")],
        detail_title: "Title".into(),
        detail_lines: vec![Line::raw("line")],
    };
    assert!(matches!(full.wizard(80), Some(WizardOffer::Both { .. })));
}
