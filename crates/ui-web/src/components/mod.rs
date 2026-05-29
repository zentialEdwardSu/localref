//! Page composition for the Localref server-rendered UI.

mod category;
mod detail;
mod events;
mod files;
mod script;
mod sidebar;

use leptos::prelude::*;

use crate::state::{RulesNotice, UiModel};
use detail::render_detail;
use events::render_events;
use script::INTERACTION_SCRIPT;
use sidebar::render_sidebar;

/// Render the complete server-side HTML document.
pub(crate) fn render_page(model: UiModel) -> String {
    let view = {
        let status = model.status_label();
        let selected_count = model.selected_ids.len();
        let detail_title = detail_title(&model);
        let watcher_paused = model.watcher_paused();
        let events_open = model.tab == "events";
        let single_item_tabs = selected_count == 0;
        view! {
            <html lang="en">
                <head>
                    <meta charset="utf-8"/>
                    <meta name="viewport" content="width=device-width, initial-scale=1"/>
                    <title>"Localref"</title>
                    <link rel="icon" href="/assets/favicon.ico" type="image/x-icon"/>
                    <link rel="stylesheet" href="/assets/localref-ui.css"/>
                    <script type="module" src="/assets/localref-ui.js"></script>
                    <script>{INTERACTION_SCRIPT}</script>
                </head>
                <body>
                    {render_rules_notice(&model)}
                    <main class="app-shell">
                        <header class="topbar">
                            <div class="brand-block">
                                <div class="brand-row">
                                    <h1>"Localref"</h1>
                                    <span class="status-pill">{status}</span>
                                </div>
                                </div>
                                <div class="top-actions">
                                <div class="stats">
                                <span>{model.items.len()} " items"</span>
                                <span>{model.categories.len()} " categories"</span>
                                <span>{model.events.len()} " events"</span>
                                <span>{model.pending_count} " pending"</span>
                                </div>
                                <div class="control-row">
                                    <button class=top_event_class(events_open) type="button" data-events-toggle="true" aria-pressed={events_open.to_string()}>"Events"</button>
                                    <form method="post" action="/ui/action">
                                        <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                                        <input type="hidden" name="action" value="scan"/>
                                        <button class="button secondary" type="submit">"Run Scan"</button>
                                    </form>
                                    <form method="post" action="/ui/action">
                                        <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                                        <input type="hidden" name="mode" value="watcher"/>
                                        <div class="radio-pair">
                                            <label class=watcher_class(!watcher_paused)>
                                                <input type="radio" name="action" value="resume" checked={!watcher_paused} onchange="this.form.submit()"/>
                                                <span>"Watcher On"</span>
                                            </label>
                                            <label class=watcher_class(watcher_paused)>
                                                <input type="radio" name="action" value="pause" checked={watcher_paused} onchange="this.form.submit()"/>
                                                <span>"Watcher Paused"</span>
                                            </label>
                                        </div>
                                    </form>
                                </div>
                            </div>
                        </header>

                        <section class="workspace">
                            {render_sidebar(&model, selected_count)}
                            <section class="detail-pane">
                                <div data-primary-detail="true" hidden=events_open>
                                    <div class="detail-head" data-primary-detail-head="true">
                                        <div class="title-block">
                                        <h2>{detail_title}</h2>
                                    </div>
                                    <nav class="right-tabs">
                                        {if single_item_tabs {
                                            view! {
                                            <button class=right_tab_class(&model.tab, "metadata") type="button" data-route-tab="metadata">"Metadata"</button>
                                            <button class=right_tab_class(&model.tab, "files") type="button" data-route-tab="files">"Files"</button>
                                            }.into_any()
                                        } else {
                                            view! {}.into_any()
                                        }}
                                            <button class=right_tab_class(&model.tab, "rules") type="button" data-route-tab="rules">"Rules"</button>
                                    </nav>
                                </div>
                                    <div class="detail-body">
                                        {render_detail(&model)}
                                    </div>
                                </div>
                                <section class="event-panel" hidden={!events_open}>
                                    {render_events(&model)}
                                </section>
                            </section>
                        </section>
                    </main>
                </body>
            </html>
        }
    }
    .to_html();
    format!("<!doctype html>{view}")
}

/// Render the floating rules save result dialog when present.
fn render_rules_notice(model: &UiModel) -> impl IntoView {
    match &model.rules_notice {
        Some(RulesNotice::Saved(rules)) => view! {
            <aside class="rules-result-dialog" role="status">
                <div class="dialog-head">
                    <h3>"Parsed Rules"</h3>
                    <button class="button compact" type="button" data-dismiss-dialog="true">"Close"</button>
                </div>
                <div class="rules-result-list">
                    {rules.iter().map(|rule| view! {
                        <div class="rules-result-row">
                            <strong>{rule.name.clone()}</strong>
                            <span>{rule.target.clone()}</span>
                            <code>{rule.query.clone()}</code>
                        </div>
                    }).collect::<Vec<_>>()}
                </div>
            </aside>
        }
        .into_any(),
        Some(RulesNotice::Error(message)) => view! {
            <aside class="rules-result-dialog is-error" role="alert">
                <div class="dialog-head">
                    <h3>"Rules Error"</h3>
                    <button class="button compact" type="button" data-dismiss-dialog="true">"Close"</button>
                </div>
                <p>{message.clone()}</p>
            </aside>
        }
        .into_any(),
        None => view! { <div></div> }.into_any(),
    }
}

/// Return the right pane title for either single-item or bulk selection mode.
fn detail_title(model: &UiModel) -> String {
    if model.tab == "rules" {
        return "Rules Editor".to_string();
    }
    match model.selected_ids.len() {
        0 => model
            .active_item
            .as_ref()
            .map(|item| item.title.clone())
            .unwrap_or_else(|| "No item selected".to_string()),
        1 => "Selected 1 item".to_string(),
        count => format!("Selected {count} items"),
    }
}

/// Return the top navigation class for the events view.
fn top_event_class(active: bool) -> &'static str {
    if active { "top-link is-active" } else { "top-link" }
}

/// Return the right-pane tab class for metadata and file views.
fn right_tab_class(current: &str, tab: &str) -> &'static str {
    if current == tab { "right-tab is-active" } else { "right-tab" }
}

/// Return the visual class for the active watcher radio option.
fn watcher_class(active: bool) -> &'static str {
    if active { "radio-option is-active" } else { "radio-option" }
}
