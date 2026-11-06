//! Deciding whether to start, from what the host and guest drivers turned out to be.
//!
//! Driver data layouts change with a driver branch and hardware rules change with an architecture.
//! Those are separate things, each with its own profile inside a vendor backend, so a survey says
//! which branches are known without saying anything about cards.

use crate::PciCard;
use std::fmt;
use std::path::PathBuf;

/// A Windows driver version, as four numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverVersion {
    pub parts: [u16; 4],
}

impl DriverVersion {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = [0; 4];
        let mut numbers = text.trim().split('.');
        for part in &mut parts {
            *part = numbers.next()?.parse().ok()?;
        }
        numbers.next().is_none().then_some(Self { parts })
    }

    /// The form adapter queries use: one number, most significant part first.
    pub fn packed(&self) -> u64 {
        self.parts.iter().fold(0, |packed, &part| packed << 16 | u64::from(part))
    }
}

impl fmt::Display for DriverVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.parts;
        write!(f, "{a}.{b}.{c}.{d}")
    }
}

/// Finds the version on the `DriverVer` line of an INF file. That line reads
/// `DriverVer = date,version`, and may have a comment after it.
pub fn driver_version_from_inf(text: &str) -> Option<DriverVersion> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("DriverVer") {
            return None;
        }
        let value = value.split(';').next()?;
        DriverVersion::parse(value.split(',').nth(1)?)
    })
}

/// What setup found out and stored, which startup must not guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    /// The guest driver's version, from its INF. Nothing else may decide what the guest is told.
    pub guest_driver_version: DriverVersion,
    /// The guest driver's branch, in the vendor's own naming.
    pub guest_branch: String,
    /// Where setup unpacked the guest driver, which a vendor may read answers from.
    pub driver_folder: PathBuf,
    /// The card setup chose, if it chose one.
    pub card: Option<PciCard>,
}

/// One driver's version and branch, in the vendor's own naming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverFound {
    pub version: String,
    pub branch: String,
}

/// How much testing a card has had. This is a label for people, and changes no behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLabel {
    Tested,
    Experimental,
}

/// What a backend found before touching anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverSurvey {
    pub host: DriverFound,
    pub guest: DriverFound,
    /// The backend has a profile for the host's branch.
    pub host_branch_known: bool,
    /// The backend has a profile for the guest's branch.
    pub guest_branch_known: bool,
    /// This exact host version has been tested.
    pub host_version_tested: bool,
    /// This pair of different host and guest branches has been tested.
    pub mixed_branches_tested: bool,
    pub card: CardLabel,
}

impl fmt::Display for DriverSurvey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tested = if self.host_version_tested { "tested" } else { "not tested" };
        write!(
            f,
            "host driver {} (branch {}, {tested}), guest driver {} (branch {}), {:?} card",
            self.host.version, self.host.branch, self.guest.version, self.guest.branch, self.card
        )
    }
}

/// What startup should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartDecision {
    Run,
    RunWithWarning(String),
    Refuse(String),
}

/// Judges a survey. A refusal comes before any request reaches the host's driver, because talking to
/// a branch whose structures we do not know could hand it the wrong sizes.
pub fn decide_start(survey: &DriverSurvey) -> StartDecision {
    let (host, guest) = (&survey.host, &survey.guest);
    if !survey.host_branch_known {
        return StartDecision::Refuse(format!(
            "The host driver {} is on branch {}, which this build has no profile for.",
            host.version, host.branch
        ));
    }
    if !survey.guest_branch_known {
        return StartDecision::Refuse(format!(
            "The guest driver {} is on branch {}, which this build has no profile for.",
            guest.version, guest.branch
        ));
    }
    if host.branch != guest.branch && !survey.mixed_branches_tested {
        return StartDecision::Refuse(format!(
            "The host driver is on branch {} and the guest driver on branch {}. They must match \
             until that pair has been tested.",
            host.branch, guest.branch
        ));
    }
    if !survey.host_version_tested {
        return StartDecision::RunWithWarning(format!(
            "The host driver {} is a known branch, but this point release has not been tested.",
            host.version
        ));
    }
    StartDecision::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn survey() -> DriverSurvey {
        let found =
            |version: &str, branch: &str| DriverFound { version: version.into(), branch: branch.into() };
        DriverSurvey {
            host: found("100.20", "100"),
            guest: found("32.0.15.1020", "100"),
            host_branch_known: true,
            guest_branch_known: true,
            host_version_tested: true,
            mixed_branches_tested: false,
            card: CardLabel::Tested,
        }
    }

    #[test]
    fn a_guest_branch_with_no_profile_is_the_guests_problem_and_says_so() {
        let survey = DriverSurvey { guest_branch_known: false, ..survey() };
        let StartDecision::Refuse(reason) = decide_start(&survey) else { panic!("it should refuse") };
        assert!(reason.starts_with("The guest driver 32.0.15.1020 is on branch 100"), "{reason}");
    }

    #[test]
    fn a_known_branch_at_a_tested_version_runs() {
        assert_eq!(decide_start(&survey()), StartDecision::Run);
    }

    #[test]
    fn another_point_release_runs_with_a_warning() {
        let survey = DriverSurvey { host_version_tested: false, ..survey() };
        assert!(matches!(decide_start(&survey), StartDecision::RunWithWarning(_)));
    }

    #[test]
    fn an_unknown_branch_is_refused() {
        let survey = DriverSurvey { host_branch_known: false, ..survey() };
        assert!(matches!(decide_start(&survey), StartDecision::Refuse(_)));
    }

    #[test]
    fn mismatched_branches_are_refused_until_tested() {
        let mut survey = survey();
        survey.guest.branch = "101".into();
        assert!(matches!(decide_start(&survey), StartDecision::Refuse(_)));
        survey.mixed_branches_tested = true;
        assert_eq!(decide_start(&survey), StartDecision::Run);
    }

    #[test]
    fn a_refusal_beats_a_warning() {
        let mut survey = DriverSurvey { host_version_tested: false, ..survey() };
        survey.guest.branch = "101".into();
        assert!(matches!(decide_start(&survey), StartDecision::Refuse(_)));
    }

    #[test]
    fn the_card_label_changes_nothing() {
        let survey = DriverSurvey { card: CardLabel::Experimental, ..survey() };
        assert_eq!(decide_start(&survey), StartDecision::Run);
    }

    #[test]
    fn versions_pack_most_significant_first() {
        let version = DriverVersion::parse("1.2.3.4").unwrap();
        assert_eq!(version.packed(), 0x0001_0002_0003_0004);
        assert_eq!(version.to_string(), "1.2.3.4");
    }

    #[test]
    fn bad_versions_are_refused() {
        for text in ["", "1.2.3", "1.2.3.4.5", "a.b.c.d", "1.2.3.70000"] {
            assert_eq!(DriverVersion::parse(text), None, "{text}");
        }
    }

    #[test]
    fn the_inf_line_gives_the_version() {
        let inf = "[Version]\nSignature = \"$Windows NT$\"\ndriverver = 01/02/2026, 1.2.3.4 ; note\n";
        assert_eq!(driver_version_from_inf(inf), DriverVersion::parse("1.2.3.4"));
        assert_eq!(driver_version_from_inf("[Version]\n"), None);
    }
}
