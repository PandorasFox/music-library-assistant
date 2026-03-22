//! Startup and lifecycle action handlers.

use super::super::App;
use super::HandleAction;

impl HandleAction for super::super::ExitConfirmAction {
    fn handle(self, app: &mut App, _witness: Option<&super::witness::ConfirmationGesture>) {
        use super::super::ExitConfirmAction;
        match self {
            ExitConfirmAction::None => {}
            ExitConfirmAction::Quit => {
                app.should_quit = true;
            }
            ExitConfirmAction::QuitAndShutdown => {
                let _ = app.shutdown();
                app.should_quit = true;
            }
            ExitConfirmAction::Cancel => {
                app.start_health_view();
            }
        }
    }
}
