use crate::{
    bstr::{BStr, BString, ByteSlice},
    tree, Tree, TreeRef,
};
use std::cell::RefCell;
use std::cmp::Ordering;

///
pub mod editor;

mod ref_iter;
///
pub mod write;

/// The state needed to apply edits instantly to in-memory trees.
///
/// It's made so that each tree is looked at in the object database at most once, and held in memory for
/// all edits until everything is flushed to write all changed trees.
///
/// The editor is optimized to edit existing trees, but can deal with building entirely new trees as well
/// with some penalties.
#[doc(alias = "TreeUpdateBuilder", alias = "git2")]
#[derive(Clone)]
pub struct Editor<'a> {
    /// A way to lookup trees.
    find: &'a dyn crate::FindExt,
    /// The kind of hashes to produce
    object_hash: gix_hash::Kind,
    /// All trees we currently hold in memory. Each of these may change while adding and removing entries.
    /// null-object-ids mark tree-entries whose value we don't know yet, they are placeholders that will be
    /// dropped when writing at the latest.
    trees: std::collections::HashMap<BString, Tree>,
    /// A buffer to build up paths when finding the tree to edit.
    path_buf: RefCell<BString>,
    /// Our buffer for storing tree-data in, right before decoding it.
    tree_buf: Vec<u8>,
}

/// Parse a valid git mode into a u16
/// A valid git mode can be represented by a set of 5-6 octal digits. The leftmost octal digit can
/// be at most one. These conditions guarantee that it can be represented as a `u16`, although that
/// representation is lossy compared to the byte slice it came from as `"040000"` and `"40000"` will
/// both be represented as `0o40000`.
/// Input:
///     We accept input that contains exactly a valid git mode or a valid git mode followed by a
///     space, then anything (in case we are just pointing in the memory from the Git Tree
///     representation)
/// Return value:
///     The value (`u16`) given a valid input or `None` otherwise
pub const fn parse_git_mode(i: &[u8]) -> Option<u16> {
    let mut mode = 0;
    let mut idx = 0;
    // const fn, this is why we can't have nice things (like `.iter().any()`)
    while idx < i.len() {
        let b = i[idx];
        // Delimiter, return what we got
        if b == b' ' {
            return Some(mode);
        }
        // Not a pure octal input
        if b < b'0' || b > b'7' {
            return None;
        }
        // More than 6 octal digits we must have hit the delimiter or the input was malformed
        if idx > 6 {
            return None;
        }
        mode = (mode << 3) + (b - b'0') as u16;
        idx += 1;
    }
    Some(mode)
}

/// Just a place-holder until the `slice_split_once` feature stabilizes
fn split_once(slice: &[u8], value: u8) -> Option<(&'_ [u8], &'_ [u8])> {
    let mut iterator = slice.splitn(2, |b| *b == value);
    let first = iterator.next();
    let second = iterator.next();
    first.and_then(|first| second.map(|second| (first, second)))
}

/// From the slice we get from a Git Tree representation, extract and parse the "mode" part
/// Return:
///   * `Some((mode as u16, (mode as slice, rest as slice)))` if the input slice is valid
///   * `None` otherwise
#[allow(clippy::type_complexity)]
fn extract_git_mode(i: &[u8]) -> Option<(u16, (&[u8], &[u8]))> {
    if let Some((mode_slice, rest_slice)) = split_once(i, b' ') {
        if rest_slice.is_empty() {
            return None;
        }

        parse_git_mode(mode_slice).map(|mode_num| (mode_num, (mode_slice, rest_slice)))
    } else {
        None
    }
}

impl TryFrom<u32> for tree::EntryMode {
    type Error = u32;

    fn try_from(mode: u32) -> Result<Self, Self::Error> {
        Ok(match mode {
            0o40000 | 0o120000 | 0o160000 => (mode as u16).into(),
            blob_mode if blob_mode & 0o100000 == 0o100000 => (mode as u16).into(),
            _ => return Err(mode),
        })
    }
}

/// The mode of items storable in a tree, similar to the file mode on a unix file system.
///
/// Used in [`mutable::Entry`][crate::tree::Entry].
///
/// Note that even though it can be created from any `u16`, it should be preferable to
/// create it by converting [`EntryKind`] into `EntryMode`.
#[derive(Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntryMode {
    pub(crate) value: u16,
    pub(crate) git_representation: [u8; 6],
}

impl std::fmt::Debug for EntryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntryMode(0o{})", self.as_bstr())
    }
}

impl std::fmt::Octal for EntryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_bstr())
    }
}

/// A discretized version of ideal and valid values for entry modes.
///
/// Note that even though it can represent every valid [mode](EntryMode), it might
/// loose information due to that as well.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Ord, PartialOrd, Hash)]
#[repr(u16)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EntryKind {
    /// A tree, or directory
    Tree = 0o040000u16,
    /// A file that is not executable
    Blob = 0o100644,
    /// A file that is executable
    BlobExecutable = 0o100755,
    /// A symbolic link
    Link = 0o120000,
    /// A commit of a git submodule
    Commit = 0o160000,
}

// Mask away the bottom 12 bits and the top 2 bits
const IFMT: u16 = 0o170000;
const fn parse_entry_kind_from_value(mode: u16) -> EntryKind {
    let etype = mode & IFMT;
    if etype == 0o100000 {
        if mode & 0o000100 == 0o000100 {
            EntryKind::BlobExecutable
        } else {
            EntryKind::Blob
        }
    } else if etype == EntryKind::Link as u16 {
        EntryKind::Link
    } else if etype == EntryKind::Tree as u16 {
        EntryKind::Tree
    } else {
        EntryKind::Commit
    }
}

impl From<u16> for EntryKind {
    fn from(mode: u16) -> Self {
        parse_entry_kind_from_value(mode)
    }
}

impl From<u16> for EntryMode {
    fn from(value: u16) -> Self {
        let kind: EntryKind = value.into();
        Self::from(kind)
    }
}

impl From<EntryMode> for u16 {
    fn from(value: EntryMode) -> Self {
        value.value
    }
}

impl From<EntryMode> for EntryKind {
    fn from(value: EntryMode) -> Self {
        value.kind()
    }
}

/// Serialization
impl EntryKind {
    /// Return the representation as used in the git internal format.
    pub fn as_octal_str(&self) -> &'static BStr {
        self.as_octal_bytes().as_bstr()
    }

    /// Return the representation as used in the git internal format.
    pub fn as_octal_bytes(&self) -> &'static [u8] {
        use EntryKind::*;
        match self {
            Tree => b"40000",
            Blob => b"100644",
            BlobExecutable => b"100755",
            Link => b"120000",
            Commit => b"160000",
        }
    }
    /// Return the representation as a human readable description
    pub fn as_descriptive_str(&self) -> &'static str {
        use EntryKind::*;
        match self {
            Tree => "tree",
            Blob => "blob",
            BlobExecutable => "exe",
            Link => "link",
            Commit => "commit",
        }
    }
}

impl From<EntryKind> for EntryMode {
    fn from(value: EntryKind) -> Self {
        let mut git_representation = [b' '; 6];
        git_representation[..value.as_octal_str().len()].copy_from_slice(value.as_octal_str());
        EntryMode {
            git_representation,
            value: value as u16,
        }
    }
}

impl EntryMode {
    /// Discretize the raw mode into an enum with well-known state while dropping unnecessary details.
    pub const fn kind(&self) -> EntryKind {
        parse_entry_kind_from_value(self.value)
    }

    /// Return true if this entry mode represents the commit of a submodule.
    pub const fn is_commit(&self) -> bool {
        matches!(self.kind(), EntryKind::Commit)
    }

    /// Return true if this entry mode represents a symbolic link
    pub const fn is_link(&self) -> bool {
        matches!(self.kind(), EntryKind::Link)
    }

    /// Return true if this entry mode represents anything BUT Tree/directory
    pub const fn is_tree(&self) -> bool {
        matches!(self.kind(), EntryKind::Tree)
    }

    /// Return true if this entry mode represents anything BUT Tree/directory
    pub const fn is_no_tree(&self) -> bool {
        !matches!(self.kind(), EntryKind::Tree)
    }

    /// Return true if the entry is any kind of blob.
    pub const fn is_blob(&self) -> bool {
        matches!(self.kind(), EntryKind::Blob | EntryKind::BlobExecutable)
    }

    /// Return true if the entry is an executable blob.
    pub const fn is_executable(&self) -> bool {
        matches!(self.kind(), EntryKind::BlobExecutable)
    }

    /// Return true if the entry is any kind of blob or symlink.
    pub const fn is_blob_or_symlink(&self) -> bool {
        matches!(
            self.kind(),
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link
        )
    }

    /// How many bytes of the backing representation are significant?
    pub fn len(&self) -> usize {
        if let Some(delim) = self.git_representation.iter().position(|b| *b == b' ') {
            delim
        } else {
            self.git_representation.len()
        }
    }

    /// Return the representation as used in the git internal format, which is octal and written
    /// to the `backing` buffer. The respective sub-slice that was written to is returned.
    pub fn as_bytes(&self) -> &'_ [u8] {
        &self.git_representation[..self.len()]
    }

    /// Return the representation as used in the git internal format, which is octal and written
    /// to the `backing` buffer. The respective sub-slice that was written to is returned.
    pub fn as_bstr(&self) -> &'_ BStr {
        self.as_bytes().as_bstr()
    }
}

impl TreeRef<'_> {
    /// Convert this instance into its own version, creating a copy of all data.
    ///
    /// This will temporarily allocate an extra copy in memory, so at worst three copies of the tree exist
    /// at some intermediate point in time. Use [`Self::into_owned()`] to avoid this.
    pub fn to_owned(&self) -> Tree {
        self.clone().into()
    }

    /// Convert this instance into its own version, creating a copy of all data.
    pub fn into_owned(self) -> Tree {
        self.into()
    }
}

/// An element of a [`TreeRef`][crate::TreeRef::entries].
#[derive(PartialEq, Eq, Debug, Hash, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntryRef<'a> {
    /// The kind of object to which `oid` is pointing.
    pub mode: tree::EntryMode,
    /// The name of the file in the parent tree.
    pub filename: &'a BStr,
    /// The id of the object representing the entry.
    // TODO: figure out how these should be called. id or oid? It's inconsistent around the codebase.
    //       Answer: make it 'id', as in `git2`
    #[cfg_attr(feature = "serde", serde(borrow))]
    pub oid: &'a gix_hash::oid,
}

impl PartialOrd for EntryRef<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EntryRef<'_> {
    fn cmp(&self, b: &Self) -> Ordering {
        let a = self;
        let common = a.filename.len().min(b.filename.len());
        a.filename[..common].cmp(&b.filename[..common]).then_with(|| {
            let a = a.filename.get(common).or_else(|| a.mode.is_tree().then_some(&b'/'));
            let b = b.filename.get(common).or_else(|| b.mode.is_tree().then_some(&b'/'));
            a.cmp(&b)
        })
    }
}

/// An entry in a [`Tree`], similar to an entry in a directory.
#[derive(PartialEq, Eq, Debug, Hash, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entry {
    /// The kind of object to which `oid` is pointing to.
    pub mode: EntryMode,
    /// The name of the file in the parent tree.
    pub filename: BString,
    /// The id of the object representing the entry.
    pub oid: gix_hash::ObjectId,
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, b: &Self) -> Ordering {
        let a = self;
        let common = a.filename.len().min(b.filename.len());
        a.filename[..common].cmp(&b.filename[..common]).then_with(|| {
            let a = a.filename.get(common).or_else(|| a.mode.is_tree().then_some(&b'/'));
            let b = b.filename.get(common).or_else(|| b.mode.is_tree().then_some(&b'/'));
            a.cmp(&b)
        })
    }
}
