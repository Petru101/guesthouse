//! USB devices: what is plugged in, what the host may be using it for, and passing it to a VM.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Machine, sorted_names};
use crate::monitor::{Command, failed};
use crate::tasks::{State, slot_of, status};
use rustix::fs::{Access, access};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

const HUB_CLASS: &str = "09";
const QEMU_USB_TYPE: &str = "child<usb-host>";
const QEMU_USB_BUS: &str = "xhci.0";
/// How QEMU opens a file it was handed, from the set it was put in.
const HANDED_FILE: &str = "/dev/fdset/";
/// A set of its own for each device, by bus and address, so two at once don't meet.
const SET_PER_BUS: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub bus: u32,
    pub address: u32,
    /// Four hex digits.
    pub vendor: String,
    pub product: String,
    pub name: String,
    /// What the host may be using it for, as words to show the user.
    pub roles: Vec<String>,
}

impl UsbDevice {
    fn node(&self, context: &Context) -> PathBuf {
        context.usb_nodes.join(format!("{:03}/{:03}", self.bus, self.address))
    }

    pub fn qemu_id(&self) -> String {
        format!("usb-{}-{}", self.bus, self.address)
    }

    fn file_set(&self) -> u32 {
        self.bus * SET_PER_BUS + self.address
    }

    /// QEMU runs as this user. It would only say it failed to open the device.
    pub fn can_open(&self, context: &Context) -> bool {
        access(self.node(context), Access::READ_OK | Access::WRITE_OK).is_ok()
    }

    /// The one step that needs root. uaccess gives the device to whoever is logged in.
    pub fn access_command(&self) -> String {
        let (vendor, product) = (&self.vendor, &self.product);
        let rule = format!(
            "SUBSYSTEM==\"usb\", ENV{{DEVTYPE}}==\"usb_device\", ATTR{{idVendor}}==\"{vendor}\", \
             ATTR{{idProduct}}==\"{product}\", TAG+=\"uaccess\""
        );
        format!(
            "echo '{rule}' | sudo tee /etc/udev/rules.d/70-guesthouse-{vendor}-{product}.rules\n\
             sudo udevadm control --reload\n\
             sudo udevadm trigger --attr-match=idVendor={vendor} --attr-match=idProduct={product}"
        )
    }
}

/// A text file in sysfs, which is empty if it isn't there.
pub fn sysfs_text(path: &Path) -> String {
    fs::read(path).map(|data| String::from_utf8_lossy(&data).trim().to_owned()).unwrap_or_default()
}

fn role_of(class: &str, protocol: &str) -> Option<&'static str> {
    match (class, protocol) {
        ("03", "01") => Some("keyboard"),
        ("03", "02") => Some("mouse"),
        ("01", _) => Some("sound"),
        ("03", _) => Some("controls"),
        ("08", _) => Some("disk"),
        ("e0", _) => Some("wireless"),
        _ => None,
    }
}

fn usb_roles(root: &Path, name: &str) -> Vec<String> {
    let mut roles = Vec::new();
    let prefix = format!("{name}:");
    for entry in sorted_names(root).iter().filter(|entry| entry.starts_with(&prefix)) {
        let class = sysfs_text(&root.join(entry).join("bInterfaceClass"));
        let protocol = sysfs_text(&root.join(entry).join("bInterfaceProtocol"));
        if let Some(role) = role_of(&class, &protocol).filter(|role| !roles.iter().any(|old| old == role)) {
            roles.push(role.to_owned());
        }
    }
    roles
}

/// Everything plugged in but hubs, which QEMU can't pass on.
pub fn usb_devices(context: &Context) -> Vec<UsbDevice> {
    let root = &context.usb_devices;
    let mut found = Vec::new();
    for name in sorted_names(root) {
        let folder = root.join(&name);
        let text = |field: &str| sysfs_text(&folder.join(field));
        if name.contains(':') || text("bDeviceClass") == HUB_CLASS {
            continue;
        }
        let (vendor, product) = (text("idVendor"), text("idProduct"));
        let words = [text("manufacturer"), text("product")];
        let title = words.iter().filter(|word| !word.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        found.push(UsbDevice {
            bus: text("busnum").parse().unwrap_or(0),
            address: text("devnum").parse().unwrap_or(0),
            name: if title.is_empty() { format!("USB device {vendor}:{product}") } else { title },
            roles: usb_roles(root, &name),
            vendor,
            product,
        });
    }
    found
}

/// The ids of what is attached to the running VM. They're gone when it stops.
pub fn attached_usb(context: &Context, machine: &Machine) -> BTreeSet<String> {
    let command = Command::new("qom-list", json!({"path": "/machine/peripheral"}));
    let answer = context.ask_monitor(slot_of(context, machine), &command);
    let children = answer.as_ref().and_then(|found| found["return"].as_array().cloned()).unwrap_or_default();
    let wanted = children.iter().filter(|child| child["type"] == QEMU_USB_TYPE);
    wanted.filter_map(|child| Some(child["name"].as_str()?.to_owned())).collect()
}

/// The app opens the device and hands it to QEMU, as a jailed QEMU can't open one itself. The bus and
/// address stay, so a QEMU that can't take a handed device still opens this one and no other.
pub fn attach_usb(context: &Context, machine: &Machine, device: &UsbDevice) -> Result<()> {
    if status(context, machine).state != State::Running {
        return Err(Failure::new("The VM must be running."));
    }
    let opened = device
        .can_open(context)
        .then(|| OpenOptions::new().read(true).write(true).open(device.node(context)));
    let Some(Ok(node)) = opened else {
        return Err(Failure(format!("This user cannot open {}.", device.name)));
    };
    let set = device.file_set();
    let commands = [
        Command::new("add-fd", json!({"fdset-id": set})),
        Command::new(
            "device_add",
            json!({"driver": "usb-host", "id": device.qemu_id(), "bus": QEMU_USB_BUS,
                   "hostdevice": format!("{HANDED_FILE}{set}"),
                   "hostbus": device.bus, "hostaddr": device.address}),
        ),
        // The device holds its own copy now, and the set's goes.
        Command::new("remove-fd", json!({"fdset-id": set})),
    ];
    let answers = context.ask_monitor_handing(slot_of(context, machine), node.as_fd(), &commands);
    let added = answers.get(1).cloned();
    if failed(&added) {
        let refusal = answers.iter().find_map(|answer| answer["error"]["desc"].as_str());
        let reason = refusal.unwrap_or("QEMU did not answer.");
        return Err(Failure(format!("Could not attach {}: {reason}", device.name)));
    }
    Ok(())
}

pub fn detach_usb(context: &Context, machine: &Machine, device: &UsbDevice) -> Result<()> {
    let command = Command::new("device_del", json!({"id": device.qemu_id()}));
    if failed(&context.ask_monitor(slot_of(context, machine), &command)) {
        return Err(Failure(format!("Could not detach {}.", device.name)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::stand_in::RecordingMonitor;
    use crate::processes::stand_in::fake_process;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    fn fake_usb(
        scratch: &Scratch,
        name: &str,
        (bus, address): (u32, u32),
        ids: (&str, &str),
        title: &str,
        class: &str,
        interfaces: &[(&str, &str)],
    ) {
        let bus = bus.to_string();
        let address = address.to_string();
        let fields = [
            ("busnum", bus.as_str()),
            ("devnum", &address),
            ("idVendor", ids.0),
            ("idProduct", ids.1),
            ("product", title),
            ("bDeviceClass", class),
        ];
        for (field, value) in fields {
            scratch.write(&format!("usb/{name}/{field}"), format!("{value}\n"));
        }
        for (number, (class, protocol)) in interfaces.iter().enumerate() {
            scratch.write(&format!("usb/{name}:1.{number}/bInterfaceClass"), format!("{class}\n"));
            scratch.write(&format!("usb/{name}:1.{number}/bInterfaceProtocol"), format!("{protocol}\n"));
        }
    }

    fn found(context: &Context) -> std::collections::HashMap<String, UsbDevice> {
        usb_devices(context).into_iter().map(|device| (device.name.clone(), device)).collect()
    }

    fn roles(device: &UsbDevice) -> Vec<&str> {
        device.roles.iter().map(String::as_str).collect()
    }

    #[test]
    fn devices_are_read_with_what_the_host_uses_them_for() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.usb_devices = scratch.path("usb");
        fake_usb(&scratch, "usb1", (1, 1), ("1d6b", "0002"), "Root hub", "09", &[]);
        fake_usb(&scratch, "1-1", (1, 2), ("31e3", "1402"), "Keyboard", "00", &[("03", "01"), ("03", "00")]);
        fake_usb(&scratch, "1-2", (1, 3), ("046d", "c54d"), "Receiver", "00", &[("03", "01"), ("03", "02")]);
        fake_usb(&scratch, "1-3", (1, 4), ("0b05", "1b7c"), "Sound", "00", &[("01", "00"), ("01", "00")]);
        fake_usb(&scratch, "2-1", (2, 7), ("13fe", "6700"), "", "00", &[("08", "50")]);
        fake_usb(&scratch, "2-3", (2, 10), ("0b05", "1d98"), "Fans", "00", &[("03", "00")]);
        fake_usb(&scratch, "2-4", (2, 11), ("0489", "e13a"), "Radio", "00", &[("e0", "01")]);
        fake_usb(&scratch, "2-2", (2, 9), ("abcd", "0001"), "Gadget", "00", &[("ff", "00")]);
        let found = found(&context);
        assert!(!found.contains_key("Root hub"), "a hub cannot be passed on");
        assert_eq!(roles(&found["Keyboard"]), ["keyboard", "controls"]);
        assert_eq!(roles(&found["Receiver"]), ["keyboard", "mouse"]);
        assert_eq!(roles(&found["Sound"]), ["sound"]);
        assert_eq!(roles(&found["USB device 13fe:6700"]), ["disk"]);
        assert_eq!(roles(&found["Fans"]), ["controls"]);
        assert_eq!(roles(&found["Radio"]), ["wireless"]);
        assert!(found["Gadget"].roles.is_empty());
        assert_eq!(found["Gadget"].qemu_id(), "usb-2-9");
    }

    fn gadget() -> UsbDevice {
        UsbDevice {
            bus: 2,
            address: 9,
            vendor: "abcd".into(),
            product: "0001".into(),
            name: "Gadget".into(),
            roles: Vec::new(),
        }
    }

    #[test]
    fn the_fix_names_the_device_and_keeps_uaccess_working() {
        let command = gadget().access_command();
        assert!(command.contains(r#"ATTR{idVendor}=="abcd", ATTR{idProduct}=="0001", TAG+="uaccess""#));
        assert!(
            command.contains("70-guesthouse-abcd-0001.rules"),
            "the name decides whether uaccess still applies"
        );
    }

    #[test]
    fn a_device_is_only_passed_on_when_it_can_work() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let monitor = Arc::new(RecordingMonitor::answering(Some(json!({"return": {}}))));
        context.monitor = monitor.clone();
        context.usb_nodes = scratch.path("nodes");
        let device = gadget();
        assert!(!device.can_open(&context));
        let node = scratch.write("nodes/002/009", "");
        let mode = |bits| fs::set_permissions(&node, fs::Permissions::from_mode(bits)).unwrap();
        mode(0o444);
        assert!(!device.can_open(&context), "QEMU needs to write to it");
        let machine = Machine::new("vm", "VM", "/d", "/v");
        mode(0o666);
        assert!(attach_usb(&context, &machine, &device).unwrap_err().0.contains("must be running"));
        let qemu = context.paths.qemu.display().to_string();
        let socket =
            format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
        fake_process(&context, 70, &[&qemu, "-qmp", &socket, "-drive", "file=/d,format=qcow2"], &[], "");
        mode(0o444);
        assert!(attach_usb(&context, &machine, &device).unwrap_err().0.contains("cannot open"));
        assert!(
            monitor.asked().is_empty(),
            "QEMU would only say it failed, and the user needs the way to fix it"
        );
        mode(0o666);
        attach_usb(&context, &machine, &device).unwrap();
        let expected = json!({"driver": "usb-host", "id": "usb-2-9", "bus": "xhci.0",
                              "hostdevice": "/dev/fdset/2009", "hostbus": 2, "hostaddr": 9});
        let names: Vec<String> = monitor.asked().iter().map(|command| command.name.clone()).collect();
        assert_eq!(names, ["add-fd", "device_add", "remove-fd"], "the device goes to QEMU open");
        assert_eq!(monitor.asked()[0], Command::new("add-fd", json!({"fdset-id": 2009})));
        assert_eq!(monitor.asked()[1], Command::new("device_add", expected));
        detach_usb(&context, &machine, &device).unwrap();
        assert_eq!(monitor.asked()[3], Command::new("device_del", json!({"id": "usb-2-9"})));
        monitor.set_answer(Some(json!({"error": {"desc": "no such device"}})));
        assert!(attach_usb(&context, &machine, &device).unwrap_err().0.contains("no such device"));
        assert!(detach_usb(&context, &machine, &device).unwrap_err().0.contains("Could not detach"));
        monitor.set_answer(None);
        assert!(attach_usb(&context, &machine, &device).unwrap_err().0.contains("did not answer"));
        assert!(attached_usb(&context, &machine).is_empty());
        monitor.set_answer(Some(json!({"return": [{"name": "usb-2-9", "type": "child<usb-host>"},
                                                  {"name": "hd0", "type": "child<ide-hd>"}]})));
        assert_eq!(attached_usb(&context, &machine), BTreeSet::from(["usb-2-9".to_owned()]));
    }
}
