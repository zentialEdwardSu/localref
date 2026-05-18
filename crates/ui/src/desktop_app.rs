//! Iced desktop application for Localref.
//!
//! The desktop surface keeps filesystem writes behind the daemon REST API. It
//! presents a multi-window workflow: the main window handles browsing,
//! searching, and metadata edits; category membership and event logs open in
//! separate task windows.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use iced::theme::Palette;
use iced::widget::{
    button, checkbox, column, container, horizontal_rule, pick_list, row,
    scrollable, text, text_input, vertical_rule,
};
use iced::{Element, Length, Subscription, Task, Theme, time, window};
use model::{Creator, Event, ItemDocument, Metadata};

use crate::{CategorySummary, DashboardSnapshot, RestClient};

/// Desktop launch policy selected by the process host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopLaunchOptions {
    show_main_window: bool,
}

impl DesktopLaunchOptions {
    /// Create launch options that either show or hide the initial main window.
    pub fn new(show_main_window: bool) -> Self {
        Self { show_main_window }
    }

    /// Return launch options that open the main window immediately.
    pub fn visible() -> Self {
        Self::new(true)
    }

    /// Return launch options that wait for a tray command before opening UI.
    pub fn hidden() -> Self {
        Self::new(false)
    }
}

impl Default for DesktopLaunchOptions {
    fn default() -> Self {
        Self::visible()
    }
}

/// Launch the Localref iced desktop application from the config file.
pub fn launch() -> Result<(), String> {
    let client = RestClient::from_config_file();
    launch_with_client_result(client, DesktopLaunchOptions::visible())
}

/// Launch the Localref iced desktop application with an explicit REST client.
pub fn launch_with_client(client: RestClient) -> Result<(), String> {
    launch_with_client_result(Ok(client), DesktopLaunchOptions::visible())
}

/// Launch with tray-originated desktop control signals.
pub fn launch_with_client_and_signals(
    client: RestClient,
    signals: mpsc::Receiver<DesktopSignal>,
) -> Result<(), String> {
    launch_with_client_signals_and_options(
        client,
        signals,
        DesktopLaunchOptions::visible(),
    )
}

/// Launch with tray control signals and explicit window visibility options.
pub fn launch_with_client_signals_and_options(
    client: RestClient,
    signals: mpsc::Receiver<DesktopSignal>,
    options: DesktopLaunchOptions,
) -> Result<(), String> {
    launch_with_options(Ok(client), Some(signals), options)
}

/// Run the iced daemon and map its error into the local UI error type.
fn launch_with_client_result(
    client: Result<RestClient, String>,
    options: DesktopLaunchOptions,
) -> Result<(), String> {
    launch_with_options(client, None, options)
}

fn launch_with_options(
    client: Result<RestClient, String>,
    signals: Option<mpsc::Receiver<DesktopSignal>>,
    options: DesktopLaunchOptions,
) -> Result<(), String> {
    let signals = signals.map(|receiver| Arc::new(Mutex::new(receiver)));
    iced::daemon(
        LocalrefDesktopApp::title,
        LocalrefDesktopApp::update,
        LocalrefDesktopApp::view,
    )
    .subscription(LocalrefDesktopApp::subscription)
    .theme(LocalrefDesktopApp::theme)
    .run_with(move || {
        LocalrefDesktopApp::new(client.clone(), signals.clone(), options)
    })
    .map_err(|error| error.to_string())
}

/// Top-level iced app state shared by all Localref windows.
pub struct LocalrefDesktopApp {
    client: Result<RestClient, String>,
    signals: Option<Arc<Mutex<mpsc::Receiver<DesktopSignal>>>>,
    endpoint: String,
    windows: BTreeMap<window::Id, WindowKind>,
    data: DesktopData,
    browser_selection: BrowserSelection,
    search: String,
    search_filter: Option<BTreeSet<String>>,
    selected_items: BTreeSet<String>,
    edit_revision: String,
    draft: MetadataDraft,
    category_input: String,
    notice: String,
    error: String,
}

impl LocalrefDesktopApp {
    /// Create app state and optionally open the main window.
    pub fn new(
        client: Result<RestClient, String>,
        signals: Option<Arc<Mutex<mpsc::Receiver<DesktopSignal>>>>,
        options: DesktopLaunchOptions,
    ) -> (Self, Task<Message>) {
        let endpoint = client
            .as_ref()
            .map(|client| client.endpoint().to_string())
            .unwrap_or_else(|error| error.clone());
        let mut app = Self {
            client,
            signals,
            endpoint,
            windows: BTreeMap::new(),
            data: DesktopData::default(),
            browser_selection: BrowserSelection::All,
            search: String::new(),
            search_filter: None,
            selected_items: BTreeSet::new(),
            edit_revision: String::new(),
            draft: MetadataDraft::default(),
            category_input: String::new(),
            notice: String::new(),
            error: String::new(),
        };
        app.refresh();
        let task = if options.show_main_window {
            app.open_main_window()
        } else {
            Task::none()
        };
        (app, task)
    }

    /// Return the title for one window.
    pub fn title(&self, id: window::Id) -> String {
        match self.windows.get(&id) {
            Some(WindowKind::Main) => "Localref".to_string(),
            Some(WindowKind::Categories(_)) => {
                "Localref Categories".to_string()
            }
            Some(WindowKind::Events) => "Localref Events".to_string(),
            None => "Localref".to_string(),
        }
    }

    /// Update app state from one UI message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowOpened(id, kind) => {
                self.windows.insert(id, kind);
            }
            Message::WindowClosed(id) => {
                self.windows.remove(&id);
                if self.windows.is_empty() && self.signals.is_none() {
                    return iced::exit();
                }
            }
            Message::Refresh => self.refresh(),
            Message::RunScan => self.run_scan(),
            Message::BrowserSelectionChanged(selection) => {
                self.browser_selection = selection;
            }
            Message::SearchChanged(value) => {
                self.search = value;
                if self.search.trim().is_empty() {
                    self.search_filter = None;
                }
            }
            Message::RunSearch => self.run_search(),
            Message::ToggleItem(item_id, checked) => {
                if checked {
                    self.selected_items.insert(item_id.clone());
                    self.load_metadata(&item_id);
                } else {
                    self.selected_items.remove(&item_id);
                }
            }
            Message::DraftTitle(value) => self.draft.title = value,
            Message::DraftAuthors(value) => self.draft.authors = value,
            Message::DraftType(value) => self.draft.item_type = value,
            Message::DraftYear(value) => self.draft.year = value,
            Message::DraftDoi(value) => self.draft.doi = value,
            Message::DraftVenue(value) => self.draft.venue = value,
            Message::DraftLanguage(value) => self.draft.language = value,
            Message::DraftUri(value) => self.draft.uri = value,
            Message::DraftAbstract(value) => self.draft.abstract_note = value,
            Message::SaveMetadata => self.save_metadata(),
            Message::CategoryInputChanged(value) => {
                self.category_input = value
            }
            Message::OpenCategoryWindow => {
                if self.selected_items.is_empty() {
                    self.error = "select at least one item".to_string();
                } else {
                    let state = CategoryWindowState {
                        selected_item_ids: self
                            .selected_items
                            .iter()
                            .cloned()
                            .collect(),
                    };
                    let (_, open) =
                        window::open(window_settings(860.0, 620.0));
                    return open.map(move |id| {
                        Message::WindowOpened(
                            id,
                            WindowKind::Categories(state.clone()),
                        )
                    });
                }
            }
            Message::OpenEventsWindow => {
                if self
                    .windows
                    .values()
                    .any(|kind| matches!(kind, WindowKind::Events))
                {
                    return Task::none();
                }
                let (_, open) = window::open(window_settings(920.0, 620.0));
                return open
                    .map(|id| Message::WindowOpened(id, WindowKind::Events));
            }
            Message::CreateCategory => self.create_category(),
            Message::AddCategoryToSelection(path) => {
                self.add_category_to_selection(&path)
            }
            Message::RemoveCategoryFromSelection(path) => {
                self.remove_category_from_selection(&path);
            }
            Message::TrayOpen => {
                if let Some((id, _)) = self
                    .windows
                    .iter()
                    .find(|(_, kind)| matches!(kind, WindowKind::Main))
                {
                    return window::gain_focus(*id);
                }
                return self.open_main_window();
            }
            Message::Tick => return self.drain_signals(),
            Message::AutoRefresh => self.refresh(),
        }
        Task::none()
    }

    /// Render one window.
    pub fn view(&self, id: window::Id) -> Element<'_, Message> {
        match self.windows.get(&id) {
            Some(WindowKind::Main) | None => self.main_window(),
            Some(WindowKind::Categories(state)) => self.category_window(state),
            Some(WindowKind::Events) => self.events_window(),
        }
    }

    /// Subscribe to close requests from every window.
    pub fn subscription(&self) -> Subscription<Message> {
        let close = window::close_events().map(Message::WindowClosed);
        let auto_refresh =
            time::every(Duration::from_secs(3)).map(|_| Message::AutoRefresh);
        if self.signals.is_some() {
            Subscription::batch([
                close,
                auto_refresh,
                time::every(Duration::from_millis(250)).map(|_| Message::Tick),
            ])
        } else {
            Subscription::batch([close, auto_refresh])
        }
    }

    /// Return the Swiss desktop theme.
    pub fn theme(&self, _id: window::Id) -> Theme {
        Theme::custom(
            "Localref Swiss".to_string(),
            Palette {
                background: iced::Color::from_rgb8(0xF7, 0xF7, 0xF8),
                text: iced::Color::from_rgb8(0x17, 0x20, 0x2A),
                primary: iced::Color::from_rgb8(0x00, 0x2F, 0xA7),
                success: iced::Color::from_rgb8(0x06, 0x76, 0x47),
                danger: iced::Color::from_rgb8(0xB4, 0x23, 0x18),
            },
        )
    }

    fn client(&self) -> Result<RestClient, String> {
        self.client.clone()
    }

    fn open_main_window(&self) -> Task<Message> {
        let (_, open) = window::open(window_settings(1180.0, 760.0));
        open.map(|id| Message::WindowOpened(id, WindowKind::Main))
    }

    fn drain_signals(&mut self) -> Task<Message> {
        let Some(signals) = self.signals.clone() else {
            return Task::none();
        };
        loop {
            let signal = {
                let receiver =
                    signals.lock().expect("desktop signal receiver poisoned");
                receiver.try_recv()
            };
            match signal {
                Ok(DesktopSignal::Open) => {
                    return self.update(Message::TrayOpen);
                }
                Ok(DesktopSignal::Refresh) => self.refresh(),
                Ok(DesktopSignal::Scan) => self.run_scan(),
                Ok(DesktopSignal::PauseWatcher) => self.pause("watcher"),
                Ok(DesktopSignal::PauseWrites) => self.pause("writes"),
                Ok(DesktopSignal::ResumeWatcher) => self.resume("watcher"),
                Ok(DesktopSignal::ResumeWrites) => self.resume("writes"),
                Ok(DesktopSignal::Quit) => return iced::exit(),
                Err(mpsc::TryRecvError::Empty) => return Task::none(),
                Err(mpsc::TryRecvError::Disconnected) => return Task::none(),
            }
        }
    }

    fn refresh(&mut self) {
        let Ok(client) = self.client() else {
            if let Err(error) = &self.client {
                self.error = error.clone();
            }
            return;
        };
        match load_data(&client) {
            Ok(data) => {
                self.data = data;
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn run_scan(&mut self) {
        let Ok(client) = self.client() else {
            return;
        };
        match client.scan() {
            Ok(_) => {
                self.notice = "Scan completed".to_string();
                self.refresh();
            }
            Err(error) => self.error = error,
        }
    }

    fn pause(&mut self, mode: &str) {
        let Ok(client) = self.client() else {
            return;
        };
        match client.pause(mode) {
            Ok(_) => {
                self.notice = format!("Paused {mode}");
                self.refresh();
            }
            Err(error) => self.error = error,
        }
    }

    fn resume(&mut self, mode: &str) {
        let Ok(client) = self.client() else {
            return;
        };
        match client.resume(mode) {
            Ok(_) => {
                self.notice = format!("Resumed {mode}");
                self.refresh();
            }
            Err(error) => self.error = error,
        }
    }

    fn run_search(&mut self) {
        let Ok(client) = self.client() else {
            return;
        };
        if self.search.trim().is_empty() {
            self.search_filter = None;
            return;
        }
        match client.search(&self.search) {
            Ok(hits) => {
                let hit_ids = hits
                    .into_iter()
                    .map(|hit| hit.id)
                    .collect::<BTreeSet<_>>();
                self.notice = format!("{} item(s)", hit_ids.len());
                self.search_filter = Some(hit_ids);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn load_metadata(&mut self, item_id: &str) {
        let Ok(client) = self.client() else {
            return;
        };
        match client.get_metadata(item_id) {
            Ok(document) => {
                self.edit_revision = document.metadata_revision;
                self.draft = MetadataDraft::from_metadata(document.metadata);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn save_metadata(&mut self) {
        if self.draft.id.is_empty() {
            self.error = "select one item before saving metadata".to_string();
            return;
        }
        let Ok(client) = self.client() else {
            return;
        };
        match client.patch_metadata(
            &self.draft.id,
            self.edit_revision.clone(),
            self.draft.to_metadata(),
        ) {
            Ok(item) => {
                self.notice = format!("Saved metadata for {}", item.id);
                self.refresh();
                let id = item.id;
                self.load_metadata(&id);
            }
            Err(error) => self.error = error,
        }
    }

    fn create_category(&mut self) {
        let path = self.category_input.trim().to_string();
        if path.is_empty() {
            self.error = "category path is required".to_string();
            return;
        }
        let Ok(client) = self.client() else {
            return;
        };
        match client.create_category(path.clone()) {
            Ok(_) => {
                self.notice = format!("Created category {path}");
                self.refresh();
            }
            Err(error) => self.error = error,
        }
    }

    fn add_category_to_selection(&mut self, category: &str) {
        let selected = self.selected_items.iter().cloned().collect::<Vec<_>>();
        let Ok(client) = self.client() else {
            return;
        };
        for item_id in selected {
            if let Err(error) = client.add_item_category(&item_id, category) {
                self.error = error;
                return;
            }
        }
        self.notice = format!("Added category {category}");
        self.refresh();
    }

    fn remove_category_from_selection(&mut self, category: &str) {
        let selected = self.selected_items.iter().cloned().collect::<Vec<_>>();
        let Ok(client) = self.client() else {
            return;
        };
        for item_id in selected {
            if let Err(error) = client.remove_item_category(&item_id, category)
            {
                self.error = error;
                return;
            }
        }
        self.notice = format!("Removed category {category}");
        self.refresh();
    }

    fn main_window(&self) -> Element<'_, Message> {
        let header = row![
            text(&self.endpoint).size(14),
            button("Refresh").on_press(Message::Refresh),
            button("Run Scan").on_press(Message::RunScan),
            button("Edit Categories").on_press(Message::OpenCategoryWindow),
            button("Event Log").on_press(Message::OpenEventsWindow),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let feedback = if !self.error.is_empty() {
            text(&self.error)
        } else {
            text(&self.notice)
        };

        let browser = column![
            row![
                pick_list(
                    self.browser_options(),
                    Some(self.browser_selection.clone()),
                    Message::BrowserSelectionChanged
                ),
                text_input("Search", &self.search)
                    .on_input(Message::SearchChanged)
                    .on_submit(Message::RunSearch),
                button("Search").on_press(Message::RunSearch),
            ]
            .spacing(8),
            text(format!(
                "{} items, {} categories, {} events",
                self.data.snapshot.item_count,
                self.data.snapshot.category_count,
                self.data.snapshot.event_count
            ))
            .size(13),
            horizontal_rule(1),
            self.browser_list(),
        ]
        .spacing(10)
        .width(Length::Fixed(360.0));

        let metadata = column![
            text("Metadata").size(24),
            self.metadata_form(),
            row![
                button("Save Metadata").on_press(Message::SaveMetadata),
                text(format!("Revision {}", self.edit_revision)).size(12),
            ]
            .spacing(10)
        ]
        .spacing(10)
        .width(Length::Fill);

        container(
            column![
                header,
                horizontal_rule(1),
                row![browser, vertical_rule(1), metadata]
                    .spacing(16)
                    .height(Length::Fill),
                feedback,
            ]
            .spacing(14)
            .padding(18),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn browser_list(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(6);
        for item in self.visible_items() {
            let id = item.id.clone();
            let checked = self.selected_items.contains(&item.id);
            let mut summary = column![
                text(&item.title).size(15),
                text(format!("{}  {}", item.id, item.item_type)).size(12),
            ]
            .spacing(2);
            if !item.authors.is_empty() {
                summary =
                    summary.push(text(author_summary(&item.authors)).size(12));
            }
            let row = row![
                checkbox("", checked).on_toggle(move |value| {
                    Message::ToggleItem(id.clone(), value)
                }),
                summary
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            list = list.push(row);
        }
        scrollable(list).height(Length::Fill).into()
    }

    fn browser_options(&self) -> Vec<BrowserSelection> {
        let mut options = vec![BrowserSelection::All];
        options.extend(self.data.categories.iter().map(|category| {
            BrowserSelection::Category(category.path.clone())
        }));
        options
    }

    fn visible_items(&self) -> Vec<&ItemDocument> {
        let category_filter = match &self.browser_selection {
            BrowserSelection::All => None,
            BrowserSelection::Category(path) => self
                .data
                .categories
                .iter()
                .find(|category| &category.path == path)
                .map(|category| {
                    category.item_ids.iter().cloned().collect::<BTreeSet<_>>()
                }),
        };
        self.data
            .items
            .iter()
            .filter(|item| {
                category_filter
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&item.id))
            })
            .filter(|item| {
                self.search_filter
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&item.id))
            })
            .collect()
    }

    fn metadata_form(&self) -> Element<'_, Message> {
        column![
            text(format!("ID {}", self.draft.id)).size(13),
            text_input("Title", &self.draft.title)
                .on_input(Message::DraftTitle),
            text_input("Authors", &self.draft.authors)
                .on_input(Message::DraftAuthors),
            row![
                text_input("Type", &self.draft.item_type)
                    .on_input(Message::DraftType),
                text_input("Year", &self.draft.year)
                    .on_input(Message::DraftYear),
            ]
            .spacing(8),
            row![
                text_input("DOI", &self.draft.doi).on_input(Message::DraftDoi),
                text_input("Venue", &self.draft.venue)
                    .on_input(Message::DraftVenue),
            ]
            .spacing(8),
            row![
                text_input("Language", &self.draft.language)
                    .on_input(Message::DraftLanguage),
                text_input("URI", &self.draft.uri).on_input(Message::DraftUri),
            ]
            .spacing(8),
            text_input("Abstract", &self.draft.abstract_note)
                .on_input(Message::DraftAbstract)
                .padding(10),
        ]
        .spacing(8)
        .into()
    }

    fn category_window(
        &self,
        state: &CategoryWindowState,
    ) -> Element<'_, Message> {
        let selected_count = state.selected_item_ids.len();
        let common =
            common_categories(&self.data.items, &state.selected_item_ids);

        let mut all_categories = column![].spacing(6);
        for category in &self.data.categories {
            let path = category.path.clone();
            all_categories = all_categories.push(
                row![
                    text(&category.path).width(Length::Fill),
                    button("Add")
                        .on_press(Message::AddCategoryToSelection(path)),
                ]
                .spacing(8),
            );
        }

        let mut current = column![].spacing(6);
        for category in common {
            let path = category.clone();
            current = current.push(
                row![
                    text(category).width(Length::Fill),
                    button("Remove")
                        .on_press(Message::RemoveCategoryFromSelection(path)),
                ]
                .spacing(8),
            );
        }

        container(
            column![
                text(format!("{} selected item(s)", selected_count)).size(24),
                row![
                    text_input("Category path", &self.category_input)
                        .on_input(Message::CategoryInputChanged),
                    button("Create Category")
                        .on_press(Message::CreateCategory),
                ]
                .spacing(8),
                horizontal_rule(1),
                row![
                    column![
                        text("All Categories").size(18),
                        scrollable(all_categories)
                    ]
                    .spacing(10)
                    .width(Length::Fill),
                    vertical_rule(1),
                    column![
                        text("Shared Categories").size(18),
                        scrollable(current)
                    ]
                    .spacing(10)
                    .width(Length::Fill),
                ]
                .spacing(16)
                .height(Length::Fill),
            ]
            .spacing(14)
            .padding(18),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn events_window(&self) -> Element<'_, Message> {
        let mut rows = column![].spacing(8);
        for event in self.data.events.iter().rev() {
            rows = rows.push(
                column![
                    row![
                        text(event.id.to_string()).size(13),
                        text(event_kind(event)).size(13),
                    ]
                    .spacing(12),
                    text(&event.message),
                ]
                .spacing(3),
            );
        }

        container(
            column![
                text("Event Log").size(28),
                horizontal_rule(1),
                scrollable(rows)
            ]
            .spacing(12)
            .padding(18),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    /// A window was created by iced.
    WindowOpened(window::Id, WindowKind),
    /// A window close request was received.
    WindowClosed(window::Id),
    /// Refresh desktop state from REST.
    Refresh,
    /// Run daemon scan.
    RunScan,
    /// Switch the browser list between All and one category.
    BrowserSelectionChanged(BrowserSelection),
    /// Update search input text.
    SearchChanged(String),
    /// Execute the current search.
    RunSearch,
    /// Select or deselect an item.
    ToggleItem(String, bool),
    /// Edit title.
    DraftTitle(String),
    /// Edit semicolon-separated author names.
    DraftAuthors(String),
    /// Edit item type.
    DraftType(String),
    /// Edit publication year.
    DraftYear(String),
    /// Edit DOI.
    DraftDoi(String),
    /// Edit venue.
    DraftVenue(String),
    /// Edit language.
    DraftLanguage(String),
    /// Edit URI.
    DraftUri(String),
    /// Edit abstract.
    DraftAbstract(String),
    /// Save metadata.
    SaveMetadata,
    /// Update category input.
    CategoryInputChanged(String),
    /// Open category membership window.
    OpenCategoryWindow,
    /// Open event log window.
    OpenEventsWindow,
    /// Create a category directory.
    CreateCategory,
    /// Add category to selected items.
    AddCategoryToSelection(String),
    /// Remove category from selected items.
    RemoveCategoryFromSelection(String),
    /// Bring the main window forward from tray.
    TrayOpen,
    /// Poll tray-originated signals.
    Tick,
    /// Refresh desktop state on a timer.
    AutoRefresh,
}

/// Control signal sent by the native tray shell.
#[derive(Clone, Debug)]
pub enum DesktopSignal {
    /// Show or focus the main window.
    Open,
    /// Refresh desktop state.
    Refresh,
    /// Request a daemon scan.
    Scan,
    /// Pause watcher work.
    PauseWatcher,
    /// Pause writes.
    PauseWrites,
    /// Resume watcher work.
    ResumeWatcher,
    /// Resume writes.
    ResumeWrites,
    /// Exit the desktop process.
    Quit,
}

/// Kind and state of each open window.
#[derive(Clone, Debug)]
pub enum WindowKind {
    /// Main browsing and metadata window.
    Main,
    /// Category membership editing window.
    Categories(CategoryWindowState),
    /// Event log window.
    Events,
}

/// State for one category membership editor window.
#[derive(Clone, Debug)]
pub struct CategoryWindowState {
    /// Item ids being edited.
    pub selected_item_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrowserSelection {
    /// Browse `All/` item documents.
    All,
    /// Browse item documents linked to one category.
    Category(String),
}

impl fmt::Display for BrowserSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::Category(path) => write!(f, "{path}"),
        }
    }
}

#[derive(Default)]
struct DesktopData {
    snapshot: DashboardSnapshot,
    items: Vec<ItemDocument>,
    categories: Vec<CategorySummary>,
    events: Vec<Event>,
}

#[derive(Clone, Default)]
struct MetadataDraft {
    id: String,
    item_type: String,
    title: String,
    authors: String,
    abstract_note: String,
    doi: String,
    uri: String,
    year: String,
    venue: String,
    language: String,
    metadata: Option<Metadata>,
}

impl MetadataDraft {
    fn from_metadata(metadata: Metadata) -> Self {
        Self {
            id: metadata.id.clone(),
            item_type: metadata.item_type.clone(),
            title: metadata.title.clone(),
            authors: author_summary(&metadata.author_names()),
            abstract_note: metadata.abstract_note.clone().unwrap_or_default(),
            doi: metadata.doi.clone().unwrap_or_default(),
            uri: metadata.uri.clone().unwrap_or_default(),
            year: metadata
                .year
                .map(|year| year.to_string())
                .unwrap_or_default(),
            venue: metadata.venue.clone().unwrap_or_default(),
            language: metadata.language.clone().unwrap_or_default(),
            metadata: Some(metadata),
        }
    }

    fn to_metadata(&self) -> Metadata {
        let mut metadata = self.metadata.clone().unwrap_or_else(|| Metadata {
            id: self.id.clone(),
            item_type: self.item_type.clone(),
            title: self.title.clone(),
            creators: Vec::new(),
            abstract_note: None,
            doi: None,
            uri: None,
            year: None,
            venue: None,
            language: None,
            files: model::MetadataFiles::default(),
            tags: model::MetadataTags::default(),
            import: model::MetadataImport::default(),
            state: model::MetadataState::default(),
            raw_connector: Default::default(),
        });
        metadata.item_type = self.item_type.clone();
        metadata.title = self.title.clone();
        replace_author_creators(
            &mut metadata.creators,
            parse_author_names(&self.authors),
        );
        metadata.abstract_note = optional_text(&self.abstract_note);
        metadata.doi = optional_text(&self.doi);
        metadata.uri = optional_text(&self.uri);
        metadata.year = optional_i32(&self.year);
        metadata.venue = optional_text(&self.venue);
        metadata.language = optional_text(&self.language);
        metadata
    }
}

fn load_data(client: &RestClient) -> Result<DesktopData, String> {
    Ok(DesktopData {
        snapshot: client.dashboard_snapshot()?,
        items: client.list_items()?,
        categories: client.list_categories()?,
        events: client.list_events()?,
    })
}

fn window_settings(width: f32, height: f32) -> window::Settings {
    window::Settings {
        size: iced::Size::new(width, height),
        min_size: Some(iced::Size::new(760.0, 520.0)),
        ..window::Settings::default()
    }
}

fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn optional_i32(value: &str) -> Option<i32> {
    value.trim().parse().ok()
}

/// Join author names into the UI's editable display format.
fn author_summary(authors: &[String]) -> String {
    authors
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|author| !author.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Parse the UI's semicolon-separated author field into metadata creators.
fn parse_author_names(value: &str) -> Vec<Creator> {
    value
        .split(';')
        .map(str::trim)
        .filter(|author| !author.is_empty())
        .map(|author| Creator {
            role: "author".to_string(),
            given: None,
            family: None,
            name: Some(author.to_string()),
        })
        .collect()
}

/// Replace only author creators while preserving other contributor roles.
fn replace_author_creators(
    creators: &mut Vec<Creator>,
    authors: Vec<Creator>,
) {
    creators.retain(|creator| !is_author_role(&creator.role));
    creators.extend(authors);
}

/// Return whether a creator role is an author-like Zotero role.
fn is_author_role(role: &str) -> bool {
    role.to_ascii_lowercase().contains("author")
}

fn event_kind(event: &Event) -> String {
    serde_json::to_string(&event.kind)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

fn common_categories(items: &[ItemDocument], ids: &[String]) -> Vec<String> {
    let mut common: Option<BTreeSet<String>> = None;
    for id in ids {
        let Some(item) = items.iter().find(|item| &item.id == id) else {
            continue;
        };
        let categories =
            item.categories.iter().cloned().collect::<BTreeSet<_>>();
        common = Some(match common {
            Some(current) => {
                current.intersection(&categories).cloned().collect()
            }
            None => categories,
        });
    }
    common.unwrap_or_default().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_text_trims_blank_values() {
        assert_eq!(optional_text("  "), None);
        assert_eq!(optional_text("RIS"), Some("RIS".to_string()));
    }

    #[test]
    fn desktop_launch_options_encode_initial_window_visibility() {
        assert!(DesktopLaunchOptions::visible().show_main_window);
        assert!(!DesktopLaunchOptions::hidden().show_main_window);
        assert!(DesktopLaunchOptions::default().show_main_window);
    }

    #[test]
    fn author_field_round_trips_semicolon_separated_names() {
        let creators = parse_author_names("Ada Lovelace; Near Field Group");

        assert_eq!(creators.len(), 2);
        assert_eq!(creators[0].name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(
            author_summary(
                &creators
                    .iter()
                    .filter_map(Creator::display_name)
                    .collect::<Vec<_>>()
            ),
            "Ada Lovelace; Near Field Group"
        );
    }

    #[test]
    fn author_replacement_preserves_non_author_creators() {
        let mut creators = vec![Creator {
            role: "editor".to_string(),
            given: None,
            family: None,
            name: Some("Editor Name".to_string()),
        }];

        replace_author_creators(
            &mut creators,
            parse_author_names("Ada Lovelace"),
        );

        assert_eq!(creators.len(), 2);
        assert_eq!(creators[0].role, "editor");
        assert_eq!(creators[1].role, "author");
        assert_eq!(creators[1].name.as_deref(), Some("Ada Lovelace"));
    }

    #[test]
    fn common_categories_returns_only_shared_categories() {
        let items = vec![
            item_with_categories("a", &["Wireless", "Archive"]),
            item_with_categories("b", &["Wireless", "Inbox"]),
        ];

        assert_eq!(
            common_categories(&items, &["a".to_string(), "b".to_string()]),
            vec!["Wireless".to_string()]
        );
    }

    fn item_with_categories(id: &str, categories: &[&str]) -> ItemDocument {
        ItemDocument {
            id: id.to_string(),
            object_path: format!("All/{id}"),
            metadata_revision: String::new(),
            title: id.to_string(),
            authors: Vec::new(),
            abstract_note: None,
            item_type: "document".to_string(),
            doi: None,
            uri: None,
            main_file: None,
            extra_files: Vec::new(),
            tags: Vec::new(),
            venue: None,
            year: None,
            categories: categories
                .iter()
                .map(|category| category.to_string())
                .collect(),
        }
    }
}
