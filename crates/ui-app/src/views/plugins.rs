//! Plugin rendering: pages, slots, fields.

use leptos::prelude::*;

use crate::model::{PluginFieldDef, PluginPageDef, UiState};

/// Render one declarative plugin page as a native form.
pub fn render_plugin_page(
    page: PluginPageDef,
    return_to: String,
    selected_csv: String,
    active_value: String,
) -> impl IntoView {
    let action_url = format!("/plugin/{}/action", page.plugin_name);
    let action_id = page.action_id.clone().unwrap_or_default();
    let fields = page.fields.clone();
    let displays = page.displays.clone();
    let preview_attr = page.preview.as_ref()
        .map(|p| format!("{}:{}:{}", p.action, p.debounce_ms, p.into))
        .unwrap_or_default();

    view! {
        <form
            class="plugin-form grid gap-3"
            method="post"
            action=action_url
            data-plugin=page.plugin_name.clone()
            data-plugin-page=page.page_id.clone()
            data-plugin-preview=preview_attr
        >
            <input type="hidden" name="plugin_action" value=action_id />
            <input type="hidden" name="return_to" value=return_to />
            <input type="hidden" name="selected" value=selected_csv />
            <input type="hidden" name="active" value=active_value />
            <h3 class="text-sm font-semibold">{page.label.clone()}</h3>
            {displays.into_iter().map(|d| view! {
                <p class="plugin-display text-sm text-muted-foreground" data-display=d.id.clone()
                   data-template=d.text.clone()>{d.text.clone()}</p>
            }).collect_view()}
            {fields.into_iter().map(render_plugin_field).collect_view()}
            <button type="submit" class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium h-9 px-4 py-2 bg-primary text-primary-foreground shadow-xs hover:bg-primary/90 w-fit">
                "Run"
            </button>
        </form>
    }
}

/// Render one declarative field as a native control.
fn render_plugin_field(field: PluginFieldDef) -> impl IntoView {
    let default = field.default.clone().unwrap_or_default();
    let required = field.required;
    let control = match field.kind.as_str() {
        "textarea" => view! {
            <textarea name=field.name.clone() class="plugin-field-input" required=required>
                {default.clone()}
            </textarea>
        }.into_any(),
        "select" => view! {
            <select name=field.name.clone() class="plugin-field-input" required=required>
                {field.options.clone().into_iter().map(|opt| {
                    let selected = opt == default;
                    view! { <option value=opt.clone() selected=selected>{opt.clone()}</option> }
                }).collect_view()}
            </select>
        }.into_any(),
        "radio" => view! {
            <div class="plugin-field-radio">
                {field.options.clone().into_iter().map(|opt| {
                    let checked = opt == default;
                    view! {
                        <label class="flex items-center gap-2 text-sm">
                            <input type="radio" name=field.name.clone()
                                   value=opt.clone() checked=checked />
                            {opt}
                        </label>
                    }
                }).collect_view()}
            </div>
        }.into_any(),
        "checkbox" => view! {
            <input type="checkbox" name=field.name.clone()
                   class="plugin-field-input" value="true" required=required />
        }.into_any(),
        "number" => view! {
            <input type="number" name=field.name.clone()
                   class="plugin-field-input" value=default.clone() required=required />
        }.into_any(),
        _ => view! {
            <input type="text" name=field.name.clone()
                   class="plugin-field-input" value=default.clone() required=required />
        }.into_any(),
    };

    view! {
        <label class="plugin-field grid gap-1.5 text-sm font-medium" data-field=field.name.clone()>
            <span>{field.label}</span>
            {control}
        </label>
    }
}

/// Render plugin pages mounted into one fixed host slot.
pub fn render_plugin_slots(state: &UiState, mount: &'static str) -> impl IntoView {
    let return_to = state.return_to.clone();
    let selected_csv = state.selected_ids.join(",");
    let active_value = state.active_id.clone().unwrap_or_default();
    let slots: Vec<_> = state.plugin_slots.iter()
        .filter(|s| s.mount == mount)
        .cloned()
        .collect();

    view! {
        {slots.into_iter().map(move |slot| {
            let rt = return_to.clone();
            let sel = selected_csv.clone();
            let act = active_value.clone();
            let pn = slot.plugin_name.clone();
            let pid = slot.page_id.clone();
            view! {
                <section class="plugin-slot" data-plugin=pn data-plugin-page=pid>
                    {render_plugin_page(slot, rt, sel, act)}
                </section>
            }
        }).collect_view()}
    }
}
