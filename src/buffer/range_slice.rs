use std::ops::Range;

#[derive(Debug, Clone)]
pub struct RangeSlice<'a> {
    pub range: Range<usize>,
    pub slice: &'a [u8],
}

impl<'a> AsRef<[u8]> for RangeSlice<'a> {
    fn as_ref(&self) -> &[u8] {
        self.slice
    }
}

impl<'a> RangeSlice<'a> {
    /// Create a [`RangeSlice`] from a parent buffer (likely a `Vec<u8>`)
    /// and a child slice (`&[u8]`) from within the parent buffer,
    /// populating the `range` field with the child slice's start and end indices.
    ///
    /// # Safety
    /// `child` **must** be a subslice of `parent`, i.e. both slices come from the same
    /// allocation and `child` lies **entirely** within `parent`.
    ///
    /// # Panics
    ///
    /// Debug Builds: Panics if the child slice is empty, larger than the parent,
    /// or if the child's pointers do not lie within the parent's pointer bounds.
    ///
    /// Release Builds: Assertions skipped.
    pub unsafe fn from_parent_and_child(parent: &'a [u8], child: &'a [u8]) -> Self {
        // Fail-fast checks
        debug_assert!(!parent.is_empty());
        debug_assert!(!child.is_empty());
        debug_assert!(
            child.len() <= parent.len(),
            "child can't be larger than parent"
        );
        let parent_range = parent.as_ptr_range();
        let child_range = child.as_ptr_range();

        // Ensure child pointers lies within parent pointer bounds
        debug_assert!(
            child_range.start >= parent_range.start,
            "child_range.start must be >= parent_range.start",
        );
        debug_assert!(
            child_range.end <= parent_range.end,
            "child_range.end must be <= parent_range.end",
        );

        // Getting difference between pointers in T-sized chunks.
        //
        // SAFETY:
        //      Trivial to ensure safety, as long as documented
        //      precondition of ensuring `child` is always a
        //      subslice of `parent`.
        let offset = unsafe { child_range.start.offset_from(parent_range.start) } as usize;

        Self {
            range: offset..offset + child.len(),
            slice: child,
        }
    }
}

impl<'a> std::fmt::Display for RangeSlice<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{start}..{end}",
            start = self.range.start,
            end = self.range.end + 1
        )
    }
}
