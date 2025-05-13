//! Module for the more generic helper traits I've needed while working on this project

use std::borrow::Cow;

use bstr::ByteVec;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::tui::buffer::LineEnding;

/// Trait that provides simple methods to get the last valid index of a collection or slice.
pub trait LastIndex {
    /// Returns `true` if the given index matches the index of the last element in the collection.
    ///
    /// Returns `false` if the index doesn't match, or if the collection is empty.
    fn last_index_eq(&self, index: usize) -> bool;
    /// Returns `true` if the given index matches or is greater than the index of the last element in the collection.
    ///
    /// Returns `false` if the index doesn't fit either condition, or if the collection is empty.
    fn last_index_eq_or_greater(&self, index: usize) -> bool;
    /// Returns the index of the last element in the collection.
    ///
    /// **Panics** if the collection is empty.
    fn last_index(&self) -> usize {
        self.last_index_checked()
            .expect("empty collection; no final element exists")
    }
    /// Returns the index of the last element in the collection.
    ///
    /// Returns `None` if the collection is empty.
    fn last_index_checked(&self) -> Option<usize>;
}
impl<T> LastIndex for [T] {
    fn last_index_eq(&self, index: usize) -> bool {
        if self.is_empty() {
            false
        } else if index == self.len() - 1 {
            true
        } else {
            false
        }
    }
    fn last_index_eq_or_greater(&self, index: usize) -> bool {
        if self.is_empty() {
            false
        } else if index >= self.len() - 1 {
            true
        } else {
            false
        }
    }
    fn last_index_checked(&self) -> Option<usize> {
        if self.is_empty() {
            None
        } else {
            Some(self.len() - 1)
        }
    }
}

/// Trait that provides a method to check if a `[u8]` ends with the contents of another supplied `[u8]`
pub trait ByteSuffixCheck {
    /// Returns `true` if the collection ends with the supplied byte slice.
    ///
    /// Returns `false` if there's any mismatch, or if the checked collection is shorter than the suffix.
    fn has_byte_suffix(&self, expected: &[u8]) -> bool;
    /// Returns `true` if the collection ends with the supplied line ending.
    ///
    /// Returns `false` if there's any mismatch, if the checked collection is shorter than the suffix, or if the line ending is None.
    fn has_line_ending(&self, line_ending: &LineEnding) -> bool;
}
impl ByteSuffixCheck for [u8] {
    fn has_byte_suffix(&self, expected: &[u8]) -> bool {
        if self.len() < expected.len() {
            false
        } else {
            let start = self.len() - expected.len();

            &self[start..] == expected
        }
    }
    fn has_line_ending(&self, line_ending: &LineEnding) -> bool {
        match line_ending {
            LineEnding::None => false,
            LineEnding::Byte(byte) => self.has_byte_suffix(&[*byte]),
            LineEnding::MultiByte(finder) => self.has_byte_suffix(finder.needle()),
        }
    }
}

/// Trait that provides a single method to get the first `N` "Unicode Scalar Values" from a string slice.
pub trait FirstChars {
    /// Return the first `N` "Unicode Scalar Values" from a string slice.
    fn first_chars(&self, char_count: usize) -> Option<&str>;
}
impl FirstChars for str {
    fn first_chars(&self, char_count: usize) -> Option<&str> {
        let actual_count = self.chars().count();
        // Not enough characters to make a slice
        if actual_count < char_count {
            None
        } else if actual_count == char_count {
            Some(self)
        } else {
            let end = self
                .char_indices()
                .nth(char_count)
                .map(|(i, _)| i)
                .expect("Not enough chars?");
            Some(&self[..end])
        }
    }
}

pub trait LineHelpers<'a> {
    /// Removes all tabs, carriage returns, newlines, and control characters from all spans in the line.
    ///
    /// Any changed spans become owned if they weren't already. Unchanged spans are untouched. (Subject to change.)
    fn remove_unsavory_chars(&mut self);
    /// Returns `true` if either the `Line` or any of it's `Spans` are styled.
    fn is_styled(&self) -> bool;
    /// Returns `true` if no `Spans` exist, or if all `Spans` are also empty.
    fn is_empty(&self) -> bool;
    /// Iterates through all Spans and sets the given style to all.
    fn style_all_spans(&mut self, new_style: Style);
    /// Consumes the `Line` and returns a new one with all `Span`'s styles set to the specified style.
    fn all_spans_styled(self, new_style: Style) -> Line<'a>;
    /// Returns an owned `Line` that borrows from the current line's spans.
    fn new_borrowing(&'a self) -> Line<'a>;
    /// Generates an iterator that creates owned `Span` objects whose content borrows from the original line's spans.
    fn borrowed_spans_iter(&'a self) -> impl DoubleEndedIterator<Item = Span<'a>>;
}

impl<'a> LineHelpers<'a> for Line<'a> {
    fn remove_unsavory_chars(&mut self) {
        self.spans.iter_mut().for_each(|s| {
            let mut new_string = s.content.replace(&['\t', '\n', '\r'][..], "");
            new_string.retain(|c| !c.is_control() && !c.is_ascii_control());
            if s.content != new_string {
                s.content = Cow::Owned(new_string);
            }
        });
    }
    fn is_styled(&self) -> bool {
        if self.style != Style::default() {
            return true;
        }
        for span in &self.spans {
            if span.style != Style::default() {
                return true;
            }
        }
        false
    }
    fn is_empty(&self) -> bool {
        if self.spans.is_empty() {
            return true;
        }
        for span in &self.spans {
            if !span.content.is_empty() {
                return false;
            }
        }
        true
    }
    fn style_all_spans(&mut self, new_style: Style) {
        for span in self.spans.iter_mut() {
            span.style = new_style;
        }
    }
    fn borrowed_spans_iter(&'a self) -> impl DoubleEndedIterator<Item = Span<'a>> {
        self.spans
            .iter()
            .map(|s| Span::styled(Cow::Borrowed(s.content.as_ref()), s.style))
    }
    fn new_borrowing(&self) -> Line<'_> {
        let mut line = Line::from_iter(self.borrowed_spans_iter());
        if self.alignment.is_some() {
            line.alignment = self.alignment;
        }
        line.style = self.style;
        line
    }
    fn all_spans_styled(mut self, new_style: Style) -> Line<'a> {
        self.style_all_spans(new_style);
        self
    }
}

pub trait ToggleBool {
    /// Flips the boolean value in-place, returning the new value.
    fn flip(&mut self) -> bool;
}

impl ToggleBool for bool {
    fn flip(&mut self) -> bool {
        *self = !*self;
        *self
    }
}

pub trait HasEscapedBytes {
    fn has_escaped_bytes(&self) -> bool;
}

impl HasEscapedBytes for str {
    /// Returns `true` only if the given &str contains escaped bytes.
    ///
    /// Note: unescapes whole to check, so isn't the cheapest check yet.
    fn has_escaped_bytes(&self) -> bool {
        // Fast path: if not even a single backslash exists, then bail.
        if memchr::memchr(b'\\', self.as_bytes()).is_none() {
            return false;
        }

        // Otherwise, directly compare the results of a full unescape.

        let unescaped = Vec::unescape_bytes(self);

        unescaped != self.as_bytes()
    }
}
