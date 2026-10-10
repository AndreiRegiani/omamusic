pub struct FuzzySearch {
    pattern_bytes: Vec<u8>,
    tokens: Vec<Range>,
}

#[derive(Clone, Copy)]
struct Range {
    start: usize,
    end: usize,
}

impl FuzzySearch {
    pub fn new(query: &str) -> Self {
        let mut pattern_bytes = Vec::new();
        let mut tokens = Vec::new();

        for word in query.split_whitespace() {
            let folded = Self::fold(word);
            let start = pattern_bytes.len();
            pattern_bytes.extend(folded.as_bytes());
            tokens.push(Range {
                start,
                end: pattern_bytes.len(),
            });
        }

        Self {
            pattern_bytes,
            tokens,
        }
    }

    fn fold(text: &str) -> String {
        text.chars()
            .flat_map(|c| c.to_lowercase())
            .filter(|c| !c.is_control())
            .collect()
    }

    pub fn score(&self, text: &str) -> i32 {
        if self.tokens.is_empty() {
            return 100;
        }

        let text_folded = Self::fold(text);
        let text_bytes = text_folded.as_bytes();

        let mut best_total = 0;

        for start in 0..text_bytes.len() {
            if text_bytes[start] != self.pattern_bytes[0] {
                continue;
            }

            let mut total_score = 0;
            let mut all_matched = true;

            for token in &self.tokens {
                let token_bytes = &self.pattern_bytes[token.start..token.end];
                let score = Self::score_token(token_bytes, text_bytes, start);

                if score == 0 {
                    all_matched = false;
                    break;
                }
                total_score += score;
            }

            if all_matched {
                best_total = best_total.max(total_score);
            }
        }

        if self.pattern_bytes.len() > 1 && best_total < self.pattern_bytes.len() as i32 + 4 {
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
