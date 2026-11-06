//! Keeping the window true to what is running. The looking happens on a thread, as nothing here may
//! hold up the screen, and it runs each second.

use super::Window;
use crate::common::in_background;
use gtk::glib;
use guesthouse_app_core::actions::rebooted;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::machine::{Machine, load_all};
use guesthouse_app_core::processes::all_leftovers;
use guesthouse_app_core::slots;
use guesthouse_app_core::stopped::stopped_by_itself;
use guesthouse_app_core::tasks::{State, Status, free_slot, is_paused, status};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

const TICK: Duration = Duration::from_secs(1);
/// Seconds between looks at QEMU's run state and at what a VM left behind.
const CHECK_TICKS: u32 = 3;

#[derive(Default)]
pub struct RefreshState {
    reading: bool,
    ticks: u32,
    paused_before: BTreeSet<String>,
    /// The machines that were up at the last look, and their slots.
    running_before: BTreeMap<String, u8>,
    leftovers_before: BTreeMap<u8, u64>,
    problem: Option<String>,
}

struct Report {
    statuses: BTreeMap<String, Status>,
    paused: Vec<Machine>,
    left: BTreeMap<u8, u64>,
    rebooted: Vec<Machine>,
    checked: bool,
    saved: Vec<Machine>,
    vm_running: bool,
}

fn look(context: &Arc<Context>, machines: &[Machine], checked: bool) -> Report {
    let statuses: BTreeMap<String, Status> =
        machines.iter().map(|machine| (machine.id.clone(), status(context, machine))).collect();
    let slot_of = |machine: &Machine| statuses[&machine.id].slot.map(slots::number);
    let is_idle_gpu = |machine: &&Machine| {
        let found = &statuses[&machine.id];
        found.state == State::Running && !found.busy && machine.has_gpu()
    };
    let running: Vec<&Machine> = machines.iter().filter(is_idle_gpu).collect();
    let paused = |machine: &&&Machine| {
        checked && context.run_state(slot_of(machine)).is_some_and(|state| is_paused(&state))
    };
    let restarted = |machine: &&&Machine| checked && rebooted(context, slot_of(machine));
    let idle = !statuses.values().any(|found| found.busy);
    Report {
        paused: running.iter().filter(paused).map(|machine| (**machine).clone()).collect(),
        rebooted: running.iter().filter(restarted).map(|machine| (**machine).clone()).collect(),
        left: if checked && idle && !machines.is_empty() { all_leftovers(context) } else { BTreeMap::new() },
        saved: load_all(context).0,
        vm_running: free_slot(context).is_none(),
        checked,
        statuses,
    }
}

pub fn start_ticking(window: &Window) {
    let window = window.clone();
    glib::timeout_add_local(TICK, move || {
        window.refresh();
        glib::ControlFlow::Continue
    });
}

impl Window {
    /// Looks at the VMs, unless a look is already under way.
    pub fn refresh(&self) {
        let mut state = self.0.refresh.borrow_mut();
        if state.reading {
            return;
        }
        state.reading = true;
        state.ticks += 1;
        let checked = state.ticks.is_multiple_of(CHECK_TICKS);
        drop(state);
        let machines: Vec<Machine> =
            self.0.rows.borrow().values().map(|row| row.machine().borrow().clone()).collect();
        let context = self.context();
        let looking = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(look(&looking, &machines, checked)),
            move |result| match result {
                Ok(report) => this.show_status(report),
                Err(problem) => this.on_refresh_failed(problem),
            },
        );
    }

    /// This runs every second, so one failure must not become a toast a second.
    fn on_refresh_failed(&self, text: String) {
        let mut state = self.0.refresh.borrow_mut();
        state.reading = false;
        if state.problem.as_ref() != Some(&text) {
            state.problem = Some(text.clone());
            drop(state);
            self.problem(&text);
        }
    }

    /// A task changes these on disk, and a stale copy here would write them back.
    fn take_saved(&self, saved: &[Machine]) {
        for machine in saved {
            if let Some(row) = self.0.rows.borrow().get(&machine.id) {
                let handle = row.machine();
                let mut current = handle.borrow_mut();
                current.installing = machine.installing;
                current.iso.clone_from(&machine.iso);
            }
        }
    }

    /// What is only looked at now and then. Twice in a row counts, as something can stop the VM for a
    /// moment, or be about to tidy up.
    fn act_on_checks(&self, report: &Report) {
        let (paused_before, leftovers_before) = {
            let state = self.0.refresh.borrow();
            (state.paused_before.clone(), state.leftovers_before.clone())
        };
        for machine in report.paused.iter().filter(|machine| paused_before.contains(&machine.id)) {
            self.toast("The VM was left paused. Resuming it.", adw::ToastPriority::Normal);
            self.launch("resume", machine, &[], None);
        }
        let any_machine = self.0.rows.borrow().values().next().map(|row| row.machine());
        for (slot, file) in &report.left {
            if let Some(machine) = any_machine.as_ref().filter(|_| leftovers_before.get(slot) == Some(file)) {
                let machine = machine.borrow().clone();
                self.launch("clean_up", &machine, &[], Some(slots::number(*slot)));
            }
        }
        for machine in &report.rebooted {
            self.launch("after_reboot", machine, &[], None);
        }
        let mut state = self.0.refresh.borrow_mut();
        state.paused_before = report.paused.iter().map(|machine| machine.id.clone()).collect();
        state.leftovers_before.clone_from(&report.left);
    }

    fn show_machine_status(&self, id: &str, status: &Status, vm_running: bool) {
        let rows = self.0.rows.borrow();
        // A row can be gone if it was deleted while this was being read.
        let Some(row) = rows.get(id) else { return };
        if !status.error.is_empty() && status.error != row.status().error {
            self.problem(&status.error);
        }
        row.set_status(status, vm_running);
        if let Some(settings) = self.0.settings.borrow().as_ref().filter(|dialog| dialog.machine_id() == id) {
            settings.set_status(status);
        }
        let page = self.0.screen_page.borrow().clone();
        if let Some(page) = page.filter(|page| page.machine_id() == id) {
            if status.state == State::Off {
                self.0.pages.pop_to_tag("library");
            } else {
                page.show_hint(Some(status));
            }
        }
    }

    /// A VM can go at any moment, and the row showing Off doesn't say that it wasn't meant to.
    fn say_what_stopped(&self, report: &Report) {
        let up = |(id, status): (&String, &Status)| {
            Some((id.clone(), status.slot.filter(|_| status.state == State::Running)?))
        };
        let running: BTreeMap<String, u8> = report.statuses.iter().filter_map(up).collect();
        let before = std::mem::replace(&mut self.0.refresh.borrow_mut().running_before, running);
        for (id, slot) in before {
            let gone = report.statuses.get(&id).is_some_and(|found| found.state == State::Off && !found.busy);
            let machine = self.0.rows.borrow().get(&id).map(|row| row.machine().borrow().clone());
            if let Some(machine) = machine.filter(|_| gone)
                && let Some(text) = stopped_by_itself(&self.0.context, &machine, slot)
            {
                self.toast(&text, adw::ToastPriority::High);
            }
        }
    }

    fn show_status(&self, report: Report) {
        {
            let mut state = self.0.refresh.borrow_mut();
            state.reading = false;
            state.problem = None;
        }
        self.0.vm_running.set(report.vm_running);
        if let Some(dialog) = self.0.new_machine.borrow().as_ref() {
            dialog.set_vm_running(report.vm_running);
        }
        self.take_saved(&report.saved);
        if report.checked {
            self.act_on_checks(&report);
        }
        self.say_what_stopped(&report);
        for (id, status) in &report.statuses {
            self.show_machine_status(id, status, report.vm_running);
        }
    }
}
