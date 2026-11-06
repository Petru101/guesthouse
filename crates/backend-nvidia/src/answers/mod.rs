//! What the guest's drivers are told about the adapter, worked out from the live card and the user's
//! own driver.

pub mod adapter;
mod caps;
mod guid;
pub mod nodes;
pub mod perf_data;
pub mod queries;
pub mod registry;

pub use adapter::{AdapterAnswers, Inputs};
