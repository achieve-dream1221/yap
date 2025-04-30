use std::collections::BTreeSet;

pub struct Macros {
    pub macros: BTreeSet<Macro>,
}

impl Macros {
    pub fn new() -> Self {
        Self {
            macros: BTreeSet::from([
                Macro::new_string("Mrow!", "mrow", None),
                Macro::new_string("Get Version", "version", None),
                Macro::new_bytes("Backspace", "\x08".as_bytes().into(), None),
            ]),
        }
    }
    pub fn len(&self) -> usize {
        self.macros.len()
    }
}

#[derive(Debug)]
pub struct Macro {
    title: String,
    keybinding: Option<u8>,
    content: MacroContent,
}

// Custom Eq+Ord impls to avoid checking `content` when sorting.
impl PartialEq for Macro {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title && self.keybinding == other.keybinding
    }
}

impl Eq for Macro {}

impl PartialOrd for Macro {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.title.partial_cmp(&other.title) {
            Some(std::cmp::Ordering::Equal) => self.keybinding.partial_cmp(&other.keybinding),
            ord => ord,
        }
    }
}

impl Ord for Macro {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.title.cmp(&other.title) {
            std::cmp::Ordering::Equal => self.keybinding.cmp(&other.keybinding),
            ord => ord,
        }
    }
}

#[derive(Debug)]
pub enum MacroContent {
    Empty,
    Text(String),
    Bytes(Vec<u8>),
}

impl Macro {
    pub fn new_bytes<T: AsRef<str>>(title: T, bytes: Vec<u8>, keybinding: Option<u8>) -> Self {
        Self {
            title: title.as_ref().into(),
            content: MacroContent::Bytes(bytes),
            keybinding,
        }
    }
    pub fn new_string<T: AsRef<str>, S: AsRef<str>>(
        title: T,
        s: S,
        keybinding: Option<u8>,
    ) -> Self {
        Self {
            title: title.as_ref().into(),
            content: MacroContent::Text(s.as_ref().into()),
            keybinding,
        }
    }
}
