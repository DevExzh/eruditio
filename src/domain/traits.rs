use crate::error::Result;
use crate::pipeline::load_filter::LoadFilter;
use std::io::{Read, Write};

use super::book::Book;

/// Reads an ebook from a byte source and produces a `Book`.
pub trait FormatReader: Send + Sync {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book>;

    /// Reads a book, honouring `filter` to skip resource categories the
    /// output format does not need.
    ///
    /// The default implementation ignores the filter and delegates to
    /// [`read_book`](Self::read_book), so existing readers work unchanged.
    fn read_book_filtered(&self, reader: &mut dyn Read, _filter: LoadFilter) -> Result<Book> {
        self.read_book(reader)
    }
}

/// Writes a `Book` to a byte destination in a specific format.
pub trait FormatWriter: Send + Sync {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()>;
}

/// A transform that takes a `Book` by value and returns a (possibly modified) `Book`.
///
/// Transforms are applied as a pipeline between reading and writing.
/// Taking ownership avoids cloning the entire book (including binary
/// resources) at every pipeline step — transforms that need no changes
/// simply return the input unchanged at zero cost.
pub trait Transform: Send + Sync {
    /// A human-readable name for this transform (for logging/debugging).
    fn name(&self) -> &str;

    /// Applies this transform to a book, returning the (possibly modified) book.
    fn apply(&self, book: Book) -> Result<Book>;
}
