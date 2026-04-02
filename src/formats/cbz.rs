use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use mime_guess::from_path;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use std::io::{Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// CBZ (Comic Book Zip) format reader.
#[derive(Default)]
pub struct CbzReader;

impl CbzReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for CbzReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        let cursor = std::io::Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)?;

        let mut book = Book::new();
        let mut image_files = Vec::new();

        // Collect all image files
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i)
                && file.is_file()
            {
                let name = file.name().to_string();
                let mime = from_path(&name).first_or_octet_stream();
                if mime.type_() == "image" {
                    image_files.push(name);
                }
            }
        }

        // Sort image files alphabetically to determine page order
        image_files.sort();

        // Read images into resources and create chapters (pages)
        for (index, name) in image_files.iter().enumerate() {
            let mut file = archive
                .by_name(name)
                .map_err(|_| EruditioError::Format(format!("Missing file: {}", name)))?;
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;

            let media_type = from_path(name)
                .first()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into());
            let resource_id = format!("page_{:04}", index);
            let chapter_id = format!("chapter_{:04}", index);

            book.add_resource(&resource_id, name, data, &media_type);

            // In CBZ, each image is typically a page.
            book.add_chapter(&Chapter {
                title: Some(format!("Page {}", index + 1)),
                content: format!("<img src=\"{}\" alt=\"Page {}\" />", resource_id, index + 1),
                id: Some(chapter_id),
            });
        }

        // Try to parse ComicInfo.xml for metadata.
        let comic_info_name = (0..archive.len())
            .filter_map(|i| {
                let file = archive.by_index(i).ok()?;
                let name = file.name().to_string();
                if name.eq_ignore_ascii_case("comicinfo.xml") {
                    Some(name)
                } else {
                    None
                }
            })
            .next();

        if let Some(ref ci_name) = comic_info_name
            && let Ok(mut file) = archive.by_name(ci_name)
        {
            let mut xml = String::new();
            if file.read_to_string(&mut xml).is_ok() {
                parse_comic_info(&xml, &mut book);
            }
        }

        if book.metadata.title.is_none() {
            book.metadata.title = Some("Unknown Comic".into());
        }

        Ok(book)
    }
}

/// Parses ComicInfo.xml and populates book metadata.
fn parse_comic_info(xml: &str, book: &mut Book) {
    let mut reader = XmlReader::from_str(xml);
    let mut current_tag = String::new();
    let mut series = String::new();
    let mut number = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                current_tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
            },
            Ok(Event::Text(ref e)) => {
                let text = String::from_utf8_lossy(&e.clone().into_inner()).into_owned();
                if text.trim().is_empty() {
                    continue;
                }
                match current_tag.as_str() {
                    "Title" => {
                        book.metadata.title = Some(text);
                    },
                    "Writer" | "Penciller" => {
                        if book.metadata.authors.is_empty()
                            || !book.metadata.authors.contains(&text)
                        {
                            book.metadata.authors.push(text);
                        }
                    },
                    "Series" => series = text,
                    "Number" => number = text,
                    "Summary" => {
                        book.metadata.description = Some(text);
                    },
                    "Publisher" => {
                        book.metadata.publisher = Some(text);
                    },
                    "LanguageISO" => {
                        book.metadata.language = Some(text);
                    },
                    "Year" => {
                        book.metadata.extended.entry("year".into()).or_insert(text);
                    },
                    "Genre" => {
                        for genre in text.split(',') {
                            let g = genre.trim().to_string();
                            if !g.is_empty() {
                                book.metadata.subjects.push(g);
                            }
                        }
                    },
                    _ => {},
                }
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }

    // If we have series info but no explicit title, build one.
    if !series.is_empty() && book.metadata.title.is_none() {
        let title = if number.is_empty() {
            series
        } else {
            format!("{} #{}", series, number)
        };
        book.metadata.title = Some(title);
    }
}

/// CBZ format writer.
#[derive(Default)]
pub struct CbzWriter;

impl CbzWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for CbzWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        // ZIP creation requires Seek, so buffer into a Cursor first.
        let mut cursor = std::io::Cursor::new(Vec::new());
        write_cbz(book, &mut cursor)?;
        writer.write_all(cursor.get_ref())?;
        Ok(())
    }
}

fn write_cbz<W: Write + std::io::Seek>(book: &Book, writer: W) -> Result<()> {
    let mut zip = ZipWriter::new(writer);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // Collect image resources, sorted by href for deterministic page order.
    let mut images: Vec<_> = book
        .manifest
        .iter()
        .filter(|item| item.media_type.starts_with("image/"))
        .filter_map(|item| {
            let data = item.data.as_bytes()?;
            Some((&item.href, data))
        })
        .collect();
    images.sort_by_key(|(href, _)| href.as_str());

    if images.is_empty() {
        return Err(EruditioError::Format(
            "No image resources found in book for CBZ output".into(),
        ));
    }

    for (href, data) in &images {
        zip.start_file(href.as_str(), options)?;
        zip.write_all(data)?;
    }

    zip.finish()?;

    Ok(())
}
