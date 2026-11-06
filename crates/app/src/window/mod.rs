//! The main window, and what every page asks of it: telling the user about things, starting tasks and
//! saving machines. The pages it shows are modules of their own.

mod library_page;
mod machine_row;
mod refresh;

pub use machine_row::{MachineHandle, MachineRow};

use crate::common::in_background;
use crate::log_dialog;
use crate::new_machine::NewMachineDialog;
use crate::settings::SettingsDialog;
use crate::viewer::ScreenPage;
use crate::wizard;
use adw::prelude::*;
use gtk::glib;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::library::{delete, import_existing, remove_entry};
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::{Machine, load_all};
use guesthouse_app_core::settings::{Fields, read_prefs, write_prefs};
use guesthouse_app_core::setup::setup_done;
use guesthouse_app_core::tasks::launch;
use guesthouse_daemon::slot::Slot;
use library_page::LibraryPage;
use refresh::RefreshState;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

const DEFAULT_SIZE: (i32, i32) = (1120, 720);
const SMALLEST_WINDOW: (i32, i32) = (360, 300);
const TOAST_SECONDS: u32 = 6;
const PROBLEM_SECONDS: u32 = 10;
/// In fullscreen the viewer's header lies over the picture, and it has no background of its own.
const STYLE: &str = ".screen-header { background: var(--headerbar-bg-color); }
.machine-thumbnail { border-radius: 6px; }";

struct Inner {
    context: Arc<Context>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    pages: adw::NavigationView,
    library: LibraryPage,
    rows: RefCell<BTreeMap<String, MachineRow>>,
    load_errors: RefCell<Vec<String>>,
    settings: RefCell<Option<SettingsDialog>>,
    new_machine: RefCell<Option<NewMachineDialog>>,
    screen_page: RefCell<Option<ScreenPage>>,
    vm_running: Cell<bool>,
    refresh: RefCell<RefreshState>,
}

/// A handle to the window, which any page can hold.
#[derive(Clone)]
pub struct Window(Rc<Inner>);

fn saved_size(prefs: &Fields, name: &str, smallest: i32, default: i32) -> i32 {
    prefs.get(name).and_then(Value::as_i64).map_or(default, |size| (size as i32).max(smallest))
}

impl Window {
    pub fn new(application: &adw::Application, context: Arc<Context>) -> Self {
        let prefs = read_prefs(&context);
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Guesthouse")
            .default_width(saved_size(&prefs, "width", SMALLEST_WINDOW.0, DEFAULT_SIZE.0))
            .default_height(saved_size(&prefs, "height", SMALLEST_WINDOW.1, DEFAULT_SIZE.1))
            .maximized(prefs.get("maximized") == Some(&Value::Bool(true)))
            .build();
        let toasts = adw::ToastOverlay::new();
        let pages = adw::NavigationView::new();
        toasts.set_child(Some(&pages));
        window.set_content(Some(&toasts));
        let library = LibraryPage::new();
        pages.add(&library.page);
        let this = Self(Rc::new(Inner {
            context,
            window,
            toasts,
            pages,
            library,
            rows: RefCell::default(),
            load_errors: RefCell::default(),
            settings: RefCell::default(),
            new_machine: RefCell::default(),
            screen_page: RefCell::default(),
            vm_running: Cell::new(false),
            refresh: RefCell::default(),
        }));
        this.add_style();
        this.0.library.connect_new_machine(&this);
        this.connect_close();
        this.load_machines();
        this.refresh();
        refresh::start_ticking(&this);
        this
    }

    fn add_style(&self) {
        let style = gtk::CssProvider::new();
        style.load_from_string(STYLE);
        // Above USER, as a theme's gtk.css can make header bars see-through.
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&self.0.window),
            &style,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
    }

    fn connect_close(&self) {
        let this = self.clone();
        self.0.window.connect_close_request(move |_| {
            if let Some(page) = this.0.screen_page.borrow().as_ref() {
                page.save_thumbnail_now();
            }
            this.remember_size();
            glib::Propagation::Proceed
        });
    }

    fn remember_size(&self) {
        let window = &self.0.window;
        if !window.is_fullscreen() {
            let maximized = window.is_maximized();
            let mut fields = json!({"maximized": maximized});
            if !maximized {
                fields["width"] = window.width().into();
                fields["height"] = window.height().into();
            }
            self.remember(fields);
        }
    }

    pub fn context(&self) -> Arc<Context> {
        self.0.context.clone()
    }

    pub fn widget(&self) -> &adw::ApplicationWindow {
        &self.0.window
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    pub fn pages(&self) -> &adw::NavigationView {
        &self.0.pages
    }

    pub fn vm_running(&self) -> bool {
        self.0.vm_running.get()
    }

    fn load_machines(&self) {
        let context = &self.0.context;
        let problem = import_existing(context).err();
        let (machines, mut errors) = load_all(context);
        for machine in machines {
            self.add_machine(machine);
        }
        errors.extend(problem.map(|failure| failure.0));
        *self.0.load_errors.borrow_mut() = errors;
        self.show_problems();
    }

    fn show_problems(&self) {
        let errors = self.0.load_errors.borrow();
        self.0.library.show(!self.0.rows.borrow().is_empty(), &errors);
    }

    pub fn add_machine(&self, machine: Machine) -> MachineHandle {
        let id = machine.id.clone();
        let row = MachineRow::new(self, machine);
        let handle = row.machine();
        self.0.library.add(row.widget());
        self.0.rows.borrow_mut().insert(id, row);
        self.show_problems();
        handle
    }

    /// On a thread, as a disk can take a while to delete.
    pub fn remove(&self, machine: Machine) {
        let context = self.context();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || {
                let owned = machine.owns_files(&working);
                if owned { delete(&working, &machine) } else { remove_entry(&working, &machine) }
                    .map(|()| machine.id)
            },
            move |result| match result {
                Ok(id) => this.on_removed(&id),
                Err(problem) => this.problem(&problem),
            },
        );
    }

    fn on_removed(&self, id: &str) {
        if let Some(row) = self.0.rows.borrow_mut().remove(id) {
            self.0.library.remove(row.widget());
        }
        self.show_problems();
    }

    pub fn toast(&self, text: &str, priority: adw::ToastPriority) {
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(text).as_str())
            .timeout(TOAST_SECONDS)
            .priority(priority)
            .build();
        self.0.toasts.add_toast(toast);
    }

    /// Tells the user about a failure, and leaves it in the log with the rest of the story.
    pub fn problem(&self, text: &str) {
        // If the log can't be written, the toast still says what matters.
        let _ = write_log(&self.0.context, &format!("app: {text}"));
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(text).as_str())
            .timeout(PROBLEM_SECONDS)
            .button_label("See Details")
            .build();
        let this = self.clone();
        toast.connect_button_clicked(move |_| this.show_log());
        self.0.toasts.add_toast(toast);
    }

    /// Starts a task. Nothing waits for it, and a thread only collects it when it ends.
    pub fn launch(&self, action: &str, machine: &Machine, arguments: &[String], slot: Option<Slot>) {
        match launch(&self.0.context, action, machine, arguments, slot) {
            Ok(mut child) => {
                thread::spawn(move || child.wait());
            }
            Err(failure) => self.problem(&failure.0),
        }
        self.refresh();
    }

    pub fn save_machine(&self, machine: &Machine) {
        if let Err(failure) = machine.save(&self.0.context) {
            self.problem(&failure.0);
        }
    }

    pub fn remember(&self, fields: Value) {
        let Value::Object(fields) = fields else { return };
        if let Err(failure) = write_prefs(&self.0.context, fields) {
            let _ = write_log(&self.0.context, &format!("app: could not remember a setting: {failure}"));
        }
    }

    pub fn show_thumbnail(&self, id: &str) {
        if let Some(row) = self.0.rows.borrow().get(id) {
            row.show_thumbnail();
        }
    }

    pub fn show_screen(&self, machine: MachineHandle) {
        let page = ScreenPage::new(self, machine);
        self.0.pages.push(page.widget());
        *self.0.screen_page.borrow_mut() = Some(page);
    }

    pub fn screen_closed(&self) {
        *self.0.screen_page.borrow_mut() = None;
    }

    /// Once by itself. After that it's in the menu, and New Machine says when it's needed.
    pub fn offer_setup(&self) {
        let context = &self.0.context;
        let offered =
            read_prefs(context).get("setup_offered").is_some_and(|value| value != &Value::Bool(false));
        if !setup_done(context) && !offered {
            self.remember(json!({"setup_offered": true}));
            self.show_setup();
        }
    }

    pub fn show_setup(&self) {
        wizard::show(self);
    }

    pub fn show_log(&self) {
        log_dialog::show(self);
    }

    pub fn show_new_machine(&self) {
        let dialog = NewMachineDialog::new(self);
        dialog.present();
        *self.0.new_machine.borrow_mut() = Some(dialog);
    }

    pub fn new_machine_closed(&self) {
        *self.0.new_machine.borrow_mut() = None;
    }

    pub fn show_settings(&self, machine: MachineHandle) {
        let id = machine.borrow().id.clone();
        let status = self.0.rows.borrow().get(&id).map(MachineRow::status).unwrap_or_default();
        let dialog = SettingsDialog::new(self, machine);
        dialog.set_status(&status);
        dialog.present();
        *self.0.settings.borrow_mut() = Some(dialog);
    }

    pub fn settings_closed(&self) {
        *self.0.settings.borrow_mut() = None;
    }

    /// The settings dialog open now, for the page checks.
    pub fn open_settings(&self) -> Option<SettingsDialog> {
        self.0.settings.borrow().clone()
    }

    /// The New Machine dialog open now, for the page checks.
    pub fn open_new_machine(&self) -> Option<NewMachineDialog> {
        self.0.new_machine.borrow().clone()
    }

    /// The screen page open now, for the page checks.
    pub fn open_screen(&self) -> Option<ScreenPage> {
        self.0.screen_page.borrow().clone()
    }
}
