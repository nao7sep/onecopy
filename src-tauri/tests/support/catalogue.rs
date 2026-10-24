// Test-only access to the source catalogue, without the runtime English fallback.
pub fn entries(language: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../src/i18n/locales").join(format!("{language}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
