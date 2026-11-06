//! What a person reads. A line continuation that went missing leaves a run of spaces in the middle of
//! a sentence, and nothing else would catch it.

use std::fs;
use std::path::{Path, PathBuf};

const SOURCES: [&str; 2] = ["src", "../app/src"];
/// Holds the format of `pci.ids`, which has two spaces in it on purpose.
const NOT_FOR_READING: &str = "cards.rs";
const TESTS_START: &str = "#[cfg(test)]";

fn rust_files(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
        } else {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.ends_with(".rs") && !name.ends_with("tests.rs") && name != NOT_FOR_READING {
                found.push(path);
            }
        }
    }
}

/// Each string in Rust source, as the compiler would read it where that matters here: a line
/// continuation drops the line end and the spaces after it. Raw strings and strings that hold real line
/// ends are scripts and style sheets, not sentences.
fn strings_in(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut letters = source.chars().peekable();
    let mut before = ' ';
    while let Some(letter) = letters.next() {
        match letter {
            '/' if letters.peek() == Some(&'/') => {
                letters.by_ref().find(|next| *next == '\n');
            }
            '\'' if letters.peek() == Some(&'"') || letters.peek() == Some(&'\\') => {
                letters.by_ref().find(|next| *next == '\'');
            }
            '"' if before != 'r' && before != '#' => {
                let mut text = String::new();
                let mut has_line_end = false;
                while let Some(inside) = letters.next() {
                    match inside {
                        '"' => break,
                        '\\' if letters.peek() == Some(&'\n') => {
                            while letters.peek().is_some_and(|next| next.is_whitespace()) {
                                letters.next();
                            }
                        }
                        '\\' => text.push(if letters.next() == Some('n') { '\n' } else { '.' }),
                        '\n' => has_line_end = true,
                        other => text.push(other),
                    }
                }
                if !has_line_end {
                    found.push(text);
                }
            }
            '"' => {
                letters.by_ref().find(|next| *next == '"');
            }
            _ => {}
        }
        before = letter;
    }
    found
}

/// Spaces at the start or end of a line are layout.
fn has_a_run_of_spaces(text: &str) -> bool {
    text.lines().any(|line| line.trim().contains("  "))
}

#[test]
fn a_run_of_spaces_in_a_string_is_found_and_a_continuation_is_not() {
    let source = "const A: &str = \"one,    two\";\nconst B: &str = \"one, \\\n      two\";\n";
    assert_eq!(strings_in(source), ["one,    two", "one, two"]);
}

#[test]
fn no_text_has_a_double_space_in_it() {
    let mut files = Vec::new();
    for folder in SOURCES {
        rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join(folder), &mut files);
    }
    assert!(files.len() > 50, "the sources moved");
    let mut bad = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        let source = source.split(TESTS_START).next().unwrap_or_default();
        bad.extend(
            strings_in(source)
                .into_iter()
                .filter(|text| has_a_run_of_spaces(text))
                .map(|text| format!("{}: {text}", file.display())),
        );
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
