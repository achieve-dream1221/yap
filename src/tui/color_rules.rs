use serde_with::{DisplayFromStr, TryFromInto};

use std::{ops::Range, path::Path};

use compact_str::CompactString;
use fs_err as fs;
use memchr::memmem::Finder;
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use regex::bytes::Regex;
use serde_with::serde_as;
use tracing::info;

use crate::{
    buffer::RangeSlice,
    traits::{LineHelpers, LineMutator},
};

pub const COLOR_RULES_PATH: &str = "yap_colors.toml";

#[derive(Debug, Default)]
#[cfg_attr(test, derive(Clone))]
pub struct ColorRules {
    regex_lines: Vec<(RegexRule, RuleType)>,
    regex_words: Vec<(RegexRule, RuleType)>,
    literal_lines: Vec<(LiteralRule, RuleType)>,
    literal_words: Vec<(LiteralRule, RuleType)>,
}
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
struct RegexRule {
    regex: Regex,
}
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
struct LiteralRule {
    finder: Finder<'static>,
}
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
enum RuleType {
    Color(Color),
    Hide,
    Censor(Option<Color>),
}
#[derive(Debug, serde::Deserialize)]
struct ColorRulesFile {
    #[serde(default)]
    regex: Vec<SerializedRegexRule>,
    #[serde(default)]
    literal: Vec<SerializedLiteralRule>,
}

#[serde_as]
#[derive(Debug, serde::Deserialize)]
struct SerializedRegexRule {
    #[serde_as(as = "TryFromInto<String>")]
    rule: Regex,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    line: bool,
    #[serde(default)]
    hide: bool,
    #[serde(default)]
    censor: bool,
}

#[serde_as]
#[derive(Debug, serde::Deserialize)]
struct SerializedLiteralRule {
    rule: CompactString,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    line: bool,
    #[serde(default)]
    hide: bool,
    #[serde(default)]
    censor: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ColorRuleLoadError {
    #[error("failed reading from color rules file")]
    FileRead(#[source] std::io::Error),
    #[error("failed saving to color rules file")]
    FileWrite(#[source] std::io::Error),
    #[error("invalid color rule format")]
    Deser(#[from] toml::de::Error),
    #[error("rule must either be hiding, censoring, or coloring: \"{0}\"")]
    UnspecifiedRule(String),
}

impl ColorRules {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, ColorRuleLoadError> {
        let path = path.as_ref();

        if !path.exists() {
            info!("Color rules file not found at specified path, saving example file.");
            fs::write(
                path,
                include_bytes!("../../example_configs/yap_colors.toml.blank"),
            )
            .map_err(ColorRuleLoadError::FileWrite)?;

            return Ok(Self::default());
        }

        let buffer = fs::read_to_string(path).map_err(ColorRuleLoadError::FileRead)?;
        let ColorRulesFile { regex, literal } = toml::from_str(&buffer)?;

        let mut regex_lines = Vec::new();
        let mut regex_words = Vec::new();
        let mut literal_lines = Vec::new();
        let mut literal_words = Vec::new();

        for rule in regex {
            let regex = rule.rule;
            let rule_type = {
                if rule.hide {
                    RuleType::Hide
                } else if rule.censor {
                    RuleType::Censor(rule.color)
                } else if let Some(color) = rule.color {
                    RuleType::Color(color)
                } else {
                    return Err(ColorRuleLoadError::UnspecifiedRule(
                        regex.as_str().to_owned(),
                    ));
                }
            };

            if rule.line {
                regex_lines.push((RegexRule { regex }, rule_type));
            } else {
                regex_words.push((RegexRule { regex }, rule_type));
            }
        }

        for rule in literal {
            let rule_type = {
                if rule.hide {
                    RuleType::Hide
                } else if rule.censor {
                    RuleType::Censor(rule.color)
                } else if let Some(color) = rule.color {
                    RuleType::Color(color)
                } else {
                    return Err(ColorRuleLoadError::UnspecifiedRule(rule.rule.to_string()));
                }
            };

            let finder = Finder::new(rule.rule.as_bytes()).into_owned();
            if rule.line {
                literal_lines.push((LiteralRule { finder }, rule_type));
            } else {
                literal_words.push((LiteralRule { finder }, rule_type));
            }
        }

        Ok(Self {
            regex_lines,
            regex_words,
            literal_lines,
            literal_words,
        })
    }

    // Thinking a lot about how to redo this whole module:
    //
    // need a
    // struct ColorActions {
    //     whole_line_color: Option<Color>
    //     whole_line_hide/censor: Option<Hide/Censor> (Both Hide and Censor can never be undone, with Hide taking priority.)
    //     word_actions: Vec<(ColorCensorRemove, Range)>
    // }
    //
    // for whole line rules, all that matters is the end results flatten out to the same
    // and honestly, ditto goes for word actions, but thats less of a likelyhood.
    //
    // with
    //
    // fn unchanged_up_to(usize, actions) -> bool
    // checking each action bound up to the index, also making sure the new actions
    // doesn't go any further.
    //
    // or saving hashes for whole line actions and word actions would work the same
    //
    // Extra notes:
    //
    // Could use a RegexSet for lines/words (each)!
    // Would need to build when loading all rules from disk after all have been verified to be valid Regex,
    // but could speed up some simple cases (lines) and can help skip searches for (words).

    pub fn apply_onto<'a>(&self, original: &'a [u8], mut line: Line<'a>) -> Option<Line<'a>> {
        if line.is_empty() {
            return None;
        }
        // tracing::debug!("{}", original.len());
        // let now = std::time::Instant::now();

        struct ByteVisiblityTracker {
            records: Vec<Bytes>,
        }
        impl ByteVisiblityTracker {
            /// Accounts for shifts in content due to Bytes::Replaced
            fn get_corrected_range(&self, in_original: Range<usize>) -> Option<Range<usize>> {
                // Track our position in the original and rendered string.
                let mut orig_idx = 0;
                let mut rendered_idx = 0;
                let mut new_start = None;
                let mut new_end = None;

                let Range {
                    start: req_start,
                    end: req_end,
                } = in_original;

                for append in &self.records {
                    match append {
                        Bytes::Untouched(len) => {
                            assert_ne!(*len, 0, "can't have 0 length visible slice");
                            let next_orig = orig_idx + len;
                            let next_rendered = rendered_idx + len;

                            // Is start inside this block?
                            if new_start.is_none() && req_start >= orig_idx && req_start < next_orig
                            {
                                new_start = Some(rendered_idx + (req_start - orig_idx));
                            }
                            // Is end inside this block?
                            if new_end.is_none() && req_end > orig_idx && req_end <= next_orig {
                                new_end = Some(rendered_idx + (req_end - orig_idx));
                            }

                            orig_idx = next_orig;
                            rendered_idx = next_rendered;
                        }
                        Bytes::Replaced {
                            original_len,
                            new_len,
                        } => {
                            assert_ne!(*original_len, 0, "can't have 0 length redacted slice");
                            let next_orig = orig_idx + original_len;
                            let next_rendered = rendered_idx + new_len;

                            // If the start is buried in replaced area, use beginning of replacement
                            if new_start.is_none() && req_start >= orig_idx && req_start < next_orig
                            {
                                new_start = Some(rendered_idx);
                            }
                            // If the end is buried in replaced area, snap to end of replacement
                            if new_end.is_none() && req_end > orig_idx && req_end <= next_orig {
                                new_end = Some(rendered_idx + *new_len);
                            }

                            orig_idx = next_orig;
                            rendered_idx = next_rendered;
                        }
                    }

                    // If we've found both ends, break
                    if new_start.is_some() && new_end.is_some() {
                        break;
                    }
                }

                // if new_start.is_none() && new_end.is_none() {
                //     break;
                // }

                let (Some(start), Some(end)) = (new_start, new_end) else {
                    panic!(
                        "no bound found for given range? len: {in_original:?} -> {new_start:?}, {new_end:?}"
                    );
                };

                if start < end { Some(start..end) } else { None }
            }

            fn push_untouched(&mut self, len: usize) {
                self.records.push(Bytes::Untouched(len));
            }
            fn push_replaced(&mut self, original_len: usize, new_len: usize) {
                self.records.push(Bytes::Replaced {
                    original_len,
                    new_len,
                });
            }
            // Might make more sense to keep the existing removed_ranges logic,
            // since this would break AXM rules with the visibility tracker in the iterator
            // fn hide_visible(&mut self, range: Range<usize>)
        }

        let mut visibility = ByteVisiblityTracker {
            records: Vec::with_capacity(line.spans.len()),
        };
        #[derive(Debug)]
        enum Bytes {
            Untouched(usize),
            Replaced { original_len: usize, new_len: usize },
        }
        impl Bytes {
            fn original_len(&self) -> usize {
                match self {
                    Bytes::Untouched(len) => *len,
                    Bytes::Replaced { original_len, .. } => *original_len,
                }
            }
        }

        let line_len: usize = line.iter().map(|s| s.content.len()).sum();

        let mut last_end = 0;
        let mut queued_replacement = 0;
        for span in line.spans.iter() {
            let orig_ptr_range = original.as_ptr_range();

            let span_bytes = span.content.as_bytes();

            let span_within_original = orig_ptr_range.contains(&span_bytes.as_ptr());

            if span_within_original {
                let RangeSlice { range, .. } =
                    unsafe { RangeSlice::from_parent_and_child(original, span_bytes) };

                if range.start > last_end {
                    visibility.push_replaced(range.start - last_end, queued_replacement);
                    queued_replacement = 0;
                }
                visibility.push_untouched(span_bytes.len());
                last_end = range.end;
            } else {
                queued_replacement += span_bytes.len();
            }
        }

        if last_end < original.len() {
            visibility.push_replaced(original.len() - last_end, 0);
        }

        assert_eq!(
            visibility
                .records
                .iter()
                .map(|b| b.original_len())
                .sum::<usize>(),
            original.len(),
            "every byte must be accounted for"
        );

        // debug!("indexing slice took {:?}", now.elapsed());
        // let now2 = Instant::now();

        // For style_all_spans, we don't care -- just color the whole line.
        // For style_slice, we use byte_to_char to map the matched byte span to the rendered string indices.

        for (lit_rule, rule_type) in &self.literal_lines {
            if lit_rule.finder.find(original).is_some() {
                match rule_type {
                    RuleType::Color(color) => line.style_all_spans(Style::from(*color)),
                    RuleType::Hide => return None,
                    RuleType::Censor(color_opt) => {
                        line.censor_slice(0..line_len, color_opt.map(Style::from))
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
                        line.censor_slice(0..line_len, color_opt.map(Style::from))
                    }
                }
            }
        }

        let mut removed_ranges: Vec<Range<usize>> = Vec::new();

        for (lit_rule, rule_type) in &self.literal_words {
            let rule_len = lit_rule.finder.needle().len();
            let ranges_iter = lit_rule
                .finder
                .find_iter(original)
                .filter_map(|oc_idx| visibility.get_corrected_range(oc_idx..oc_idx + rule_len));

            match rule_type {
                RuleType::Color(color) => {
                    for range in ranges_iter {
                        if let Some(new_range) = act_if_possible(line_len, &removed_ranges, range) {
                            // debug!("styling range {range:?} {color}");
                            line.style_slice(new_range, Style::from(*color));
                        }
                    }
                }
                RuleType::Hide => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            remove_if_possible(line_len, &mut removed_ranges, range)
                        {
                            line.remove_slice(new_range);
                        }
                    }
                }
                RuleType::Censor(color_opt) => {
                    for range in ranges_iter {
                        if let Some(new_range) = act_if_possible(line_len, &removed_ranges, range) {
                            line.censor_slice(new_range, color_opt.map(Style::from));
                        }
                    }
                }
            }
        }
        for (reg_rule, rule_type) in &self.regex_words {
            let ranges_iter = reg_rule
                .regex
                .find_iter(original)
                .filter_map(|occ| visibility.get_corrected_range(occ.start()..occ.end()));
            match rule_type {
                RuleType::Color(color) => {
                    for range in ranges_iter {
                        if let Some(new_range) = act_if_possible(line_len, &removed_ranges, range) {
                            line.style_slice(new_range, Style::from(*color));
                        }
                    }
                }
                RuleType::Hide => {
                    for range in ranges_iter {
                        if let Some(new_range) =
                            remove_if_possible(line_len, &mut removed_ranges, range)
                        {
                            line.remove_slice(new_range);
                        }
                    }
                }
                RuleType::Censor(color_opt) => {
                    for range in ranges_iter {
                        if let Some(new_range) = act_if_possible(line_len, &removed_ranges, range) {
                            line.censor_slice(new_range, color_opt.map(Style::from));
                        }
                    }
                }
            }
        }

        // TODO check if anything is left to be visible to return None instead

        // debug!("applying to slice took {:?}", now2.elapsed());
        // tracing::debug!("apply color rules took {:?}", now.elapsed());
        Some(line)
    }
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

/// Returns the adjusted current range (in the *current* string),
/// if none of its positions overlap an already_removed range from the original.
fn act_if_possible(
    slice_len: usize,
    already_removed: &[Range<usize>],
    current: Range<usize>,
) -> Option<Range<usize>> {
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
