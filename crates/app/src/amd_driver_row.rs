//! The row that takes AMD's Windows driver out of AMD's installer, for a Windows machine on an AMD
//! card. It's there only while the driver is missing, and says whether the installer covers the card.

use crate::common::{choose_file, in_background};
use crate::rows::row_button;
use crate::window::Window;
use adw::prelude::*;
use gtk::glib;
use guesthouse_app_core::Failure;
use guesthouse_app_core::setup::amd_driver::{AMD_RELEASE, read_amd_installer, unpack_amd};
use guesthouse_app_core::setup::driver::Extraction;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

const TITLE: &str = "AMD's driver";
const ASK: &str = "Choose AMD's installer for Adrenalin {release}. The display driver is taken out of it.";

struct Parts {
    window: Window,
    row: adw::ActionRow,
    choose: gtk::Button,
    bar: gtk::ProgressBar,
    /// The card the installer must cover.
    device: Cell<u32>,
    extraction: RefCell<Option<Arc<Extraction>>>,
    /// What runs once the driver is unpacked.
    ready: RefCell<Option<Rc<dyn Fn()>>>,
}

#[derive(Clone)]
pub struct AmdDriverRow(Rc<Parts>);

impl AmdDriverRow {
    pub fn new(window: &Window, group: &adw::PreferencesGroup) -> Self {
        let row = adw::ActionRow::builder().title(TITLE).visible(false).build();
        let choose = row_button("Choose");
        row.add_suffix(&choose);
        let bar = gtk::ProgressBar::builder().visible(false).valign(gtk::Align::Center).build();
        row.add_suffix(&bar);
        group.add(&row);
        let this = Self(Rc::new(Parts {
            window: window.clone(),
            row,
            choose: choose.clone(),
            bar,
            device: Cell::new(0),
            extraction: RefCell::default(),
            ready: RefCell::default(),
        }));
        let choosing = this.clone();
        choose.connect_clicked(move |_| choosing.on_choose());
        this
    }

    /// `ready` runs once the driver is unpacked.
    pub fn connect_ready(&self, ready: impl Fn() + 'static) {
        *self.0.ready.borrow_mut() = Some(Rc::new(ready));
    }

    /// Shows the row for the AMD card `device` while its driver is missing, and hides it otherwise.
    pub fn show_for(&self, device: Option<u32>) {
        let parts = &self.0;
        parts.row.set_visible(device.is_some());
        if let Some(device) = device {
            parts.device.set(device);
            parts.row.set_subtitle(&ASK.replace("{release}", AMD_RELEASE));
        }
    }

    fn on_choose(&self) {
        let window = self.0.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_file(
            window.widget(),
            "Choose AMD's Driver Installer",
            "Windows installers",
            "exe",
            move |path| this.unpack(path),
            move |text| failing.problem(&text),
        );
    }

    fn unpack(&self, path: String) {
        let parts = &self.0;
        parts.choose.set_sensitive(false);
        parts.row.set_subtitle("Reading the installer");
        parts.bar.set_fraction(0.0);
        parts.bar.set_visible(true);
        let extraction = Arc::new(Extraction::new(&path));
        *parts.extraction.borrow_mut() = Some(extraction.clone());
        let (progress, percentages) = async_channel::unbounded();
        let (bar, row) = (parts.bar.clone(), parts.row.clone());
        glib::spawn_future_local(async move {
            while let Ok(percent) = percentages.recv().await {
                row.set_subtitle("Copying the driver's files");
                bar.set_fraction(f64::from(percent) / 100.0);
            }
        });
        let device = parts.device.get();
        let context = parts.window.context();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || {
                let package = read_amd_installer(&path, device)?;
                if extraction.cancelled() {
                    return Err(Failure::new("Stopped."));
                }
                unpack_amd(&extraction, &working, &package, &|percent| {
                    let _ = progress.try_send(percent);
                })
            },
            move |result| {
                let parts = &this.0;
                *parts.extraction.borrow_mut() = None;
                parts.bar.set_visible(false);
                parts.choose.set_sensitive(true);
                match result {
                    Ok(_) => {
                        let ready = parts.ready.borrow().clone();
                        if let Some(ready) = ready {
                            ready();
                        }
                    }
                    Err(text) => parts.row.set_subtitle(&glib::markup_escape_text(&text)),
                }
            },
        );
    }

    /// Stops an unpacking that is under way, as when its dialog closes.
    pub fn stop(&self) {
        if let Some(extraction) = self.0.extraction.borrow().as_ref() {
            extraction.cancel();
        }
    }
}
