//! mm-web-client: WASM entry point for the Music Magic web UI.
//!
//! Builds mm-ui Node trees from API data and mounts them to the DOM.
//! Session token persisted in localStorage. View state encoded in URL hash
//! via the shared [`mm_ui::route::Route`] type.
//! v1 uses full re-render via innerHTML — no diffing.

mod api;
mod views;

use std::cell::Cell;
use std::cell::RefCell;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use mm_meta::decisions::{Decision, DecisionKey};
use mm_meta::paths::PathResolver;
use mm_ui::html::widgets::render_titlebar;
use mm_ui::html::{self, div, footer, Node};
use mm_ui::lateral_view::LateralView;
use mm_ui::resolutions::dispatch::{DispatchResult, Dispatchable};
use mm_ui::route::{self, Route};

// ============================================================================
// Active Resolution State
// ============================================================================

/// Holds the in-flight resolution state for Dispatchable resolution types.
///
/// Stored in a thread_local so that `mm_resolve_action()` can dispatch typed
/// actions against the held state without re-fetching data from the server.
enum ActiveResolution {
    // -- Cluster-nav (V3) modals --
    TagCanonicity {
        state: mm_ui::resolutions::tag_canonicity::TagCanonicityViewState,
        title_prefix: String,
    },
    CompoundSplit(mm_ui::resolutions::compound_split::CompoundSplitViewState),
    DirectoryCluster(mm_ui::resolutions::directory_cluster::DirectoryClusterState),
    ManualReview {
        state: mm_ui::resolutions::manual_review::ManualReviewState,
        title: String,
    },
    MissingAlbum(mm_ui::resolutions::missing_album::MissingAlbumState),
    DiscExtraction(mm_ui::resolutions::disc_extraction::DiscExtractionState),
    // -- Simple-batch modals --
    MissingDirectory(mm_ui::resolutions::missing_directory::MissingDirectoryState),
    CorruptFile(mm_ui::resolutions::corrupt_file::CorruptFileState),
    MovedFile(mm_ui::resolutions::moved_file::MovedFileState),
    SubparDuplicate(mm_ui::resolutions::subpar_duplicate::SubparDuplicateState),
    MissingFile(mm_ui::resolutions::missing_file::MissingFilePreviewState),
    LosslessRemux(mm_ui::resolutions::lossless_remux::LosslessRemuxPreviewState),
}

thread_local! {
    static CONFIG_EDITOR: RefCell<Option<mm_ui::config_editor::ConfigEditorState>> = const { RefCell::new(None) };
    /// JS timeout handle for search debounce. Cleared on new keystrokes.
    static SEARCH_DEBOUNCE: RefCell<Option<i32>> = const { RefCell::new(None) };
    /// Guard flag: when true, the hashchange listener skips its load because
    /// the programmatic caller (navigate_to / mm_navigate_route) already
    /// handles it.  Set before set_hash(), cleared by the hashchange handler.
    static PROGRAMMATIC_NAV: Cell<bool> = const { Cell::new(false) };
    /// Active resolution state for Dispatchable resolution types.
    /// Set by `load_resolution_view()`, consumed by `mm_resolve_action()`.
    static ACTIVE_RESOLUTION: RefCell<Option<ActiveResolution>> = const { RefCell::new(None) };
    /// Cached server build timestamp, fetched once at init.
    static BUILD_ID: RefCell<String> = const { RefCell::new(String::new()) };
    /// Latest WitchStatus from server-pushed events. Updated by the WebSocket
    /// callback, read by view loading and event-driven refresh logic.
    static LAST_STATUS: RefCell<Option<mm_meta::witch_types::WitchStatus>> = const { RefCell::new(None) };
}

// ============================================================================
// Entry Point
// ============================================================================

#[wasm_bindgen(start)]
pub fn main() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("mm-web-client panic: {info}").into());
    }));

    spawn_local(async {
        if let Err(e) = init().await {
            web_sys::console::error_1(&format!("init error: {e:?}").into());
            mount_error(&format!("Failed to connect: {e:?}"));
        }
    });
}

async fn init() -> Result<(), JsValue> {
    let status = api::setup_check().await?;
    if status.needs_setup {
        mount(&render_setup_form(None, status.suggested_root.as_deref()));
        return Ok(());
    }

    // Fetch build identifier once (best-effort; status bar shows empty on failure).
    if let Ok(build) = api::get_build_info().await {
        BUILD_ID.with(|cell| *cell.borrow_mut() = build);
    }

    // Listen for browser-initiated hash changes (back/forward, anchor clicks).
    // Programmatic navigations set PROGRAMMATIC_NAV so we skip the redundant load.
    register_hashchange_listener();

    if api::has_token() {
        match load_from_hash().await {
            Ok(()) => {
                start_event_stream();
                return Ok(());
            }
            Err(_) => api::clear_token(),
        }
    }

    mount(&render_login(None));
    Ok(())
}

// ============================================================================
// View State (URL hash ↔ Route)
// ============================================================================

/// Parse the current URL hash into a Route.
/// Falls back to Health on parse failure.
fn current_route() -> Route {
    let hash = web_sys::window()
        .unwrap()
        .location()
        .hash()
        .unwrap_or_default();
    let raw = hash.trim_start_matches('#');

    // Split path and query params.
    let (path, query) = if let Some((p, q)) = raw.split_once('?') {
        let pairs: Vec<(String, String)> = q
            .split('&')
            .filter(|s| !s.is_empty())
            .filter_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect();
        (p, pairs)
    } else {
        (raw, vec![])
    };

    Route::from_url(path, &query).unwrap_or(Route::Health(route::HealthRoute::default()))
}

/// Determine the LateralView for titlebar highlighting.
/// Overlay routes map to their parent lateral view.
fn lateral_view_for_route(route: &Route) -> LateralView {
    route.parent_lateral()
}

/// Navigate to a route by updating the URL hash.
///
/// Sets [`PROGRAMMATIC_NAV`] so the hashchange listener knows to skip
/// its redundant load — the caller is expected to call `load_from_hash()`
/// itself.
fn navigate_to(route: &Route) {
    let url = route.to_url();
    // Strip leading '/' for hash — browser prepends '#'.
    let hash = url.strip_prefix('/').unwrap_or(&url);
    PROGRAMMATIC_NAV.with(|flag| flag.set(true));
    web_sys::window()
        .unwrap()
        .location()
        .set_hash(hash)
        .ok();
}

/// Convenience: build a default route for a lateral view.
fn route_for_lateral(view: LateralView) -> Route {
    match view {
        LateralView::Config => Route::Config(Default::default()),
        LateralView::Search => Route::Search(Default::default()),
        LateralView::Files => Route::Files(Default::default()),
        LateralView::Health => Route::Health(Default::default()),
        LateralView::History => Route::History(Default::default()),
        LateralView::Transaction => Route::Transaction(Default::default()),
        LateralView::Deploy => Route::Deploy(Default::default()),
        LateralView::ExternalMatches => Route::ExternalMatches(Default::default()),
    }
}

// ============================================================================
// DOM Mounting
// ============================================================================

fn get_root() -> web_sys::Element {
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id("mm-root")
        .expect("missing #mm-root element")
}

fn mount(tree: &Node) {
    let root = get_root();
    root.set_inner_html(&tree.to_html());
    // Auto-focus the first keyboard-navigable list so arrow keys work immediately.
    if let Ok(Some(el)) = root.query_selector(".mm-list[tabindex]") {
        if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {
            html_el.focus().ok();
        }
    }
}

fn mount_error(msg: &str) {
    let tree = div()
        .class("mm-login")
        .child(div().class("mm-login__title").text("Music Magic"))
        .child(div().class("mm-login__error").text(msg));
    mount(&tree.into());
}

/// Redirect to the login screen. Called by the api module on HTTP 401.
pub(crate) fn redirect_to_login() {
    api::stop_event_stream();
    ACTIVE_RESOLUTION.with(|cell| cell.borrow_mut().take());
    LAST_STATUS.with(|cell| cell.borrow_mut().take());
    mount(&render_login(Some("Session expired")));
}

/// Start the WebSocket event stream for server-pushed status updates.
/// Called once after login succeeds (or when the app initializes with a valid token).
fn start_event_stream() {
    api::start_event_stream(|status| {
        let doc = match web_sys::window().and_then(|w| w.document()) {
            Some(d) => d,
            None => return,
        };

        // Detect generation counter changes before storing the new status.
        let data_changed = LAST_STATUS.with(|cell| {
            let prev = cell.borrow();
            match prev.as_ref() {
                Some(prev) => {
                    prev.mutations_generation != status.mutations_generation
                        || prev.computations_generation != status.computations_generation
                        || prev.error_generation != status.error_generation
                        || prev.config_generation != status.config_generation
                }
                None => true, // First push — treat as changed
            }
        });

        // Store the latest status for use by view loading and on-demand reads.
        LAST_STATUS.with(|cell| *cell.borrow_mut() = Some(status.clone()));

        // Update the global status bar footer on every push.
        if let Some(el) = doc.get_element_by_id("mm-status-bar") {
            let build_id = BUILD_ID.with(|cell| cell.borrow().clone());
            let node = views::render_status_bar_inner(&status, &build_id);
            el.set_inner_html(&node.to_html());
        }

        // Update fetch progress if visible (ExternalMatches view).
        if let Some(el) = doc.get_element_by_id("mm-fetch-progress") {
            let node = views::render_fetch_progress_section(&status);
            el.set_inner_html(&node.to_html());
        }

        // On generation counter change, refresh view-specific data that isn't in WitchStatus.
        if data_changed {
            spawn_local(async {
                let doc = web_sys::window().unwrap().document().unwrap();

                // Refresh insights data if Health view is active.
                if doc.get_element_by_id("mm-insights-section").is_some() {
                    if let Ok(insights) = api::get_insights().await {
                        if let Some(el) = doc.get_element_by_id("mm-insights-section") {
                            let node = views::render_insights_content(&insights);
                            el.set_inner_html(&node.to_html());
                        }
                    }
                }

                // Refresh external matches data if that view is active.
                if doc.get_element_by_id("mm-external-data").is_some() {
                    if let Ok(data) = api::get_external_matches().await {
                        if let Some(el) = doc.get_element_by_id("mm-external-data") {
                            let node = views::render_external_matches_data(&data);
                            el.set_inner_html(&node.to_html());
                        }
                    }
                }
            });
        }
    });
}

/// Get the latest pushed WitchStatus, or fetch it via HTTP if none available yet.
async fn get_status_cached() -> Result<mm_meta::witch_types::WitchStatus, JsValue> {
    let cached = LAST_STATUS.with(|cell| cell.borrow().clone());
    match cached {
        Some(s) => Ok(s),
        None => api::get_status().await,
    }
}

// ============================================================================
// View Renderers (login / shell)
// ============================================================================

fn render_setup_form(error: Option<&str>, suggested_root: Option<&str>) -> Node {
    let mut form = div().class("mm-login");
    form = form.child(div().class("mm-login__title").text("Music Magic"));
    form = form.child(div().class("mm-login__subtitle").text("First-Time Setup"));

    let mut root_input = html::input()
        .attr("type", "text")
        .attr("id", "setup-root")
        .attr("placeholder", "/path/to/music")
        .attr("onkeydown", "if(event.key==='Enter')window.__mm_setup()");
    if let Some(root) = suggested_root {
        root_input = root_input.attr("value", root);
    }

    let fields = div()
        .class("mm-login__form")
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Archive Root"))
                .child(root_input),
        )
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Username"))
                .child(
                    html::input()
                        .attr("type", "text")
                        .attr("id", "setup-user")
                        .attr("autocomplete", "username")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_setup()"),
                ),
        )
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Password"))
                .child(
                    html::input()
                        .attr("type", "password")
                        .attr("id", "setup-pass")
                        .attr("autocomplete", "new-password")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_setup()"),
                ),
        )
        .child(
            html::button()
                .class("mm-login__submit")
                .attr("onclick", "window.__mm_setup()")
                .text("Complete Setup"),
        );

    form = form.child(fields);

    if let Some(err) = error {
        form = form.child(div().class("mm-login__error").text(err));
    }

    form.into()
}

fn render_login(error: Option<&str>) -> Node {
    let mut form = div().class("mm-login");
    form = form.child(div().class("mm-login__title").text("Music Magic"));

    let fields = div()
        .class("mm-login__form")
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Username"))
                .child(
                    html::input()
                        .attr("type", "text")
                        .attr("id", "login-user")
                        .attr("autocomplete", "username")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_login()"),
                ),
        )
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Password"))
                .child(
                    html::input()
                        .attr("type", "password")
                        .attr("id", "login-pass")
                        .attr("autocomplete", "current-password")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_login()"),
                ),
        )
        .child(
            html::button()
                .class("mm-login__submit")
                .attr("onclick", "window.__mm_login()")
                .text("Login"),
        );

    form = form.child(fields);

    if let Some(err) = error {
        form = form.child(div().class("mm-login__error").text(err));
    }

    form.into()
}

fn render_app_shell(
    active_view: LateralView,
    transactions_open: bool,
    content: Node,
    status: Option<&mm_meta::witch_types::WitchStatus>,
) -> Node {
    let build_id = BUILD_ID.with(|cell| cell.borrow().clone());
    let status_inner = if let Some(s) = status {
        views::render_status_bar_inner(s, &build_id)
    } else {
        div().child(div().class("mm-status__line").text("disconnected")).into()
    };
    let status_bar = footer()
        .class("mm-status")
        .attr("id", "mm-status-bar")
        .child(status_inner);

    div()
        .attr("id", "mm-app")
        .child(render_titlebar(active_view, transactions_open))
        .child(div().class("mm-content").child(content))
        .child(status_bar)
        .into()
}


// ============================================================================
// View Loading
// ============================================================================

/// Fetch data and render content for a route.
async fn load_view_for_route(route: &Route) -> Result<Node, JsValue> {
    match route {
        // -- Lateral views --
        Route::Health(_) => {
            let insights = api::get_insights().await.ok();
            if let Some(ref ins) = insights {
                Ok(views::render_insights_content(ins))
            } else {
                Ok(html::span().class("mm-kv__val").text("Loading insights\u{2026}").into())
            }
        }
        Route::Config(_) => {
            let config_json = api::get_config_json().await?;
            let config: mm_meta::config::Config = serde_json::from_value(config_json)
                .map_err(|e| JsValue::from_str(&format!("deserialize config: {e}")))?;
            let kdl = api::get_config_kdl().await.ok();
            let state = mm_ui::config_editor::ConfigEditorState::new(&config, kdl);
            let node = views::render_config_editor(&state);
            CONFIG_EDITOR.with(|cell| *cell.borrow_mut() = Some(state));
            Ok(node)
        }
        Route::Deploy(_) => {
            let data = api::get_deploy_status().await?;
            let modal = if data.needs_action {
                api::get_deploy_data().await.ok()
            } else {
                None
            };
            Ok(views::render_deploy_content(&data, modal.as_ref()))
        }
        Route::History(_) => {
            let data = api::get_edit_history().await?;
            Ok(views::render_edit_history_content(&data))
        }
        Route::ExternalMatches(_) => {
            let data = api::get_external_matches().await?;
            let status = get_status_cached().await.ok();
            Ok(views::render_external_matches_page(&data, status.as_ref()))
        }
        Route::Transaction(_) => {
            let status = get_status_cached().await?;
            let details = api::tx_details().await.unwrap_or_default();
            Ok(views::render_transaction_content(&status, &details))
        }
        Route::Search(_) => {
            Ok(views::render_search_view())
        }
        Route::Files(_) => {
            let data = api::get_directory_listing(None).await?;
            Ok(views::render_files_view(&data))
        }

        // -- Overlay views --
        Route::PackingBrowser(r) => {
            let data = api::get_query_with(
                "packing-browser-data",
                &format!("category_prefix={}", r.category),
            ).await?;
            Ok(views::render_packing_browser_data(&r.category, &data))
        }
        Route::TagEditor(r) => {
            let inode = r.inodes.first().copied().ok_or_else(|| {
                JsValue::from_str("tag editor requires at least one inode")
            })?;
            let tags = api::get_query_with(
                "corpus-tags",
                &format!("inode={inode}"),
            ).await?;
            let path = inode.to_string();
            Ok(views::render_tag_editor(inode, &path, &tags))
        }

        Route::TransactionReview(_) => {
            let status = get_status_cached().await?;
            let details = api::tx_details().await.unwrap_or_default();
            Ok(views::render_transaction_review(&status, &details))
        }

        // -- External match overlay views --
        Route::ExternalMatchOverlay(ref r) => {
            load_external_match_overlay(r).await
        }

        // -- Resolution views --
        Route::Resolution(ref res) => {
            load_resolution_view(res).await
        }

        // KnotBrowser — not yet wired to web renderers.
        // Fall back to insights view for now.
        _ => {
            let insights = api::get_insights().await.ok();
            let mut children = Vec::new();
            if let Some(ref ins) = insights {
                children.push(views::render_insights_content(ins));
            }
            Ok(div().children(children).into())
        }
    }
}

/// Map a Zone enum to the serde variant name expected by the server's `parse_enum`.
fn zone_api_str(zone: &mm_meta::db_types::Zone) -> &'static str {
    use mm_meta::db_types::Zone;
    match zone {
        Zone::Corpus => "Corpus",
        Zone::Library => "Library",
    }
}

/// Fetch data and render content for a resolution route.
///
/// For Dispatchable resolution types, also constructs the typed mm-ui state
/// and stores it in [`ACTIVE_RESOLUTION`] so that [`mm_resolve_action`] can
/// dispatch typed actions without re-fetching data.
async fn load_resolution_view(res: &route::ResolutionRoute) -> Result<Node, JsValue> {
    use route::ResolutionRoute;

    // Clear any previously held resolution state. Non-Dispatchable routes
    // don't set it, so this prevents stale state from lingering.
    ACTIVE_RESOLUTION.with(|cell| cell.borrow_mut().take());

    match res {
        // === Simple-batch routes (Dispatchable) ===
        ResolutionRoute::MissingDirectories { .. } => {
            let raw = api::get_query("missing-directory-data").await?;
            let typed: mm_meta::views::health_modals::MissingDirectoryModalData =
                api_deserialize(&raw, "MissingDirectoryModalData")?;
            let data = mm_ui::resolutions::missing_directory::MissingDirectoryData(typed);
            let state = mm_ui::resolutions::missing_directory::MissingDirectoryState::new(data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::MissingDirectory(state));
            });
            Ok(views::render_missing_directories(&raw))
        }
        ResolutionRoute::CorruptFiles { .. } => {
            let raw = api::get_query("corrupt-file-data").await?;
            let typed: mm_meta::views::health_modals::CorruptFileModalData =
                api_deserialize(&raw, "CorruptFileModalData")?;
            let data = mm_ui::resolutions::corrupt_file::CorruptFileData(typed);
            let state = mm_ui::resolutions::corrupt_file::CorruptFileState::new(data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::CorruptFile(state));
            });
            Ok(views::render_corrupt_files(&raw))
        }
        ResolutionRoute::MovedFiles { .. } => {
            let raw = api::get_query("moved-files").await?;
            let typed: Vec<mm_meta::views::MovedFileInfo> =
                api_deserialize(&raw, "Vec<MovedFileInfo>")?;
            let data = mm_ui::resolutions::moved_file::MovedFileData { files: typed };
            let state = mm_ui::resolutions::moved_file::MovedFileState::new(data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::MovedFile(state));
            });
            Ok(views::render_moved_files(&raw))
        }
        ResolutionRoute::SubparDuplicates { .. } => {
            let raw = api::get_query("subpar-duplicate-data").await?;
            let typed: mm_meta::views::health_modals::SubparDuplicateModalData =
                api_deserialize(&raw, "SubparDuplicateModalData")?;
            let data = mm_ui::resolutions::subpar_duplicate::SubparDuplicateData(typed);
            let state = mm_ui::resolutions::subpar_duplicate::SubparDuplicateState::new(data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::SubparDuplicate(state));
            });
            Ok(views::render_subpar_duplicates(&raw))
        }
        ResolutionRoute::LosslessRemux { .. } => {
            let raw = api::get_query("lossless-remux-data").await?;
            let typed: mm_meta::views::cluster_deploy::LosslessRemuxModalData =
                api_deserialize(&raw, "LosslessRemuxModalData")?;
            let state = mm_ui::resolutions::lossless_remux::LosslessRemuxPreviewState::new(typed);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::LosslessRemux(state));
            });
            Ok(views::render_lossless_remux(&raw))
        }
        ResolutionRoute::MissingFilesRestorable { .. } => {
            let raw = api::get_query("missing-file-data").await?;
            let typed: mm_meta::views::health_modals::MissingFileModalData =
                api_deserialize(&raw, "MissingFileModalData")?;
            let state = mm_ui::resolutions::missing_file::MissingFilePreviewState::new(typed);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::MissingFile(state));
            });
            Ok(views::render_missing_files_restorable(&raw))
        }
        ResolutionRoute::MissingFilesPermanent { .. } => {
            let raw = api::get_query("missing-file-data").await?;
            let typed: mm_meta::views::health_modals::MissingFileModalData =
                api_deserialize(&raw, "MissingFileModalData")?;
            let state = mm_ui::resolutions::missing_file::MissingFilePreviewState::new(typed);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::MissingFile(state));
            });
            Ok(views::render_missing_files_permanent(&raw))
        }
        ResolutionRoute::OobResolution { bucket, .. } => {
            let data = match bucket {
                Some(b) => {
                    let bucket_str = match b {
                        mm_meta::views::ConflictBucket::MtimeOnly => "MtimeOnly",
                        mm_meta::views::ConflictBucket::DbOnly => "DbOnly",
                        mm_meta::views::ConflictBucket::DiskOnly => "DiskOnly",
                        mm_meta::views::ConflictBucket::Conflict => "Conflict",
                    };
                    api::get_query_with(
                        "oob-files",
                        &format!("bucket={bucket_str}"),
                    ).await?
                }
                None => api::get_query("oob-files").await?,
            };
            let title = match bucket {
                Some(mm_meta::views::ConflictBucket::MtimeOnly) => "OOB Resolution — Mtime Only",
                Some(mm_meta::views::ConflictBucket::DbOnly) => "OOB Resolution — DB Only",
                Some(mm_meta::views::ConflictBucket::DiskOnly) => "OOB Resolution — Disk Only",
                Some(mm_meta::views::ConflictBucket::Conflict) => "OOB Resolution — Two-Way",
                None => "OOB Resolution — All",
            };
            Ok(views::render_oob_conflict_bucket(title, &data))
        }

        // === Cluster-nav routes (Dispatchable) ===
        ResolutionRoute::TagCanonicity { tag_name, zone, .. } => {
            let raw = api::get_query_with(
                "tag-canonicity-resolution",
                &format!(
                    "tag_name={}&zone={}&filter_existing_canonicals=true",
                    js_sys::encode_uri_component(tag_name),
                    zone_api_str(zone),
                ),
            ).await?;
            let typed: mm_meta::views::canonicity_compound::TagCanonicityResolutionData =
                api_deserialize(&raw, "TagCanonicityResolutionData")?;
            let prefill: String = typed.clusters.first()
                .and_then(|c| c.suggested_canonical.clone())
                .unwrap_or_default();
            let ui_data = mm_ui::resolutions::tag_canonicity::TagCanonicityData::new(
                typed,
                *zone,
                mm_ui::resolutions::tag_canonicity::CanonicityMode::TagCanonicity,
            );
            let state = mm_ui::resolutions::tag_canonicity::TagCanonicityViewState::new(
                ui_data, "Squash to:", &prefill,
            );
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::TagCanonicity {
                    state,
                    title_prefix: "Tag Canonicity".into(),
                });
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::CompoundSplit { tag_name, zone, safe_mode, .. } => {
            let raw = api::get_query_with(
                "compound-split-resolution",
                &format!(
                    "tag_name={}&zone={}&safe_only={}",
                    js_sys::encode_uri_component(tag_name),
                    zone_api_str(zone),
                    safe_mode,
                ),
            ).await?;
            let typed: mm_meta::views::canonicity_compound::CompoundSplitResolutionData =
                api_deserialize(&raw, "CompoundSplitResolutionData")?;
            let prefill = typed.groups.first()
                .map(|g| g.split_parts.join("; "))
                .unwrap_or_default();
            let ui_data = mm_ui::resolutions::compound_split::CompoundSplitData::new(
                typed, *zone, *safe_mode,
            );
            let state = mm_ui::resolutions::compound_split::CompoundSplitViewState::new(
                ui_data, &prefill,
            );
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::CompoundSplit(state));
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::MissingAlbum { .. } => {
            let raw = api::get_query("missing-album-single-signals").await?;
            let typed: Vec<mm_meta::domain_queries::MissingAlbumSingleSignalWire> =
                api_deserialize(&raw, "Vec<MissingAlbumSingleSignalWire>")?;
            let config = fetch_config().await?;
            let suffix = config.opinions.health_detection.single_album_suffix.clone();
            let ui_data = mm_ui::resolutions::missing_album::MissingAlbumData::new(
                typed, suffix,
            );
            let state = mm_ui::resolutions::missing_album::MissingAlbumState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::MissingAlbum(state));
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::DirectoryCluster { .. } => {
            let raw = api::get_query("directory-cluster-data").await?;
            let typed: mm_meta::views::cluster_deploy::DirectoryClusterModalData =
                api_deserialize(&raw, "DirectoryClusterModalData")?;
            let ui_data = mm_ui::resolutions::directory_cluster::DirectoryClusterData::new(typed);
            let state = mm_ui::resolutions::directory_cluster::DirectoryClusterState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::DirectoryCluster(state));
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::InconsistentAlbumArtist { tag_name, .. } => {
            let raw = api::get_query_with(
                "tag-canonicity-resolution",
                &format!(
                    "tag_name={}&zone={}&filter_existing_canonicals=true",
                    js_sys::encode_uri_component(tag_name),
                    zone_api_str(&mm_meta::db_types::Zone::Corpus),
                ),
            ).await?;
            let typed: mm_meta::views::canonicity_compound::TagCanonicityResolutionData =
                api_deserialize(&raw, "TagCanonicityResolutionData")?;
            let prefill: String = typed.clusters.first()
                .and_then(|c| c.suggested_canonical.clone())
                .unwrap_or_default();
            let ui_data = mm_ui::resolutions::tag_canonicity::TagCanonicityData::new(
                typed,
                mm_meta::db_types::Zone::Corpus,
                mm_ui::resolutions::tag_canonicity::CanonicityMode::InconsistentAlbumArtist,
            );
            let state = mm_ui::resolutions::tag_canonicity::TagCanonicityViewState::new(
                ui_data, "Album artist:", &prefill,
            );
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::TagCanonicity {
                    state,
                    title_prefix: "Inconsistent Album Artist".into(),
                });
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::DiscExtraction { .. } => {
            let raw = api::get_query_with(
                "disc-extraction-data",
                "map_letters_to_numbers=false",
            ).await?;
            let typed: mm_meta::domain_queries::DiscExtractionModalData =
                api_deserialize(&raw, "DiscExtractionModalData")?;
            let config = fetch_config().await?;
            let disc_tag = config.opinions.disc_extraction.disc_tag_name.clone();
            let ui_data = mm_ui::resolutions::disc_extraction::DiscExtractionData::new(
                typed, disc_tag,
            );
            let state = mm_ui::resolutions::disc_extraction::DiscExtractionState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::DiscExtraction(state));
            });
            Ok(render_active_resolution_node())
        }

        // === Group-review routes (Dispatchable) ===
        ResolutionRoute::RedundantDuplicates { .. } => {
            let raw = api::get_query_with(
                "manual-review-data",
                "kind=RedundantDuplicate",
            ).await?;
            let typed: mm_meta::views::review_match::ManualReviewData =
                api_deserialize(&raw, "ManualReviewData")?;
            let ui_data = mm_ui::resolutions::manual_review::ManualReviewResolutionData::new(
                typed,
                mm_meta::views::review_match::ReviewKind::RedundantDuplicate,
            );
            let state = mm_ui::resolutions::manual_review::ManualReviewState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::ManualReview {
                    state,
                    title: "Redundant Duplicates".into(),
                });
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::DeployConflicts { .. } => {
            let raw = api::get_query_with(
                "manual-review-data",
                "kind=DeployConflict",
            ).await?;
            let typed: mm_meta::views::review_match::ManualReviewData =
                api_deserialize(&raw, "ManualReviewData")?;
            let ui_data = mm_ui::resolutions::manual_review::ManualReviewResolutionData::new(
                typed,
                mm_meta::views::review_match::ReviewKind::DeployConflict,
            );
            let state = mm_ui::resolutions::manual_review::ManualReviewState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::ManualReview {
                    state,
                    title: "Deploy Conflicts".into(),
                });
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::MetadataDuplicates { .. } => {
            let raw = api::get_query_with(
                "manual-review-data",
                "kind=MetadataDuplicate",
            ).await?;
            let typed: mm_meta::views::review_match::ManualReviewData =
                api_deserialize(&raw, "ManualReviewData")?;
            let ui_data = mm_ui::resolutions::manual_review::ManualReviewResolutionData::new(
                typed,
                mm_meta::views::review_match::ReviewKind::MetadataDuplicate,
            );
            let state = mm_ui::resolutions::manual_review::ManualReviewState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::ManualReview {
                    state,
                    title: "Metadata Duplicates".into(),
                });
            });
            Ok(render_active_resolution_node())
        }
        ResolutionRoute::SameRecording { .. } => {
            let raw = api::get_query_with(
                "manual-review-data",
                "kind=SameRecordingDifferentRelease",
            ).await?;
            let typed: mm_meta::views::review_match::ManualReviewData =
                api_deserialize(&raw, "ManualReviewData")?;
            let ui_data = mm_ui::resolutions::manual_review::ManualReviewResolutionData::new(
                typed,
                mm_meta::views::review_match::ReviewKind::SameRecordingDifferentRelease,
            );
            let state = mm_ui::resolutions::manual_review::ManualReviewState::new(ui_data);
            ACTIVE_RESOLUTION.with(|cell| {
                *cell.borrow_mut() = Some(ActiveResolution::ManualReview {
                    state,
                    title: "Same Recording".into(),
                });
            });
            Ok(render_active_resolution_node())
        }

    }
}

/// Deserialize a serde_json::Value into a typed struct, with a readable error.
fn api_deserialize<T: serde::de::DeserializeOwned>(
    val: &serde_json::Value,
    type_name: &str,
) -> Result<T, JsValue> {
    serde_json::from_value(val.clone())
        .map_err(|e| JsValue::from_str(&format!("deserialize {type_name}: {e}")))
}

/// Fetch and deserialize the server config. Uses the cached editor state if
/// available, otherwise fetches from the server.
async fn fetch_config() -> Result<mm_meta::config::Config, JsValue> {
    let cached = CONFIG_EDITOR.with(|cell| {
        cell.borrow().as_ref().map(|s| s.original_config.clone())
    });
    match cached {
        Some(c) => Ok(c),
        None => {
            let json = api::get_config_json().await?;
            serde_json::from_value(json)
                .map_err(|e| JsValue::from_str(&format!("deserialize Config: {e}")))
        }
    }
}

/// Fetch data and render content for an external match overlay route.
async fn load_external_match_overlay(r: &route::ExternalMatchRoute) -> Result<Node, JsValue> {
    use route::ExternalMatchRoute;
    match r {
        ExternalMatchRoute::AcoustidBrowse { confidence, .. } => {
            let data = api::get_query_with(
                "acoustid-matches",
                &format!("confidence={}", serde_json::to_value(confidence)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| format!("{confidence:?}"))),
            ).await?;
            Ok(views::render_acoustid_matches(&data))
        }
        ExternalMatchRoute::ReleaseReview { filter, .. } => {
            let data: mm_meta::views::external_matches::ReleaseReviewData = {
                let raw = api::get_query_with(
                    "release-review",
                    &format!("filter={}", serde_json::to_value(filter)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_else(|| format!("{filter:?}"))),
                ).await?;
                serde_json::from_value(raw)
                    .map_err(|e| JsValue::from_str(&format!("deserialize release-review: {e}")))?
            };
            Ok(views::render_release_review(&data))
        }
        ExternalMatchRoute::ReleaseDetail { release_id, .. } => {
            // Single release detail query not yet implemented server-side.
            Ok(views::render_release_detail_placeholder(release_id))
        }
    }
}

/// Parse the current URL hash into a Route, load its content, and mount.
async fn load_from_hash() -> Result<(), JsValue> {
    let route = current_route();
    let lateral = lateral_view_for_route(&route);

    // Use cached pushed status for transaction tab visibility and status bar.
    let status = get_status_cached().await.ok();
    let tx_open = status.as_ref().map_or(false, |s| s.transaction.is_some());

    let content = load_view_for_route(&route).await?;
    mount(&render_app_shell(lateral, tx_open, content, status.as_ref()));
    Ok(())
}


// ============================================================================
// Global callbacks (wasm_bindgen exports → window globals via index.html)
// ============================================================================

#[wasm_bindgen]
pub fn mm_login() {
    spawn_local(async {
        if let Err(e) = do_login().await {
            web_sys::console::error_1(&format!("login error: {e:?}").into());
        }
    });
}

async fn do_login() -> Result<(), JsValue> {
    let doc = web_sys::window().unwrap().document().unwrap();
    let user_el = doc
        .get_element_by_id("login-user")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?;
    let pass_el = doc
        .get_element_by_id("login-pass")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?;

    match api::login(&user_el.value(), &pass_el.value()).await {
        Ok(_) => {
            start_event_stream();
            navigate_to(&Route::Health(Default::default()));
            load_from_hash().await?;
            Ok(())
        }
        Err(e) => {
            mount(&render_login(Some(&format!("{e:?}"))));
            Ok(())
        }
    }
}

#[wasm_bindgen]
pub fn mm_navigate(view_name: &str) {

    let view = match view_name {
        "Config" => LateralView::Config,
        "Search" => LateralView::Search,
        "Files" => LateralView::Files,
        "Health" => LateralView::Health,
        "History" => LateralView::History,
        "Transaction" => LateralView::Transaction,
        "Deploy" => LateralView::Deploy,
        "Ext. Authorities" => LateralView::ExternalMatches,
        _ => return,
    };
    navigate_to(&route_for_lateral(view));
    spawn_local(async move {
        if let Err(e) = load_from_hash().await {
            web_sys::console::error_1(&format!("navigate error: {e:?}").into());
        }
    });
}

#[wasm_bindgen]
pub fn mm_logout() {
    api::clear_token();
    mount(&render_login(None));
}

/// Cancel a resolution modal — navigate back to the Health view.
#[wasm_bindgen]
pub fn mm_resolve_cancel() {

    ACTIVE_RESOLUTION.with(|cell| cell.borrow_mut().take());
    navigate_to(&Route::Health(Default::default()));
    spawn_local(async move {
        if let Err(e) = load_from_hash().await {
            web_sys::console::error_1(&format!("resolve cancel error: {e:?}").into());
        }
    });
}

/// Dispatch a resolution action against the held `ACTIVE_RESOLUTION` state.
///
/// `action_name` is the string name of the action variant (e.g. "Confirm",
/// "FlagCanonical", "Stash", "Skip", "Cancel", etc.). Each resolution type
/// maps these strings to its typed action enum.
///
/// On `Stage`/`StageKeep`: starts a transaction (if needed), adds the decision,
/// then either advances to the next group or navigates to transaction review.
/// On `Skip`: advances without staging. On `Cancel`: navigates to Health.
#[wasm_bindgen]
pub fn mm_resolve_action(action_name: &str) {
    let action = action_name.to_string();
    spawn_local(async move {
        if let Err(e) = do_resolve_action(&action).await {
            web_sys::console::error_1(&format!("resolve action error: {e:?}").into());
        }
    });
}

/// Inner async handler for `mm_resolve_action`.
async fn do_resolve_action(action_name: &str) -> Result<(), JsValue> {
    // Take the state out of the thread_local so we can mutate it (for advance).
    let state = ACTIVE_RESOLUTION.with(|cell| cell.borrow_mut().take());
    let Some(mut state) = state else {
        return Err(JsValue::from_str("no active resolution state"));
    };

    // Build a PathResolver from config for dispatch calls that need path resolution.
    let config = fetch_config().await?;
    let resolver = PathResolver::from_config(&config);

    // Dispatch the action and get the result.
    let result = dispatch_active_resolution(&state, action_name, &resolver)?;

    match result {
        DispatchResult::Stage { key, label, mutations } => {
            stage_decision(&key, &label, &mutations).await?;
            let has_more = advance_active_resolution(&mut state);
            if has_more {
                // Put state back and re-render from held state (no re-fetch).
                ACTIVE_RESOLUTION.with(|cell| *cell.borrow_mut() = Some(state));
                render_active_resolution();
            } else {
                // Last group processed — navigate to transaction review.
                navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
                load_from_hash().await?;
            }
        }
        DispatchResult::StageKeep { key, label, mutations } => {
            stage_decision(&key, &label, &mutations).await?;
            // Don't advance — put state back and re-render from held state.
            ACTIVE_RESOLUTION.with(|cell| *cell.borrow_mut() = Some(state));
            render_active_resolution();
        }
        DispatchResult::Skip => {
            let has_more = advance_active_resolution(&mut state);
            if has_more {
                ACTIVE_RESOLUTION.with(|cell| *cell.borrow_mut() = Some(state));
                render_active_resolution();
            } else {
                navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
                load_from_hash().await?;
            }
        }
        DispatchResult::Cancel => {
            navigate_to(&Route::Health(Default::default()));
            load_from_hash().await?;
        }
        DispatchResult::Handled => {
            // No-op — put state back unchanged.
            ACTIVE_RESOLUTION.with(|cell| *cell.borrow_mut() = Some(state));
        }
    }
    Ok(())
}

/// Map an action name string to a typed action and dispatch it on the active resolution.
fn dispatch_active_resolution(
    state: &ActiveResolution,
    action_name: &str,
    resolver: &PathResolver,
) -> Result<DispatchResult, JsValue> {
    match state {
        ActiveResolution::TagCanonicity { state: s, .. } => {
            use mm_ui::resolutions::tag_canonicity::CanonicityAction;
            let action = match action_name {
                "Confirm" => CanonicityAction::Confirm,
                "FlagCanonical" => CanonicityAction::FlagCanonical,
                "Cancel" => CanonicityAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown TagCanonicity action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::CompoundSplit(s) => {
            use mm_ui::resolutions::compound_split::CompoundSplitAction;
            let action = match action_name {
                "Confirm" => CompoundSplitAction::Confirm,
                "Canonicalize" => CompoundSplitAction::Canonicalize,
                "Cancel" => CompoundSplitAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown CompoundSplit action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::DirectoryCluster(s) => {
            use mm_ui::resolutions::directory_cluster::DirectoryClusterAction;
            let action = match action_name {
                "Stash" => DirectoryClusterAction::Stash,
                "MarkExpected" => DirectoryClusterAction::MarkExpected,
                "Cancel" => DirectoryClusterAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown DirectoryCluster action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::ManualReview { state: s, .. } => {
            use mm_ui::resolutions::manual_review::ReviewAction;
            let action = match action_name {
                "Stash" => ReviewAction::Stash,
                "MarkExpected" => ReviewAction::MarkExpected,
                "Cancel" => ReviewAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown ManualReview action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::MissingAlbum(s) => {
            use mm_ui::resolutions::missing_album::MissingAlbumAction;
            let action = match action_name {
                "PerTrackTitle" => MissingAlbumAction::PerTrackTitle,
                "AllSingles" => MissingAlbumAction::AllSingles,
                "Suppress" => MissingAlbumAction::Suppress,
                "Cancel" => MissingAlbumAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown MissingAlbum action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::DiscExtraction(s) => {
            use mm_ui::resolutions::disc_extraction::DiscExtractionAction;
            let action = match action_name {
                "Apply" => DiscExtractionAction::Apply,
                "Skip" => DiscExtractionAction::Skip,
                "Cancel" => DiscExtractionAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown DiscExtraction action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::MissingDirectory(s) => {
            use mm_ui::resolutions::missing_directory::MissingDirectoryAction;
            let action = match action_name {
                "ConfirmDrop" => MissingDirectoryAction::ConfirmDrop,
                "Cancel" => MissingDirectoryAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown MissingDirectory action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::CorruptFile(s) => {
            use mm_ui::resolutions::corrupt_file::CorruptFileAction;
            let action = match action_name {
                "ConfirmStashAll" => CorruptFileAction::ConfirmStashAll,
                "Cancel" => CorruptFileAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown CorruptFile action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::MovedFile(s) => {
            use mm_ui::resolutions::moved_file::MovedFileAction;
            let action = match action_name {
                "Acknowledge" => MovedFileAction::Acknowledge,
                "Cancel" => MovedFileAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown MovedFile action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::SubparDuplicate(s) => {
            use mm_ui::resolutions::subpar_duplicate::SubparDuplicateAction;
            let action = match action_name {
                "ConfirmStashAll" => SubparDuplicateAction::ConfirmStashAll,
                "Cancel" => SubparDuplicateAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown SubparDuplicate action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::MissingFile(s) => {
            use mm_ui::resolutions::missing_file::MissingFileAction;
            let action = match action_name {
                "ConfirmRestore" => MissingFileAction::ConfirmRestore,
                "ConfirmDrop" => MissingFileAction::ConfirmDrop,
                "Cancel" => MissingFileAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown MissingFile action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
        ActiveResolution::LosslessRemux(s) => {
            use mm_ui::resolutions::lossless_remux::LosslessRemuxAction;
            let action = match action_name {
                "Confirm" => LosslessRemuxAction::Confirm,
                "Cancel" => LosslessRemuxAction::Cancel,
                _ => return Err(JsValue::from_str(&format!("unknown LosslessRemux action: {action_name}"))),
            };
            Ok(s.dispatch(action, resolver))
        }
    }
}

/// Advance the active resolution state to the next group.
/// Returns `true` if there are more groups, `false` if the last group was processed.
fn advance_active_resolution(state: &mut ActiveResolution) -> bool {
    match state {
        ActiveResolution::TagCanonicity { state: s, .. } => s.advance(),
        ActiveResolution::CompoundSplit(s) => s.advance(),
        ActiveResolution::DirectoryCluster(s) => s.advance(),
        ActiveResolution::ManualReview { state: s, .. } => s.advance(),
        ActiveResolution::MissingAlbum(s) => s.advance(),
        ActiveResolution::DiscExtraction(s) => s.advance(),
        // Simple-batch modals are single-shot — no group advancement.
        ActiveResolution::MissingDirectory(_)
        | ActiveResolution::CorruptFile(_)
        | ActiveResolution::MovedFile(_)
        | ActiveResolution::SubparDuplicate(_)
        | ActiveResolution::MissingFile(_)
        | ActiveResolution::LosslessRemux(_) => false,
    }
}

/// Render the current group of the active resolution as a Node.
///
/// Called from `load_resolution_view()` when building the initial page content.
/// Borrows `ACTIVE_RESOLUTION` and matches the variant to call the appropriate
/// per-group renderer from views.rs.
fn render_active_resolution_node() -> Node {
    ACTIVE_RESOLUTION.with(|cell| {
        let borrow = cell.borrow();
        match borrow.as_ref() {
            Some(ActiveResolution::TagCanonicity { state, title_prefix }) => {
                views::render_canonicity_group_titled(title_prefix, state)
            }
            Some(ActiveResolution::CompoundSplit(state)) => {
                views::render_compound_split_group(state)
            }
            Some(ActiveResolution::DirectoryCluster(state)) => {
                views::render_directory_cluster_group(state)
            }
            Some(ActiveResolution::ManualReview { state, title }) => {
                views::render_manual_review_group(title, state)
            }
            Some(ActiveResolution::MissingAlbum(state)) => {
                views::render_missing_album_group(state)
            }
            Some(ActiveResolution::DiscExtraction(state)) => {
                views::render_disc_extraction_group(state)
            }
            // Simple-batch modals don't use per-group rendering.
            _ => html::span().class("mm-kv__val").text("No active resolution").into(),
        }
    })
}

/// Re-render the active resolution into the DOM content area.
///
/// Used after advance/skip to update the display without re-fetching data
/// from the server. Replaces the content inside `.mm-content`.
fn render_active_resolution() {
    let node = render_active_resolution_node();
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Ok(Some(el)) = doc.query_selector(".mm-content") {
        el.set_inner_html(&node.to_html());
    }
}

/// Navigate between groups in the active resolution (prev/next pagination).
///
/// Purely navigational — no mutations. Adjusts the current group index on the
/// held state and re-renders.
#[wasm_bindgen]
pub fn mm_resolve_page(direction: &str) {
    ACTIVE_RESOLUTION.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };

        match state {
            ActiveResolution::TagCanonicity { state: s, .. } => {
                let total = s.state.data.inner.clusters.len();
                let cur = s.state.data.current_cluster;
                match direction {
                    "prev" if cur > 0 => {
                        s.state.data.current_cluster -= 1;
                        s.state.reset_list();
                        let prefill = s.state.data.inner.clusters
                            .get(s.state.data.current_cluster)
                            .and_then(|c| c.suggested_canonical.as_deref())
                            .unwrap_or("");
                        s.field.set_value(prefill);
                    }
                    "next" if cur + 1 < total => {
                        s.state.data.current_cluster += 1;
                        s.state.reset_list();
                        let prefill = s.state.data.inner.clusters
                            .get(s.state.data.current_cluster)
                            .and_then(|c| c.suggested_canonical.as_deref())
                            .unwrap_or("");
                        s.field.set_value(prefill);
                    }
                    _ => {}
                }
            }
            ActiveResolution::CompoundSplit(s) => {
                let total = s.state.data.inner.groups.len();
                let cur = s.state.data.current_group;
                match direction {
                    "prev" if cur > 0 => {
                        s.state.data.current_group -= 1;
                        s.state.reset_list();
                        let prefill = s.state.data.inner.groups
                            .get(s.state.data.current_group)
                            .map(|g| g.split_parts.join("; "))
                            .unwrap_or_default();
                        s.field.set_value(&prefill);
                    }
                    "next" if cur + 1 < total => {
                        s.state.data.current_group += 1;
                        s.state.reset_list();
                        let prefill = s.state.data.inner.groups
                            .get(s.state.data.current_group)
                            .map(|g| g.split_parts.join("; "))
                            .unwrap_or_default();
                        s.field.set_value(&prefill);
                    }
                    _ => {}
                }
            }
            ActiveResolution::DirectoryCluster(s) => {
                let total = s.data.inner.clusters.len();
                let cur = s.data.current_cluster;
                match direction {
                    "prev" if cur > 0 => {
                        s.data.current_cluster -= 1;
                        s.reset_list();
                    }
                    "next" if cur + 1 < total => {
                        s.data.current_cluster += 1;
                        s.reset_list();
                    }
                    _ => {}
                }
            }
            ActiveResolution::ManualReview { state: s, .. } => {
                let total = s.data.inner.groups.len();
                let cur = s.data.current_group;
                match direction {
                    "prev" if cur > 0 => {
                        s.data.current_group -= 1;
                        s.reset_list();
                    }
                    "next" if cur + 1 < total => {
                        s.data.current_group += 1;
                        s.reset_list();
                    }
                    _ => {}
                }
            }
            ActiveResolution::MissingAlbum(s) => {
                let total = s.data.signals.len();
                let cur = s.data.current_group;
                match direction {
                    "prev" if cur > 0 => {
                        s.data.current_group -= 1;
                        s.reset_list();
                    }
                    "next" if cur + 1 < total => {
                        s.data.current_group += 1;
                        s.reset_list();
                    }
                    _ => {}
                }
            }
            ActiveResolution::DiscExtraction(s) => {
                let total = s.data.inner.groups.len();
                let cur = s.data.current_group;
                match direction {
                    "prev" if cur > 0 => {
                        s.data.current_group -= 1;
                        s.reset_list();
                    }
                    "next" if cur + 1 < total => {
                        s.data.current_group += 1;
                        s.reset_list();
                    }
                    _ => {}
                }
            }
            // Simple-batch modals have no pagination.
            _ => {}
        }
    });
    render_active_resolution();
}

/// Confirm action for resolutions that need a text field value
/// (TagCanonicity and CompoundSplit).
///
/// Reads the value from `#mm-decision-field` DOM input, sets it on the held
/// state's DecisionField, then dispatches a Confirm action.
#[wasm_bindgen]
pub fn mm_resolve_confirm_with_field() {
    // Read the field value from the DOM input.
    let field_value = web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id("mm-decision-field")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_default();

    // Set the value on the held state's DecisionField.
    ACTIVE_RESOLUTION.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if let Some(state) = borrow.as_mut() {
            match state {
                ActiveResolution::TagCanonicity { state: s, .. } => {
                    s.field.set_value(&field_value);
                }
                ActiveResolution::CompoundSplit(s) => {
                    s.field.set_value(&field_value);
                }
                _ => {}
            }
        }
    });

    // Now dispatch the Confirm action through the normal path.
    spawn_local(async move {
        if let Err(e) = do_resolve_action("Confirm").await {
            web_sys::console::error_1(&format!("resolve confirm_with_field error: {e:?}").into());
        }
    });
}

/// Stage a decision: ensure a transaction is active, then add the decision.
/// Caller navigates to transaction review afterward — never auto-confirms.
async fn stage_decision(
    key: &DecisionKey,
    label: &str,
    mutations: &[mm_meta::mutations::Mutation],
) -> Result<(), JsValue> {
    // Check if a transaction is already active.
    let status = get_status_cached().await?;
    if status.transaction.is_none() {
        api::tx_start(label).await?;
    }

    let decision = Decision {
        label: label.to_string(),
        mutations: mutations.to_vec(),
    };
    api::tx_add(key, &decision).await?;
    Ok(())
}

#[wasm_bindgen]
pub fn mm_tx_confirm() {
    let witness = api::ConfirmationWitness::new();
    spawn_local(async {
        match api::tx_confirm(witness).await {
            Ok(_) => { load_from_hash().await.ok(); }
            Err(e) => web_sys::console::error_1(&format!("tx confirm error: {e:?}").into()),
        }
    });
}

#[wasm_bindgen]
pub fn mm_tx_discard() {
    spawn_local(async {
        match api::tx_discard().await {
            Ok(_) => { load_from_hash().await.ok(); }
            Err(e) => web_sys::console::error_1(&format!("tx discard error: {e:?}").into()),
        }
    });
}

/// Stage deploy mutations into a transaction and navigate to transaction review.
#[wasm_bindgen]
pub fn mm_stage_deploy() {
    spawn_local(async {
        if let Err(e) = do_stage_deploy().await {
            web_sys::console::error_1(&format!("stage deploy error: {e:?}").into());
        }
    });
}

async fn do_stage_deploy() -> Result<(), JsValue> {
    let data = api::get_deploy_data().await?;
    let config = fetch_config().await?;
    let resolver = PathResolver::from_config(&config);
    let prepared = mm_ui::deploy::prepare_deploy_decisions(&data, &resolver);

    if prepared.skipped > 0 {
        web_sys::console::warn_1(
            &format!("{} new files skipped: no library_name", prepared.skipped).into(),
        );
    }

    if prepared.decisions.is_empty() {
        web_sys::console::log_1(&"No deploy operations needed".into());
        return Ok(());
    }

    for dd in &prepared.decisions {
        stage_decision(&dd.key, &dd.label, &dd.mutations).await?;
    }

    navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
    load_from_hash().await?;
    Ok(())
}

#[wasm_bindgen]
pub fn mm_tx_remove(key_json: &str) {
    let json_str = key_json.to_string();
    spawn_local(async move {
        let key: mm_meta::decisions::DecisionKey = match serde_json::from_str(&json_str) {
            Ok(k) => k,
            Err(e) => {
                web_sys::console::error_1(&format!("tx_remove parse error: {e}").into());
                return;
            }
        };
        match api::tx_remove(&key).await {
            Ok(_) => { load_from_hash().await.ok(); }
            Err(e) => web_sys::console::error_1(&format!("tx remove error: {e:?}").into()),
        }
    });
}

/// Navigate to an arbitrary hash route and reload the view.
#[wasm_bindgen]
pub fn mm_navigate_route(hash: &str) {

    let hash = hash.to_string();
    PROGRAMMATIC_NAV.with(|flag| flag.set(true));
    web_sys::window()
        .unwrap()
        .location()
        .set_hash(&hash)
        .ok();
    spawn_local(async move {
        if let Err(e) = load_from_hash().await {
            web_sys::console::error_1(&format!("navigate error: {e:?}").into());
        }
    });
}

#[wasm_bindgen]
pub fn mm_expand_session(session_id: &str) {
    let sid = session_id.to_string();
    spawn_local(async move {
        let target_id = format!("session-{sid}");
        let doc = web_sys::window().unwrap().document().unwrap();
        let container = match doc.get_element_by_id(&target_id) {
            Some(el) => el,
            None => return,
        };
        if !container.inner_html().is_empty() {
            container.set_inner_html("");
            return;
        }
        match api::get_session_detail(&sid).await {
            Ok(typed) => {
                let node = views::render_session_detail_typed(&typed);
                container.set_inner_html(&node.to_html());
            }
            Err(e) => {
                container.set_inner_html(&format!("<span class='mm-kv__val'>Error: {e:?}</span>"));
            }
        }
    });
}

/// Switch active deploy tab (called from tab bar onclick).
///
/// Shows/hides panels by ID and updates tab active classes.
#[wasm_bindgen]
pub fn mm_deploy_tab(tab_name: &str) {
    let doc = web_sys::window().unwrap().document().unwrap();
    // Show/hide content panels.
    for tab in mm_ui::domain_types::DeployTab::all() {
        let panel_id = format!("deploy-tab-{}", tab.label().to_lowercase());
        if let Some(el) = doc.get_element_by_id(&panel_id) {
            let display = if tab.label().to_lowercase() == tab_name {
                ""
            } else {
                "display:none"
            };
            el.set_attribute("style", display).ok();
        }
    }
    // Update active class on tab buttons via className replacement.
    if let Ok(tabs) = doc.query_selector_all(".mm-tab") {
        for i in 0..tabs.length() {
            if let Some(el) = tabs.item(i) {
                if let Some(html_el) = el.dyn_ref::<web_sys::HtmlElement>() {
                    let text = html_el.inner_text().to_lowercase();
                    let is_active = text.starts_with(tab_name);
                    let current = html_el.class_name();
                    let new_class = if is_active {
                        if !current.contains("mm-tab--active") {
                            format!("{current} mm-tab--active")
                        } else {
                            current
                        }
                    } else {
                        current.replace("mm-tab--active", "").trim().to_string()
                    };
                    html_el.set_class_name(&new_class);
                }
            }
        }
    }
}

/// Complete first-time setup.
#[wasm_bindgen]
pub fn mm_setup() {
    spawn_local(async {
        if let Err(e) = do_setup().await {
            web_sys::console::error_1(&format!("setup error: {e:?}").into());
        }
    });
}

async fn do_setup() -> Result<(), JsValue> {
    let doc = web_sys::window().unwrap().document().unwrap();
    let root = doc
        .get_element_by_id("setup-root")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?
        .value();
    let user = doc
        .get_element_by_id("setup-user")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?
        .value();
    let pass = doc
        .get_element_by_id("setup-pass")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?
        .value();

    if root.is_empty() {
        mount(&render_setup_form(Some("Archive root is required"), None));
        return Ok(());
    }

    match api::setup_complete(&root, &user, &pass).await {
        Ok(()) => {
            // Setup done. If credentials were provided, log in automatically.
            if !user.is_empty() && !pass.is_empty() {
                match api::login(&user, &pass).await {
                    Ok(_) => {
                        navigate_to(&Route::Health(Default::default()));
                        load_from_hash().await?;
                    }
                    Err(_) => mount(&render_login(None)),
                }
            } else {
                mount(&render_login(None));
            }
            Ok(())
        }
        Err(e) => {
            mount(&render_setup_form(Some(&format!("{e:?}")), None));
            Ok(())
        }
    }
}

/// Navigate into a packing category browser.
#[wasm_bindgen]
pub fn mm_packing_browse(category: &str) {
    let route = Route::PackingBrowser(route::PackingBrowserRoute {
        category: category.to_string(),
        cursor: None,
    });
    navigate_to(&route);
    spawn_local(async move {
        if let Err(e) = load_from_hash().await {
            web_sys::console::error_1(&format!("packing browse error: {e:?}").into());
        }
    });
}

/// Register a one-time `hashchange` listener on `window`.
///
/// Fires when the user clicks an `<a href="#...">` link or uses browser
/// back/forward.  Programmatic navigations (via [`navigate_to`] /
/// [`mm_navigate_route`]) set [`PROGRAMMATIC_NAV`] so this listener
/// skips the redundant load.
fn register_hashchange_listener() {
    let window = web_sys::window().unwrap();
    let cb = wasm_bindgen::closure::Closure::wrap(Box::new(|| {
        let dominated = PROGRAMMATIC_NAV.with(|flag| {
            let was = flag.get();
            flag.set(false);
            was
        });
        if dominated {
            return;
        }
        if !api::has_token() {
            return;
        }
    
        spawn_local(async {
            if let Err(e) = load_from_hash().await {
                web_sys::console::error_1(&format!("hashchange error: {e:?}").into());
            }
        });
    }) as Box<dyn Fn()>);
    window
        .add_event_listener_with_callback("hashchange", cb.as_ref().unchecked_ref())
        .expect("failed to add hashchange listener");
    cb.forget(); // Lives for the lifetime of the page.
}

/// Queue a background task (e.g. "SchemaReconciliation") and refresh the view.
#[wasm_bindgen]
pub fn mm_queue_task(task: &str) {
    let task = task.to_string();
    spawn_local(async move {
        match api::queue_task(&task).await {
            Ok(resp) => {
                if resp.get("ok").and_then(|v| v.as_bool()) == Some(false) {
                    let reason = resp.get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown error");
                    let window = web_sys::window().unwrap();
                    let _ = window.alert_with_message(&format!("Task failed: {}", reason));
                } else {
                    // Refresh current view to update counts.
                    load_from_hash().await.ok();
                }
            }
            Err(e) => web_sys::console::error_1(&format!("queue-task error: {e:?}").into()),
        }
    });
}

/// Stage a typed protocol binding as a decision and refresh the view.
#[wasm_bindgen]
pub fn mm_stage_decision(binding_json: &str) {
    let json_str = binding_json.to_string();
    spawn_local(async move {
        let binding: serde_json::Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                web_sys::console::error_1(&format!("mm_stage_decision parse error: {e}").into());
                return;
            }
        };
        match api::stage_action(&binding).await {
            Ok(_) => {
                load_from_hash().await.ok();
            }
            Err(e) => web_sys::console::error_1(&format!("mm_stage_decision error: {e:?}").into()),
        }
    });
}


/// Approve releases: load staging data, build decisions, stage to transaction.
#[wasm_bindgen]
pub fn mm_approve_releases(release_ids_json: &str) {
    let json_str = release_ids_json.to_string();
    spawn_local(async move {
        if let Err(e) = do_approve_releases(&json_str).await {
            let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
            web_sys::window()
                .and_then(|w| w.alert_with_message(&format!("Approve failed: {msg}")).ok());
        }
    });
}

async fn do_approve_releases(release_ids_json: &str) -> Result<(), JsValue> {
    let release_ids: Vec<String> = serde_json::from_str(release_ids_json)
        .map_err(|e| JsValue::from_str(&format!("parse release IDs: {e}")))?;

    if release_ids.is_empty() {
        return Ok(());
    }

    // Server builds decisions and stages the transaction.
    let resp = api::post("/tx/approve-releases", &serde_json::json!({
        "release_ids": release_ids,
    })).await?;

    let staged = resp.get("staged").and_then(|v| v.as_u64()).unwrap_or(0);
    let skipped = resp.get("skipped").and_then(|v| v.as_u64()).unwrap_or(0);
    web_sys::console::log_1(
        &format!("[approve] server staged {staged} decisions, skipped {skipped}").into(),
    );

    // Navigate to transaction review.
    navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
    load_from_hash().await?;
    Ok(())
}

/// Debounced server-side search. Cancels any pending search timer, then
/// fires a new one after 300ms. The search hits the server and replaces
/// the results container with server-returned results.
#[wasm_bindgen]
pub fn mm_search(query: &str) {
    let window = web_sys::window().unwrap();
    // Cancel any pending debounce timer.
    SEARCH_DEBOUNCE.with(|cell| {
        if let Some(handle) = cell.borrow_mut().take() {
            window.clear_timeout_with_handle(handle);
        }
    });

    let q = query.to_string();
    if q.is_empty() {
        // Clear results immediately for empty query.
        let doc = window.document().unwrap();
        if let Some(container) = doc.get_element_by_id("mm-search-results") {
            container.set_inner_html("");
        }
        return;
    }

    let cb = wasm_bindgen::closure::Closure::once(move || {
        spawn_local(async move {
            match api::search_corpus(&q, 200).await {
                Ok(data) => {
                    let node = views::render_search_results(&data);
                    let doc = web_sys::window().unwrap().document().unwrap();
                    if let Some(container) = doc.get_element_by_id("mm-search-results") {
                        container.set_inner_html(&node.to_html());
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("search error: {e:?}").into());
                }
            }
        });
    });
    let handle = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            300,
        )
        .unwrap_or(-1);
    cb.forget();
    SEARCH_DEBOUNCE.with(|cell| *cell.borrow_mut() = Some(handle));
}

/// Save edited config — reads form values into ConfigEditorState, builds mutation.
#[wasm_bindgen]
pub fn mm_config_save() {
    spawn_local(async {
        if let Err(e) = do_config_save().await {
            web_sys::console::error_1(&format!("config save error: {e:?}").into());
        }
    });
}

async fn do_config_save() -> Result<(), JsValue> {
    use mm_meta::mutations::config_edit::ApplyConfigEditsMutation;
    use mm_meta::mutations::Mutation;

    // Collect form values back into the config editor state.
    CONFIG_EDITOR.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(ref mut state) = *borrow else { return };
        views::collect_config_form_values(state);
    });

    let (old_config, new_config, original_kdl) = CONFIG_EDITOR.with(|cell| {
        let borrow = cell.borrow();
        let state = borrow.as_ref().ok_or_else(|| JsValue::from_str("no config editor state"))?;
        Ok::<_, JsValue>((
            state.original_config.clone(),
            state.build_config(),
            state.original_kdl.clone().unwrap_or_default(),
        ))
    })?;

    let mutation = Mutation::ApplyConfigEdits(Box::new(ApplyConfigEditsMutation {
        original_kdl,
        old_config,
        new_config,
    }));

    let decision = mm_meta::decisions::Decision {
        label: "Config edit".to_string(),
        mutations: vec![mutation],
    };

    stage_decision(
        &mm_ui::decision_keys::config_edit(),
        &decision.label,
        &decision.mutations,
    ).await?;

    // Navigate to transaction review for operator confirmation.
    navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
    load_from_hash().await?;
    Ok(())
}

// ============================================================================
// Tag Editor Actions
// ============================================================================

/// Save tag edits — collects changed/added/deleted tags, builds mutations, stages decision.
#[wasm_bindgen]
pub fn mm_tag_save() {
    spawn_local(async {
        if let Err(e) = do_tag_save().await {
            show_tag_status(&format!("Save failed: {e:?}"), true);
        }
    });
}

/// Add a new empty tag row to the editor DOM.
#[wasm_bindgen]
pub fn mm_tag_add() {
    let doc = web_sys::window().unwrap().document().unwrap();
    let container = match doc.get_element_by_id("mm-tag-rows") {
        Some(el) => el,
        None => return,
    };

    // Count existing rows for unique IDs.
    let existing = doc
        .query_selector_all("#mm-tag-rows .mm-tag-row")
        .map(|nl| nl.length())
        .unwrap_or(0);
    let idx = existing;

    let row_html = format!(
        r#"<div class="mm-tag-row">
  <input type="text" id="tag-name-{idx}" class="mm-tag-name mm-tag-name--editable" data-new="true" placeholder="TAG_NAME">
  <input type="text" id="tag-val-{idx}" class="mm-tag-value" data-new="true" placeholder="value">
  <button type="button" class="mm-btn mm-tag-delete" onclick="this.parentElement.remove()">&times;</button>
</div>"#,
    );

    // Append using innerHTML on a temporary wrapper to parse the HTML.
    let wrapper = doc.create_element("div").unwrap();
    wrapper.set_inner_html(&row_html);
    if let Some(child) = wrapper.first_element_child() {
        container.append_child(&child).ok();
        // Focus the new name input.
        if let Ok(Some(name_input)) = doc.query_selector(&format!("#tag-name-{idx}")) {
            if let Ok(el) = name_input.dyn_into::<web_sys::HtmlElement>() {
                el.focus().ok();
            }
        }
    }
}

/// Delete a tag row from the editor DOM and track it for save-time removal.
#[wasm_bindgen]
pub fn mm_tag_delete(tag_name: &str, button: &web_sys::HtmlElement) {
    let doc = web_sys::window().unwrap().document().unwrap();

    // Walk up to the .mm-tag-row parent.
    let row: web_sys::Element = match button.closest(".mm-tag-row") {
        Ok(Some(el)) => el,
        _ => return,
    };

    // Read the original value from the value input (for building the drop mutation at save time).
    let value_input = row.query_selector("input.mm-tag-value").ok().flatten();
    let original = value_input
        .as_ref()
        .and_then(|el| el.get_attribute("data-original"))
        .unwrap_or_default();
    let is_new = value_input
        .as_ref()
        .and_then(|el| el.get_attribute("data-new"))
        .is_some();

    // New (unsaved) rows just get removed from the DOM.
    if is_new {
        row.remove();
        return;
    }

    // For existing tags, record the deletion in a hidden container so save can find it.
    let deletions = match doc.get_element_by_id("mm-tag-deletions") {
        Some(el) => el,
        None => {
            let el = doc.create_element("div").unwrap();
            el.set_id("mm-tag-deletions");
            el.set_attribute("style", "display:none").ok();
            if let Some(editor) = doc.query_selector(".mm-tag-editor").ok().flatten() {
                editor.append_child(&el).ok();
            }
            el
        }
    };

    let marker = doc.create_element("span").unwrap();
    marker.set_attribute("data-deleted-tag", tag_name).ok();
    marker.set_attribute("data-deleted-value", &original).ok();
    // Propagate value→inodes breakdown from the delete button (bulk editors only).
    if let Some(vi) = button.get_attribute("data-value-inodes") {
        marker.set_attribute("data-value-inodes", &vi).ok();
    }
    deletions.append_child(&marker).ok();

    row.remove();
}

fn show_tag_status(msg: &str, is_error: bool) {
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Some(el) = doc.get_element_by_id("mm-tag-status") {
        el.set_text_content(Some(msg));
        let class = if is_error {
            "mm-tag-status mm-tag-status--error"
        } else {
            "mm-tag-status mm-tag-status--ok"
        };
        el.set_attribute("class", class).ok();
        el.set_attribute("style", "").ok();
    }
}

async fn do_tag_save() -> Result<(), JsValue> {
    use mm_meta::db_types::Zone;
    use mm_meta::decisions::Decision;
    use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
    use mm_meta::mutations::{Mutation, TagOp};

    let doc = web_sys::window().unwrap().document().unwrap();

    // Read inode from the editor container.
    let inode: i64 = doc
        .query_selector(".mm-tag-editor[data-inode]")
        .ok()
        .flatten()
        .and_then(|el| el.get_attribute("data-inode"))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| JsValue::from_str("cannot determine inode"))?;

    let mut ops = Vec::new();

    // 1. Collect edits and additions from visible rows.
    let rows = doc
        .query_selector_all("#mm-tag-rows .mm-tag-row")
        .map_err(|e| JsValue::from_str(&format!("querySelectorAll: {e:?}")))?;

    for i in 0..rows.length() {
        let row = rows.get(i).unwrap();
        let row_el: web_sys::Element = row.dyn_into()?;

        let name_input = row_el
            .query_selector("input.mm-tag-name")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok());
        let val_input = row_el
            .query_selector("input.mm-tag-value")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok());

        let (Some(name_el), Some(val_el)) = (name_input, val_input) else {
            continue;
        };

        let is_new = val_el.get_attribute("data-new").is_some()
            || name_el.get_attribute("data-new").is_some();
        let tag_name = if is_new {
            name_el.value()
        } else {
            val_el.get_attribute("data-tag").unwrap_or_default()
        };
        let new_value = val_el.value();

        if tag_name.is_empty() {
            continue;
        }

        if is_new {
            // Add new tag (skip if value is empty).
            if !new_value.is_empty() {
                ops.push(TagOp::add_tag(inode, &tag_name, &new_value));
            }
        } else {
            // Existing tag — check for value change.
            let original = val_el.get_attribute("data-original").unwrap_or_default();
            if new_value != original {
                ops.push(TagOp::replace_tag(inode, &tag_name, &original, &new_value));
            }
        }
    }

    // 2. Collect deletions from the hidden deletion tracker.
    if let Some(deletions) = doc.get_element_by_id("mm-tag-deletions") {
        let markers = deletions
            .query_selector_all("span[data-deleted-tag]")
            .map_err(|e| JsValue::from_str(&format!("querySelectorAll: {e:?}")))?;
        for i in 0..markers.length() {
            let marker = markers.get(i).unwrap();
            let el: web_sys::Element = marker.dyn_into()?;
            let tag = el.get_attribute("data-deleted-tag").unwrap_or_default();
            let val = el.get_attribute("data-deleted-value").unwrap_or_default();
            if !tag.is_empty() {
                ops.push(TagOp::drop_tag(inode, &tag, &val));
            }
        }
    }

    // Filter no-ops.
    ops.retain(|op| !op.is_nop());

    if ops.is_empty() {
        show_tag_status("No changes to save.", false);
        return Ok(());
    }

    let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
        ops,
        zone: Zone::Corpus,
    });

    let mutations = vec![mutation];
    let key_item = format!("{inode}");
    let decision = Decision {
        label: format!("Tag edit (inode {inode})"),
        mutations,
    };

    stage_decision(
        &mm_ui::decision_keys::tag_edit(key_item),
        &decision.label,
        &decision.mutations,
    ).await?;

    show_tag_status("Decision staged.", false);

    // Navigate to transaction review for operator confirmation.
    navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
    load_from_hash().await?;
    Ok(())
}

/// Expand/collapse a directory in the Files view. On first expand, fetches
/// children from the server. Subsequent toggles just show/hide.
#[wasm_bindgen]
pub fn mm_expand_dir(path: &str) {
    let dir_path = path.to_string();
    let dir_id = format!("mm-dir-{}", views::simple_hash(&dir_path));
    let doc = web_sys::window().unwrap().document().unwrap();

    if let Some(el) = doc.get_element_by_id(&dir_id) {
        let current = el.get_attribute("style").unwrap_or_default();
        let hidden = current.contains("display:none") || current.contains("display: none");
        if hidden {
            // Show the container.
            el.set_attribute("style", "").ok();
            // If empty (not yet loaded), fetch from server.
            if el.inner_html().is_empty() {
                spawn_local(async move {
                    match api::get_directory_listing(Some(&dir_path)).await {
                        Ok(data) => {
                            let node = views::render_directory_children(&data);
                            let doc = web_sys::window().unwrap().document().unwrap();
                            if let Some(container) = doc.get_element_by_id(&dir_id) {
                                container.set_inner_html(&node.to_html());
                            }
                        }
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("expand dir error: {e:?}").into(),
                            );
                        }
                    }
                });
            }
        } else {
            // Collapse.
            el.set_attribute("style", "display:none").ok();
        }
    }
}

// ============================================================================
// Bulk Tag Editor
// ============================================================================

/// Open a bulk tag editor for all audio files in a directory.
/// Fetches server-aggregated tag data and injects the editor into the content area.
#[wasm_bindgen]
pub fn mm_bulk_tag_dir(path: &str) {
    let dir_path = path.to_string();
    spawn_local(async move {
        let encoded = js_sys::encode_uri_component(&dir_path);
        let data = match api::get_query_with(
            "bulk-tag-aggregate",
            &format!("rel_path={encoded}"),
        ).await {
            Ok(d) => d,
            Err(e) => {
                web_sys::console::error_1(
                    &format!("bulk tag aggregate fetch error: {e:?}").into(),
                );
                return;
            }
        };

        let file_count = data.get("file_count").and_then(|v| v.as_u64()).unwrap_or(0);
        if file_count == 0 {
            web_sys::console::warn_1(
                &format!("No indexed files found in {dir_path}").into(),
            );
            return;
        }

        let node = views::render_bulk_tag_editor(&data);
        let doc = web_sys::window().unwrap().document().unwrap();
        if let Ok(Some(content)) = doc.query_selector(".mm-content") {
            content.set_inner_html(&node.to_html());
        }
    });
}

/// Save bulk tag edits — collects changed/added/deleted tags, builds mutations for ALL inodes.
#[wasm_bindgen]
pub fn mm_bulk_tag_save() {
    spawn_local(async {
        if let Err(e) = do_bulk_tag_save().await {
            show_tag_status(&format!("Save failed: {e:?}"), true);
        }
    });
}

async fn do_bulk_tag_save() -> Result<(), JsValue> {
    use mm_meta::db_types::Zone;
    use mm_meta::decisions::Decision;
    use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
    use mm_meta::mutations::{Mutation, TagOp};

    let doc = web_sys::window().unwrap().document().unwrap();

    // Read inodes from the bulk editor container.
    let inodes_csv = doc
        .query_selector(".mm-tag-editor--bulk[data-inodes]")
        .ok()
        .flatten()
        .and_then(|el| el.get_attribute("data-inodes"))
        .ok_or_else(|| JsValue::from_str("cannot determine inodes for bulk edit"))?;

    let inodes: Vec<i64> = inodes_csv
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();

    if inodes.is_empty() {
        return Err(JsValue::from_str("no inodes found"));
    }

    let mut ops = Vec::new();

    // 1. Collect edits and additions from visible rows.
    let rows = doc
        .query_selector_all("#mm-tag-rows .mm-tag-row")
        .map_err(|e| JsValue::from_str(&format!("querySelectorAll: {e:?}")))?;

    for i in 0..rows.length() {
        let row = rows.get(i).unwrap();
        let row_el: web_sys::Element = row.dyn_into()?;

        let name_input = row_el
            .query_selector("input.mm-tag-name")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok());
        let val_input = row_el
            .query_selector("input.mm-tag-value")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok());

        let (Some(name_el), Some(val_el)) = (name_input, val_input) else {
            continue;
        };

        // Skip mixed/partial tags (readonly, not editable).
        if val_el.has_attribute("readonly") {
            continue;
        }

        let is_new = val_el.get_attribute("data-new").is_some()
            || name_el.get_attribute("data-new").is_some();
        let tag_name = if is_new {
            name_el.value()
        } else {
            val_el.get_attribute("data-tag").unwrap_or_default()
        };
        let new_value = val_el.value();

        if tag_name.is_empty() {
            continue;
        }

        // Apply to ALL inodes.
        for &inode in &inodes {
            if is_new {
                if !new_value.is_empty() {
                    ops.push(TagOp::add_tag(inode, &tag_name, &new_value));
                }
            } else {
                let original = val_el.get_attribute("data-original").unwrap_or_default();
                if new_value != original {
                    ops.push(TagOp::replace_tag(inode, &tag_name, &original, &new_value));
                }
            }
        }
    }

    // 2. Collect deletions from the hidden deletion tracker.
    if let Some(deletions) = doc.get_element_by_id("mm-tag-deletions") {
        let markers = deletions
            .query_selector_all("span[data-deleted-tag]")
            .map_err(|e| JsValue::from_str(&format!("querySelectorAll: {e:?}")))?;
        for i in 0..markers.length() {
            let marker = markers.get(i).unwrap();
            let el: web_sys::Element = marker.dyn_into()?;
            let tag = el.get_attribute("data-deleted-tag").unwrap_or_default();
            if tag.is_empty() {
                continue;
            }

            // Check for value→inodes breakdown (non-uniform tags in bulk editor).
            if let Some(vi_json) = el.get_attribute("data-value-inodes") {
                // Parse [[value, [inodes]], ...] and generate per-file drop ops.
                if let Ok(vi) = serde_json::from_str::<Vec<(String, Vec<i64>)>>(&vi_json) {
                    for (val, val_inodes) in &vi {
                        for &inode in val_inodes {
                            ops.push(TagOp::drop_tag(inode, &tag, val));
                        }
                    }
                }
            } else {
                // Uniform tag — same value on all files.
                let val = el.get_attribute("data-deleted-value").unwrap_or_default();
                for &inode in &inodes {
                    ops.push(TagOp::drop_tag(inode, &tag, &val));
                }
            }
        }
    }

    ops.retain(|op| !op.is_nop());

    if ops.is_empty() {
        show_tag_status("No changes to save.", false);
        return Ok(());
    }

    let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
        ops,
        zone: Zone::Corpus,
    });

    let decision = Decision {
        label: format!("Bulk tag edit ({} files)", inodes.len()),
        mutations: vec![mutation],
    };

    let key_item = format!("bulk-{}", inodes.len());
    stage_decision(
        &mm_ui::decision_keys::tag_edit(key_item),
        &decision.label,
        &decision.mutations,
    ).await?;

    show_tag_status("Decision staged.", false);
    navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
    load_from_hash().await?;
    Ok(())
}

// ============================================================================
// Dir Config Editor
// ============================================================================

/// Open the dir config editor as an inline overlay (no route navigation).
/// Fetches config from server, renders editor DOM, injects it.
#[wasm_bindgen]
pub fn mm_open_dir_config(path: &str) {
    let dir_path = path.to_string();
    spawn_local(async move {
        let source_dir = match api::get_dir_config(&dir_path).await {
            Ok(sd) => sd,
            Err(e) => {
                web_sys::console::error_1(&format!("dir config fetch error: {e:?}").into());
                return;
            }
        };
        let editor = views::render_dir_config_editor(&dir_path, &source_dir);
        let doc = web_sys::window().unwrap().document().unwrap();

        // Remove any existing overlay first.
        if let Some(existing) = doc.get_element_by_id("mm-dir-config-overlay") {
            existing.remove();
        }

        // Inject overlay into the content area.
        if let Some(content) = doc.query_selector(".mm-content").ok().flatten() {
            let wrapper = doc.create_element("div").unwrap();
            wrapper.set_id("mm-dir-config-overlay");
            wrapper.set_class_name("mm-dir-config-overlay");
            wrapper.set_inner_html(&editor.to_html());
            content.append_child(&wrapper).ok();
        }
    });
}

/// Cancel the dir config editor — remove the overlay, no route navigation.
#[wasm_bindgen]
pub fn mm_dir_config_cancel() {
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Some(overlay) = doc.get_element_by_id("mm-dir-config-overlay") {
        overlay.remove();
    }
}

/// Save dir config edits — reads form values, builds mutation, stages decision.
#[wasm_bindgen]
pub fn mm_dir_config_save(path: &str) {
    let dir_path = path.to_string();
    spawn_local(async move {
        if let Err(e) = do_dir_config_save(&dir_path).await {
            web_sys::console::error_1(&format!("dir config save error: {e:?}").into());
        }
    });
}

async fn do_dir_config_save(dir_path: &str) -> Result<(), JsValue> {
    use mm_meta::config::{Config, SourceDir};
    use mm_meta::decisions::Decision;
    use mm_meta::mutations::dir_config_edit::ApplyDirConfigEditMutation;
    use mm_meta::mutations::Mutation;
    use std::path::PathBuf;

    let doc = web_sys::window().unwrap().document().unwrap();

    // Read form values
    let libraries_str = get_input_value(&doc, "dc-libraries");
    let libraries: Vec<String> = libraries_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let can_stash_dupes = parse_tri_state(&doc, "dc-can-stash-dupes");
    let interior_dupes = parse_tri_state(&doc, "dc-interior-dupes");
    let enable_acoustid = parse_tri_state(&doc, "dc-enable-acoustid");

    let path_schema_str = get_input_value(&doc, "dc-path-schema");
    let path_schema = if path_schema_str.is_empty() { None } else { Some(path_schema_str) };

    let pinned_release_str = get_input_value(&doc, "dc-pinned-release");
    let pinned_release = if pinned_release_str.is_empty() { None } else { Some(pinned_release_str) };

    let source_path = PathBuf::from(dir_path);

    // Fetch current state for old_dir
    let old_source_dir = api::get_dir_config(dir_path).await?;
    let old_dir = old_source_dir.unwrap_or_else(|| SourceDir {
        path: source_path.clone(),
        libraries: vec![],
        can_stash_dupes: None,
        interior_dupes: None,
        path_schema: None,
        enable_acoustid: None,
        pinned_release: None,
    });

    let new_dir = SourceDir {
        path: source_path.clone(),
        libraries,
        can_stash_dupes,
        interior_dupes,
        path_schema: path_schema
            .as_ref()
            .and_then(|t| mm_meta::config::path_schema::parse_path_schema(t).ok()),
        enable_acoustid,
        pinned_release,
    };

    // Build new_config with the edit applied
    let old_config: Config = api_deserialize(&api::get_config_json().await?, "Config")?;
    let mut new_config = old_config;
    let mut found = false;
    for sd in &mut new_config.source_dirs {
        if sd.path == source_path {
            *sd = new_dir.clone();
            found = true;
            break;
        }
    }
    if !found {
        new_config.source_dirs.push(new_dir.clone());
    }
    new_config.source_dirs.retain(|sd| !sd.is_default());

    let mutation = Mutation::ApplyDirConfigEdit(Box::new(ApplyDirConfigEditMutation {
        source_path: source_path.clone(),
        old_dir,
        new_dir,
        new_config,
    }));

    let decision = Decision {
        label: format!("Dir config: {}", dir_path),
        mutations: vec![mutation],
    };

    stage_decision(
        &mm_ui::decision_keys::dir_config_edit(source_path),
        &decision.label,
        &decision.mutations,
    ).await?;

    // Close the overlay.
    mm_dir_config_cancel();

    // Navigate to transaction review for operator confirmation.
    navigate_to(&Route::TransactionReview(route::TransactionReviewRoute::default()));
    load_from_hash().await?;
    Ok(())
}

fn get_input_value(doc: &web_sys::Document, id: &str) -> String {
    doc.get_element_by_id(id)
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value())
        .or_else(|| {
            doc.get_element_by_id(id)
                .and_then(|el| el.dyn_into::<web_sys::HtmlSelectElement>().ok())
                .map(|el| el.value())
        })
        .unwrap_or_default()
}

fn parse_tri_state(doc: &web_sys::Document, id: &str) -> Option<bool> {
    match get_input_value(doc, id).as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
