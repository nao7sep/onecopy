// Only tests participating in serial(scan_cancel) belong in this process.
// Ordinary library tests read the app's global cancellation state without a
// test lock, so serializing just its writers in that process is insufficient.
#[path = "../core_integration_tests.rs"]
mod core_integration_tests;
#[path = "../scan_cancellation_tests.rs"]
mod scan_cancellation_tests;

struct ResetScanCancellation;

impl Drop for ResetScanCancellation {
    fn drop(&mut self) {
        onecopy_lib::scanner::SCAN_CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
