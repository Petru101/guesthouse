//! Starting a backend, or refusing to.

use guesthouse_backend::{Backend, Refusal, Setup, StartDecision, decide_start};

/// Surveys the installed drivers, judges what was found, and opens the card if that allows it. No
/// request reaches the host's driver unless the survey passes.
pub fn start_backend<B: Backend>(setup: &Setup) -> Result<B, Refusal> {
    let survey = B::survey(setup)?;
    log::info!("{survey}");
    match decide_start(&survey) {
        StartDecision::Run => {}
        StartDecision::RunWithWarning(warning) => log::warn!("{warning}"),
        StartDecision::Refuse(reason) => return Err(Refusal(reason)),
    }
    B::open(setup, &survey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::DriverVersion;
    use guesthouse_backend_standin::StandIn;

    #[test]
    fn a_matching_stand_in_starts() {
        let setup = Setup {
            guest_driver_version: DriverVersion::parse("1.2.3.4").unwrap(),
            guest_branch: "1".into(),
            driver_folder: Default::default(),
            card: None,
        };
        let backend: StandIn = start_backend(&setup).unwrap();
        assert_eq!(backend.facts().guest_driver_version, 0x0001_0002_0003_0004);
    }
}
