//! The guest's network, as QEMU's words.

use super::arguments::words;
use super::{PASST_FD, SHARE_ADDRESS, TAP_FD, VmPlan};
use crate::network::{self, CONTROL_MAC, GUEST_AGENT_PORT, Mode, bridge_helper};
use crate::text::{qemu_value, shell_quote};

/// Passt copies the host's address by default, and these can't clash with the private link. The
/// guest gets DNS through this address and no other way to the host. Its default MTU is more than an
/// e1000e takes. QEMU's passt network and passt itself take the same names.
const PASST_NETWORK: [(&str, &str); 7] = [
    ("mtu", "1500"),
    ("address", "10.0.3.15"),
    ("netmask", "255.255.255.0"),
    ("gateway", "10.0.3.2"),
    ("map-host-loopback", "none"),
    ("dns", "10.0.3.3"),
    ("dns-forward", "10.0.3.3"),
];
/// passt tells nothing but what goes wrong.
const PASST_QUIET: &str = "--quiet";
/// Where the jail's launcher notes passt's pid and start time, in the run folder, so it can be found
/// like QEMU.
pub const PASST_PID_FILE: &str = "passt.pid";

impl VmPlan {
    fn passt_settings(&self) -> Vec<(&'static str, String)> {
        let mut settings: Vec<(&str, String)> =
            PASST_NETWORK.iter().map(|(name, value)| (*name, (*value).to_owned())).collect();
        if let Some(address) = &self.network.resolver {
            settings.push(("dns-host", address.clone()));
        }
        settings
    }

    /// passt's own words, for the jail's launcher to start it with. It's told of its socket there.
    pub(super) fn passt_words(&self) -> Vec<String> {
        let mut words = vec![PASST_QUIET.to_owned()];
        for (name, value) in self.passt_settings() {
            words.extend([format!("--{name}"), value]);
        }
        words
    }

    /// The agent, the share and any forwards always go through QEMU's user network, as nothing else
    /// can run a program per connection. Restricted, that network carries them and nothing more.
    fn private_link(&self) -> String {
        let mut link = format!("hostfwd=tcp:127.0.0.1:{}-:{GUEST_AGENT_PORT}", self.agent_port);
        if self.share.is_some() {
            let program = shell_quote(&self.share_program.to_string_lossy());
            link += &format!(",guestfwd=tcp:{SHARE_ADDRESS}:80-cmd:{}", qemu_value(&program));
        }
        for item in &self.network.forwards {
            link += &format!(",hostfwd={}:127.0.0.1:{}-:{}", item.protocol.word(), item.host, item.guest);
        }
        link
    }

    fn first_nic_model(&self) -> String {
        match self.network.mac.as_str() {
            "" => "model=e1000e".to_owned(),
            mac => format!("model=e1000e,mac={mac}"),
        }
    }

    /// The first NIC stays `-nic` in every mode. `-device` would come after the others and move them.
    pub(super) fn first_nic(&self) -> String {
        let (model, link) = (self.first_nic_model(), self.private_link());
        match (self.network.mode, self.network.passt) {
            // The jail's launcher started passt, and QEMU has its end of the connection.
            (Mode::Nat, true) if self.jail.is_some() => format!("socket,{model},fd={PASST_FD}"),
            (Mode::Nat, true) => {
                let settings = self.passt_settings();
                let settings: Vec<String> =
                    settings.iter().map(|(name, value)| format!("{name}={value}")).collect();
                format!("passt,{model},{}", settings.join(","))
            }
            (Mode::Nat, false) => format!("user,{model},{link}"),
            (Mode::None, _) => format!("user,{model},restrict=on,{link}"),
            // The jail's launcher has the device from the helper already, as QEMU can't run it.
            (Mode::Bridged, _) if self.jail.is_some() => format!("tap,{model},fd={TAP_FD}"),
            (Mode::Bridged, _) => {
                let bridge = if self.network.bridge.is_empty() {
                    network::DEFAULT_BRIDGE
                } else {
                    &self.network.bridge
                };
                format!("bridge,{model},br={bridge},helper={}", bridge_helper())
            }
        }
    }

    /// The second NIC goes last, in a slot of its own, so that nothing the guest knows about moves.
    pub(super) fn second_nic(&self) -> Vec<String> {
        let wanted = self.network.mode == Mode::Bridged || self.network.passt;
        if !wanted {
            return Vec::new();
        }
        words(&[
            "-netdev",
            &format!("user,id=link,restrict=on,{}", self.private_link()),
            "-device",
            &format!("e1000e,netdev=link,mac={CONTROL_MAC},bus=pcie.0,addr=0x1e"),
        ])
    }
}
