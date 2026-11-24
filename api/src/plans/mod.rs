//! The plans' catalogue: what `config/plans.toml` holds, how `admin plans
//! sync` writes it into the database, and the answer of `GET /v1/plans` and
//! `GET /internal/plans` built from the stored rows.
//!
//! The catalogue's format is the extension's (its plan model and the rules by
//! which it reads a catalogue and skips a plan); the server holds every text
//! and price to the same rules, so that it never sends what the extension
//! would skip.

pub mod catalogue;
pub mod currency;
pub mod file;
pub mod sync;

/// Format version of the catalogue (`v`).
pub const FORMAT: u8 = 1;
/// At most this many plans in `GET /v1/plans`; more make the whole answer
/// unusable to the extension.
pub const MAX_PLANS: usize = 6;
/// Lines of what a plan includes.
pub const MAX_INCLUDES: usize = 8;
/// The largest amount the extension accepts, in minor units.
pub const MAX_AMOUNT: i64 = 100_000_000;
/// The largest answer the extension reads.
pub const MAX_ANSWER_BYTES: usize = 32 * 1024;
/// The longest address the extension accepts, in code points.
pub const MAX_URL: usize = 300;

/// Lengths of the card's texts in Unicode code points, after trimming.
pub mod limits {
    pub const NAME: usize = 32;
    pub const SUMMARY: usize = 140;
    pub const INCLUDE: usize = 100;
    pub const PRICE_NOTE: usize = 60;
    pub const BADGE: usize = 24;
}

/// The languages a plan's texts come in; English is required and the only
/// one `GET /v1/plans` sends.
pub const LOCALES: [&str; 2] = ["en", "ru"];

/// A plan id: `[a-z0-9-]{1,32}`.
pub fn is_plan_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A catalogue version: `[A-Za-z0-9._-]{1,40}`.
pub fn is_catalog_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A provider's name: `[a-z0-9-]{2,32}`.
pub fn is_provider(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Control characters and the characters that steer the direction of text,
/// which the extension refuses: they can turn a price or an address round.
pub fn forbidden(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// The ends a text loses: JavaScript's `String.prototype.trim` removes white
/// space and U+FEFF.
fn trimmed(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// A text of a plan's card, held to the extension's rule: no forbidden
/// character, trimmed, at most `max` code points; empty after trimming is
/// absent (`None`), which only an optional text may be.
pub fn card_text(
    field: &str,
    value: Option<&str>,
    max: usize,
    optional: bool,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return if optional {
            Ok(None)
        } else {
            Err(format!("{field} is required"))
        };
    };
    if value.chars().any(forbidden) {
        return Err(format!(
            "{field} carries a control or text-direction character"
        ));
    }
    let text = trimmed(value);
    if text.is_empty() {
        return if optional {
            Ok(None)
        } else {
            Err(format!("{field} is empty"))
        };
    }
    let length = text.chars().count();
    if length > max {
        return Err(format!("{field} has {length} characters, more than {max}"));
    }
    Ok(Some(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_the_extension_s_rules() {
        assert!(is_plan_id("pro-monthly"));
        assert!(!is_plan_id("Pro"));
        assert!(!is_plan_id(&"a".repeat(33)));
        assert!(is_catalog_id("2026-11-24.1"));
        assert!(!is_catalog_id("2026 11 24"));
        assert!(is_provider("fake"));
        assert!(!is_provider("f"));
    }

    #[test]
    fn texts_count_code_points_after_trimming() {
        assert_eq!(
            card_text("name", Some("  Monthly \u{feff}"), 32, false),
            Ok(Some("Monthly".to_owned()))
        );
        // Thirty-two letters outside the basic plane are thirty-two characters.
        let wide = "\u{1f600}".repeat(32);
        assert_eq!(card_text("name", Some(&wide), 32, false), Ok(Some(wide)));
        assert!(card_text("name", Some(&"\u{1f600}".repeat(33)), 32, false).is_err());
        assert_eq!(card_text("badge", Some("   "), 24, true), Ok(None));
        assert!(card_text("name", Some("   "), 32, false).is_err());
        assert!(card_text("name", None, 32, false).is_err());
        assert_eq!(card_text("badge", None, 24, true), Ok(None));
    }

    #[test]
    fn control_and_direction_characters_are_refused() {
        for text in [
            "Pro\u{0}",
            "Pro\u{7}",
            "Pro\u{85}",
            "\u{202e}orP",
            "Pro\u{2066}",
            "a\nb",
        ] {
            assert!(
                card_text("name", Some(text), 32, false).is_err(),
                "{text:?}"
            );
        }
    }
}
