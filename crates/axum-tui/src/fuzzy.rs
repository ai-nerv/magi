//! Fuzzy matching, ported from Pi's `fuzzy.ts`.
//!
//! A query matches when its characters appear in order, not necessarily adjacent. Scores are
//! inverted — lower is better — and the weights are Pi's, so ranking behaves the same.

/// How well a query matched, if it matched at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// Lower is better.
    pub score: f64,
}

/// Score `query` against `text`, or `None` if the characters do not appear in order.
#[must_use]
pub fn score(query: &str, text: &str) -> Option<Match> {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    if let Some(m) = score_exact(&query_lower, &text_lower) {
        return Some(m);
    }

    // A transposed alphanumeric query is a typo worth forgiving: "2fa" for "fa2" and back.
    // Pi does the same, and it is the difference between finding a file and retyping it.
    score_exact(&swap_alnum(&query_lower)?, &text_lower)
}

fn score_exact(query: &str, text: &str) -> Option<Match> {
    if query.is_empty() {
        return Some(Match { score: 0.0 });
    }

    let text_chars: Vec<char> = text.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    if query_chars.len() > text_chars.len() {
        return None;
    }

    let mut query_index = 0;
    let mut score = 0.0_f64;
    let mut last_match: Option<usize> = None;
    let mut consecutive = 0_i32;

    for (i, &c) in text_chars.iter().enumerate() {
        if query_index >= query_chars.len() {
            break;
        }
        if c != query_chars[query_index] {
            continue;
        }

        let at_boundary = i == 0
            || matches!(
                text_chars[i - 1],
                ' ' | '\t' | '\n' | '-' | '_' | '.' | '/' | ':'
            );

        if last_match == Some(i.wrapping_sub(1)) && i > 0 {
            consecutive += 1;
            score -= f64::from(consecutive) * 5.0;
        } else {
            consecutive = 0;
            if let Some(last) = last_match {
                score += (i - last - 1) as f64 * 2.0;
            }
        }

        if at_boundary {
            score -= 10.0;
        }
        score += i as f64 * 0.1;

        last_match = Some(i);
        query_index += 1;
    }

    if query_index < query_chars.len() {
        return None;
    }
    if query == text {
        score -= 100.0;
    }
    Some(Match { score })
}

/// Swap a leading letter run with a trailing digit run, or the reverse.
fn swap_alnum(query: &str) -> Option<String> {
    let letters: String = query
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    if !letters.is_empty() {
        let rest = &query[letters.len()..];
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("{rest}{letters}"));
        }
    }
    let digits: String = query.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let rest = &query[digits.len()..];
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some(format!("{rest}{digits}"));
        }
    }
    None
}

/// Rank candidates by score, best first, dropping non-matches.
#[must_use]
pub fn filter<'a>(query: &str, candidates: &'a [String]) -> Vec<&'a String> {
    let mut scored: Vec<(f64, &String)> = candidates
        .iter()
        .filter_map(|c| score(query, c).map(|m| (m.score, c)))
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_matches_everything() {
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn characters_must_appear_in_order() {
        assert!(score("abc", "a_b_c").is_some());
        assert!(score("cba", "a_b_c").is_none());
    }

    #[test]
    fn a_query_longer_than_the_text_cannot_match() {
        assert!(score("abcdef", "abc").is_none());
    }

    #[test]
    fn an_exact_match_outscores_a_prefix_match() {
        let exact = score("main", "main").expect("exact");
        let prefix = score("main", "maintain").expect("prefix");
        assert!(exact.score < prefix.score);
    }

    #[test]
    fn consecutive_characters_outscore_scattered_ones() {
        let tight = score("abc", "abcxyz").expect("tight");
        let loose = score("abc", "axbxcx").expect("loose");
        assert!(tight.score < loose.score);
    }

    #[test]
    fn a_word_boundary_match_is_rewarded() {
        let boundary = score("s", "src/main.rs").expect("boundary");
        let interior = score("c", "src/main.rs").expect("interior");
        assert!(boundary.score < interior.score);
    }

    #[test]
    fn a_transposed_alphanumeric_query_still_matches() {
        assert!(score("2fa", "fa2").is_some());
        assert!(score("fa2", "2fa").is_some());
    }

    #[test]
    fn filtering_ranks_best_first_and_drops_misses() {
        let candidates: Vec<String> = ["main.rs", "maintain.txt", "zzz"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let ranked = filter("main", &candidates);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0], "main.rs");
    }
}
