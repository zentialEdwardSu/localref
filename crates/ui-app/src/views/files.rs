//! Files tab: file list, upload zone, open/set-main actions.

use leptos::prelude::*;

use crate::model::{FileEntry, UiState};
use crate::ui::button::{Button, ButtonVariant, ButtonSize};
use crate::ui::card::*;
use crate::ui::badge::{Badge, BadgeVariant};
use crate::ui::table::*;

/// Render local file actions and file rows.
pub fn render_files(
    state: &UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView + use<> {
    let item_id = state.active_id.clone().unwrap_or_default();
    let return_to = state.return_to.clone();
    let upload_item_id = item_id.clone();
    let upload_return_to = return_to.clone();
    let picker_item_id = item_id.clone();
    let picker_return_to = return_to.clone();
    let return_to_open_folder = state.return_to.clone();
    let return_to_upload = state.return_to.clone();
    let active_id_upload = state.active_id.clone().unwrap_or_default();
    let file_rows: Vec<_> = state.files.iter().map(|file| {
        let file_path = file.path.clone();
        let file_is_main = file.is_main;
        let file_size_str = format_file_size(file);
        let is_candidate = is_main_candidate(file);
        let row_return_to = state.return_to.clone();
        let row_active_id = state.active_id.clone().unwrap_or_default();
        let row_revision = state.active_detail.as_ref()
            .map(|d| d.metadata_revision.clone()).unwrap_or_default();
        (file_path, file_is_main, file_size_str, is_candidate, row_return_to, row_active_id, row_revision)
    }).collect();

    view! {
        <div class="p-4 grid gap-4">
            <Card>
                <CardContent class="pt-4">
                    <div class="flex gap-3 items-stretch flex-wrap">
                        <form method="post" action="/ui/action">
                            <input type="hidden" name="return_to" value=return_to_open_folder />
                            <input type="hidden" name="action" value="open_folder" />
                            <input type="hidden" name="item_id" value=item_id.clone() />
                            <Button variant=ButtonVariant::Outline size=ButtonSize::Default attr:r#type="submit">
                                "Open Folder"
                            </Button>
                        </form>
                        <form
                            method="post"
                            action="/ui/upload"
                            enctype="multipart/form-data"
                            class="flex-1 grid min-h-20 place-items-center gap-2 border border-dashed border-input rounded-md bg-muted/30 px-4 py-3 text-sm text-muted-foreground hover:border-ring hover:bg-muted/50 transition"
                            on:dragover=move |event| { event.prevent_default(); }
                            on:drop=move |event| {
                                event.prevent_default();
                                super::upload_dropped_files(
                                    event, upload_item_id.clone(), upload_return_to.clone(),
                                    set_state, set_events_open,
                                );
                            }
                        >
                            <input type="hidden" name="return_to" value=return_to_upload />
                            <input type="hidden" name="item_id" value=active_id_upload />
                            <input
                                id="item-file-picker"
                                class="sr-only"
                                type="file"
                                name="file"
                                multiple
                                on:change=move |event| {
                                    super::upload_input_files(
                                        event, picker_item_id.clone(), picker_return_to.clone(),
                                        set_state, set_events_open,
                                    );
                                }
                            />
                            <label class="cursor-pointer text-primary font-medium" r#for="item-file-picker">"Add Files"</label>
                            <span>"or drop files here"</span>
                        </form>
                    </div>
                </CardContent>
            </Card>

            <TableWrapper>
                <Table>
                    <TableHeader>
                        <TableRow>
                            <TableHead>"File"</TableHead>
                            <TableHead>"Size"</TableHead>
                            <TableHead class="text-right">"Actions"</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        {file_rows.into_iter().map(|(file_path, file_is_main, file_size_str, is_candidate, row_return_to, row_active_id, row_revision)| {
                            render_file_row_owned(file_path, file_is_main, file_size_str, is_candidate, row_return_to, row_active_id, row_revision)
                        }).collect::<Vec<_>>()}
                    </TableBody>
                </Table>
            </TableWrapper>
        </div>
    }
}

/// One file row — takes owned data extracted from state/file.
fn render_file_row_owned(
    file_path: String,
    file_is_main: bool,
    file_size_str: String,
    is_candidate: bool,
    return_to: String,
    active_id: String,
    revision: String,
) -> impl IntoView {
    let return_to2 = return_to.clone();
    let active_id2 = active_id.clone();
    let file_path_main = file_path.clone();
    let file_path_open = file_path.clone();

    view! {
        <TableRow>
            <TableCell class="p-2">
                <span class="flex items-center gap-2">
                    {file_path.clone()}
                    {if file_is_main {
                        view! { <Badge variant=BadgeVariant::Outline size=crate::ui::badge::BadgeSize::Sm>"Main"</Badge> }.into_any()
                    } else {
                        ().into_any()
                    }}
                </span>
            </TableCell>
            <TableCell class="p-2 text-muted-foreground text-sm">
                {file_size_str}
            </TableCell>
            <TableCell class="p-2 text-right">
                <div class="flex gap-1 justify-end">
                    {if !file_is_main && is_candidate {
                        view! {
                            <form method="post" action="/ui/action">
                                <input type="hidden" name="return_to" value=return_to />
                                <input type="hidden" name="action" value="set_main_file" />
                                <input type="hidden" name="item_id" value=active_id />
                                <input type="hidden" name="file_path" value=file_path_main />
                                <input type="hidden" name="expected_revision" value=revision />
                                <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm attr:r#type="submit">"Set Main"</Button>
                            </form>
                        }.into_any()
                    } else {
                        ().into_any()
                    }}
                    <form method="post" action="/ui/action">
                        <input type="hidden" name="return_to" value=return_to2 />
                        <input type="hidden" name="action" value="open_file" />
                        <input type="hidden" name="item_id" value=active_id2 />
                        <input type="hidden" name="file_path" value=file_path_open />
                        <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm attr:r#type="submit">"Open"</Button>
                    </form>
                </div>
            </TableCell>
        </TableRow>
    }
}

fn format_file_size(file: &FileEntry) -> String {
    match file.bytes {
        Some(b) if b < 1024 => format!("{b} B"),
        Some(b) if b < 1024 * 1024 => format!("{} KB", b / 1024),
        Some(b) => format!("{} MB", b / (1024 * 1024)),
        None => file.kind.clone(),
    }
}

fn is_main_candidate(file: &FileEntry) -> bool {
    file.kind == "file" && file.path != "metadata.toml"
}
