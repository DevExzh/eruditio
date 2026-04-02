use crate::error::Result;
use std::io::{Read, Write};

use super::book::Book;

/// Reads an ebook from a byte source and produces a `Book`.
pub trait FormatReader: Send + Sync {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book>;
}

/// Writes a `Book` to a byte destination in a specific format.
pub trait FormatWriter: Send + Sync {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()>;
}

/// A transform that takes a `Book` and returns a new, modified `Book`.
///
/// Transforms are applied as a pipeline between reading and writing.
/// Each transform receives an immutable reference and produces a new `Book`,
/// preserving the immutability principle.
pub trait Transform: Send + Sync {
    /// A human-readable name for this transform (for logging/debugging).
    fn name(&self) -> &str;

    /// Applies this transform to a book, returning a new book.
    fn apply(&self, book: &Book) -> Result<Book>;
}
