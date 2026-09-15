//! Text post-processing.

/// Letters that look identical in Latin and Cyrillic: (latin, cyrillic).
const HOMOGLYPHS: &[(char, char)] = &[
    ('A', 'А'), ('B', 'В'), ('C', 'С'), ('E', 'Е'), ('H', 'Н'), ('K', 'К'), ('M', 'М'),
    ('O', 'О'), ('P', 'Р'), ('T', 'Т'), ('X', 'Х'), ('Y', 'У'),
    ('a', 'а'), ('c', 'с'), ('e', 'е'), ('o', 'о'), ('p', 'р'), ('x', 'х'), ('y', 'у'),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Script {
    Latin,
    Cyrillic,
}

fn is_ambiguous(c: char) -> bool {
    HOMOGLYPHS.iter().any(|&(l, k)| c == l || c == k)
}

fn script_of(c: char) -> Option<Script> {
    match c {
        'a'..='z' | 'A'..='Z' | '\u{00C0}'..='\u{024F}' => Some(Script::Latin),
        '\u{0400}'..='\u{04FF}' => Some(Script::Cyrillic),
        _ => None,
    }
}

/// The script of a word judged only by letters that exist in one alphabet.
fn dominant_script(word: &str) -> Option<Script> {
    let (mut latin, mut cyrillic) = (0, 0);
    for c in word.chars().filter(|&c| !is_ambiguous(c)) {
        match script_of(c) {
            Some(Script::Latin) => latin += 1,
            Some(Script::Cyrillic) => cyrillic += 1,
            None => {}
        }
    }
    match (latin, cyrillic) {
        (0, 0) => None,
        (l, k) if l > k => Some(Script::Latin),
        (l, k) if k > l => Some(Script::Cyrillic),
        _ => None,
    }
}

fn convert(word: &str, to: Script) -> String {
    word.chars()
        .map(|c| {
            HOMOGLYPHS
                .iter()
                .find_map(|&(l, k)| match to {
                    Script::Latin if c == k => Some(l),
                    Script::Cyrillic if c == l => Some(k),
                    _ => None,
                })
                .unwrap_or(c)
        })
        .collect()
}

/// Fixes words mixing Latin and Cyrillic look-alike letters ("сhanges" -> "changes").
/// Words consisting only of look-alikes follow the line, if all other words share one script.
pub fn fix_mixed_scripts(line: &str) -> String {
    let words: Vec<&str> = line.split(' ').collect();
    let scripts: Vec<Option<Script>> = words.iter().map(|w| dominant_script(w)).collect();
    let latin = scripts.contains(&Some(Script::Latin));
    let cyrillic = scripts.contains(&Some(Script::Cyrillic));
    let line_script = match (latin, cyrillic) {
        (true, false) => Some(Script::Latin),
        (false, true) => Some(Script::Cyrillic),
        _ => None,
    };

    words
        .iter()
        .zip(scripts)
        .map(|(word, script)| match script.or(line_script) {
            Some(s) => convert(word, s),
            None => word.to_string(),
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_cyrillic_letters_in_english_words() {
        assert_eq!(fix_mixed_scripts("Save сhanges before сlosing?"), "Save changes before closing?");
    }

    #[test]
    fn fixes_latin_letters_in_russian_words() {
        assert_eq!(fix_mixed_scripts("перед зaкрытием"), "перед закрытием");
    }

    #[test]
    fn ambiguous_words_follow_the_line() {
        assert_eq!(fix_mixed_scripts("OK Отмена"), "ОК Отмена");
        assert_eq!(fix_mixed_scripts("ОК Cancel"), "OK Cancel");
    }

    #[test]
    fn keeps_mixed_lines_intact() {
        assert_eq!(fix_mixed_scripts("Цена: 1 299,00 Р email: test@example.com"), "Цена: 1 299,00 Р email: test@example.com");
    }
}
