//! Module for the more generic helper traits I've needed while working on this project

use std::{borrow::Cow, ops::Range};

use bstr::ByteVec;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::buffer::LineEnding;

// use crate::tui::buffer::LineEnding;

/// Trait that provides simple methods to get the last valid index of a collection or slice.
pub trait LastIndex {
    /// Returns the index of the last element in the collection.
    ///
    /// Returns `None` if the collection is empty.
    ///
    /// Under most cirumstances, this should be the only trait function you need to define.
    fn last_index_checked(&self) -> Option<usize>;
    /// Returns `true` if the given index matches the index of the last element in the collection.
    ///
    /// Returns `false` if the index doesn't match, or if the collection is empty.
    fn last_index_eq(&self, index: usize) -> bool {
        if let Some(last_index) = self.last_index_checked() {
            last_index == index
        } else {
            false
        }
    }
    /// Returns `true` if the given index matches or is greater than the index of the last element in the collection.
    ///
    /// Returns `false` if the index doesn't fit either condition, or if the collection is empty.
    fn last_index_eq_or_under(&self, index: usize) -> bool {
        if let Some(last_index) = self.last_index_checked() {
            index >= last_index
        } else {
            false
        }
    }
    /// Returns the index of the last element in the collection.
    ///
    /// **Panics** if the collection is empty.
    fn last_index(&self) -> usize {
        self.last_index_checked()
            .expect("empty collection; no final element exists")
    }
}
impl<T> LastIndex for [T] {
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

pub trait LineMutator {
    fn style_slice(&mut self, range: Range<usize>, style: Style);
    fn censor_slice(&mut self, range: Range<usize>, style: Option<Style>);
    fn remove_slice(&mut self, range: Range<usize>);
}

impl LineMutator for Line<'_> {
    /// ## Panics if range intersects char boundaries or goes out of bounds!
    fn style_slice(&mut self, range: Range<usize>, style: Style) {
        // #[cfg(debug_assertions)]
        // debug!("Styling {range:?} with {style:?}");
        let spans = &mut self.spans;
        let mut current = 0;
        for (index, span) in spans.iter_mut().enumerate() {
            let span_len = span.content.len();
            let span_start = current;
            let span_end = current + span_len;

            let (overlap_start, overlap_end) =
                overlap_region((span_start, span_end), (range.start, range.end));

            if let Some((overlap_start, overlap_end)) = overlap_start.zip(overlap_end) {
                if overlap_start < overlap_end {
                    // This span intersects the range. May need to split span.
                    let offset_start = overlap_start - span_start;
                    let offset_end = overlap_end - span_start;

                    if offset_start == 0 && offset_end == span_len {
                        // Entire span is inside range, style whole span.
                        span.style = style;
                    } else {
                        let (pre, mid, post) =
                            split_span_content(&span.content, offset_start, offset_end);
                        let orig_style = span.style;

                        let mut new_spans = Vec::new();
                        // TODO try to borrow again if already borrowed
                        // naive attempt with cow:borrowed didn't work due to borrow checkin' nonsense
                        if !pre.is_empty() {
                            new_spans.push(Span::styled(Cow::Owned(pre.to_string()), orig_style));
                        }
                        if !mid.is_empty() {
                            new_spans.push(Span::styled(Cow::Owned(mid.to_string()), style));
                        }
                        if !post.is_empty() {
                            new_spans.push(Span::styled(Cow::Owned(post.to_string()), orig_style));
                        }
                        spans.splice(index..=index, new_spans);
                        // Because we changed the spans vector structure, any reference to span is invalid,
                        // so bail out of this function. A re-call would be necessary if the styling range
                        // covers multiple, non-contiguous spans that require additional splitting or updates;
                        // after this early return, the caller should call the function again to complete
                        // styling the full range if needed.
                        return;
                    }
                }
            }
            current += span_len;
        }
    }
    fn censor_slice(&mut self, range: Range<usize>, style: Option<Style>) {
        // #[cfg(debug_assertions)]
        // debug!("Censoring {range:?} with style {style:?}");
        let spans = &mut self.spans;
        let mut current = 0;
        for (index, span) in spans.iter_mut().enumerate() {
            let span_len = span.content.len();
            let span_start = current;
            let span_end = current + span_len;

            let (overlap_start, overlap_end) =
                overlap_region((span_start, span_end), (range.start, range.end));

            if let Some((overlap_start, overlap_end)) = overlap_start.zip(overlap_end) {
                if overlap_start < overlap_end {
                    // This span is at least partially in the censor range.
                    let offset_start = overlap_start - span_start;
                    let offset_end = overlap_end - span_start;

                    let (pre, mid, post) =
                        split_span_content(&span.content, offset_start, offset_end);
                    let orig_style = span.style;

                    let mut new_spans = Vec::new();

                    if !pre.is_empty() {
                        new_spans.push(Span::styled(Cow::Owned(pre.to_string()), orig_style));
                    }
                    if !mid.is_empty() {
                        // Actually perform the censorship: replace internal bytes with '*'
                        // (bytes instead of chars due to us working on the byte-scale, and we might replace
                        // a multi-byte char)
                        let bullet = "*";
                        for _ in 0..mid.len() {
                            new_spans.push(Span::styled(
                                Cow::Borrowed(bullet),
                                style.unwrap_or(orig_style),
                            ));
                        }
                    }
                    if !post.is_empty() {
                        new_spans.push(Span::styled(Cow::Owned(post.to_string()), orig_style));
                    }

                    spans.splice(index..=index, new_spans);
                    // As above, because we changed the spans, exit so that a caller can re-call over remaining regions if needed
                    return;
                }
            }
            current += span_len;
        }
    }
    fn remove_slice(&mut self, range: Range<usize>) {
        // #[cfg(debug_assertions)]
        // debug!("Removing slice {:?}", range);
        let spans = &mut self.spans;
        let mut current = 0;
        for (index, span) in spans.iter_mut().enumerate() {
            let span_len = span.content.len();
            let span_start = current;
            let span_end = current + span_len;

            let (overlap_start, overlap_end) =
                overlap_region((span_start, span_end), (range.start, range.end));

            if let Some((overlap_start, overlap_end)) = overlap_start.zip(overlap_end) {
                if overlap_start < overlap_end {
                    // This span is at least partly in the removal range.
                    let offset_start = overlap_start - span_start;
                    let offset_end = overlap_end - span_start;

                    let (pre, _mid, post) =
                        split_span_content(&span.content, offset_start, offset_end);
                    let orig_style = span.style;

                    let mut new_spans = Vec::new();
                    if !pre.is_empty() {
                        new_spans.push(Span::styled(Cow::Owned(pre.to_string()), orig_style));
                    }
                    if !post.is_empty() {
                        new_spans.push(Span::styled(Cow::Owned(post.to_string()), orig_style));
                    }
                    spans.splice(index..=index, new_spans);
                    // We changed spans, exit: to remove the full range, caller must re-call if needed.
                    return;
                }
            }
            current += span_len;
        }
    }
}

/// Compute the overlap (start, end) region between two (start, end) pairs.
/// Both start/end are exclusive bounds.
fn overlap_region(a: (usize, usize), b: (usize, usize)) -> (Option<usize>, Option<usize>) {
    let (a_start, a_end) = a;
    let (b_start, b_end) = b;
    let overlap_start = a_start.max(b_start);
    let overlap_end = a_end.min(b_end);
    if overlap_start < overlap_end {
        (Some(overlap_start), Some(overlap_end))
    } else {
        (None, None)
    }
}

/// Splits the span's content into (pre, mid, post) based on byte offsets.
/// Assumes offset_start/offset_end are on valid char boundaries.
fn split_span_content<'a>(
    content: &'a Cow<'a, str>,
    offset_start: usize,
    offset_end: usize,
) -> (&'a str, &'a str, &'a str) {
    let content = content.as_ref();
    let pre = &content[..offset_start];
    let mid = &content[offset_start..offset_end];
    let post = &content[offset_end..];
    (pre, mid, post)
}

// Not traits, but helpful functions used in a few spots

#[inline]
pub fn interleave<A, B, I>(left: A, right: B) -> impl Iterator<Item = I>
where
    A: Iterator<Item = I>,
    B: Iterator<Item = I>,
    I: Ord,
{
    let mut left = left.peekable();
    let mut right = right.peekable();
    std::iter::from_fn(move || match (left.peek(), right.peek()) {
        (Some(li), Some(ri)) => {
            if li <= ri {
                left.next()
            } else {
                right.next()
            }
        }
        (Some(_), None) => left.next(),
        (None, Some(_)) => right.next(),
        (None, None) => None,
    })
}

#[inline]
pub fn interleave_by<A, B, I, F>(left: A, right: B, mut decider: F) -> impl Iterator<Item = I>
where
    A: Iterator<Item = I>,
    B: Iterator<Item = I>,
    F: FnMut(&I, &I) -> bool,
{
    let mut left = left.peekable();
    let mut right = right.peekable();
    std::iter::from_fn(move || match (left.peek(), right.peek()) {
        (Some(li), Some(ri)) => {
            if decider(li, ri) {
                left.next()
            } else {
                right.next()
            }
        }
        (Some(_), None) => left.next(),
        (None, Some(_)) => right.next(),
        (None, None) => None,
    })
}
