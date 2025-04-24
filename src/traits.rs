//! Module for the more generic helper traits I've needed while working on this project

use ratatui::{style::Stylize, text::Line};

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
    /// Returns `0` if the collection is empty.
    fn last_index(&self) -> usize {
        self.last_index_checked().unwrap_or(0)
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
}

/// Trait that provides a single method to get the first `N` "Unicode Scalar Values" from a string slice.
trait FirstChars {
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

pub trait RemoveUnsavory {
    fn remove_unsavory_chars(&mut self);
}

impl RemoveUnsavory for Line<'_> {
    fn remove_unsavory_chars(&mut self) {
        self.spans.iter_mut().for_each(|s| {
            // let std::borrow::Cow::Owned(_) = &s.content else {
            //     panic!()
            // };

            let mut new_string = s.content.replace(&['\t', '\n', '\r'][..], "");
            new_string.retain(|c| !c.is_control() && !c.is_ascii_control());
            s.content = std::borrow::Cow::Owned(new_string);
        });
    }
}
