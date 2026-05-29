//! Event log components for daemon activity.

use leptos::prelude::*;

use crate::state::UiModel;

/// Render the newest daemon events in reverse chronological order.
pub(super) fn render_events(model: &UiModel) -> impl IntoView {
    view! {
        <div class="event-list">
            {model.events.iter().rev().take(50).map(|event| view! {
                <div class="event-row">
                    <span>{event.id}</span>
                    <span>{format!("{:?}", event.kind)}</span>
                    <strong>{event.message.clone()}</strong>
                </div>
            }).collect::<Vec<_>>()}
        </div>
    }
}
