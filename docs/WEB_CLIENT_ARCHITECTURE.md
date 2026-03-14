# Web Client Architecture

Vision document for MM's web client layer. Captures the architectural decisions
made during design; implementation details (especially the (x)html mapping from
widget types) will be refined as the extraction work proceeds.

## Overview

MM gains a web client alongside mm-tui. The key insight: UI **business logic**
(state machines, layout structure, theming, content) is extracted into a shared
crate (`mm-ui`), with backend-specific rendering in mm-tui (ratatui) and mm-web
(HTML + WASM).

The web client is not a terminal-in-browser. Widget types map to semantic HTML
elements — lists become `<ul>/<li>`, modals become `<dialog>`, tab bars become
`<header><nav>`, buttons become `<button>`. HTML structure is baked in at compile
time via proc macros; data is filled client-side by WASM.

## Crate Layout

```
mm-meta          Protocol, decisions, mutations, domain types
                 (unchanged — already UI-agnostic)
    |
mm-ui            Widget business logic, theming, content types
    |            (new crate — extracted from mm-tui)
   / \
mm-tui          mm-web
ratatui          HTTP server + JSON API + static assets
rendering        mm-web/client/ — WASM module (wasm-bindgen)
crossterm          DOM events, fetch/WebSocket, state management
input
```

### mm-ui (new crate)

Extracted from mm-tui. Contains everything that isn't rendering or
platform-specific input handling:

- **State machines**: `StandardListState`, `FocusPane`, `ButtonRowState<B>`,
  wizard state
- **Traits**: `ModalFrame` (structural declarations), `ModalButtons`,
  `ListEntry`, `WizardItem`
- **Layout structures**: `ContentLayout` enum and layout declarations
- **Content types**: `RichText` / structured content that backends lower to
  their native representations
- **Theming**: semantic style tokens (see Theming section)
- **Input vocabulary**: `InputAction` enum, dispatch routing logic
- **View state structs**: data ownership, cursor, selection

mm-ui depends on ratatui for type compatibility, so mm-tui can re-import without
a rewrite. The ratatui dependency is for **compatibility**, not core logic — mm-web
does not need to touch ratatui.

### mm-tui (slimmed)

Retains:
- ratatui rendering implementations that consume mm-ui types
- crossterm event -> `InputAction` translation
- `render_*` functions (Frame + Rect drawing)
- `ConfirmGesture` witness pattern (TUI-specific enforcement)
- Event loop

### mm-web

HTTP server crate. Contains both the server and the WASM client source:

```
mm-web/
  src/           HTTP server (axum or similar)
    api/         JSON API endpoints mapping protocol surface
    static/      Asset serving (compiled WASM, HTML, CSS)
  client/        WASM client module (separate wasm-pack build target)
    src/         Rust -> wasm-bindgen
```

**Server responsibilities:**
- JSON API mapping the protocol surface (queries, transactions, commands)
- WebSocket endpoint for push updates (Witch status, transaction results,
  background task progress)
- Static asset serving (compiled WASM, HTML shell, CSS)
- `WitchHandle` via Unix socket to the Witch

**Client (WASM) responsibilities:**
- Holds view state (ActiveView equivalent, compiled from mm-ui logic)
- DOM event handling (click, keyboard -> actions)
- Fetch + WebSocket client for JSON API
- Fills data into HTML structure

## MVC Mapping

```
Model:       mm-meta    Protocol, decisions, mutations, domain types
                        Shared by both backends.

View logic:  mm-ui      Widget state, layout structure, theming tokens,
                        RichText content, InputAction vocabulary.
                        Shared by both backends.

View render: mm-tui     ratatui Frame/Buffer
             mm-web     HTML templates (proc macro generated) + CSS

Controller:  mm-tui     crossterm events + ConfirmGesture witness
             mm-web     DOM events + fetch POST to transaction endpoint
```

## Semantic HTML Mapping

Widget types translate to semantic HTML elements:

| mm-ui type        | HTML element                          |
|-------------------|---------------------------------------|
| UnifiedTitleBar   | `<header>` with `<nav>` tabs          |
| ModalFrame        | `<dialog>` or `<section>`             |
| StandardList      | `<ul>` + `<li>`                       |
| ButtonRowState    | `<nav>` of `<button>`s                |
| ControlsHint      | `<footer>` or `<aside>`              |
| FocusPane         | `aria-active` on focused section      |
| ThreePaneLayout   | CSS grid/flexbox sections             |
| Modal titles      | `<h2>` / `<h3>`                       |
| Click targets     | `<a>` (navigation) / `<button>` (action) |

HTML structure is generated at compile time via proc macros on the layout types
(`#[derive(HtmlWidget)]` or similar). Widget changes without corresponding HTML
updates produce build failures — structural sync is compiler-enforced.

The detailed (x)html mapping — how `ContentLayout` variants map to CSS grid
templates, how multi-select lists render, how ARIA attributes flow from focus
state — is a separate design pass once mm-ui exists and widget trait boundaries
are concrete.

## Theming

Semantic style tokens replace direct ratatui `Color`/`Style` usage. The token
vocabulary:

| Category     | Tokens                                         |
|--------------|------------------------------------------------|
| Structural   | `Focused`, `Unfocused`, `Disabled`             |
| Semantic     | `Healthy`, `Info`, `Warning`, `Critical`       |
| Text         | `Primary`, `Secondary`, `Muted`, `Accent`      |
| Interactive  | `Active`, `Inactive`, `Destructive`            |

A theme **definition** maps tokens to concrete values:
- TUI: ratatui `Color` / `Style`
- Web: CSS custom properties (`var(--token-name)`)

Call site usage is trivially simple: `theme.color(Semantic::Warning)` instead of
`Color::Yellow`. User preferences and client preferences swap which mapping is
active.

`HealthStatus` widget already uses the Semantic vocabulary — the theming system
generalizes this pattern across all widgets.

## Content Abstraction

`render_list_item()` and `render_detail()` currently return ratatui types
(`ListItem`, write to `Frame`). After extraction, they return mm-ui-native
structured content — `RichText` or a similar typed content representation.

Each backend lowers structured content to its native form:
- TUI: `RichText` -> `ratatui::text::Line` / `Span` / `ListItem`
- Web: `RichText` -> `<span>` elements with CSS classes

This also makes widget content **testable** without a terminal.

## Action Dispatch & Confirmation

The `ConfirmGesture` witness pattern is TUI-specific — it proves an Enter
keypress occurred in an action handler at compile time. It stays in mm-tui.

The web equivalent uses native mechanisms: DOM click handler on a `<button>` ->
fetch POST to the transaction endpoint with auth token. The **intent** is shared
(operator confirmed a mutation), the **enforcement mechanism** is
backend-native.

The shared layer is the Decision/Transaction protocol in mm-meta, which both
backends funnel into:
- TUI: `ConfirmGesture` -> `Decision` -> `WitchHandle`
- Web: click handler -> fetch POST -> JSON API -> `WitchHandle` (server-side)

## Transport

- **JSON API** (HTTP): queries, transaction lifecycle, commands. Maps directly
  to the existing protocol surface in mm-meta — the types are already serde.
- **WebSocket**: server -> client push for Witch status changes, transaction
  confirmations, background task progress. Mirrors the subscription pattern
  that mm-tui already uses via WitchHandle.

## What This Enables

The JSON API is the **stable contract**. The HTML rendering layer is swappable —
v1 uses proc-macro-generated semantic HTML from widget types, a future v2 could
replace it with a richer web frontend (e.g., a JS framework) consuming the same
API. The API bindings remain intact either way.
