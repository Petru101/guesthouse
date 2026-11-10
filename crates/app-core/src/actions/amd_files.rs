//! AMD's driver files for a Windows guest on an AMD card. Setup puts together the folder the guest
//! gets, but a folder unpacked by an older version of the app may lack files the daemon now names, so
//! it's brought up to date here before an install or a start uses it.

use super::Task;
use crate::failure::Result;
use crate::log::write_log;
use crate::setup::amd::complete_amd_driver;

/// Any problem is said and doesn't stop the task: Direct3D works without what's missing.
pub(super) fn complete_host_driver(task: &mut Task) -> Result<()> {
    if task.machine.windows_amd_card().is_none() {
        return Ok(());
    }
    if let Some(problem) = complete_amd_driver(task.context) {
        write_log(task.context, &format!("AMD driver: {problem}"))?;
        task.error = problem;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Machine;
    use crate::setup::amd::write_amd_setup;
    use crate::setup::amd_guest_folder::stand_in::lay_out_driver;
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_host::stand_in::RecordingHost;
    use std::fs;

    #[test]
    fn a_machine_on_amd_gets_its_driver_completed_and_one_on_nvidia_is_left_alone() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        lay_out_driver(&scratch, "amd/WT6A_INF", "B026716", false);
        let folder = scratch.path("amd/WT6A_INF/B026716");
        write_amd_setup(&context, &folder, "32.0.32015.12012").unwrap();
        let host = RecordingHost::default();
        let windows = Machine::new("w", "W", "/d", "/v");
        complete_host_driver(&mut Task::new(&context, &host, "start", windows.clone())).unwrap();
        assert!(!folder.join("atio6axx.dll").exists(), "not for a machine on NVIDIA");
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..windows };
        let mut task = Task::new(&context, &host, "start", on_amd);
        complete_host_driver(&mut task).unwrap();
        assert_eq!(task.error, "");
        assert!(folder.join("atio6axx.dll").is_file());
    }

    #[test]
    fn a_driver_that_still_lacks_files_is_said_and_the_task_goes_on() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        lay_out_driver(&scratch, "amd/WT6A_INF", "B026716", false);
        fs::remove_dir_all(scratch.path("amd/WT6A_INF/amdvlk")).unwrap();
        write_amd_setup(&context, &scratch.path("amd/WT6A_INF/B026716"), "32.0.32015.12012").unwrap();
        let host = RecordingHost::default();
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..Machine::new("w", "W", "/d", "/v") };
        let mut task = Task::new(&context, &host, "start", on_amd);
        complete_host_driver(&mut task).unwrap();
        assert!(task.error.contains("amd-vulkan64.json"), "{}", task.error);
    }
}
