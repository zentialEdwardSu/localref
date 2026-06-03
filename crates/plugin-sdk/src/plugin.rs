//! The `Plugin` trait, action/page builders, and output helper types.

/// Trait that every Localref plugin must implement.
pub trait Plugin: Send + Sync + Default {
    /// Plugin machine name (used in URLs: `/plugin/<name>/`).
    fn name(&self) -> &str;

    /// Optional human-readable description.
    fn description(&self) -> Option<&str> {
        None
    }

    /// Actions exposed to the UI (buttons, context menu items).
    fn actions(&self) -> Vec<Action> {
        vec![]
    }

    /// SSR pages exposed to the UI (detail tabs, sidebar panels).
    fn pages(&self) -> Vec<Page> {
        vec![]
    }

    /// Render an SSR HTML fragment for the given page.
    ///
    /// # Errors
    ///
    /// Returns an error string when the page is unknown or rendering cannot
    /// complete.
    fn render(
        &self,
        page: &str,
        state: &crate::PluginState,
    ) -> Result<RenderOutput, String>;

    /// Execute an action.
    ///
    /// # Errors
    ///
    /// Returns an error string when the action is unknown or cannot complete.
    fn run(
        &self,
        action: &str,
        params: &crate::Params,
        state: &crate::PluginState,
    ) -> Result<RunOutput, String>;
}

// ── Action builder ──────────────────────────────────────────────

/// Describes one plugin action mounted in the UI.
pub struct Action {
    pub id: String,
    pub label: String,
    pub mount: ActionMount,
}

/// Where an action appears in the host UI.
pub enum ActionMount {
    ActionButton,
    ContextMenu,
}

impl Action {
    /// Create a new action with the given id and display label.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            mount: ActionMount::ActionButton,
        }
    }

    /// Mount this action as a top-bar button.
    #[must_use]
    pub const fn mount_action_button(mut self) -> Self {
        self.mount = ActionMount::ActionButton;
        self
    }

    /// Mount this action in the right-click context menu.
    #[must_use]
    pub const fn mount_context_menu(mut self) -> Self {
        self.mount = ActionMount::ContextMenu;
        self
    }
}

// ── Page builder ────────────────────────────────────────────────

/// Describes one SSR page mounted in the host UI.
pub struct Page {
    pub id: String,
    pub label: String,
    pub mount: PageMount,
    pub route: String,
}

/// Where a page appears in the host UI.
pub enum PageMount {
    DetailTab,
    MetadataPage,
    SelectionPage,
}

impl Page {
    /// Create a new page with the given id and tab label.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            mount: PageMount::DetailTab,
            route: String::new(),
        }
    }

    /// Mount this page as a detail-pane tab with the given URL route.
    #[must_use]
    pub fn mount_detail_tab(mut self, route: impl Into<String>) -> Self {
        self.mount = PageMount::DetailTab;
        self.route = route.into();
        self
    }

    /// Mount this page inline on the single-item metadata page.
    #[must_use]
    pub fn mount_metadata_page(mut self, route: impl Into<String>) -> Self {
        self.mount = PageMount::MetadataPage;
        self.route = route.into();
        self
    }

    /// Mount this page inline on the multi-selection page.
    #[must_use]
    pub fn mount_selection_page(mut self, route: impl Into<String>) -> Self {
        self.mount = PageMount::SelectionPage;
        self.route = route.into();
        self
    }
}

// ── Re-export host output types ─────────────────────────────────

pub use localref_plugin::state::{RenderOutput, RunOutput};
