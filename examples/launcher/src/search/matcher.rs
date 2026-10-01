/// How well a query matched a text. Higher is better.
pub type Score = u32;

const MATCH: Score = 1;
const CONSECUTIVE: Score = 4;
const WORD_START: Score = 8;
const TEXT_START: Score = 16;

/// Scores `text` against `query`, or returns `None` when the query's
/// characters do not all appear in the text, in order.
///
/// An empty query matches everything with the lowest score, so an unfiltered
/// list keeps its original order.
pub fn score(query: &str, text: &str) -> Option<Score> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }

    let mut total = 0;
    let mut query_ix = 0;
    let mut previous: Option<char> = None;
    let mut previous_matched = false;

    for (text_ix, character) in text.chars().enumerate() {
        if query_ix == query.len() {
            break;
        }
        let matched = character
            .to_lowercase()
            .eq(std::iter::once(query[query_ix]));
        if matched {
            total += MATCH;
            if text_ix == 0 {
                total += TEXT_START;
            } else if is_word_start(previous, character) {
                total += WORD_START;
            }
            if previous_matched {
                total += CONSECUTIVE;
            }
            query_ix += 1;
        }
        previous_matched = matched;
        previous = Some(character);
    }

    (query_ix == query.len()).then_some(total)
}

/// A word starts after a separator, or at a lower-to-upper case change, which
/// is how `vsc` finds the initials of "Visual Studio Code" and `gh` those of
/// "GitHub".
fn is_word_start(previous: Option<char>, character: char) -> bool {
    match previous {
        None => true,
        Some(previous) => {
            !previous.is_alphanumeric() || (previous.is_lowercase() && character.is_uppercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requires_every_query_character_in_order() {
        assert!(score("vsc", "Visual Studio Code").is_some());
        assert!(score("csv", "Visual Studio Code").is_none());
        assert!(score("xyz", "Visual Studio Code").is_none());
    }

    #[test]
    fn test_is_case_insensitive_and_ignores_query_spaces() {
        assert_eq!(score("CODE", "code"), score("code", "Code"));
        assert!(score("studio code", "Visual Studio Code").is_some());
    }

    #[test]
    fn test_empty_query_matches_with_lowest_score() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
    }

    #[test]
    fn test_prefers_initials_and_prefixes_over_scattered_matches() {
        // Initials beat the same letters scattered through one word.
        assert!(score("vsc", "Visual Studio Code") > score("vsc", "vasculature"));
        // A prefix beats the same word further in.
        assert!(score("term", "Terminal") > score("term", "Hyper Terminal"));
        // Consecutive characters beat a spread-out match.
        assert!(score("stu", "Studio") > score("stu", "Start Tune"));
        // Case changes count as word starts.
        assert!(score("gh", "GitHub") > score("gh", "Laugh"));
    }
}
