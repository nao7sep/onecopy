pub fn phrase_loop(text: &str) -> bool {
    let segments = text
        .lines()
        .map(|line| {
            let content = line
                .split_once(']')
                .map(|(_, remainder)| remainder)
                .unwrap_or(line);
            content
                .split_whitespace()
                .map(|token| token.to_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|tokens| !tokens.is_empty())
        .collect::<Vec<_>>();
    if segments.windows(2).any(|pair| pair.first() == pair.get(1)) {
        return true;
    }
    let tokens = segments.into_iter().flatten().collect::<Vec<_>>();
    for width in 3..=16.min(tokens.len() / 3) {
        for start in 0..=tokens.len() - width * 3 {
            if tokens[start..start + width] == tokens[start + width..start + width * 2]
                && tokens[start..start + width] == tokens[start + width * 2..start + width * 3]
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::phrase_loop;

    #[test]
    fn probe_distinguishes_decoder_loops_from_ordinary_repetition() {
        assert!(phrase_loop(
            "[0:00] please remove the coordinates\n[0:02] please remove the coordinates\n"
        ));
        assert!(phrase_loop(
            "[0:00] please remove the coordinates please remove the coordinates please remove the coordinates\n"
        ));
        assert!(!phrase_loop(
            "[0:00] thank you, thank you for removing the location before sharing\n"
        ));
        assert!(!phrase_loop(
            "[0:00] please remove the coordinates\n[0:02] then share the photograph\n"
        ));
    }
}
