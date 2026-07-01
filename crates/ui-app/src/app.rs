//! Shared Leptos component tree for the Localref web UI.

use leptos::prelude::*;

use crate::components::hooks::use_theme_mode::ThemeMode;
use crate::model::UiState;
pub use crate::views;

/// Render the full HTML document around the hydratable body app.
#[cfg(feature = "ssr")]
#[must_use]
pub fn document(initial_state: UiState) -> impl IntoView {
    let title = initial_state.repo_name.clone();
    view! {
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>{title}</title>
                <link rel="icon" href="/assets/favicon.ico" type="image/x-icon"/>
                <link rel="stylesheet" href="/assets/localref-ui.css"/>
                <script>
                    {"(function(){try{var d=localStorage.getItem('darkmode');if(d==='true'||(d===null&&matchMedia('(prefers-color-scheme:dark)').matches)){document.documentElement.classList.add('dark')}}catch(e){}})()"}
                </script>
                <script type="module" src="/assets/localref-ui.js"></script>
            </head>
            <body class="h-screen overflow-hidden bg-background text-foreground">
                {body_app(initial_state)}
            </body>
        </html>
    }
}

/// Render the body children that are hydrated in the browser.
#[must_use]
pub fn body_app(initial_state: UiState) -> impl IntoView {
    let initial_detail_open = initial_state.active_id.is_some()
        || !initial_state.selected_ids.is_empty()
        || initial_state.tab == "rules"
        || initial_state.tab.starts_with("plugin:");
    let (state, set_state) = signal(initial_state);
    let (detail_open, set_detail_open) = signal(initial_detail_open);
    let (rules_open, set_rules_open) = signal(false);
    let (plugins_open, set_plugins_open) = signal(false);
    let (context_menu, set_context_menu) = signal::<Option<ItemContextMenu>>(None);

    let _ = ThemeMode::init();
    start_live_refresh(state, set_state);

    view! {
        {move || state.with(state_json_script)}
        // Context menu overlay
        {move || state.with(|s| {
            views::context_menu::render_item_context_menu(
                s, context_menu.get(), set_state, set_context_menu,
            )
        })}
        // Floating rules editor dialog
        {move || {
            if !rules_open.get() { return ().into_any(); }
            let s = state.with(|s| s.clone());
            views::rules::render_rules_floating(&s, set_state, set_rules_open).into_any()
        }}
        // Floating plugin management dialog
        {move || {
            if !plugins_open.get() { return ().into_any(); }
            let s = state.with(|s| s.clone());
            views::plugins_admin::render_plugins_admin_floating(&s, set_plugins_open).into_any()
        }}
        // Rules notice toast
        {move || state.with(|s| views::rules::render_rules_notice(s.clone(), set_state))}
        <main
            class="h-screen flex flex-col overflow-hidden"
            on:click=move |_| {
                set_context_menu.set(None);
                views::topbar::close_view_menu();
            }
        >
            {views::topbar::render_topbar(state, set_state, detail_open, set_detail_open, rules_open, set_rules_open, plugins_open, set_plugins_open)}
            <div
                id="localref-split"
                class="flex-1 min-h-0 grid"
                style=move || {
                    if detail_open.get() {
                        "grid-template-columns: minmax(0, 1fr) 6px minmax(250px, 40%)".to_string()
                    } else {
                        "grid-template-columns: minmax(0, 1fr)".to_string()
                    }
                }
            >
                // Item table (always visible)
                <div class="min-w-0 min-h-0 h-full overflow-hidden">
                    {views::sidebar::render_sidebar(state, set_state, set_context_menu)}
                </div>
                // Resizer + detail panel (only when detail is open)
                {move || {
                    if !detail_open.get() { return ().into_any(); }
                    view! {
                        <div class="resizer" id="localref-resizer" />
                        <section class="min-w-0 min-h-0 overflow-auto border-l border-border">
                            <div class="border-b border-border px-4 py-2 flex items-center justify-between">
                                <h2 class="text-sm font-semibold truncate">{move || state.with(views::detail::detail_title)}</h2>
                                <button
                                    type="button"
                                    class="text-muted-foreground hover:text-foreground text-xs px-2 py-1"
                                    on:click=move |_| set_detail_open.set(false)
                                >"Close"</button>
                            </div>
                            {views::detail::render_detail_tabs(state, set_state)}
                            <div>
                                {views::detail::render_detail_body(state, set_state)}
                            </div>
                        </section>
                    }.into_any()
                }}
            </div>
        </main>
        <script>
            {"(function(){try{var s=localStorage.getItem('localref-split');var r=document.getElementById('localref-resizer');var ok=s&&s.indexOf('minmax(0')===0&&s.endsWith(') 6px minmax(250px, 1fr)');if(ok&&r){var el=document.getElementById('localref-split');if(el)el.style.gridTemplateColumns=s}}catch(e){}})()"}
        </script>
    }
}

/// Internal representation for item context menu.
#[derive(Clone, Debug)]
pub struct ItemContextMenu {
    /// Stored item id.
    pub item_id: String,
    /// Stored x.
    pub x: i32,
    /// Stored y.
    pub y: i32,
}

/// Render a JSON script containing the state used by hydration.
#[must_use]
pub fn state_json_script(state: &UiState) -> AnyView {
    let state_json = serde_json::to_string(state)
        .expect("Localref UI state should serialize")
        .replace('<', "\\u003C");
    view! {
        <script id="localref-ui-state" type="application/json">
            {state_json}
        </script>
    }
    .into_any()
}

/// Start the live-refresh `EventSource` in the hydrated browser; no-op in SSR.
#[allow(clippy::single_call_fn)]
fn start_live_refresh(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::start_live_refresh(state, set_state);

    #[cfg(not(feature = "hydrate"))]
    let _ = (state, set_state);
}
