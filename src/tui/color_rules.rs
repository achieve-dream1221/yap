use std::{path::Path, str::FromStr};

use compact_str::CompactString;
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
                line.style_slice(occurance_idx..occurance_idx + rule_len, Style::from(*color));
            }
        }
        for (reg_rule, color) in &self.regex_words {
            for occurance in reg_rule.regex.find_iter(original) {
                let start = occurance.start();
                let end = occurance.end();
                line.style_slice(start..end, Style::from(*color));
            }
        }
    }
}
