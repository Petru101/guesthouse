//! Changing a machine's network, and judging forwards against what else is running.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::library::unused_mac;
use crate::machine::{Kind, Machine, load_all};
use crate::network::{self, Forward, InUse, Mode, Protocol};
use crate::processes::{opens_disk, qemu_processes, running_slots, uses_disk};
use crate::slots;
use std::collections::BTreeMap;

/// The machines running in other slots, as far as the library knows them.
pub fn other_machines(context: &Context) -> Vec<Machine> {
    let own = context.slot.number();
    let pids: Vec<u32> =
        running_slots(context).into_iter().filter(|(slot, _)| *slot != own).map(|found| found.1).collect();
    let is_running = |machine: &Machine| pids.iter().any(|pid| uses_disk(context, *pid, machine));
    load_all(context).0.into_iter().filter(is_running).collect()
}

/// The TCP ports on this PC that belong to VMs, whichever slot a machine ends up in.
pub fn reserved_ports(context: &Context) -> BTreeMap<u32, String> {
    let mut found = BTreeMap::from([(context.agent_port(None).into(), "this VM's agent".to_owned())]);
    for slot in slots::all() {
        let agent = u32::from(slot.agent_port());
        found.entry(agent).or_insert_with(|| "the port Guesthouse talks to a VM's agent on".to_owned());
        found.insert(slot.debug_port().into(), "a VM's debug port".to_owned());
    }
    found
}

/// Why this forward can't be used beside these running machines.
pub fn forward_problem(
    context: &Context,
    machine: &Machine,
    item: &Forward,
    others: &[Machine],
) -> Option<String> {
    let lowest = network::lowest_free_port();
    let windows = machine.kind == Kind::Windows;
    network::forward_problem(item, &reserved_ports(context), windows, lowest).or_else(|| {
        let others: Vec<(&str, &[Forward])> =
            others.iter().map(|other| (other.name.as_str(), other.forwards.as_slice())).collect();
        let name = network::forward_clash(item, &others)?;
        Some(format!("{} is in use by {name}, which is running.", item.port_text()))
    })
}

/// The forward the user typed, or says what is wrong with it.
pub fn read_forward(
    context: &Context,
    machine: &Machine,
    protocol: Protocol,
    host_text: &str,
    guest_text: &str,
) -> Result<Forward> {
    let (Some(host), Some(guest)) = (network::read_port(host_text), network::read_port(guest_text)) else {
        return Err(Failure(format!("Ports must be whole numbers from 1 to {}.", network::MOST_PORT)));
    };
    let item = Forward::new(protocol, host, guest);
    if machine.forwards.iter().any(|old| (old.protocol, old.host) == (item.protocol, item.host)) {
        return Err(Failure(format!("{} on the host is already forwarded.", item.port_text())));
    }
    match forward_problem(context, machine, &item, &other_machines(context)) {
        Some(problem) => Err(Failure(problem)),
        None => Ok(item),
    }
}

pub fn add_forward(context: &Context, machine: &mut Machine, item: Forward) -> Result<()> {
    machine.forwards.push(item);
    machine.save(context)
}

pub fn remove_forward(context: &Context, machine: &mut Machine, item: &Forward) -> Result<()> {
    machine.forwards.retain(|old| old != item);
    machine.save(context)
}

/// A LAN can't hold two cards with one address, so Bridged is where a machine gets its own.
pub fn set_network(context: &Context, machine: &mut Machine, mode: Mode) -> Result<()> {
    if mode == Mode::Bridged && machine.mac.is_empty() {
        machine.mac = unused_mac(context);
    }
    machine.network_mode = mode;
    machine.save(context)
}

pub fn set_bridge(context: &Context, machine: &mut Machine, text: &str) -> Result<()> {
    let name = text.trim();
    if let Some(problem) = network::bridge_name_problem(name) {
        return Err(Failure(problem));
    }
    machine.bridge = name.to_owned();
    machine.save(context)
}

pub fn set_mac(context: &Context, machine: &mut Machine, text: &str) -> Result<()> {
    let mac = network::read_mac(text).unwrap_or_else(|| text.to_owned());
    if let Some(problem) = network::mac_problem(&mac) {
        return Err(Failure(problem));
    }
    machine.mac = mac;
    machine.save(context)
}

/// What stops this machine's network now. A VM that can't get its forward wouldn't start at all, and
/// QEMU's message wouldn't say which port or why.
pub fn network_start_problems(context: &Context, machine: &Machine, others: &[Machine]) -> Vec<String> {
    let mut found = Vec::new();
    if machine.network_mode == Mode::Bridged {
        for problem in context.bridge.problems(&machine.bridge) {
            found.push(format!("{problem} The machine's network settings say what to do."));
        }
        if let Some(same) = others.iter().find(|other| other.mac == machine.mac) {
            found.push(format!(
                "{} is running with the same MAC address, and they cannot share a network.",
                same.name
            ));
        }
    }
    for item in &machine.forwards {
        let taken = || {
            let host = u16::try_from(item.host).ok()?;
            network::port_taken(item.protocol, host)
                .then(|| format!("{} on this PC is in use by another program.", item.port_text()))
        };
        found.extend(forward_problem(context, machine, item, others).or_else(taken));
    }
    found
}

/// What this running machine's network really is, or `None` if it's off. It may not be what the
/// settings say now: they can change while it runs, and passt can be removed.
pub fn network_in_use(context: &Context, machine: &Machine) -> Option<InUse> {
    let processes = qemu_processes(context);
    let opens = |word: &Vec<u8>| opens_disk(word, machine);
    let (_, words) = processes.iter().find(|(_, words)| words.iter().any(opens))?;
    network::in_use_of(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::load;
    use crate::testing::{Scratch, scratch_context};
    use std::net::TcpListener;

    fn saved(context: &Context) -> Machine {
        let machine = Machine { kind: Kind::Plain, ..Machine::new("older", "Older", "/d", "/v") };
        machine.save(context).unwrap();
        machine
    }

    #[test]
    fn only_bridged_needs_a_mac_of_its_own_and_it_is_made_once() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut older = saved(&context);
        set_network(&context, &mut older, Mode::None).unwrap();
        assert_eq!(older.mac, "");
        set_network(&context, &mut older, Mode::Bridged).unwrap();
        assert!(!older.mac.is_empty() && older.mac != network::DEFAULT_MAC);
        let kept = older.mac.clone();
        set_network(&context, &mut older, Mode::Nat).unwrap();
        set_network(&context, &mut older, Mode::Bridged).unwrap();
        assert_eq!(older.mac, kept, "generated once");
    }

    #[test]
    fn a_refused_change_leaves_the_setting_alone() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut older = saved(&context);
        assert!(set_mac(&context, &mut older, "nothing").unwrap_err().0.contains("six pairs"));
        assert!(set_bridge(&context, &mut older, "br0,x").unwrap_err().0.contains("up to 15"));
        assert_eq!((older.mac.as_str(), older.bridge.as_str()), ("", "br0"));
    }

    #[test]
    fn forwards_are_checked_as_they_are_typed() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut older = saved(&context);
        let item = read_forward(&context, &older, Protocol::Tcp, "28080", "80").unwrap();
        add_forward(&context, &mut older, item).unwrap();
        assert_eq!(load(&context, "older").unwrap().forwards, [item]);
        let refusal = |host, guest| read_forward(&context, &older, Protocol::Tcp, host, guest).unwrap_err().0;
        assert!(refusal("28080", "81").contains("already forwarded"));
        assert!(refusal("ssh", "22").contains("whole numbers"));
        remove_forward(&context, &mut older, &item).unwrap();
        assert!(load(&context, "older").unwrap().forwards.is_empty());
    }

    #[test]
    fn a_clash_with_a_running_machine_names_it() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let older = saved(&context);
        let other = Machine {
            id: "other".into(),
            name: "Other".into(),
            forwards: vec![Forward::new(Protocol::Tcp, 28081, 22)],
            ..older.clone()
        };
        let item = Forward::new(Protocol::Tcp, 28081, 22);
        assert!(forward_problem(&context, &older, &item, &[other]).unwrap().contains("Other"));
    }

    #[test]
    fn a_port_another_program_has_stops_the_start_by_name() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = Forward::new(Protocol::Tcp, listener.local_addr().unwrap().port(), 80);
        let machine = Machine { forwards: vec![taken], ..saved(&context) };
        assert!(network_start_problems(&context, &machine, &[])[0].contains("another program"));
    }

    #[test]
    fn every_slots_agent_port_is_reserved() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let reserved = reserved_ports(&context);
        assert!(slots::all().all(|slot| reserved.contains_key(&slot.agent_port().into())));
    }
}
