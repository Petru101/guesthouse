//! Where everything is in the Linux guest's sources. This is the only file that knows, so a new layout
//! is a change here.

/// The sources' own folder, inside the program folder.
pub const SOURCES_FOLDER: &str = "linux";
/// A file that tells the repo's folder from any other folder of that name.
pub const SOURCES_MARK: &str = "allow.txt";
/// What the guest's first boot runs.
pub const GUEST_SETUP: &str = "setup/guest";
/// What the unattended install reads.
pub const SEED_FOLDER: &str = "setup/seed";
pub const USER_DATA_TEMPLATE: &str = "setup/seed/user-data.template";
pub const META_DATA: &str = "setup/seed/meta-data";
/// Needed in the guest before any driver image has been seen.
pub const FIRST_BOOT_FILES: [&str; 2] = ["cast/Xwayland", "cast/firefox.desktop"];
/// The calls the device lets through.
pub const ALLOW_LIST: &str = "allow.txt";
/// What a guest builds.
pub const BUILT_SOURCES: [&str; 5] = ["guest", "kapi", "layer", "cast", "tests"];
