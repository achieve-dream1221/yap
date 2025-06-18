use std::{ops::Range, path::Path, str::FromStr};

use compact_str::{CompactString, CompactStringExt};
use fs_err as fs;
use memchr::memmem::Finder;
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use regex::bytes::Regex;
use tracing::debug;

use crate::traits::{LineHelpers, LineMutator};

#[derive(Debug)]
pub struct ColorRules {
    regex_lines: Vec<(RegexRule, RuleType)>,
    regex_words: Vec<(RegexRule, RuleType)>,
    literal_lines: Vec<(LiteralRule, RuleType)>,
    literal_words: Vec<(LiteralRule, RuleType)>,
}
#[derive(Debug)]
struct RegexRule {
    regex: Regex,
}
#[derive(Debug)]
struct LiteralRule {
    finder: Finder<'static>,
}
#[derive(Debug)]
enum RuleType {
    Color(Color),
    Hide,
    Censor(Option<Color>),
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
    color: Option<CompactString>,
    #[serde(default)]
    line: bool,
    #[serde(default)]
    hide: bool,
    #[serde(default)]
    censor: bool,
}

impl ColorRules {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Self {
        // TODO check for missing file and fill with commented example contents
        let buffer = fs::read_to_string(path.as_ref()).unwrap();
        let ColorRulesFile { regex, literal } = toml::from_str(&buffer).unwrap();

        let mut regex_lines = Vec::new();
        let mut regex_words = Vec::new();
        let mut literal_lines = Vec::new();
        let mut literal_words = Vec::new();

        for rule in regex {
            let color_opt = rule
                .color
                .as_ref()
                .map(|color_str| Color::from_str(color_str).unwrap());

            let rule_type = {
                if rule.hide {
                    RuleType::Hide
                } else if rule.censor {
                    RuleType::Censor(color_opt)
                } else if let Some(color) = color_opt {
                    RuleType::Color(color)
                } else {
                    todo!("invalid rule; TODO make proper error")
                }
            };

            let regex = Regex::new(&rule.rule).unwrap();
            if rule.line {
                regex_lines.push((RegexRule { regex }, rule_type));
            } else {
                regex_words.push((RegexRule { regex }, rule_type));
            }
        }

        for rule in literal {
            let color_opt = rule
                .color
                .as_ref()
                .map(|color_str| Color::from_str(color_str).unwrap());

            let rule_type = {
                if rule.hide {
                    RuleType::Hide
                } else if rule.censor {
                    RuleType::Censor(color_opt)
                } else if let Some(color) = color_opt {
                    RuleType::Color(color)
                } else {
                    todo!("invalid rule; TODO make proper error")
                }
            };

            let finder = Finder::new(rule.rule.as_bytes()).into_owned();
            if rule.line {
                literal_lines.push((LiteralRule { finder }, rule_type));
            } else {
                literal_words.push((LiteralRule { finder }, rule_type));
            }
        }

        Self {
            regex_lines,
            regex_words,
            literal_lines,
            literal_words,
        }
    }

    pub fn apply_onto<'a, 'b>(&self, original: &[u8], mut line: Line<'b>) -> Option<Line<'b>> {
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

        for (lit_rule, rule_type) in &self.literal_lines {
            if lit_rule.finder.find(original).is_some() {
                match rule_type {
                    RuleType::Color(color) => line.style_all_spans(Style::from(*color)),
                    RuleType::Hide => return None,
                    RuleType::Censor(color_opt) => {
                        line.censor_slice(0..rendered.len(), color_opt.map(Style::from))
                    }
                }
            }
        }
        for (reg_rule, rule_type) in &self.regex_lines {
            if reg_rule.regex.is_match(original) {
                match rule_type {
                    RuleType::Color(color) => line.style_all_spans(Style::from(*color)),
                    RuleType::Hide => return None,
                    RuleType::Censor(color_opt) => {
                        line.censor_slice(0..rendered.len(), color_opt.map(Style::from))
                    }
                }
            }
        }

        let mut removed_ranges: Vec<Range<usize>> = Vec::new();

        for (lit_rule, rule_type) in &self.literal_words {
            let rule_len = lit_rule.finder.needle().len();
            let ranges_iter = lit_rule.finder.find_iter(original).filter_map(|oc_idx| {
                let byte_start = oc_idx;
                let byte_end = oc_idx + rule_len;
                // Map these to char indices in rendered
                let char_start = if byte_start < byte_to_char.len() {
                    byte_to_char[byte_start]
                } else {
                    0
                };
                let mut char_end = if byte_end < byte_to_char.len() {
                    byte_to_char[byte_end]
                } else {
                    rendered.len()
                };
                if char_end == 0 {
                    char_end = rendered.len();
                }
                // Clamp to bounds of rendered string
                let char_start = char_start.min(rendered.len());
                let char_end = char_end.min(rendered.len());

                if char_start < char_end {
                    Some(char_start..char_end)
                } else {
                    debug!("AGH1 {char_start}..{char_end} {byte_start}..{byte_end}");
                    debug!("{byte_to_char:#?}");
                    None
                }
            });

            match rule_type {
                RuleType::Color(color) => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            act_if_possible(rendered.len(), &removed_ranges, range)
                        {
                            line.style_slice(new_range, Style::from(*color));
                        }
                    }
                }
                RuleType::Hide => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            remove_if_possible(rendered.len(), &mut removed_ranges, range)
                        {
                            line.remove_slice(new_range);
                        }
                    }
                }
                RuleType::Censor(color_opt) => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            act_if_possible(rendered.len(), &removed_ranges, range)
                        {
                            line.censor_slice(new_range, color_opt.map(Style::from));
                        }
                    }
                }
            }

            // if lit_rule.hide {

            // } else if lit_rule.else {
            //     for range in ranges_iter {
            //         line.style_slice(range, Style::from(*color));
            //     }
            // }
        }
        for (reg_rule, rule_type) in &self.regex_words {
            let ranges_iter = reg_rule.regex.find_iter(original).filter_map(|occ| {
                let byte_start = occ.start();
                let byte_end = occ.end();

                let char_start = if byte_start < byte_to_char.len() {
                    byte_to_char[byte_start]
                } else {
                    0
                };
                let mut char_end = if byte_end < byte_to_char.len() {
                    byte_to_char[byte_end]
                } else {
                    rendered.len()
                };
                if char_end == 0 {
                    char_end = rendered.len();
                }

                let char_start = char_start.min(rendered.len());
                let char_end = char_end.min(rendered.len());

                if char_start < char_end {
                    Some(char_start..char_end)
                } else {
                    debug!("AGH2 {char_start}..{char_end} {byte_start}..{byte_end}");
                    None
                }
            });
            match rule_type {
                RuleType::Color(color) => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            act_if_possible(rendered.len(), &removed_ranges, range)
                        {
                            line.style_slice(new_range, Style::from(*color));
                        }
                    }
                }
                RuleType::Hide => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            remove_if_possible(rendered.len(), &mut removed_ranges, range)
                        {
                            line.remove_slice(new_range);
                        }
                    }
                }
                RuleType::Censor(color_opt) => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            act_if_possible(rendered.len(), &removed_ranges, range)
                        {
                            line.censor_slice(new_range, color_opt.map(Style::from));
                        }
                    }
                }
            }
        }

        Some(line)
    }
}

fn bytes_to_char(bytes: &[u8]) -> Option<char> {
    std::str::from_utf8(bytes).ok()?.chars().next()
}

fn remove_if_possible(
    slice_len: usize,
    already_removed: &mut Vec<Range<usize>>,
    current: Range<usize>,
) -> Option<Range<usize>> {
    // `already_removed` contains ranges (in the initial string) that have already been removed.
    // As we remove ranges, the string shrinks. We need to compute the corresponding range for the *current* string.
    // That is, subtract the number of removed characters before the `current` range.

    // Count number of removed positions *before* current.start
    let mut shift = 0;
    for rem in already_removed.iter() {
        if rem.end <= current.start {
            shift += rem.end - rem.start;
        } else if rem.start < current.start && rem.end > current.start {
            // Overlaps the start
            shift += current.start - rem.start;
        }
    }
    // Count if the current range overlaps any already removed ranges; if so, skip (don't remove again)
    for rem in already_removed.iter() {
        // In the *initial* string, check for overlap
        if rem.start < current.end && rem.end > current.start {
            // overlap
            return None;
        }
    }

    let new_start = current.start - shift;
    let mut new_end = current.end - shift;
    // Cap at current slice_len
    let slice_len = slice_len.saturating_sub(shift);
    if new_end > slice_len {
        new_end = slice_len;
    }
    let new_range = new_start..new_end;
    already_removed.push(current);
    Some(new_range)
}

fn act_if_possible(
    slice_len: usize,
    already_removed: &[Range<usize>],
    current: Range<usize>,
) -> Option<Range<usize>> {
    // act_if_possible: Returns the adjusted current range (in the *current* string),
    // if none of its positions overlap an already_removed range from the original.

    // If any part of `current` overlaps any range already_removed, skip (None).
    for rem in already_removed {
        if rem.start < current.end && rem.end > current.start {
            // overlapping
            return None;
        }
    }

    // Calculate the shift: how many chars have been removed before current.start.
    let mut shift = 0;
    for rem in already_removed {
        if rem.end <= current.start {
            shift += rem.end - rem.start;
        } else if rem.start < current.start && rem.end > current.start {
            // Only the part before current.start is counted
            shift += current.start - rem.start;
        }
    }

    let new_start = current.start - shift;
    let mut new_end = current.end - shift;
    let slice_len = slice_len.saturating_sub(shift);
    if new_end > slice_len {
        new_end = slice_len;
    }
    let new_range = new_start..new_end;
    Some(new_range)
}
