#![no_main]
use libfuzzer_sys::fuzz_target;

use eruditio::FormatReader;
use eruditio::formats::epub::EpubReader;
use eruditio::formats::mobi::MobiReader;
use eruditio::formats::pdb::PdbReader;
use eruditio::formats::rtf::RtfReader;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    // Each reader must not panic on arbitrary input — errors are fine.
    {
        let mut cursor = Cursor::new(data);
        let _ = MobiReader::new().read_book(&mut cursor);
    }
    {
        let mut cursor = Cursor::new(data);
        let _ = PdbReader::new().read_book(&mut cursor);
    }
    {
        let mut cursor = Cursor::new(data);
        let _ = RtfReader::new().read_book(&mut cursor);
    }
    {
        let mut cursor = Cursor::new(data);
        let _ = EpubReader::new().read_book(&mut cursor);
    }
});
