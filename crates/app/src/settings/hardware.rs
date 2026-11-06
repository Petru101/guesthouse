//! Cores, memory, screen size and pinning.

use super::{Shared, is_off};
use crate::rows::{choice_row, number_row};
use adw::prelude::*;
use guesthouse_app_core::host::{cpu_list, physical_cores, ram_limit_for, vcache_cpus};
use guesthouse_app_core::machine::{Kind, MIN_CORES, MIN_RAM_GIB, SCREENS};
use guesthouse_app_core::settings::max_vram_gib;
use guesthouse_app_core::tasks::Status;

const VM_MUST_BE_OFF: &str = "The VM must be off to change these.";

pub struct Hardware {
    group: adw::PreferencesGroup,
    cores: adw::SpinRow,
    ram: adw::SpinRow,
    vram: adw::SpinRow,
    screen: adw::ComboRow,
    pin: adw::SwitchRow,
    pin_possible: bool,
}

impl Hardware {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let context = shared.window.context();
        let machine = shared.machine.borrow().clone();
        let plain = machine.kind == Kind::Plain;
        let group = adw::PreferencesGroup::builder().title("Hardware").build();
        page.add(&group);
        let host_cores = physical_cores(&context.cpus);
        let cores = number_row(
            "Cores",
            machine.cores.into(),
            if plain { 1.0 } else { MIN_CORES.into() },
            host_cores.max(machine.cores).into(),
        );
        cores.set_subtitle(&format!("This PC has {host_cores} cores."));
        group.add(&cores);
        let limit = ram_limit_for(&context, &machine).unwrap_or(0);
        let ram = number_row(
            "RAM in GiB",
            machine.ram_gib.into(),
            if plain { 1.0 } else { MIN_RAM_GIB.into() },
            limit.max(machine.ram_gib.into()) as f64,
        );
        ram.set_subtitle(&if plain {
            format!("Up to {limit} GiB.")
        } else {
            format!("Up to {limit} GiB. Guest memory lives in /dev/shm, and the host needs the rest.")
        });
        group.add(&ram);
        let vram = number_row(
            "Video memory cap in GiB",
            machine.vram_gib.into(),
            0.0,
            max_vram_gib(&context).into(),
        );
        vram.set_subtitle(
            "0 is no cap. A program that goes past it loses its GPU device, and others carry on.",
        );
        vram.set_visible(machine.kind == Kind::Windows);
        group.add(&vram);
        let screen = choice_row("Screen size", &SCREENS);
        screen.set_selected(SCREENS.iter().position(|size| *size == machine.screen).unwrap_or(0) as u32);
        screen.set_visible(machine.kind == Kind::Windows);
        group.add(&screen);
        let cpus = vcache_cpus(&context.cpus);
        let pin = adw::SwitchRow::builder().title("V-Cache pinning").active(machine.vcache_pin).build();
        pin.set_subtitle(&if cpus.is_empty() {
            "This CPU has no V-Cache cores.".to_owned()
        } else {
            format!("Keeps the VM on CPUs {}.", cpu_list(&cpus))
        });
        group.add(&pin);
        let this = Self { group, cores, ram, vram, screen, pin, pin_possible: !cpus.is_empty() };
        this.connect(shared);
        this
    }

    fn connect(&self, shared: &Shared) {
        let saving = shared.clone();
        self.cores.connect_value_notify(move |row| {
            saving.machine.borrow_mut().cores = row.value() as u32;
            saving.save();
        });
        let saving = shared.clone();
        self.ram.connect_value_notify(move |row| {
            saving.machine.borrow_mut().ram_gib = row.value() as u32;
            saving.save();
        });
        let saving = shared.clone();
        self.vram.connect_value_notify(move |row| {
            saving.machine.borrow_mut().vram_gib = row.value() as u32;
            saving.save();
        });
        let saving = shared.clone();
        self.screen.connect_selected_notify(move |row| {
            saving.machine.borrow_mut().screen = SCREENS[row.selected() as usize].to_owned();
            saving.save();
        });
        let saving = shared.clone();
        self.pin.connect_active_notify(move |row| {
            saving.machine.borrow_mut().vcache_pin = row.is_active();
            saving.save();
        });
    }

    pub fn set_status(&self, status: &Status) {
        let off = is_off(status);
        self.cores.set_sensitive(off);
        self.ram.set_sensitive(off);
        self.vram.set_sensitive(off);
        self.screen.set_sensitive(off);
        self.pin.set_sensitive(off && self.pin_possible);
        self.group.set_description((!off).then_some(VM_MUST_BE_OFF));
    }
}
