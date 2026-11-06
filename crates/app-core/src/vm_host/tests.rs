use super::*;
use crate::machine::Kind;
use crate::slots;
use crate::testing::{Scratch, scratch_context};
use crate::vm_args::{Boot, OUTPUT_FILE};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn program(scratch: &Scratch, name: &str, text: &str) -> std::path::PathBuf {
    let path = scratch.write(name, text);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_second_instance_never_reaches_what_finds_vms_by_program() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.own_run = Some(scratch.path("second"));
    let host = LocalHost { context: &context };
    let machine = Machine::new("vm", "VM", "/d", "/v");
    assert!(host.stop(&machine, 1).unwrap_err().0.contains("leaves GPU-PV alone"));
    assert!(host.release().unwrap_err().0.contains("leaves GPU-PV alone"));
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    assert!(host.start(&plan, &machine).unwrap_err().0.contains("leaves GPU-PV alone"));
}

#[test]
fn a_qemu_that_exits_at_once_says_why() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.qemu = program(&scratch, "not-qemu", "#!/bin/sh\necho 'could not open disk' >&2\nexit 1\n");
    let host = LocalHost { context: &context };
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let error = host.start(&plan, &machine).unwrap_err();
    assert_eq!(error.0, "The VM did not start. could not open disk");
}

#[test]
fn a_daemon_that_stops_says_why_and_no_qemu_is_started() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.binaries = scratch.path("bin");
    let marker = scratch.path("qemu-ran");
    context.paths.qemu = program(&scratch, "not-qemu", &format!("#!/bin/sh\ntouch {}\n", marker.display()));
    program(&scratch, "bin/guesthouse-daemon", "#!/bin/sh\necho 'no card here'\nexit 1\n");
    let host = LocalHost { context: &context };
    let machine = Machine::new("vm", "VM", "/d", "/v");
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let error = host.start(&plan, &machine).unwrap_err();
    assert!(error.0.contains("the daemon stopped: no card here"), "{error}");
    assert!(!marker.exists());
}

#[test]
fn release_frees_memory_only_when_no_other_vm_has_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let memory = context.paths.shared_file("ram", slots::first());
    fs::write(&memory, "").unwrap();
    crate::processes::stand_in::fake_process(
        &context,
        300,
        &["/other/qemu"],
        &[],
        &memory.display().to_string(),
    );
    let host = LocalHost { context: &context };
    host.release().unwrap();
    assert!(memory.exists());
    fs::remove_dir_all(context.processes.join("300")).unwrap();
    host.release().unwrap();
    assert!(!memory.exists());
}

#[test]
fn last_words_are_the_end_of_the_output() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    assert_eq!(last_words(&context), "");
    let folder = context.run_folder(None);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(OUTPUT_FILE), format!("{}end\n", "é".repeat(400))).unwrap();
    let words = last_words(&context);
    assert_eq!(words.chars().count(), 300);
    assert!(words.ends_with("end"));
}

#[test]
fn qemu_with_no_display_says_what_to_install() {
    assert!(
        super::local::start_failure("qemu-system-x86_64: Display 'dbus' is not available.")
            .contains("qemu-ui-dbus")
    );
    assert_eq!(super::local::start_failure("it broke"), "The VM did not start. it broke");
}

#[test]
fn no_qemu_at_all_says_so() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.qemu = scratch.path("not-installed");
    let host = LocalHost { context: &context };
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    assert!(host.start(&plan, &machine).unwrap_err().0.contains("isn't installed"));
}
