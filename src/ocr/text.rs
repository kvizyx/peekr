//! Turning recognized boxes into plain text.

use super::TextLine;
use super::geometry::BoundingBox;

/// Minimal vertical overlap of two boxes on the same row, relative to the shorter one.
const MIN_ROW_OVERLAP: f32 = 0.5;

/// Letters that look identical in Latin and Cyrillic: (latin, cyrillic).
#[rustfmt::skip]
const HOMOGLYPHS: &[(char, char)] = &[
    ('A', 'А'), ('B', 'В'), ('C', 'С'), ('E', 'Е'), ('H', 'Н'), ('K', 'К'), ('M', 'М'),
    ('O', 'О'), ('P', 'Р'), ('T', 'Т'), ('X', 'Х'), ('Y', 'У'),
    ('a', 'а'), ('c', 'с'), ('e', 'е'), ('o', 'о'), ('p', 'р'), ('x', 'х'), ('y', 'у'),
];

/// Joins recognized boxes into text in reading order: boxes on the same row are separated
/// by spaces, rows by newlines.
pub fn assemble_text(lines: &[TextLine]) -> String {
    struct Row<'a> {
        bounds: BoundingBox,
        items: Vec<(&'a TextLine, BoundingBox)>,
    }

    let mut boxes: Vec<(&TextLine, BoundingBox)> = lines
        .iter()
        .map(|line| (line, BoundingBox::of_quad(&line.quad)))
        .collect();
    boxes.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));

    let mut rows: Vec<Row> = Vec::new();

    for (line, bounds) in boxes {
        let same_row = rows.last_mut().filter(|row| {
            let shorter = bounds.height().min(row.bounds.height());
            row.bounds.vertical_overlap(&bounds) >= MIN_ROW_OVERLAP * shorter
        });

        match same_row {
            Some(row) => {
                row.bounds.min.y = row.bounds.min.y.min(bounds.min.y);
                row.bounds.max.y = row.bounds.max.y.max(bounds.max.y);
                row.items.push((line, bounds));
            }
            None => rows.push(Row {
                bounds,
                items: vec![(line, bounds)],
            }),
        }
    }

    rows.into_iter()
        .map(|mut row| {
            row.items.sort_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x));
            let words: Vec<&str> = row.items.iter().map(|(line, _)| line.text.as_str()).collect();

            fix_mixed_scripts(&words.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Minimal length of a word that proves which script a reading is in.
const MIN_PROOF_WORD_LEN: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Script {
    Latin,
    Cyrillic,
}

fn is_ambiguous(c: char) -> bool {
    HOMOGLYPHS.iter().any(|&(latin, cyrillic)| c == latin || c == cyrillic)
}

fn script_of(c: char) -> Option<Script> {
    match c {
        'a'..='z' | 'A'..='Z' | '\u{00C0}'..='\u{024F}' => Some(Script::Latin),
        '\u{0400}'..='\u{04FF}' => Some(Script::Cyrillic),
        _ => None,
    }
}

/// Whether the text has a real word in `script`: at least [`MIN_PROOF_WORD_LEN`] letters, all of
/// that script, including one without a look-alike in the other alphabet. Misreadings that mix
/// scripts inside a word ("Еrгог") or consist only of look-alikes ("НОС") do not count.
pub fn contains_word_in(text: &str, script: Script) -> bool {
    text.split(|c: char| !c.is_alphabetic()).any(|word| {
        let letters = word.chars().count();
        let all_in_script = word.chars().all(|c| script_of(c) == Some(script));
        let has_distinct_letter = word.chars().any(|c| !is_ambiguous(c));

        letters >= MIN_PROOF_WORD_LEN && all_in_script && has_distinct_letter
    })
}

/// The script of a word judged only by letters that exist in one alphabet.
fn dominant_script(word: &str) -> Option<Script> {
    let (mut latin, mut cyrillic) = (0, 0);

    for script in word.chars().filter(|&c| !is_ambiguous(c)).filter_map(script_of) {
        match script {
            Script::Latin => latin += 1,
            Script::Cyrillic => cyrillic += 1,
        }
    }

    match latin.cmp(&cyrillic) {
        std::cmp::Ordering::Greater => Some(Script::Latin),
        std::cmp::Ordering::Less => Some(Script::Cyrillic),
        std::cmp::Ordering::Equal => None,
    }
}

fn convert(word: &str, to: Script) -> String {
    let swap = |c: char| {
        HOMOGLYPHS.iter().find_map(|&(latin, cyrillic)| match to {
            Script::Latin if c == cyrillic => Some(latin),
            Script::Cyrillic if c == latin => Some(cyrillic),
            _ => None,
        })
    };

    word.chars().map(|c| swap(c).unwrap_or(c)).collect()
}

/// Fixes words mixing Latin and Cyrillic look-alike letters ("сhanges" -> "changes").
/// Words consisting only of look-alikes follow the line, if all other words share one script.
/// Also collapses repeated spaces.
fn fix_mixed_scripts(line: &str) -> String {
    let words: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
    let scripts: Vec<Option<Script>> = words.iter().map(|w| dominant_script(w)).collect();

    let line_script = match (
        scripts.contains(&Some(Script::Latin)),
        scripts.contains(&Some(Script::Cyrillic)),
    ) {
        (true, false) => Some(Script::Latin),
        (false, true) => Some(Script::Cyrillic),
        _ => None,
    };

    words
        .iter()
        .zip(scripts)
        .map(|(word, script)| match script.or(line_script) {
            Some(script) => convert(word, script),
            None => (*word).to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::geometry::Point;
    use super::*;

    fn line(text: &str, x: f32, y: f32) -> TextLine {
        let (w, h) = (50.0, 20.0);
        let quad = [
            Point::new(x, y),
            Point::new(x + w, y),
            Point::new(x + w, y + h),
            Point::new(x, y + h),
        ];

        TextLine {
            text: text.to_owned(),
            quad,
        }
    }

    #[test]
    fn assembles_rows_in_reading_order() {
        let lines = [
            line("world", 60.0, 2.0),
            line("second", 0.0, 40.0),
            line("hello", 0.0, 0.0),
        ];

        assert_eq!(assemble_text(&lines), "hello world\nsecond");
    }

    #[test]
    fn finds_real_cyrillic_words() {
        assert!(contains_word_in("Сохранить изменения?", Script::Cyrillic));
        assert!(contains_word_in("ОК Отмена", Script::Cyrillic));
    }

    #[test]
    fn ignores_misreadings_and_look_alikes() {
        assert!(!contains_word_in("Еrгог: 404", Script::Cyrillic));
        assert!(!contains_word_in("НОС ОК", Script::Cyrillic));
        assert!(!contains_word_in("Größe und Qualität", Script::Cyrillic));
    }

    #[test]
    fn fixes_cyrillic_letters_in_english_words() {
        assert_eq!(
            fix_mixed_scripts("Save сhanges before сlosing?"),
            "Save changes before closing?"
        );
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
        let line = "Цена: 1 299,00 Р email: test@example.com";

        assert_eq!(fix_mixed_scripts(line), line);
    }

    #[test]
    fn collapses_repeated_spaces() {
        assert_eq!(fix_mixed_scripts("идея,  сказал"), "идея, сказал");
    }
}
