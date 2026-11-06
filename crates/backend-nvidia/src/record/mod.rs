//! The record the guest's user-mode drivers read about the adapter.
//!
//! On a real install the vendor's kernel driver collects it. Here it is built when the backend
//! starts, from what the RM says about the live card and what the host says about itself.

mod builder;
pub mod card;
mod host;
mod summary;

pub use builder::{RECORD_NODE_STRIDE, build};
pub use card::{ARCHITECTURE_INFO as ARCHITECTURE, CardReader, NAME, PCI_INFO};
pub use host::{HostFacts, read_host_facts, read_host_facts_with_clocks};
pub use summary::{SummaryFacts, summary};
