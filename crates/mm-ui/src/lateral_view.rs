//! Views available in the lateral view ring.

/// Views available in the lateral view ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateralView {
    Config,
    Search,
    Files,
    Health,
    History,
    Transaction,
    Deploy,
    ExternalMatches,
}

impl LateralView {
    /// Display label for this view
    pub fn label(&self) -> &'static str {
        match self {
            LateralView::Config => "Config",
            LateralView::Search => "Search",
            LateralView::Files => "Files",
            LateralView::Health => "Health",
            LateralView::History => "History",
            LateralView::Transaction => "Transaction",
            LateralView::Deploy => "Deploy",
            LateralView::ExternalMatches => "Ext. Authorities",
        }
    }

    /// Get the next view in the ring (Tab)
    pub fn next(&self, transactions_open: bool) -> Self {
        match self {
            LateralView::Config => LateralView::Search,
            LateralView::Search => LateralView::Files,
            LateralView::Files => LateralView::Health,
            LateralView::Health => {
                if transactions_open {
                    LateralView::Transaction
                } else {
                    LateralView::Deploy
                }
            }
            LateralView::Transaction => LateralView::Deploy,
            LateralView::Deploy => LateralView::History,
            LateralView::History => LateralView::ExternalMatches,
            LateralView::ExternalMatches => LateralView::Config,
        }
    }

    /// Get the previous view in the ring (Shift-Tab)
    pub fn prev(&self, transactions_open: bool) -> Self {
        match self {
            LateralView::Config => LateralView::ExternalMatches,
            LateralView::Search => LateralView::Config,
            LateralView::Files => LateralView::Search,
            LateralView::Health => LateralView::Files,
            LateralView::Transaction => LateralView::Health,
            LateralView::Deploy => {
                if transactions_open {
                    LateralView::Transaction
                } else {
                    LateralView::Health
                }
            }
            LateralView::History => LateralView::Deploy,
            LateralView::ExternalMatches => LateralView::History,
        }
    }

    /// Convert to a default Route (no cursor/scroll position).
    pub fn to_default_route(&self) -> crate::route::Route {
        use crate::route::*;
        match self {
            LateralView::Config => Route::Config(ConfigRoute::default()),
            LateralView::Search => Route::Search(SearchRoute::default()),
            LateralView::Files => Route::Files(FilesRoute::default()),
            LateralView::Health => Route::Health(HealthRoute::default()),
            LateralView::History => Route::History(HistoryRoute::default()),
            LateralView::Transaction => Route::Transaction(TransactionRoute::default()),
            LateralView::Deploy => Route::Deploy(DeployRoute::default()),
            LateralView::ExternalMatches => Route::ExternalMatches(ExternalMatchesRoute::default()),
        }
    }

    /// All views in order
    pub fn all(transactions_open: bool) -> Vec<LateralView> {
        let mut views = vec![
            LateralView::Config,
            LateralView::Search,
            LateralView::Files,
            LateralView::Health,
        ];
        if transactions_open {
            views.push(LateralView::Transaction);
        }
        views.push(LateralView::Deploy);
        views.push(LateralView::History);
        views.push(LateralView::ExternalMatches);
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lateral_view_cycling() {
        let view = LateralView::Config;
        assert_eq!(view.next(false), LateralView::Search);
        assert_eq!(view.next(false).next(false), LateralView::Files);

        // With transactions
        assert_eq!(LateralView::Health.next(true), LateralView::Transaction);
        assert_eq!(LateralView::Transaction.next(true), LateralView::Deploy);
        assert_eq!(LateralView::Deploy.prev(true), LateralView::Transaction);

        // Without transactions
        assert_eq!(LateralView::Health.next(false), LateralView::Deploy);
        assert_eq!(LateralView::Deploy.prev(false), LateralView::Health);
    }

    #[test]
    fn lateral_view_labels() {
        assert_eq!(LateralView::Config.label(), "Config");
        assert_eq!(LateralView::ExternalMatches.label(), "Ext. Authorities");
    }
}
