//! Filesystem model: entries, listing, sorting and metadata formatting.
//! Free of UI types so it can be unit-tested directly.

pub mod entry;
pub mod listing;
pub mod meta;
pub mod sort;

pub use entry::{DirCount, Entry, EntryKind, FileClass};
pub use listing::{DirListing, ListError, count_entries, list_dir};
pub use sort::SortKey;
