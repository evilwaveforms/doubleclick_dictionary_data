use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

pub fn lookup_key(language: &str, word: &str) -> Option<String> {
    let normalized = word
        .nfc()
        .flat_map(char::to_lowercase)
        .map(|character| if character == '’' { '\'' } else { character })
        .collect::<String>();

    if !valid_word(&normalized) {
        return None;
    }

    Some(format!("{language}:{normalized}"))
}

fn valid_word(word: &str) -> bool {
    let mut characters = word.chars().peekable();
    let mut has_letter = false;
    let mut previous_was_separator = true;
    let mut count = 0;

    while let Some(character) = characters.next() {
        count += 1;
        if count > 64 {
            return false;
        }

        if character.is_alphabetic() {
            has_letter = true;
            previous_was_separator = false;
            continue;
        }
        if is_combining_mark(character) && has_letter && !previous_was_separator {
            continue;
        }
        if matches!(character, '\'' | '-')
            && !previous_was_separator
            && characters.peek().is_some_and(|next| next.is_alphabetic())
        {
            previous_was_separator = true;
            continue;
        }
        return false;
    }

    has_letter && !previous_was_separator
}

pub fn shard_index(key: &str, shard_count: usize) -> usize {
    let mut hash = 2_166_136_261_u32;
    for byte in key.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as usize & (shard_count - 1)
}

#[cfg(test)]
mod tests {
    use super::{lookup_key, shard_index};

    #[test]
    fn normalizes_case_apostrophes_and_unicode() {
        assert_eq!(lookup_key("en", "DON’T"), Some("en:don't".to_owned()));
        assert_eq!(lookup_key("en", "Cafe\u{301}"), Some("en:café".to_owned()));
        assert_eq!(lookup_key("en", "two words"), None);
    }

    #[test]
    fn hashing_is_stable() {
        assert_eq!(shard_index("en:hello", 8192), 616);
    }
}
