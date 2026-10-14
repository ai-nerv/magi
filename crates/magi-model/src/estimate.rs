//! How many tokens a piece of text will cost, before a provider has said. Counted by kind of
//! character: four characters to a token is right for prose and a third short for code, and one
//! correction factor cannot fit both.

/// Letters to a token within a word, which is never less than one token.
const LETTERS_PER_TOKEN: u64 = 5;
/// Digits to a token.
const DIGITS_PER_TOKEN: u64 = 3;
/// Hundredths of a token for a mark of punctuation, and for a character outside ASCII.
const PUNCTUATION_HUNDREDTHS: u64 = 70;
const WIDE_HUNDREDTHS: u64 = 67;

/// What one inline image is counted as, whatever its encoded size.
pub const IMAGE_TOKENS: u64 = 4096;

/// The estimate for a request's messages, each image counted as [`IMAGE_TOKENS`] rather than as
/// its base64 text.
#[must_use]
pub fn messages(messages: &[crate::Message]) -> u64 {
    let mut messages = messages.to_vec();
    let mut images = 0;
    for message in &mut messages {
        for content in &mut message.content {
            match content {
                crate::Content::Image { data, .. } => {
                    data.clear();
                    images += 1;
                }
                crate::Content::ToolResult {
                    images: attachments,
                    ..
                } => {
                    images += attachments.len() as u64;
                    attachments.clear();
                }
                _ => {}
            }
        }
    }
    serde_json::to_string(&messages)
        .map_or(0, |body| tokens(&body))
        .saturating_add(images * IMAGE_TOKENS)
}

/// The estimate for `text`. Spaces cost nothing of their own, and a line break one token.
#[must_use]
pub fn tokens(text: &str) -> u64 {
    let word = |letters: u64| {
        (letters * 100 / LETTERS_PER_TOKEN)
            .max(100)
            .min(letters * 100)
    };
    let (mut hundredths, mut whole, mut letters, mut digits) = (0u64, 0u64, 0u64, 0u64);
    for c in text.chars() {
        if c.is_ascii_alphabetic() {
            letters += 1;
            continue;
        }
        if c.is_ascii_digit() {
            digits += 1;
            continue;
        }
        hundredths += word(letters);
        whole += digits.div_ceil(DIGITS_PER_TOKEN);
        (letters, digits) = (0, 0);
        match c {
            '\n' => whole += 1,
            c if c.is_ascii_whitespace() => {}
            c if c.is_ascii() => hundredths += PUNCTUATION_HUNDREDTHS,
            _ => hundredths += WIDE_HUNDREDTHS,
        }
    }
    hundredths += word(letters);
    whole + digits.div_ceil(DIGITS_PER_TOKEN) + hundredths.div_ceil(100)
}

#[cfg(test)]
mod tests {
    use super::tokens;

    #[test]
    fn images_are_counted_as_images_not_base64_prose() {
        let mut message = crate::Message::user("inspect");
        message.content.push(crate::Content::Image {
            data: "A".repeat(170_000),
            media_type: "image/png".into(),
        });
        let large = super::messages(&[message.clone()]);
        if let crate::Content::Image { data, .. } = &mut message.content[1] {
            *data = "small".into();
        }
        let small = super::messages(&[message]);
        assert_eq!(large, small);
        assert!(large >= super::IMAGE_TOKENS);
        assert!(large < super::IMAGE_TOKENS + 100);
    }

    #[test]
    fn nothing_costs_nothing() {
        assert_eq!(tokens(""), 0);
        assert_eq!(tokens("   "), 0);
    }

    #[test]
    fn prose_is_about_a_token_to_a_word() {
        // Fourteen short words and a full stop: a tokenizer makes fifteen of them.
        let prose = "The quick brown fox jumps over the lazy dog and then sits down again. ";
        let counted = tokens(&prose.repeat(20));
        assert!((280..=320).contains(&counted), "{counted}");
    }

    #[test]
    fn code_costs_well_over_a_token_to_four_characters() {
        let line = "pub fn item_1_17(stock: u32) -> u32 { stock.saturating_add(1017) }\n";
        let counted = tokens(&line.repeat(100));
        assert!((2300..=2800).contains(&counted), "{counted}");
    }

    #[test]
    fn a_long_number_is_several_tokens() {
        assert_eq!(tokens("123456789"), 3);
        assert_eq!(tokens("12 3456"), 3);
    }
}
