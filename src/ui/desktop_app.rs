//! Iced desktop application for Localref.
//!
//! The desktop surface keeps filesystem writes behind the daemon REST API. It
//! presents a multi-window workflow: the main window handles browsing,
//! searching, and metadata edits; category membership and event logs open in
//! separate task windows.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use iced::theme::Palette;
use iced::widget::{
    button, checkbox, column, container, horizontal_rule, pick_list, row,
    scrollable, text, text_editor, text_input, vertical_rule,
};
use iced::{
    Background, Border, Color, Element, Length, Subscription, Task, Theme,
    time, window,
};
use localref_core::model::{
    Creator, Event, ItemDocument, ItemFilesDocument, Metadata,
};

use crate::ui::{CategorySummary, DashboardSnapshot, RestClient};

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
    active_item_id: Option<String>,
    detail_tab: DetailTab,
    edit_revision: String,
    draft: MetadataDraft,
    category_input: String,
    item_files: Option<ItemFilesDocument>,
    hovered_file: Option<PathBuf>,
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
            active_item_id: None,
            detail_tab: DetailTab::Metadata,
            edit_revision: String::new(),
            draft: MetadataDraft::default(),
            category_input: String::new(),
            item_files: None,
            hovered_file: None,
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
            Message::RefreshFiles => {
                if let Some(item_id) = self.current_item_id() {
                    self.load_files(&item_id);
                }
            }
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
            Message::SelectItem(item_id) => {
                self.selected_items.clear();
                self.selected_items.insert(item_id.clone());
                self.active_item_id = Some(item_id.clone());
                self.load_metadata(&item_id);
                self.load_files(&item_id);
            }
            Message::ToggleItemSelection(item_id, selected) => {
                self.toggle_item_selection(&item_id, selected);
            }
            Message::DetailTabChanged(tab) => {
                self.detail_tab = tab;
                if matches!(tab, DetailTab::Files) {
                    if let Some(item_id) = self.current_item_id() {
                        self.load_files(&item_id);
                    }
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
            Message::DraftAbstract(action) => {
                self.draft.abstract_content.perform(action);
                self.draft.abstract_note =
                    self.draft.abstract_content.text().trim_end().to_string();
            }
            Message::SaveMetadata => self.save_metadata(),
            Message::CategoryInputChanged(value) => {
                self.category_input = value
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
            Message::OpenSelectedFolder => self.open_selected_folder(),
            Message::OpenFile(path) => self.open_file(&path),
            Message::DroppedFile(path) => self.add_file_to_selection(path),
            Message::WindowEvent(event) => match event {
                window::Event::FileHovered(path) => {
                    self.hovered_file = Some(path);
                }
                window::Event::FilesHoveredLeft => {
                    self.hovered_file = None;
                }
                window::Event::FileDropped(path) => {
                    self.hovered_file = None;
                    return self.update(Message::DroppedFile(path));
                }
                _ => {}
            },
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
            Message::AutoRefresh => {
                self.refresh();
                if let Some(item_id) = self.current_item_id() {
                    self.load_files(&item_id);
                }
            }
        }
        Task::none()
    }

    /// Render one window.
    pub fn view(&self, id: window::Id) -> Element<'_, Message> {
        match self.windows.get(&id) {
            Some(WindowKind::Main) | None => self.main_window(),
            Some(WindowKind::Events) => self.events_window(),
        }
    }

    /// Subscribe to close requests from every window.
    pub fn subscription(&self) -> Subscription<Message> {
        let close = window::close_events().map(Message::WindowClosed);
        let auto_refresh =
            time::every(Duration::from_secs(3)).map(|_| Message::AutoRefresh);
        let window_events =
            window::events().map(|(_, event)| Message::WindowEvent(event));
        if self.signals.is_some() {
            Subscription::batch([
                close,
                auto_refresh,
                window_events,
                time::every(Duration::from_millis(250)).map(|_| Message::Tick),
            ])
        } else {
            Subscription::batch([close, auto_refresh, window_events])
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

    fn load_files(&mut self, item_id: &str) {
        let Ok(client) = self.client() else {
            return;
        };
        match client.list_item_files(item_id) {
            Ok(files) => {
                self.item_files = Some(files);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn current_item_id(&self) -> Option<String> {
        self.active_item_id
            .as_ref()
            .filter(|id| self.selected_items.contains(*id))
            .cloned()
            .or_else(|| self.selected_items.iter().next().cloned())
    }

    /// Add or remove one browser item without treating it as a row click.
    fn toggle_item_selection(&mut self, item_id: &str, selected: bool) {
        if selected {
            self.selected_items.insert(item_id.to_string());
            if self.active_item_id.is_none() {
                self.active_item_id = Some(item_id.to_string());
                self.load_metadata(item_id);
                self.load_files(item_id);
            }
            return;
        }

        self.selected_items.remove(item_id);
        if self.active_item_id.as_deref() != Some(item_id) {
            return;
        }

        self.active_item_id = self.selected_items.iter().next().cloned();
        if let Some(active) = self.active_item_id.clone() {
            self.load_metadata(&active);
            self.load_files(&active);
        } else {
            self.edit_revision.clear();
            self.draft = MetadataDraft::default();
            self.item_files = None;
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

    fn open_selected_folder(&mut self) {
        let Some(item_id) = self.current_item_id() else {
            self.error =
                "select one item before opening its folder".to_string();
            return;
        };
        let Ok(client) = self.client() else {
            return;
        };
        match client.open_item_folder(&item_id) {
            Ok(_) => {
                self.notice = format!("Opened folder for {item_id}");
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn open_file(&mut self, path: &str) {
        let Some(item_id) = self.current_item_id() else {
            self.error = "select one item before opening a file".to_string();
            return;
        };
        let Ok(client) = self.client() else {
            return;
        };
        match client.open_item_file(&item_id, path.to_string()) {
            Ok(_) => {
                self.notice = format!("Opened {path}");
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn add_file_to_selection(&mut self, path: PathBuf) {
        let Some(item_id) = self.current_item_id() else {
            self.error = "select one item before dropping a file".to_string();
            return;
        };
        let Ok(client) = self.client() else {
            return;
        };
        match client.add_item_file(&item_id, path.display().to_string()) {
            Ok(item) => {
                self.notice = format!("Added file to {}", item.id);
                self.refresh();
                self.load_files(&item.id);
                self.load_metadata(&item.id);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn main_window(&self) -> Element<'_, Message> {
        let header = row![
            text(&self.endpoint).size(14),
            button("Refresh").on_press(Message::Refresh),
            button("Run Scan").on_press(Message::RunScan),
            button("Event Log").on_press(Message::OpenEventsWindow),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let feedback = if !self.error.is_empty() {
            text(&self.error)
        } else {
            text(&self.notice)
        };

        let browser_content = container(
            column![
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
                text("Auto-refresh every 3 seconds").size(12),
                horizontal_rule(1),
                self.browser_list(),
            ]
            .spacing(10),
        )
        .height(Length::Shrink);
        let browser = scrollable(browser_content)
            .height(Length::Fill)
            .width(Length::FillPortion(4));

        let details = self.detail_panel();

        container(
            column![
                header,
                horizontal_rule(1),
                row![browser, vertical_rule(1), details]
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
            let selected = self.selected_items.contains(&item.id);
            let checkbox_id = item.id.clone();
            let mut summary = column![
                text(&item.title).size(15),
                text(format!(
                    "{}  {}  {}",
                    item.id,
                    item.item_type,
                    attachment_summary(item)
                ))
                .size(12),
            ]
            .spacing(2);
            if !item.authors.is_empty() {
                summary =
                    summary.push(text(author_summary(&item.authors)).size(12));
            }
            let row = row![
                checkbox("", selected)
                    .on_toggle(move |checked| Message::ToggleItemSelection(
                        checkbox_id.clone(),
                        checked
                    ))
                    .width(Length::Fixed(28.0)),
                button(
                    row![summary].spacing(8).align_y(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .padding(10)
                .style(move |_theme, status| item_row_style(selected, status))
                .on_press(Message::SelectItem(id))
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            list = list.push(row);
        }
        list.into()
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

    fn detail_panel(&self) -> Element<'_, Message> {
        let tabs = row![
            button("Metadata")
                .width(Length::Fixed(120.0))
                .on_press(Message::DetailTabChanged(DetailTab::Metadata)),
            button("Files")
                .width(Length::Fixed(120.0))
                .on_press(Message::DetailTabChanged(DetailTab::Files)),
        ]
        .spacing(8);

        let content = match self.detail_tab {
            DetailTab::Metadata => self.metadata_tab(),
            DetailTab::Files => self.files_tab(),
        };

        let detail_content =
            container(column![tabs, horizontal_rule(1), content].spacing(12))
                .height(Length::Shrink);
        scrollable(detail_content)
            .height(Length::Fill)
            .width(Length::FillPortion(7))
            .into()
    }

    fn metadata_tab(&self) -> Element<'_, Message> {
        column![
            text("Metadata").size(24),
            self.metadata_form(),
            row![
                button("Save Metadata").on_press(Message::SaveMetadata),
                text(format!("Revision {}", self.edit_revision)).size(12),
            ]
            .spacing(10),
            horizontal_rule(1),
            self.category_editor(),
        ]
        .spacing(10)
        .width(Length::Fill)
        .into()
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
            text("Abstract").size(13),
            text_editor(&self.draft.abstract_content)
                .placeholder("Abstract")
                .on_action(Message::DraftAbstract)
                .height(Length::Fixed(180.0)),
        ]
        .spacing(8)
        .into()
    }

    fn category_editor(&self) -> Element<'_, Message> {
        let selected = self.selected_items.iter().cloned().collect::<Vec<_>>();
        let common = common_categories(&self.data.items, &selected);
        let mut current = column![].spacing(6);
        for category in &common {
            let path = category.clone();
            current = current.push(
                row![
                    text(category.clone()).width(Length::Fill),
                    button("Remove")
                        .on_press(Message::RemoveCategoryFromSelection(path)),
                ]
                .spacing(8),
            );
        }

        let mut available = column![].spacing(6);
        for category in available_categories(&self.data.categories, &common) {
            let path = category.path.clone();
            available = available.push(
                row![
                    text(&category.path).width(Length::Fill),
                    button("Add")
                        .on_press(Message::AddCategoryToSelection(path)),
                ]
                .spacing(8),
            );
        }

        column![
            text(format!("Categories for {} item(s)", selected.len()))
                .size(18),
            row![
                text_input("Category path", &self.category_input)
                    .on_input(Message::CategoryInputChanged),
                button("Create Category").on_press(Message::CreateCategory),
            ]
            .spacing(8),
            row![
                column![text("Current Categories").size(16), current]
                    .spacing(8)
                    .width(Length::Fill),
                vertical_rule(1),
                column![text("Available Categories").size(16), available]
                    .spacing(8)
                    .width(Length::Fill),
            ]
            .spacing(12),
        ]
        .spacing(10)
        .into()
    }

    fn files_tab(&self) -> Element<'_, Message> {
        let selected = self.current_item_id().unwrap_or_default();
        let drop_text = self
            .hovered_file
            .as_ref()
            .map(|path| format!("Drop to add {}", path.display()))
            .unwrap_or_else(|| {
                "Drop a file here to add it to the selected item".to_string()
            });
        let mut files = column![].spacing(6);
        if let Some(document) = &self.item_files {
            for file in &document.files {
                let path = file.path.clone();
                let size = file
                    .bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| file.kind.clone());
                files = files.push(
                    button(
                        row![
                            text(&file.path).width(Length::Fill),
                            text(size).size(12),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .on_press(Message::OpenFile(path)),
                );
            }
        }

        column![
            text("Files").size(24),
            text(selected).size(13),
            row![
                button("Open Folder").on_press(Message::OpenSelectedFolder),
                button("Refresh Files").on_press(Message::RefreshFiles),
            ]
            .spacing(8),
            container(text(drop_text).size(13))
                .padding(12)
                .width(Length::Fill),
            horizontal_rule(1),
            files,
        ]
        .spacing(10)
        .width(Length::Fill)
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
    /// Refresh file listing for the selected item.
    RefreshFiles,
    /// Run daemon scan.
    RunScan,
    /// Switch the browser list between All and one category.
    BrowserSelectionChanged(BrowserSelection),
    /// Update search input text.
    SearchChanged(String),
    /// Execute the current search.
    RunSearch,
    /// Select one item from the browser list.
    SelectItem(String),
    /// Toggle one item in the multi-selection set.
    ToggleItemSelection(String, bool),
    /// Switch the right-side detail tab.
    DetailTabChanged(DetailTab),
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
    DraftAbstract(text_editor::Action),
    /// Save metadata.
    SaveMetadata,
    /// Update category input.
    CategoryInputChanged(String),
    /// Open event log window.
    OpenEventsWindow,
    /// Create a category directory.
    CreateCategory,
    /// Add category to selected items.
    AddCategoryToSelection(String),
    /// Remove category from selected items.
    RemoveCategoryFromSelection(String),
    /// Open the selected item folder in the system viewer.
    OpenSelectedFolder,
    /// Open one item-relative file path in the system viewer.
    OpenFile(String),
    /// Add a dropped file to the selected item.
    DroppedFile(PathBuf),
    /// Handle raw iced window events.
    WindowEvent(window::Event),
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
    /// Exit the desktop process.
    Quit,
}

/// Kind and state of each open window.
#[derive(Clone, Debug)]
pub enum WindowKind {
    /// Main browsing and metadata window.
    Main,
    /// Event log window.
    Events,
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

/// Right-panel page shown for the selected item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailTab {
    /// Metadata and category editing.
    Metadata,
    /// Files currently present in the item folder.
    Files,
}

#[derive(Default)]
struct DesktopData {
    snapshot: DashboardSnapshot,
    items: Vec<ItemDocument>,
    categories: Vec<CategorySummary>,
    events: Vec<Event>,
}

#[derive(Default)]
struct MetadataDraft {
    id: String,
    item_type: String,
    title: String,
    authors: String,
    abstract_note: String,
    abstract_content: text_editor::Content,
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
            abstract_content: text_editor::Content::with_text(
                metadata.abstract_note.as_deref().unwrap_or_default(),
            ),
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
            files: localref_core::model::MetadataFiles::default(),
            tags: localref_core::model::MetadataTags::default(),
            import: localref_core::model::MetadataImport::default(),
            state: localref_core::model::MetadataState::default(),
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

fn item_row_style(
    selected: bool,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let background = match (selected, status) {
        (true, iced::widget::button::Status::Hovered) => {
            Color::from_rgb8(0xDF, 0xE8, 0xFF)
        }
        (true, _) => Color::from_rgb8(0xEA, 0xF0, 0xFF),
        (false, iced::widget::button::Status::Hovered) => {
            Color::from_rgb8(0xF1, 0xF3, 0xF5)
        }
        (false, _) => Color::from_rgb8(0xFF, 0xFF, 0xFF),
    };
    let border_color = if selected {
        Color::from_rgb8(0x00, 0x2F, 0xA7)
    } else {
        Color::from_rgb8(0xDA, 0xDD, 0xE3)
    };
    let text_color = if selected {
        Color::from_rgb8(0x00, 0x2F, 0xA7)
    } else {
        Color::from_rgb8(0x17, 0x20, 0x2A)
    };

    iced::widget::button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border { color: border_color, width: 1.0, radius: 2.0.into() },
        ..iced::widget::button::Style::default()
    }
}

fn attachment_summary(item: &ItemDocument) -> String {
    let count = usize::from(item.main_file.is_some()) + item.extra_files.len();
    match count {
        0 => "No files".to_string(),
        1 => "1 file".to_string(),
        count => format!("{count} files"),
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}

/// Return categories shared by every selected item.
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

/// Return categories that can still be added to the selected items.
fn available_categories<'a>(
    categories: &'a [CategorySummary],
    current: &[String],
) -> Vec<&'a CategorySummary> {
    let current = current.iter().collect::<BTreeSet<_>>();
    categories
        .iter()
        .filter(|category| !current.contains(&category.path))
        .collect()
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

    #[test]
    fn available_categories_hide_categories_already_on_selection() {
        let categories = vec![
            category_summary("Archive"),
            category_summary("Inbox"),
            category_summary("Wireless"),
        ];
        let current = vec!["Inbox".to_string(), "Wireless".to_string()];

        let paths = available_categories(&categories, &current)
            .into_iter()
            .map(|category| category.path.clone())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["Archive".to_string()]);
    }

    fn category_summary(path: &str) -> CategorySummary {
        CategorySummary { path: path.to_string(), item_ids: Vec::new() }
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
