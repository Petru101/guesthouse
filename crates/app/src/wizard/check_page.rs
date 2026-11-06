//! Step one: what this PC has.

use super::driver_page::DriverPage;
use super::{Step, Wizard};
use crate::common::in_background;
use crate::rows::{row_button, state_row};
use adw::prelude::*;
use guesthouse_app_core::setup::checks::{Check, CheckState, check_pc};
use std::cell::RefCell;
use std::rc::Rc;

struct Parts {
    wizard: Rc<Wizard>,
    step: Step,
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<adw::ActionRow>>,
}

#[derive(Clone)]
pub struct CheckPage(Rc<Parts>);

impl CheckPage {
    pub fn new(wizard: &Rc<Wizard>, driver: &DriverPage) -> Self {
        let step = Step::new("Check This PC", "check", 1);
        let group = adw::PreferencesGroup::new();
        step.content.add(&group);
        let again = row_button("Check Again");
        group.set_header_suffix(Some(&again));
        let this = Self(Rc::new(Parts { wizard: wizard.clone(), step, group, rows: RefCell::default() }));
        let handle = this.clone();
        again.connect_clicked(move |_| handle.check());
        let pages = wizard.pages.clone();
        let next = driver.page().clone();
        this.0.step.next.connect_clicked(move |_| pages.push(&next));
        this.check();
        this
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.0.step.page
    }

    fn check(&self) {
        self.0.step.next.set_sensitive(false);
        self.0.group.set_description(Some("Looking at this PC"));
        let context = self.0.wizard.window.context();
        let looking = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(check_pc(&looking)),
            move |result| match result {
                Ok(checks) => this.show(&checks),
                Err(text) => this.0.wizard.window.problem(&text),
            },
        );
    }

    fn show(&self, checks: &[Check]) {
        let parts = &self.0;
        for row in parts.rows.borrow_mut().drain(..) {
            parts.group.remove(&row);
        }
        for check in checks {
            let row = state_row(&check.title, check.state, &check.text, &check.fix);
            parts.group.add(&row);
            parts.rows.borrow_mut().push(row);
        }
        let failed = checks.iter().any(|check| check.state == CheckState::Failed);
        parts.group.set_description(Some(if failed {
            "Windows with GPU-PV cannot run yet. Fix what has a red mark, then check again. Plain VMs do not \
             need any of this."
        } else {
            "This PC can run Windows with GPU-PV. Nothing has been changed."
        }));
        parts.step.next.set_sensitive(!failed);
    }
}
