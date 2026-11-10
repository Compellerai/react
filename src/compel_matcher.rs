//! Semantic compel matching using keyword analysis
//!
//! Finds the best matching compel when exact/contains matching fails.
//! Uses keyword overlap scoring - lightweight and works fully offline.

/// Minimum keyword overlap score to consider a match
const MIN_SCORE_THRESHOLD: usize = 1;

/// Find the best matching compel for a track using keyword overlap
///
/// Args:
/// - track: Track title
/// - artist: Artist name (optional)
/// - candidates: List of (path, display_name) for candidate compels
///
/// Returns the best matching compel path, or None if no good match
pub fn find_best_match(
    track: &str,
    artist: Option<&str>,
    candidates: &[(String, String)],
) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }

    // Build query from track + artist
    let query = if let Some(art) = artist {
        format!("{} {}", track, art)
    } else {
        track.to_string()
    };

    // Extract keywords (lowercase, alphanumeric only, skip short words)
    let query_words: Vec<String> = extract_keywords(&query);

    if query_words.is_empty() {
        return None;
    }

    let mut best_match: Option<(String, usize)> = None;

    for (path, name) in candidates {
        let name_words: Vec<String> = extract_keywords(name);
        let mut score = 0usize;

        // Count keyword matches (both directions for partial word matches)
        for qword in &query_words {
            for nword in &name_words {
                // Full word match = 3 points
                if qword == nword {
                    score += 3;
                }
                // Partial match (one contains other) = 1 point
                else if qword.contains(nword) || nword.contains(qword) {
                    score += 1;
                }
            }
        }

        if score >= MIN_SCORE_THRESHOLD {
            if best_match.is_none() || score > best_match.as_ref().unwrap().1 {
                best_match = Some((path.clone(), score));
            }
        }
    }

    if let Some((ref path, score)) = best_match {
        let file_name = path.split('/').last().unwrap_or(path);
        log::info!(
            "[CompelMatcher] Best match for '{}': {} (score: {})",
            query, file_name, score
        );
    }

    best_match.map(|(path, _)| path)
}

/// Extract keywords from text (lowercase, alphanumeric, skip short/common words)
fn extract_keywords(text: &str) -> Vec<String> {
    let stopwords = [
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
        "of", "with", "by", "from", "is", "it", "as", "be", "are", "was",
        "were", "been", "being", "have", "has", "had", "do", "does", "did",
        "will", "would", "could", "should", "may", "might", "must", "shall",
        "can", "need", "dare", "ought", "used", "feat", "ft", "vs", "remix",
        "mix", "edit", "version", "original", "extended", "radio", "club",
    ];

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() > 2)
        .filter(|s| !stopwords.contains(s))
        .map(|s| s.to_string())
        .collect()
}

/// Find best match from artist-filtered candidates first, then all candidates
///
/// Priority:
/// 1. Artist match + keyword match from those
/// 2. No artist filter + keyword match from all
pub fn find_best_match_with_artist_priority(
    track: &str,
    artist: Option<&str>,
    all_candidates: &[(String, String)],
    artist_filtered: &[(String, String)],
) -> Option<String> {
    // First try: keyword match among artist-filtered candidates
    if !artist_filtered.is_empty() {
        if let Some(path) = find_best_match(track, artist, artist_filtered) {
            return Some(path);
        }
    }

    // Fallback: keyword match among all candidates
    find_best_match(track, artist, all_candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_keywords() {
        let words = extract_keywords("Midnight Dreams (Extended Mix)");
        assert!(words.contains(&"midnight".to_string()));
        assert!(words.contains(&"dreams".to_string()));
        assert!(!words.contains(&"extended".to_string())); // stopword
        assert!(!words.contains(&"mix".to_string())); // stopword
    }

    #[test]
    fn test_find_best_match() {
        let candidates = vec![
            ("/videos/summer_vibes.mp4".to_string(), "Summer Vibes".to_string()),
            ("/videos/midnight_dreams.mp4".to_string(), "Midnight Dreams".to_string()),
            ("/videos/beach_party.mp4".to_string(), "Beach Party".to_string()),
        ];

        // Exact keyword match
        let result = find_best_match("Midnight", None, &candidates);
        assert_eq!(result, Some("/videos/midnight_dreams.mp4".to_string()));

        // Partial match
        let result = find_best_match("Summer Day", None, &candidates);
        assert_eq!(result, Some("/videos/summer_vibes.mp4".to_string()));
    }
}
