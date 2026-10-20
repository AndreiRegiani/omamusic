pub struct FuzzySearch {
    tokens: Vec<Vec<u8>>,
    pattern_len: usize,
}

impl FuzzySearch {
    pub fn new(query: &str) -> Self {
        let tokens: Vec<Vec<u8>> = query
            .split_whitespace()
            .map(|word| Self::fold(word).into_bytes())
            .filter(|token| !token.is_empty())
            .collect();
        let pattern_len = tokens.iter().map(Vec::len).sum();
        Self {
            tokens,
            pattern_len,
        }
    }

    fn fold(text: &str) -> String {
        text.chars()
            .flat_map(|c| c.to_lowercase())
            .filter(|c| !c.is_control())
            .collect()
    }

    pub fn score(&self, text: &str) -> i32 {
        let Some(first) = self.tokens.first() else {
            return 100;
        };

        let text_folded = Self::fold(text);
        let text_bytes = text_folded.as_bytes();

        let mut best_total = 0;

        for start in 0..text_bytes.len() {
            if text_bytes[start] != first[0] {
                continue;
            }

            let total: Option<i32> = self
                .tokens
                .iter()
                .map(|token| Self::score_token(token, text_bytes, start))
                .map(|score| (score > 0).then_some(score))
                .sum();

            if let Some(total) = total {
                best_total = best_total.max(total);
            }
        }

        if self.pattern_len > 1 && best_total < self.pattern_len as i32 + 4 {
            0
        } else {
            best_total
        }
    }

    fn score_token(pattern: &[u8], text: &[u8], start: usize) -> i32 {
        let mut best = 0;

        for i in start..text.len() {
            if text[i] != pattern[0] {
                continue;
            }

            let mut matched = 0;
            let mut score = 0;
            let mut last = i.saturating_sub(2);

            for j in i..text.len() {
                if text[j] == pattern[matched] {
                    score += 1;
                    if j == last + 1 {
                        score += 4;
                    }
                    if j == 0 || !text[j - 1].is_ascii_alphanumeric() {
                        score += 6;
                    }
                    last = j;
                    matched += 1;
                    if matched == pattern.len() {
                        break;
                    }
                }
            }

            if matched == pattern.len() {
                best = best.max(score);
            }
        }

        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(FuzzySearch::new("").score("anything"), 100);
        assert_eq!(FuzzySearch::new("   ").score(""), 100);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let search = FuzzySearch::new("BEATLES");
        assert!(search.score("The Beatles") > 0);
    }

    #[test]
    fn unrelated_text_does_not_match() {
        assert_eq!(FuzzySearch::new("zeppelin").score("The Beatles"), 0);
    }

    #[test]
    fn contiguous_word_start_scores_higher_than_scattered() {
        let search = FuzzySearch::new("abba");
        assert!(search.score("ABBA Gold") > search.score("xaxbxbxa"));
    }

    #[test]
    fn every_token_must_match() {
        let search = FuzzySearch::new("dark moon");
        assert!(search.score("Dark Side of the Moon") > 0);
        assert_eq!(search.score("Dark Side of the Sun"), 0);
    }
}
