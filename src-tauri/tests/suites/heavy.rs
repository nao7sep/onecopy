// Real managed tools and models over the shared test-fixture corpus. Every
// such test is #[ignore]d, so the ordinary run compiles them without running
// them; npm run test:full runs them with --ignored.
#[path = "../heavy_support.rs"]
mod heavy_support;
#[path = "../binaries_manager_heavy_tests.rs"]
mod binaries_manager_heavy_tests;
#[path = "../face_heavy_tests.rs"]
mod face_heavy_tests;
#[path = "../live_photo_heavy_tests.rs"]
mod live_photo_heavy_tests;
#[path = "../preview_heavy_tests.rs"]
mod preview_heavy_tests;
#[path = "../transcription_heavy_tests.rs"]
mod transcription_heavy_tests;
#[path = "../video_heavy_tests.rs"]
mod video_heavy_tests;
