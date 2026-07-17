//! Layout-independent shortcut matching for the TUI.
//!
//! Terminals deliver characters after keyboard-layout translation, so a
//! Ukrainian/Russian ЙЦУКЕН user pressing the physical `Q` key gets `й`, not
//! `q`. Shortcuts are defined on the US/QWERTY physical positions — map the
//! typed glyph back to that Latin key before matching.
//!
//! Text entry (search, settings paths) must keep the raw character.

use crossterm::event::KeyCode;

/// Normalize a key code for shortcut matching (not for typed text).
pub fn shortcut_code(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(character) => KeyCode::Char(shortcut_char(character)),
        other => other,
    }
}

/// Map a character produced by common layouts to the US/QWERTY key that sits
/// on the same physical keycap.
pub fn shortcut_char(character: char) -> char {
    let lower = character
        .to_lowercase()
        .next()
        .unwrap_or(character);

    match lower {
        // Ukrainian + Russian ЙЦУКЕН (same base row as QWERTY).
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'х' => '[',
        'ї' | 'ъ' => ']',
        'ф' => 'a',
        // Ukrainian І and Russian Ы share the S key.
        'і' | 'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'ж' => ';',
        'є' | 'э' => '\'',
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        'б' => ',',
        'ю' => '.',
        // Physical `/` on UK/RU yields `.`; physical `.` yields `ю` (handled
        // above). So bare `.` is the slash key and becomes the `/` shortcut.
        '.' => '/',
        // Physical Shift+/ on UK/RU often yields `,`. Comma is not a shortcut
        // on Latin layouts either, so map it to `?` (help).
        ',' => '?',
        // Latin stays lowercase so `Q` and `q` share one arm.
        c if c.is_ascii_alphabetic() => c.to_ascii_lowercase(),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ukrainian_physical_keys_map_to_qwerty_shortcuts() {
        assert_eq!(shortcut_char('й'), 'q'); // quit
        assert_eq!(shortcut_char('Й'), 'q');
        assert_eq!(shortcut_char('с'), 'c'); // continue
        assert_eq!(shortcut_char('у'), 'e'); // status editor
        assert_eq!(shortcut_char('щ'), 'o'); // open browser
        assert_eq!(shortcut_char('д'), 'l'); // library
        assert_eq!(shortcut_char('к'), 'r'); // retry
        assert_eq!(shortcut_char('в'), 'd'); // delete
        assert_eq!(shortcut_char('р'), 'h'); // help / left
        assert_eq!(shortcut_char('о'), 'j'); // down
        assert_eq!(shortcut_char('л'), 'k'); // up
        assert_eq!(shortcut_char('.'), '/'); // search (physical /)
        assert_eq!(shortcut_char('ю'), '.'); // physical .
        assert_eq!(shortcut_char(','), '?'); // help (physical Shift+/)
    }

    #[test]
    fn russian_ys_and_hard_sign_map_like_ukrainian_siblings() {
        assert_eq!(shortcut_char('ы'), 's');
        assert_eq!(shortcut_char('ъ'), ']');
        assert_eq!(shortcut_char('э'), '\'');
    }

    #[test]
    fn latin_shortcuts_are_case_insensitive() {
        assert_eq!(shortcut_char('Q'), 'q');
        assert_eq!(shortcut_char('c'), 'c');
        assert_eq!(shortcut_code(KeyCode::Char('H')), KeyCode::Char('h'));
    }

    #[test]
    fn non_char_keys_pass_through() {
        assert_eq!(shortcut_code(KeyCode::Enter), KeyCode::Enter);
        assert_eq!(shortcut_code(KeyCode::Esc), KeyCode::Esc);
        assert_eq!(shortcut_code(KeyCode::Down), KeyCode::Down);
    }
}
