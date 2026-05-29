//! Category display and transfer controls for selected literature items.

use leptos::prelude::*;

use crate::state::{UiModel, available_categories, common_categories};

/// Render collapsed category tags and the expandable category transfer editor.
pub(super) fn render_category_summary(model: &UiModel) -> impl IntoView {
    let common = common_categories(&model.items, &model.category_target_ids);
    let available = available_categories(&model.categories, &common);
    view! {
        <section class="category-summary">

            <details class="category-editor">
                <summary> "Current Categories: "
                    <div class="tag-strip">
                        {common.iter().map(|category| view! {
                            <span class="category-tag">{category.clone()}</span>
                        }).collect::<Vec<_>>()}
                    </div>
                </summary>
                <form method="post" action="/ui/action" class="new-category" data-route-action="true">
                    <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                    <input type="hidden" name="action" value="create_category"/>
                    <input name="category" placeholder="Category path"/>
                    <button class="button secondary" type="submit">"Create Category"</button>
                </form>
                <div class="transfer-grid">
                    <section>
                        <h4>"Available"</h4>
                        <div class="category-list">
                            {available.iter().map(|category| view! {
                                <form method="post" action="/ui/action" class="category-row" data-route-action="true">
                                    <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                                    <input type="hidden" name="action" value="add_category"/>
                                    <input type="hidden" name="category" value={category.path.clone()}/>
                                    <span>{category.path.clone()}</span>
                                    <button class="button tiny" type="submit">"Add"</button>
                                </form>
                            }).collect::<Vec<_>>()}
                        </div>
                    </section>
                    <section>
                        <h4>"Current"</h4>
                        <div class="category-list">
                            {common.iter().map(|category| view! {
                                <form method="post" action="/ui/action" class="category-row" data-route-action="true">
                                    <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                                    <input type="hidden" name="action" value="remove_category"/>
                                    <input type="hidden" name="category" value={category.clone()}/>
                                    <span>{category.clone()}</span>
                                    <button class="button tiny" type="submit">"Remove"</button>
                                </form>
                            }).collect::<Vec<_>>()}
                        </div>
                    </section>
                </div>
            </details>
        </section>
    }
}
