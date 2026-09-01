//! Reading command keys typed with a Korean IME active.
//!
//! With the IME on, the terminal delivers the jamo the key produces, not the
//! key: `j` arrives as `ㅓ`, `q` as `ㅂ`. Every command in this app is a Latin
//! letter, so a forgotten IME makes the whole keyboard silently inert — no
//! error, no movement, nothing to suggest what is wrong.
//!
//! Mapping the jamo back to the key that produced it costs one lookup and
//! removes that failure entirely. The layout is 두벌식 (dubeolsik), which is
//! what essentially every Korean keyboard uses.
//!
//! This is for *commands only*. Text the user is typing — a filter, a payload,
//! a profile name — must stay exactly as entered, or Korean becomes
//! untypeable. The caller decides which context it is in.

/// The Latin key that produced `c` under dubeolsik, if `c` is a jamo.
///
/// `shift` decides case, because the layout cannot: only ㅂㅈㄷㄱㅅ and ㅐㅔ
/// change when shifted, so `c` and `C` both arrive as `ㅊ`. This app binds `q`
/// to quit and `Q` to query, so guessing here would be worse than not mapping
/// at all — the modifier is the only thing that can separate them.
///
/// The seven jamo that *are* shift-only (`ㅃㅉㄸㄲㅆㅒㅖ`) report uppercase
/// whatever `shift` says: they cannot be produced without it, and not every
/// terminal reports a modifier next to a character that already implies one.
pub fn latin_key(c: char, shift: bool) -> Option<char> {
    if let Some(upper) = shift_only(c) {
        return Some(upper);
    }
    let base = base_key(c)?;
    Some(if shift {
        base.to_ascii_uppercase()
    } else {
        base
    })
}

/// The seven jamo that only exist as Shift + key.
fn shift_only(c: char) -> Option<char> {
    Some(match c {
        'ㅃ' => 'Q',
        'ㅉ' => 'W',
        'ㄸ' => 'E',
        'ㄲ' => 'R',
        'ㅆ' => 'T',
        'ㅒ' => 'O',
        'ㅖ' => 'P',
        _ => return None,
    })
}

fn base_key(c: char) -> Option<char> {
    Some(match c {
        // Consonants, top row.
        'ㅂ' => 'q',
        'ㅈ' => 'w',
        'ㄷ' => 'e',
        'ㄱ' => 'r',
        'ㅅ' => 't',
        // Vowels, top row.
        'ㅛ' => 'y',
        'ㅕ' => 'u',
        'ㅑ' => 'i',
        'ㅐ' => 'o',
        'ㅔ' => 'p',
        // Consonants, home row.
        'ㅁ' => 'a',
        'ㄴ' => 's',
        'ㅇ' => 'd',
        'ㄹ' => 'f',
        'ㅎ' => 'g',
        // Vowels, home row.
        'ㅗ' => 'h',
        'ㅓ' => 'j',
        'ㅏ' => 'k',
        'ㅣ' => 'l',
        // Consonants, bottom row.
        'ㅋ' => 'z',
        'ㅌ' => 'x',
        'ㅊ' => 'c',
        'ㅍ' => 'v',
        // Vowels, bottom row.
        'ㅠ' => 'b',
        'ㅜ' => 'n',
        'ㅡ' => 'm',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::latin_key;

    /// The Hangul Compatibility Jamo block, which is what an IME emits for a
    /// single keypress.
    const JAMO: std::ops::RangeInclusive<char> = 'ㄱ'..='ㅣ';

    #[test]
    fn the_navigation_keys_survive_a_forgotten_ime() {
        // The ones that make the app feel dead when they stop working.
        assert_eq!(latin_key('ㅓ', false), Some('j'));
        assert_eq!(latin_key('ㅏ', false), Some('k'));
        assert_eq!(latin_key('ㅗ', false), Some('h'));
        assert_eq!(latin_key('ㅣ', false), Some('l'));
        assert_eq!(latin_key('ㅂ', false), Some('q'));
    }

    #[test]
    fn case_comes_from_the_modifier_because_the_layout_cannot_supply_it() {
        // Shift+c is still ㅊ in dubeolsik — only ㅂㅈㄷㄱㅅ and ㅐㅔ change.
        assert_eq!(latin_key('ㅊ', false), Some('c'));
        assert_eq!(latin_key('ㅊ', true), Some('C'));
        assert_eq!(latin_key('ㅇ', true), Some('D'));
        assert_eq!(latin_key('ㅣ', true), Some('L'));
    }

    #[test]
    fn shift_only_jamo_report_uppercase_even_without_the_modifier() {
        // `ㅃ` cannot be typed without Shift, and not every terminal reports
        // the modifier beside a character that already implies it. `q` quits
        // and `Q` queries, so this is not a distinction to leave to chance.
        assert_eq!(latin_key('ㅃ', false), Some('Q'));
        assert_eq!(latin_key('ㅃ', true), Some('Q'));
        assert_eq!(latin_key('ㅂ', false), Some('q'));
    }

    #[test]
    fn every_command_letter_this_app_binds_is_reachable() {
        // If no (jamo, shift) pair produces a binding, a Korean-IME user
        // simply cannot press it.
        for want in "qdhjklzsfnmpyECDLPQY".chars() {
            let reachable = JAMO
                .clone()
                .any(|c| latin_key(c, false) == Some(want) || latin_key(c, true) == Some(want));
            assert!(reachable, "no jamo reaches `{want}`");
        }
    }

    #[test]
    fn no_two_jamo_claim_the_same_unshifted_key() {
        // A duplicate would mean one of them shadows a command silently.
        let mut seen = std::collections::HashMap::new();
        for c in JAMO.clone() {
            if let Some(k) = latin_key(c, false) {
                if let Some(prev) = seen.insert(k, c) {
                    panic!("`{k}` is claimed by both {prev} and {c}");
                }
            }
        }
    }

    #[test]
    fn non_jamo_is_left_alone() {
        // Latin, digits, punctuation and composed syllables all pass through:
        // the caller reads `None` as "this was not an IME artefact".
        for c in ['j', 'Q', '1', '/', ':', ' ', '한', '글'] {
            assert_eq!(latin_key(c, false), None, "{c} should not map");
            assert_eq!(latin_key(c, true), None, "{c} should not map");
        }
    }
}
