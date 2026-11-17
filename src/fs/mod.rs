//! Filesystem model: entries, listing, sorting, metadata formatting and file operations.
//! Free of UI types so it can be unit-tested directly.

pub mod entry;
pub mod listing;
pub mod meta;
pub mod ops;
pub mod path;
pub mod sort;
pub mod trash;

pub use entry::{DirCount, Entry, EntryKind, FileClass};
pub use listing::{DirListing, ListError, count_entries, list_dir};
pub use ops::PasteMode;
pub use sort::SortKey;
