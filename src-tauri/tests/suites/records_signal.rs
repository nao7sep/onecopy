// The stored-record listener is process-wide, so its test cannot share a
// process with tests that write records.
#[path = "../records_signal_tests.rs"]
mod records_signal_tests;
