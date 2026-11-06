//! Step two: NVIDIA's driver for the guest, from the user's own copy of the installer.

use super::done_page::done_page;
use super::{Step, Wizard};
use crate::common::{choose_file, in_background};
use crate::rows::{row_button, state_row};
use adw::prelude::*;
use gtk::glib;
use guesthouse_app_core::settings::driver_folder;
use guesthouse_app_core::setup::cards::remember_card;
use guesthouse_app_core::setup::checks::CheckState;
use guesthouse_app_core::setup::driver::{Extraction, WINDOWS_DRIVER, check_installer, driver_problem};
use guesthouse_app_core::{Failure, Result};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

const BAR_WIDTH: i32 = 160;

struct Parts {
    wizard: Rc<Wizard>,
    step: Step,
    group: adw::PreferencesGroup,
    file_row: adw::ActionRow,
    choose: gtk::Button,
    progress_row: adw::ActionRow,
    bar: gtk::ProgressBar,
    verdict: RefCell<Option<adw::ActionRow>>,
    extraction: RefCell<Option<Arc<Extraction>>>,
    /// The driver's files are in place, so Next can go on.
    ready: Cell<bool>,
}

#[derive(Clone)]
pub struct DriverPage(Rc<Parts>);

impl DriverPage {
    pub fn new(wizard: &Rc<Wizard>) -> Self {
        let step = Step::new("NVIDIA Driver", "driver", 2);
        let setup_folder = wizard.window.context().paths.setup_folder();
        let group = adw::PreferencesGroup::builder()
            .description(format!(
                "The guest runs NVIDIA's own driver for Windows, version {WINDOWS_DRIVER}. Download that version \
                 from NVIDIA, then choose its installer. It does not run here: Guesthouse only copies its \
                 driver files to {}.",
                setup_folder.display()
            ))
            .build();
        step.content.add(&group);
        let file_row = adw::ActionRow::builder()
            .title("Driver installer")
            .subtitle("Choose the .exe file.")
            .use_markup(false)
            .build();
        let choose = row_button("Choose");
        file_row.add_suffix(&choose);
        group.add(&file_row);
        let progress_row =
            adw::ActionRow::builder().title("Copying the driver's files").visible(false).build();
        let bar = gtk::ProgressBar::builder().valign(gtk::Align::Center).width_request(BAR_WIDTH).build();
        progress_row.add_suffix(&bar);
        let stop = row_button("Stop");
        progress_row.add_suffix(&stop);
        group.add(&progress_row);
        let this = Self(Rc::new(Parts {
            wizard: wizard.clone(),
            step,
            group,
            file_row,
            choose,
            progress_row,
            bar,
            verdict: RefCell::default(),
            extraction: RefCell::default(),
            ready: Cell::new(false),
        }));
        let handle = this.clone();
        this.0.choose.connect_clicked(move |_| handle.on_choose());
        let handle = this.clone();
        stop.connect_clicked(move |_| handle.stop());
        let handle = this.clone();
        this.0.step.next.connect_clicked(move |_| handle.on_next());
        this.look_for_files();
        this
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.0.step.page
    }

    fn show_verdict(&self, state: CheckState, title: &str, text: &str) {
        self.clear_verdict();
        let row = state_row(title, state, text, "");
        self.0.group.add(&row);
        *self.0.verdict.borrow_mut() = Some(row);
    }

    fn clear_verdict(&self) {
        if let Some(old) = self.0.verdict.borrow_mut().take() {
            self.0.group.remove(&old);
        }
    }

    fn look_for_files(&self) {
        let context = self.0.wizard.window.context();
        let looking = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(driver_problem(&driver_folder(&looking)).is_none()),
            move |result| match result {
                Ok(true) => {
                    this.use_files(
                        "These files are already here. Go on, or choose the installer to copy them again.",
                    );
                }
                Ok(false) => {}
                Err(text) => this.0.wizard.window.problem(&text),
            },
        );
    }

    fn use_files(&self, text: &str) {
        self.0.ready.set(true);
        self.0.step.next.set_sensitive(true);
        self.show_verdict(CheckState::Ok, &format!("Driver {WINDOWS_DRIVER} is ready"), text);
    }

    fn on_choose(&self) {
        let window = self.0.wizard.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_file(
            window.widget(),
            "Choose NVIDIA's Driver Installer",
            "Windows installers",
            "exe",
            move |path| this.read_installer(path),
            move |text| failing.problem(&text),
        );
    }

    fn read_installer(&self, path: String) {
        let parts = &self.0;
        parts.file_row.set_subtitle(&path);
        parts.choose.set_sensitive(false);
        parts.step.next.set_sensitive(false);
        self.clear_verdict();
        parts.progress_row.set_title("Reading the installer");
        parts.progress_row.set_visible(true);
        parts.bar.set_fraction(0.0);
        let extraction = Arc::new(Extraction::new(&path));
        *parts.extraction.borrow_mut() = Some(extraction.clone());
        let (progress, percentages) = async_channel::unbounded();
        let bar = parts.bar.clone();
        let row = parts.progress_row.clone();
        glib::spawn_future_local(async move {
            while let Ok(percent) = percentages.recv().await {
                row.set_title("Copying the driver's files");
                bar.set_fraction(f64::from(percent) / 100.0);
            }
        });
        let context = parts.wizard.window.context();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || -> Result<PathBuf> {
                check_installer(&path)?;
                if extraction.cancelled() {
                    return Err(Failure::new("Stopped."));
                }
                extraction.run(&working, &|percent| {
                    let _ = progress.try_send(percent);
                })
            },
            move |result| {
                this.finish();
                match result {
                    Ok(folder) => {
                        let text = format!("The files are in {}.", folder.display());
                        this.use_files(&text);
                    }
                    Err(text) => this.show_verdict(CheckState::Failed, &text, ""),
                }
            },
        );
    }

    fn on_next(&self) {
        let window = &self.0.wizard.window;
        match remember_card(&window.context()) {
            Ok(()) => self.0.wizard.pages.push(&done_page(&self.0.wizard)),
            Err(failure) => window.problem(&failure.0),
        }
    }

    pub fn stop(&self) {
        if let Some(extraction) = self.0.extraction.borrow().as_ref() {
            extraction.cancel();
        }
    }

    fn finish(&self) {
        let parts = &self.0;
        *parts.extraction.borrow_mut() = None;
        parts.progress_row.set_visible(false);
        parts.choose.set_sensitive(true);
        parts.step.next.set_sensitive(parts.ready.get());
    }
}
