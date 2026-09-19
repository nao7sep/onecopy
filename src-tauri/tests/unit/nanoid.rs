use super::*;

#[test]
fn generate_produces_the_documented_length() {
    assert_eq!(generate().unwrap().len(), LENGTH);
}

#[test]
fn generate_uses_only_the_documented_alphabet() {
    let id = generate().unwrap();
    assert!(id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
}

#[test]
fn generate_yields_distinct_values_across_calls() {
    // Not a proof of uniqueness, just a sanity check that the RNG is
    // actually wired up rather than, say, always returning zero bytes.
    let ids: std::collections::HashSet<String> =
        (0..1000).map(|_| generate().unwrap()).collect();
    assert_eq!(ids.len(), 1000);
}

#[test]
fn generate_never_produces_a_dot_or_slash() {
    // These would be significant if embedded in a filename; the alphabet
    // simply does not contain them, so this should hold trivially.
    for _ in 0..1000 {
        let id = generate().unwrap();
        assert!(!id.contains('.'));
        assert!(!id.contains('/'));
        assert!(!id.contains('\\'));
    }
}
