//! The dialog that makes a machine from an ISO. Looking inside the ISO decides what it will be.

use crate::common::{choose_file, in_background};
use crate::rows::{number_row, row_button};
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::gpu_support::{NO_GPU_YET, gpu_ready, kind_offered};
use guesthouse_app_core::host::{host_ram_limit, physical_cores, plain_ram_limit};
use guesthouse_app_core::iso::{Iso, read_iso};
use guesthouse_app_core::library::{
    MAX_DISK_GIB, MIN_PLAIN_DISK_GIB, MIN_WINDOWS_DISK_GIB, NO_DRIVER_FILES, NewMachine, create,
};
use guesthouse_app_core::machine::{Kind, MIN_CORES, MIN_RAM_GIB};
use guesthouse_app_core::settings::driver_folder;
use guesthouse_app_core::setup::checks::driver_warning;
use guesthouse_app_core::tasks::no_slot_text;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

const NO_GPU: &str = "This machine gets no GPU acceleration. It runs on the CPU, with a basic display.";
const UBUNTU_NOTE: &str =
    "Installs by itself. Its GPU is a preview, and if the GPU can't start it boots with a plain screen.";
const WIDTH: i32 = 520;
const HEIGHT: i32 = 620;
const NOTE_MARGIN: i32 = 12;

/// Sizes to start from. A plain machine is usually something small being tried out.
struct Sizes {
    cores: u32,
    ram: u64,
    disk: u32,
}

const WINDOWS_SIZES: Sizes = Sizes { cores: 8, ram: 16, disk: 128 };
const PLAIN_SIZES: Sizes = Sizes { cores: 4, ram: 4, disk: 32 };
const UBUNTU_SIZES: Sizes = Sizes { cores: 8, ram: 8, disk: 80 };

struct Parts {
    window: Window,
    dialog: adw::Dialog,
    create_button: gtk::Button,
    iso_row: adw::ActionRow,
    verdict: adw::ActionRow,
    hardware: adw::PreferencesGroup,
    name: adw::EntryRow,
    cores: adw::SpinRow,
    ram: adw::SpinRow,
    disk: adw::SpinRow,
    note: gtk::Label,
    iso_path: RefCell<Option<String>>,
    iso: RefCell<Option<Iso>>,
    no_driver: Cell<bool>,
    vm_running: Cell<bool>,
}

#[derive(Clone)]
pub struct NewMachineDialog(Rc<Parts>);

fn hardware_row(group: &adw::PreferencesGroup, title: &str, lowest: f64, highest: f64) -> adw::SpinRow {
    let row = number_row(title, lowest, lowest, highest);
    group.add(&row);
    row
}

impl NewMachineDialog {
    pub fn new(window: &Window) -> Self {
        let context = window.context();
        let create_button = gtk::Button::builder().label("Create").sensitive(false).build();
        create_button.add_css_class("suggested-action");
        let cancel = gtk::Button::with_label("Cancel");
        let header =
            adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
        header.pack_start(&cancel);
        header.pack_end(&create_button);
        let page = adw::PreferencesPage::new();
        let disc = adw::PreferencesGroup::new();
        page.add(&disc);
        let iso_row =
            adw::ActionRow::builder().title("ISO").subtitle("Choose the disc to install from.").build();
        let choose = row_button("Choose");
        iso_row.add_suffix(&choose);
        disc.add(&iso_row);
        let verdict = adw::ActionRow::builder().visible(false).activatable(false).build();
        disc.add(&verdict);
        let hardware = adw::PreferencesGroup::new();
        hardware.set_sensitive(false);
        page.add(&hardware);
        let name = adw::EntryRow::builder().title("Name").build();
        hardware.add(&name);
        let cores = hardware_row(&hardware, "Cores", 1.0, physical_cores(&context.cpus).max(1).into());
        let ram = hardware_row(&hardware, "RAM in GiB", 1.0, 1.0);
        let disk = hardware_row(&hardware, "Disk in GiB", MIN_PLAIN_DISK_GIB.into(), MAX_DISK_GIB.into());
        let note = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .margin_start(NOTE_MARGIN)
            .margin_end(NOTE_MARGIN)
            .margin_bottom(NOTE_MARGIN)
            .build();
        note.add_css_class("dim-label");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&page);
        column.append(&note);
        page.set_vexpand(true);
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&column));
        toolbar.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title("New Machine")
            .content_width(WIDTH)
            .content_height(HEIGHT)
            .child(&toolbar)
            .build();
        let this = Self(Rc::new(Parts {
            window: window.clone(),
            dialog,
            create_button,
            iso_row,
            verdict,
            hardware,
            name,
            cores,
            ram,
            disk,
            note,
            iso_path: RefCell::default(),
            iso: RefCell::default(),
            no_driver: Cell::new(false),
            vm_running: Cell::new(false),
        }));
        this.set_vm_running(window.vm_running());
        this.connect(&cancel, &choose);
        this
    }

    fn connect(&self, cancel: &gtk::Button, choose: &gtk::Button) {
        let this = self.clone();
        self.0.name.connect_changed(move |_| this.update());
        let dialog = self.0.dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
        let this = self.clone();
        choose.connect_clicked(move |_| this.on_choose());
        let this = self.clone();
        self.0.create_button.connect_clicked(move |_| this.on_create());
        let window = self.0.window.clone();
        self.0.dialog.connect_closed(move |_| window.new_machine_closed());
    }

    pub fn present(&self) {
        self.0.dialog.present(Some(self.0.window.widget()));
    }

    pub fn set_vm_running(&self, running: bool) {
        self.0.vm_running.set(running);
        let note = if running { format!("{} Shut one down first.", no_slot_text()) } else { String::new() };
        self.0.note.set_label(&note);
        self.update();
    }

    fn on_choose(&self) {
        let window = self.0.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_file(
            window.widget(),
            "Choose an ISO",
            "Disc images",
            "iso",
            move |path| this.read_iso(path),
            move |text| failing.problem(&text),
        );
    }

    /// Takes this ISO as if it had been chosen. It's public for the page checks, where no file dialog
    /// can be answered.
    pub fn read_iso(&self, path: String) {
        *self.0.iso_path.borrow_mut() = Some(path.clone());
        *self.0.iso.borrow_mut() = None;
        self.0.iso_row.set_subtitle(&path);
        self.0.hardware.set_sensitive(false);
        self.show_verdict("Reading the ISO", "");
        self.update();
        let context = self.0.window.context();
        let this = self.clone();
        let reading = path.clone();
        in_background(
            &context,
            move || read_iso(&reading),
            move |result| {
                // Another choice may have come since. Only the newest counts.
                if this.0.iso_path.borrow().as_deref() != Some(&path) {
                    return;
                }
                match result {
                    Ok(iso) => this.show_iso(&path, iso),
                    Err(text) => this.show_verdict(&text, ""),
                }
            },
        );
    }

    fn show_verdict(&self, title: &str, subtitle: &str) {
        let verdict = &self.0.verdict;
        verdict.set_visible(true);
        verdict.set_title(&gtk::glib::markup_escape_text(title));
        verdict.set_subtitle(&gtk::glib::markup_escape_text(subtitle));
    }

    fn describe(&self, iso: &Iso) {
        let context = self.0.window.context();
        let windows = iso.kind == Kind::Windows;
        if iso.kind == Kind::Linux {
            self.show_verdict(&iso.title, UBUNTU_NOTE);
            return;
        }
        self.0.no_driver.set(windows && !driver_folder(&context).is_dir());
        if self.0.no_driver.get() {
            self.show_verdict(&iso.title, NO_DRIVER_FILES);
        } else if windows {
            let warning = driver_warning(&context).map_or_else(String::new, |text| format!(" {text}"));
            self.show_verdict(&iso.title, &format!("Installs by itself and starts with GPU-PV.{warning}"));
        } else {
            let later = if gpu_ready(&context) { "" } else { NO_GPU_YET };
            self.show_verdict(
                &format!("{}: a plain VM", iso.title),
                format!("You install it by hand. {NO_GPU} {later}").trim(),
            );
        }
    }

    fn set_sizes(&self, kind: Kind) {
        let context = self.0.window.context();
        let windows = kind == Kind::Windows;
        let sizes = match kind {
            Kind::Windows => WINDOWS_SIZES,
            Kind::Linux => UBUNTU_SIZES,
            Kind::Plain => PLAIN_SIZES,
        };
        let limit = if windows { host_ram_limit(&context).unwrap_or(0) } else { plain_ram_limit(&context) };
        let parts = &self.0;
        let cores = physical_cores(&context.cpus);
        parts.cores.adjustment().set_lower(if windows { MIN_CORES.into() } else { 1.0 });
        parts.cores.set_value(sizes.cores.min(cores).into());
        parts.ram.adjustment().set_lower(if windows { MIN_RAM_GIB.into() } else { 1.0 });
        parts.ram.adjustment().set_upper(limit.max(1) as f64);
        parts.ram.set_value(sizes.ram.min(limit) as f64);
        parts.ram.set_subtitle(&format!("Up to {limit} GiB."));
        let least = if windows { MIN_WINDOWS_DISK_GIB } else { MIN_PLAIN_DISK_GIB };
        parts.disk.adjustment().set_lower(least.into());
        parts.disk.set_value(sizes.disk.into());
    }

    fn show_iso(&self, path: &str, iso: Iso) {
        let kind = kind_offered(&self.0.window.context(), iso.kind);
        let iso = Iso { kind, ..iso };
        self.describe(&iso);
        self.set_sizes(iso.kind);
        if self.0.name.text().is_empty() {
            let stem = Path::new(path).file_stem().map(|name| name.to_string_lossy().into_owned());
            self.0.name.set_text(&match iso.kind {
                Kind::Windows => "Windows 11".to_owned(),
                Kind::Linux => "Ubuntu".to_owned(),
                Kind::Plain => stem.unwrap_or_default(),
            });
        }
        *self.0.iso.borrow_mut() = Some(iso);
        self.0.hardware.set_sensitive(true);
        self.update();
    }

    fn update(&self) {
        let parts = &self.0;
        let ready = parts.iso.borrow().is_some()
            && !parts.name.text().trim().is_empty()
            && !parts.vm_running.get()
            && !parts.no_driver.get();
        parts.create_button.set_sensitive(ready);
    }

    fn on_create(&self) {
        let parts = &self.0;
        let (Some(iso), Some(path)) = (parts.iso.borrow().clone(), parts.iso_path.borrow().clone()) else {
            return;
        };
        parts.create_button.set_sensitive(false);
        let new = NewMachine {
            name: parts.name.text().to_string(),
            iso: path,
            kind: iso.kind,
            cores: parts.cores.value() as u32,
            ram_gib: parts.ram.value() as u32,
            disk_gib: parts.disk.value() as u32,
        };
        let context = parts.window.context();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || create(&working, &new),
            move |result| match result {
                Ok(machine) => {
                    let window = &this.0.window;
                    let action = if machine.installing { "install" } else { "start" };
                    window.add_machine(machine.clone());
                    window.launch(action, &machine, &[], None);
                    this.0.dialog.close();
                }
                Err(text) => {
                    this.0.window.problem(&text);
                    this.update();
                }
            },
        );
    }
}
