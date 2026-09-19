// The tools and models the heavy suite runs on are what Managed tools would
// leave installed on this platform: present, verified, and current.

use onecopy_lib::binaries::BinaryStatus;
use onecopy_lib::binaries_manager::{self, DEPENDENCIES};

use super::heavy_support::artifacts;

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run check:full"]
#[serial_test::serial(heavy)]
fn every_managed_tool_and_model_is_installed_verified_and_current() {
    let cache = artifacts();
    if let Err(error) = &cache.ffmpeg_check {
        panic!("ffmpeg's update check could not reach upstream, so the cached build may be stale: {error}");
    }
    for spec in DEPENDENCIES
        .iter()
        .filter(|spec| binaries_manager::spec_of(spec.id).is_some())
    {
        let state = binaries_manager::state_of(&cache.root, spec);
        assert!(
            state.installed_version.is_some(),
            "{} reports the identity of what is installed",
            spec.id
        );
        assert!(state.status == BinaryStatus::UpToDate, "{} is up to date", spec.id);
    }
}
