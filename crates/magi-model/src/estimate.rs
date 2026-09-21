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
