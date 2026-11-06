//! The guest's network, as QEMU's words.

use super::arguments::words;
use super::{SHARE_ADDRESS, VmPlan};
use crate::network::{self, BRIDGE_HELPER, CONTROL_MAC, GUEST_AGENT_PORT, Mode};
use crate::text::{qemu_value, shell_quote};

/// Passt copies the host's address by default, and these can't clash with the private link. The
/// guest gets DNS through this address and no other way to the host.
const PASST_NETWORK: &str = "address=10.0.3.15,netmask=255.255.255.0,gateway=10.0.3.2,map-host-loopback=none,\
                             dns=10.0.3.3,dns-forward=10.0.3.3";
/// Passt's default is more than an e1000e takes.
const PASST_MTU: u32 = 1500;

impl VmPlan {
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
            (Mode::Nat, true) => {
                let resolver = self.network.resolver.as_ref();
                let dns_host = resolver.map_or_else(String::new, |address| format!(",dns-host={address}"));
                format!("passt,{model},mtu={PASST_MTU},{PASST_NETWORK}{dns_host}")
            }
            (Mode::Nat, false) => format!("user,{model},{link}"),
            (Mode::None, _) => format!("user,{model},restrict=on,{link}"),
            (Mode::Bridged, _) => {
                let bridge = if self.network.bridge.is_empty() {
                    network::DEFAULT_BRIDGE
                } else {
                    &self.network.bridge
                };
                format!("bridge,{model},br={bridge},helper={BRIDGE_HELPER}")
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
