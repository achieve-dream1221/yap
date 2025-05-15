use std::{path::Path, str::FromStr};

use compact_str::{CompactString, CompactStringExt};
use fs_err as fs;
use memchr::memmem::Finder;
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use regex::bytes::Regex;

use crate::traits::{LineColor, LineHelpers};

#[derive(Debug)]
pub struct ColorRules {
    regex_lines: Vec<(RegexRule, Color)>,
    regex_words: Vec<(RegexRule, Color)>,
    literal_lines: Vec<(LiteralRule, Color)>,
    literal_words: Vec<(LiteralRule, Color)>,
}
#[derive(Debug)]
struct RegexRule {
    regex: Regex,
}
#[derive(Debug)]
struct LiteralRule {
    value: CompactString,
    // TODO use value? self-ref?
    finder: Finder<'static>,
}

#[derive(Debug, serde::Deserialize)]
struct ColorRulesFile {
    #[serde(default)]
    regex: Vec<SerializedRule>,
    #[serde(default)]
    literal: Vec<SerializedRule>,
}
#[derive(Debug, serde::Deserialize)]
struct SerializedRule {
    rule: CompactString,
    color: CompactString,
    #[serde(default)]
    line: bool,
}

impl ColorRules {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Self {
        let buffer = fs::read_to_string(path.as_ref()).unwrap();
        let ColorRulesFile { regex, literal } = toml::from_str(&buffer).unwrap();

        let mut regex_lines = Vec::new();
        let mut regex_words = Vec::new();
        let mut literal_lines = Vec::new();
        let mut literal_words = Vec::new();

        for rule in regex {
            let color = Color::from_str(&rule.color).unwrap();
            let regex = Regex::new(&rule.rule).unwrap();
            if rule.line {
                regex_lines.push((RegexRule { regex }, color));
            } else {
                regex_words.push((RegexRule { regex }, color));
            }
        }

        for rule in literal {
            let color = Color::from_str(&rule.color).unwrap();
            let value = rule.rule.clone();
            let finder = Finder::new(rule.rule.as_bytes()).into_owned();
            if rule.line {
                literal_lines.push((LiteralRule { value, finder }, color));
            } else {
                literal_words.push((LiteralRule { value, finder }, color));
            }
        }

        Self {
            regex_lines,
            regex_words,
            literal_lines,
            literal_words,
        }
    }

    pub fn apply_onto<'a>(&self, original: &[u8], line: &'a mut Line<'_>) {
        // Handle mapping between original bytes and visible characters,
        // so that styling with ANSI escapes/arbitrary bytes is handled correctly.

        // First, build a mapping from raw byte offset (in 'original') to grapheme offset (idx into 'line').
        // We'll step through 'line' content, mapping the char-by-char positions.

        // This requires two things:
        // 1. Mapping byte indices from 'original' (which may include escapes/nonsense)
        //    to character indices in 'line' (which should be clean render text).
        // 2. Skipping ANSI escape sequences ("\x1b[...m") and non-printable bytes.

        // We'll do a best-effort match: scan through original and line at the same time.

        // The buffer 'original' is bytes; but we also need the rendered text for 'line' as a str.
        let rendered: CompactString = line.iter().map(|s| s.content.as_ref()).join_compact("");

        // Build mapping: for each byte index into 'original', what's the visible char index in 'rendered'?
        // We'll do a simple, strict scan. If things don't match up, fallback to legacy approach.

        // Maps byte index in 'original' to char index in 'rendered'.
        let mut byte_to_char: Vec<usize> = vec![0; original.len() + 1]; // for every byte, its mapped char idx in rendered
        let mut i = 0; // index into original
        let mut j = 0; // char index into rendered

        // Also, build mapping from char index in rendered to byte index in original.
        let mut char_to_byte: Vec<usize> = vec![0; rendered.len() + 1];

        while i < original.len() && j < rendered.len() {
            if original[i] == 0x1b {
                // Skip an ANSI escape sequence
                let mut esc_end = i + 1;
                if esc_end < original.len() && original[esc_end] == b'[' {
                    esc_end += 1;
                    // CSI sequences: \x1b[ ... [A-Za-z] (final byte)
                    while esc_end < original.len() {
                        let b = original[esc_end];
                        if b >= 0x40 && b <= 0x7E {
                            esc_end += 1;
                            break;
                        }
                        esc_end += 1;
                    }
                }
                while i < esc_end {
                    byte_to_char[i] = j;
                    i += 1;
                }
                continue;
            }

            // decode next UTF-8 char from original

            let end = original.len().min(i + 4);

            // let c = match std::str::from_utf8() {
            //     Ok(rest) => rest.chars().next(),
            //     Err(_) => None,
            // };

            let c = bytes_to_char(&original[i..end]);

            if let Some(orig_ch) = c {
                let ch_len = orig_ch.len_utf8();
                let rendered_ch = rendered[j..].chars().next();

                // If the characters match, map all bytes of orig_ch to j in char space
                if Some(orig_ch) == rendered_ch {
                    for k in 0..ch_len {
                        byte_to_char[i + k] = j;
                    }
                    char_to_byte[j] = i;
                    i += ch_len;
                    j += orig_ch.len_utf8();
                } else {
                    // out of sync: fallback
                    // Mark remaining positions to 0, break.
                    for x in i..original.len() {
                        byte_to_char[x] = 0;
                    }
                    break;
                }
            } else {
                // Invalid byte or not a valid UTF-8 codepoint.
                byte_to_char[i] = j;
                i += 1;
            }
        }
        // The arrays cover up to these positions, but not necessarily through the whole original/line.
        for x in i..original.len() {
            byte_to_char[x] = j;
        }
        for y in j..rendered.len() {
            char_to_byte[y] = i;
        }

        // For style_all_spans, we don't care -- just color the whole line.
        // For style_slice, we use byte_to_char to map the matched byte span to the rendered string indices.

        for (lit_rule, color) in &self.literal_lines {
            if lit_rule.finder.find(original).is_some() {
                line.style_all_spans(Style::from(*color));
            }
        }
        for (reg_rule, color) in &self.regex_lines {
            if reg_rule.regex.is_match(original) {
                line.style_all_spans(Style::from(*color));
            }
        }
        for (lit_rule, color) in &self.literal_words {
            let rule_len = lit_rule.value.len();
            for occurance_idx in lit_rule.finder.find_iter(original) {
                let byte_start = occurance_idx;
                let byte_end = occurance_idx + rule_len;
                // Map these to char indices in rendered
                let char_start = if byte_start < byte_to_char.len() {
                    byte_to_char[byte_start]
                } else {
                    0
                };
                let char_end = if byte_end < byte_to_char.len() {
                    byte_to_char[byte_end]
                } else {
                    rendered.len()
                };

                // Clamp to bounds of rendered string
                let char_start = char_start.min(rendered.len());
                let char_end = char_end.min(rendered.len());

                if char_start < char_end {
                    line.style_slice(char_start..char_end, Style::from(*color));
                }
            }
        }
        for (reg_rule, color) in &self.regex_words {
            for occurance in reg_rule.regex.find_iter(original) {
                let byte_start = occurance.start();
                let byte_end = occurance.end();

                let char_start = if byte_start < byte_to_char.len() {
                    byte_to_char[byte_start]
                } else {
                    0
                };
                let char_end = if byte_end < byte_to_char.len() {
                    byte_to_char[byte_end]
                } else {
                    rendered.len()
                };

                let char_start = char_start.min(rendered.len());
                let char_end = char_end.min(rendered.len());

                if char_start < char_end {
                    line.style_slice(char_start..char_end, Style::from(*color));
                }
            }
        }
    }
}

fn bytes_to_char(bytes: &[u8]) -> Option<char> {
    std::str::from_utf8(bytes).ok()?.chars().next()
}
