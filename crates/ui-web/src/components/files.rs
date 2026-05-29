//! File interaction components for the selected literature item.

use leptos::prelude::*;

use crate::state::UiModel;

/// Render local file actions and attached files for the active item.
pub(super) fn render_files(model: &UiModel) -> impl IntoView {
    let active_id = model.active_id.clone().unwrap_or_default();
    view! {
        <div class="files-pane">
            <section class="file-actions">
                <form method="post" action="/ui/action">
                    <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                    <input type="hidden" name="action" value="open_folder"/>
                    <input type="hidden" name="item_id" value={active_id.clone()}/>
                    <button class="button secondary" type="submit">"Open Folder"</button>
                </form>
                <form method="post" action="/ui/action" class="path-form">
                    <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                    <input type="hidden" name="action" value="add_file"/>
                    <input type="hidden" name="item_id" value={active_id.clone()}/>
                    <label class="field">"Local File Path"<input name="file_path"/></label>
                    <button class="button primary" type="submit">"Add File"</button>
                </form>
                <form method="post" action="/ui/action" class="path-form">
                    <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                    <input type="hidden" name="action" value="import_file"/>
                    <label class="field">"Import Path"<input name="file_path"/></label>
                    <button class="button secondary" type="submit">"Import File"</button>
                </form>
            </section>
            <div class="file-list">
                {model.files.iter().map(|file| view! {
                    <div class="file-row">
                        <span>{file.path.clone()}</span>
                        <span>{file.bytes.map(format_bytes).unwrap_or_else(|| file.kind.clone())}</span>
                        <form method="post" action="/ui/action">
                            <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                            <input type="hidden" name="action" value="open_file"/>
                            <input type="hidden" name="item_id" value={active_id.clone()}/>
                            <input type="hidden" name="file_path" value={file.path.clone()}/>
                            <button class="button compact" type="submit">"Open"</button>
                        </form>
                    </div>
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}

/// Format a byte count for the compact file list.
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}
