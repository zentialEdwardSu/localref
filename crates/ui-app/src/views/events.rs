//! Events panel — floating dialog.

use leptos::prelude::*;

use crate::model::UiState;
use crate::ui::table::*;

/// Render recent daemon events (inline, used if needed).
pub fn render_events(state: &UiState) -> impl IntoView + use<> {
    let events: Vec<_> = state.events.iter().rev().take(50).map(|event| {
        (event.id, event.kind.clone(), event.message.clone())
    }).collect();

    view! {
        <TableWrapper class="border-0 rounded-none">
            <Table>
                <TableHeader>
                    <TableRow class="text-xs">
                        <TableHead class="w-12 px-2">"ID"</TableHead>
                        <TableHead class="w-28 px-2">"Kind"</TableHead>
                        <TableHead class="px-2">"Message"</TableHead>
                    </TableRow>
                </TableHeader>
                <TableBody>
                    {events.into_iter().map(|(id, kind, message)| view! {
                        <TableRow>
                            <TableCell class="px-2 py-1 text-xs text-muted-foreground">{id}</TableCell>
                            <TableCell class="px-2 py-1 text-xs text-muted-foreground">{kind}</TableCell>
                            <TableCell class="px-2 py-1 text-sm">{message}</TableCell>
                        </TableRow>
                    }).collect::<Vec<_>>()}
                </TableBody>
            </Table>
        </TableWrapper>
    }
}

/// Render events as a floating dialog overlay.
pub fn render_events_floating(
    state: &UiState,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView + use<> {
    let events: Vec<_> = state.events.iter().rev().take(100).map(|event| {
        (event.id, event.kind.clone(), event.message.clone())
    }).collect();

    view! {
        <div class="fixed inset-0 z-40 flex items-center justify-center">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/40"
                on:click=move |_| set_events_open.set(false)
            />
            // Dialog
            <div class="relative z-50 w-[min(800px,calc(100vw-3rem))] max-h-[min(600px,calc(100vh-6rem))] bg-card border border-border shadow-lg flex flex-col">
                <div class="flex items-center justify-between px-4 py-2 border-b border-border shrink-0">
                    <h3 class="text-sm font-semibold">"Events"</h3>
                    <button
                        type="button"
                        class="text-xs text-muted-foreground hover:text-foreground px-2 py-1"
                        on:click=move |_| set_events_open.set(false)
                    >"Close"</button>
                </div>
                <div class="flex-1 overflow-auto">
                    <TableWrapper class="border-0 rounded-none max-h-none">
                        <Table>
                            <TableHeader>
                                <TableRow class="text-xs">
                                    <TableHead class="w-12 px-2">"ID"</TableHead>
                                    <TableHead class="w-28 px-2">"Kind"</TableHead>
                                    <TableHead class="px-2">"Message"</TableHead>
                                </TableRow>
                            </TableHeader>
                            <TableBody>
                                {events.into_iter().map(|(id, kind, message)| view! {
                                    <TableRow>
                                        <TableCell class="px-2 py-1 text-xs text-muted-foreground">{id}</TableCell>
                                        <TableCell class="px-2 py-1 text-xs text-muted-foreground">{kind}</TableCell>
                                        <TableCell class="px-2 py-1 text-sm">{message}</TableCell>
                                    </TableRow>
                                }).collect::<Vec<_>>()}
                            </TableBody>
                        </Table>
                    </TableWrapper>
                </div>
            </div>
        </div>
    }
}
