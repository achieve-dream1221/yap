use std::{cmp::Ordering, ops::Range, path::Path, str::FromStr};

use ansi_to_tui::LossyFlavor;
use bstr::ByteSlice;
use compact_str::{CompactString, CompactStringExt};
use fs_err as fs;
use itertools::Itertools;
use memchr::memmem::Finder;
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use regex::bytes::Regex;
use tracing::{debug, info};

use crate::traits::{LineHelpers, LineMutator};

#[derive(Debug, Default)]
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

#[derive(Debug, thiserror::Error)]
pub enum ColorRuleError {
    #[error("error reading color rules file")]
    FileRead(#[source] std::io::Error),
    #[error("error saving color rules file")]
    FileWrite(#[source] std::io::Error),
    #[error("error deserializing color rules")]
    Deser(#[from] toml::de::Error),
    #[error("rule must either be hiding, censoring, or coloring")]
    UnspecifiedRule,
    #[error("invalid regex: \"{0}\" due to {1}")]
    InvalidRegex(String, regex::Error),
    #[error("unrecognized color: \"{0}\"")]
    UnrecognizedColor(String),
}

impl ColorRules {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, ColorRuleError> {
        let path = path.as_ref();

        if !path.exists() {
            info!("Color rules file not found at specified path, saving example file.");
            fs::write(
                path,
                include_bytes!("../../example_configs/yap_colors.toml.blank"),
            )
            .map_err(ColorRuleError::FileWrite)?;

            return Ok(Self::default());
        }

        let buffer = fs::read_to_string(path).map_err(ColorRuleError::FileRead)?;
        let ColorRulesFile { regex, literal } = toml::from_str(&buffer)?;

        let mut regex_lines = Vec::new();
        let mut regex_words = Vec::new();
        let mut literal_lines = Vec::new();
        let mut literal_words = Vec::new();

        for rule in regex {
            let color_res_opt = rule
                .color
                .as_ref()
                .map(|color_str| Color::from_str(color_str).map_err(|_| color_str));

            let color_opt = match color_res_opt.transpose() {
                Ok(c) => c,
                Err(unrecognized) => {
                    return Err(ColorRuleError::UnrecognizedColor(unrecognized.to_string()));
                }
            };

            let rule_type = {
                if rule.hide {
                    RuleType::Hide
                } else if rule.censor {
                    RuleType::Censor(color_opt)
                } else if let Some(color) = color_opt {
                    RuleType::Color(color)
                } else {
                    return Err(ColorRuleError::UnspecifiedRule);
                }
            };

            let regex = Regex::new(&rule.rule)
                .map_err(|e| ColorRuleError::InvalidRegex(rule.rule.to_string(), e))?;
            if rule.line {
                regex_lines.push((RegexRule { regex }, rule_type));
            } else {
                regex_words.push((RegexRule { regex }, rule_type));
            }
        }

        for rule in literal {
            let color_res = rule
                .color
                .as_ref()
                .map(|color_str| Color::from_str(color_str).map_err(|_| color_str));

            let color_opt = match color_res.transpose() {
                Ok(c) => c,
                Err(unrecognized) => {
                    return Err(ColorRuleError::UnrecognizedColor(unrecognized.to_string()));
                }
            };

            let rule_type = {
                if rule.hide {
                    RuleType::Hide
                } else if rule.censor {
                    RuleType::Censor(color_opt)
                } else if let Some(color) = color_opt {
                    RuleType::Color(color)
                } else {
                    return Err(ColorRuleError::UnspecifiedRule);
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

    pub fn apply_onto<'a>(
        &self,
        original: &[u8],
        mut line: Line<'a>,
        lossy_flavor: LossyFlavor,
    ) -> Option<Line<'a>> {
        // let now = std::time::Instant::now();

        struct ByteVisiblityTracker {
            records: Vec<Append>,
        }
        impl ByteVisiblityTracker {
            /// Accounts for shifts in content due to Append::Replaced
            fn get_corrected_range(&self, in_original: Range<usize>) -> Option<Range<usize>> {
                // Track our position in the original and rendered string.
                let mut orig_idx = 0;
                let mut rendered_idx = 0;
                let mut new_start = None;
                let mut new_end = None;

                let Range { start, end } = in_original;

                for append in &self.records {
                    match append {
                        Append::Visible(len) => {
                            let next_orig = orig_idx + len;
                            let next_rendered = rendered_idx + len;

                            // Is start inside this block?
                            if new_start.is_none() && start >= orig_idx && start < next_orig {
                                new_start = Some(rendered_idx + (start - orig_idx));
                            }
                            // Is end inside this block?
                            if new_end.is_none() && end > orig_idx && end <= next_orig {
                                new_end = Some(rendered_idx + (end - orig_idx));
                            }

                            orig_idx = next_orig;
                            rendered_idx = next_rendered;
                        }
                        Append::Replaced { original, new } => {
                            let next_orig = orig_idx + original;
                            let next_rendered = rendered_idx + new;

                            // If the start is buried in replaced area, snap to end of replacement
                            if new_start.is_none() && start >= orig_idx && start < next_orig {
                                new_start = Some(rendered_idx + (*new).saturating_sub(1));
                            }
                            // If the end is buried in replaced area, snap to end of replacement
                            if new_end.is_none() && end > orig_idx && end <= next_orig {
                                new_end = Some(rendered_idx + (*new).saturating_sub(0));
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

                // TODO verify this?
                // If not found, might be after all appended blocks.
                if new_start.is_none() {
                    new_start = Some(rendered_idx);
                }
                if new_end.is_none() {
                    new_end = Some(rendered_idx);
                }

                let start = new_start.unwrap_or(0);
                let end = new_end.unwrap_or(start);

                if start <= end { Some(start..end) } else { None }
            }

            fn visible_text(&mut self, text: &str, unconsumed: &[u8], cursor: &mut usize) {
                let str_index = unconsumed
                    .find(text)
                    .expect("str not found within parent slice?");

                if str_index > 0 {
                    self.records.push(Append::Replaced {
                        original: str_index,
                        new: 0,
                    });
                }

                self.records.push(Append::Visible(text.len()));

                *cursor += text.len() + str_index;
            }
            fn replaced(&mut self, original: usize, new: usize, cursor: &mut usize) {
                self.records.push(Append::Replaced { original, new });
                *cursor += original;
            }
            // Might make more sense to keep the existing removed_ranges logic,
            // since this would break AXM rules with the visibility tracker in the iterator
            // /// Amend existing records to set this range in original as hidden
            // fn hide_visible(&mut self, range: Range<usize>) {
            //     let mut passed_len = 0;
            //     let (index, visible) = self
            //         .records
            //         .iter()
            //         .find_position(|a| {
            //             passed_len += a.len();
            //             passed_len >= range.start
            //         })
            //         .expect("failed to find existing visible record!");

            //     let (pre, mid, post): (Option<Append>, Option<Append>, Option<Append>) = {
            //         let Append::Visible(len) = &visible else {
            //             panic!("hide_visible can only operate on visible blocks");
            //         };
            //         let pre_len = range.start - (passed_len - visible.len());
            //         let mid_len = (range.end - range.start).min(*len - pre_len);
            //         let post_len = len - pre_len - mid_len;
            //         let pre = if pre_len > 0 {
            //             Some(Append::Visible(pre_len))
            //         } else {
            //             None
            //         };
            //         let mid = if mid_len > 0 {
            //             Some(Append::Replaced {
            //                 original: mid_len,
            //                 new: 0,
            //             })
            //         } else {
            //             None
            //         };
            //         let post = if post_len > 0 {
            //             Some(Append::Visible(post_len))
            //         } else {
            //             None
            //         };
            //         (pre, mid, post)
            //     };

            //     self.records.splice(
            //         index..index + 1,
            //         [pre, mid, post].into_iter().filter_map(|s| s),
            //     );
            // }
        }

        let mut visibility = ByteVisiblityTracker {
            records: Vec::with_capacity(line.spans.len()),
        };
        // let mut builder = ropey::RopeBuilder::new();

        enum Append {
            Visible(usize),
            Replaced { original: usize, new: usize },
        }
        impl Append {
            fn len(&self) -> usize {
                match self {
                    Append::Visible(len) => *len,
                    Append::Replaced { original, .. } => *original,
                }
            }
        }

        // let mut append_to_builder = |data: Append, cursor: &mut usize| match data {
        //     Append::Visible(s) => {
        //         let str_index = original[*cursor..]
        //             .find(s)
        //             .expect("str not found within parent slice?");
        //         if *cursor < str_index {
        //             let deficit = str_index - *cursor;
        //             for _ in 0..deficit {
        //                 builder.append("*");
        //             }
        //         }
        //         *cursor += str_index;
        //         builder.append(s);
        //     }
        //     Append::Dummy(amount) => {
        //         *cursor += amount;
        //         for _ in 0..amount {
        //             builder.append("*");
        //         }
        //     }
        // };

        fn determine_if_visible(span_a: &str, span_b: &str, unconsumed: &[u8]) -> bool {
            let span_a_index = unconsumed.find(span_a.as_bytes());

            match span_a_index {
                None => false,
                Some(a_idx) => {
                    let span_b_index = unconsumed.find(span_b.as_bytes());
                    match span_b_index {
                        None => true,
                        Some(b_idx) => {
                            match a_idx.cmp(&b_idx) {
                                // Replacement string was not added by us!
                                // a_idx existing in original before b_idx means
                                //
                                Ordering::Less | Ordering::Equal => true,
                                // confirmed to be an ansi_to_tui replacement,
                                // since another was found after content we haven't gotten to yet
                                Ordering::Greater => false,
                            }
                        }
                    }
                }
            }
        }

        // fn determine_if_visible(span_a: &str, span_b: &str, unconsumed: &[u8]) -> bool {
        //     let span_a_index = unconsumed.find(span_a.as_bytes());
        //     let span_b_index = unconsumed.find(span_b.as_bytes());

        //     match (span_a_index, span_b_index) {
        //         (Some(a_idx), Some(b_idx)) => match a_idx.cmp(&b_idx) {
        //             // Replacement string was not added by us!
        //             // a_idx existing in original before b_idx means
        //             //
        //             Ordering::Less | Ordering::Equal => true,
        //             // confirmed to be an ansi_to_tui replacement,
        //             // since another was found after content we haven't gotten to yet
        //             Ordering::Greater => false,
        //         },
        //         (Some(_), None) => {
        //             // this one is likely a part of the original data
        //             // since we were able to find it in the original slice,
        //             // but the upcoming span likely isn't.
        //             true
        //         }
        //         (None, _) => {
        //             // must be an ansi_to_tui replacement,
        //             // char doesnt exist in slice.
        //             false
        //         }
        //     }
        // }

        let line_len: usize = line.iter().map(|s| s.content.len()).sum();
        let mut cursor = 0;
        if line.spans.len() == 1 {
            let content = &line.spans[0].content;
            if let Some(_content_idx) = original.find(content.as_bytes()) {
                visibility.visible_text(content.as_ref(), original, &mut cursor);
            }
        } else {
            for spans in line.spans.windows(2) {
                let unconsumed = &original[cursor..];

                let [span_a, span_b] = spans else {
                    unreachable!();
                };

                match span_a.content.as_ref() {
                    // could've been added by ansi_to_tui during escaping, uncertain
                    maybe_replaced @ "�"
                        if matches!(lossy_flavor, LossyFlavor::ReplacementChar(_)) =>
                    {
                        if determine_if_visible(&span_a.content, &span_b.content, unconsumed) {
                            visibility.visible_text(&span_a.content, unconsumed, &mut cursor);
                        } else {
                            let utf8_chunk = unconsumed.utf8_chunks().next().unwrap();
                            visibility.replaced(utf8_chunk.invalid().len(), maybe_replaced.len(), &mut cursor);
                        }
                    }
                    // could've been added by ansi_to_tui during escaping, uncertain
                    maybe_escaped
                        if matches!(lossy_flavor, LossyFlavor::EscapedBytes(_))
                            && maybe_escaped.starts_with("\\x")
                            // && maybe_escaped.len() == 4
                            // && maybe_escaped[2..]
                            // .chars()
                            // .all(|c| c.is_ascii_hexdigit() && c.is_uppercase())
                            =>
                    {
                        if determine_if_visible(&span_a.content, &span_b.content, unconsumed) {
                            visibility.visible_text(&span_a.content, unconsumed, &mut cursor);
                        } else {
                            let utf8_chunk = unconsumed.utf8_chunks().next().unwrap();
                            visibility.replaced(
                                utf8_chunk.invalid().len(),
                                maybe_escaped.len(),
                                &mut cursor,
                            );
                        }
                    }

                    exists_within_orig => {
                        visibility.visible_text(exists_within_orig, unconsumed, &mut cursor)
                    }
                }
            }
        }

        // debug!("indexing slice took {:?}", now.elapsed());
        // let now2 = Instant::now();

        // let rope = builder.finish();

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
