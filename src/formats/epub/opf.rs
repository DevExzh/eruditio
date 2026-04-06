use crate::domain::{
    Guide, GuideReference, GuideType, Manifest, ManifestItem, Metadata, PageProgression, Spine,
    SpineItem,
};
use crate::error::{EruditioError, Result};
use crate::formats::common::xml_utils;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use std::io::{Read, Seek};
use zip::ZipArchive;

/// Data extracted from parsing the OPF package document.
pub struct OpfData {
    pub metadata: Metadata,
    pub manifest: Manifest,
    pub spine: Spine,
    pub guide: Guide,
    /// Manifest ID of the NCX document (from `<spine toc="...">` attribute).
    pub ncx_id: Option<String>,
}

/// Parses the OPF package document from the EPUB archive.
pub fn parse_opf<R: Read + Seek>(archive: &mut ZipArchive<R>, opf_path: &str) -> Result<OpfData> {
    let mut opf_file = archive
        .by_name(opf_path)
        .map_err(|_| EruditioError::Format(format!("OPF file {} not found", opf_path)))?;

    let mut contents = String::new();
    opf_file.read_to_string(&mut contents)?;

    parse_opf_xml(&contents)
}

/// Parses OPF XML content into structured data.
pub(crate) fn parse_opf_xml(xml: &str) -> Result<OpfData> {
    let mut reader = XmlReader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut data = OpfData {
        metadata: Metadata::default(),
        manifest: Manifest::new(),
        spine: Spine::new(),
        guide: Guide::new(),
        ncx_id: None,
    };

    let mut buf = Vec::new();
    let mut section = Section::None;
    let mut current_dc_tag = String::new();
    let mut current_text = String::new();
    // Track cover meta from EPUB2 <meta name="cover" content="..."/>
    let mut cover_meta_id: Option<String> = None;
    // Track the opf:file-as attribute on <dc:creator>
    let mut current_file_as: Option<String> = None;
    // Track the opf:event attribute on <dc:date>
    let mut current_date_event: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "package" => {
                        // Capture the OPF version attribute for roundtrip preservation.
                        if let Some(ver) = xml_utils::get_attribute(e, "version") {
                            data.metadata.extended.insert("opf:version".to_string(), ver);
                        }
                    },
                    "metadata" => section = Section::Metadata,
                    "manifest" => section = Section::Manifest,
                    "spine" => {
                        section = Section::Spine;
                        parse_spine_attrs(e, &mut data);
                    },
                    "guide" => section = Section::Guide,
                    "item" if section == Section::Manifest => {
                        parse_manifest_item(e, &mut data.manifest);
                    },
                    "itemref" if section == Section::Spine => {
                        parse_spine_itemref(e, &mut data.spine);
                    },
                    "reference" if section == Section::Guide => {
                        parse_guide_ref(e, &mut data.guide);
                    },
                    _ if section == Section::Metadata => {
                        current_dc_tag = tag.to_string();
                        current_text.clear();
                        // Track the opf:file-as attribute on <dc:creator>
                        if tag == "creator" {
                            current_file_as = xml_utils::get_attribute(e, "opf:file-as")
                                .or_else(|| xml_utils::get_attribute(e, "file-as"));
                        }
                        // Track the opf:event attribute on <dc:date>
                        if tag == "date" {
                            current_date_event = xml_utils::get_attribute(e, "opf:event")
                                .or_else(|| xml_utils::get_attribute(e, "event"));
                        }
                    },
                    _ => {},
                }
            },
            Ok(Event::Empty(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "item" if section == Section::Manifest => {
                        parse_manifest_item(e, &mut data.manifest);
                    },
                    "itemref" if section == Section::Spine => {
                        parse_spine_itemref(e, &mut data.spine);
                    },
                    "reference" if section == Section::Guide => {
                        parse_guide_ref(e, &mut data.guide);
                    },
                    "meta" if section == Section::Metadata => {
                        parse_meta_element(e, &mut data.metadata, &mut cover_meta_id);
                    },
                    _ => {},
                }
            },
            Ok(Event::Text(ref e)) => {
                if section == Section::Metadata && !current_dc_tag.is_empty() {
                    current_text = xml_utils::bytes_to_string(e.as_ref());
                }
            },
            Ok(Event::End(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "metadata" | "manifest" | "spine" | "guide" => section = Section::None,
                    _ if section == Section::Metadata && !current_dc_tag.is_empty() => {
                        // Store every dc:date element for roundtrip preservation,
                        // regardless of the event attribute.
                        if current_dc_tag == "date" && !current_text.is_empty() {
                            data.metadata.additional_dates.push((
                                current_date_event.clone(),
                                current_text.clone(),
                            ));
                        }
                        // Skip dc:date elements with opf:event="conversion";
                        // don't overwrite an already-set publication_date unless
                        // the new date explicitly has event="publication".
                        let skip = if current_dc_tag == "date" {
                            let event = current_date_event.as_deref();
                            if event == Some("conversion") {
                                true
                            } else if data.metadata.publication_date.is_some()
                                && event != Some("publication")
                            {
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if !skip {
                            apply_dc_metadata(&current_dc_tag, &current_text, &mut data.metadata);
                        }
                        // Capture opf:file-as from the first creator
                        if current_dc_tag == "creator" && data.metadata.author_sort.is_none() {
                            if let Some(ref fa) = current_file_as {
                                data.metadata.author_sort = Some(fa.clone());
                            }
                        }
                        current_date_event = None;
                        current_dc_tag.clear();
                        current_text.clear();
                    },
                    _ => {},
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(EruditioError::Parse(format!("OPF XML error: {}", e))),
            _ => {},
        }
        buf.clear();
    }

    // Apply EPUB2 cover meta: <meta name="cover" content="item-id"/>
    if let Some(cover_id) = cover_meta_id {
        data.metadata.cover_image_id = Some(cover_id);
    }

    // Detect EPUB3 cover-image from manifest properties.
    if data.metadata.cover_image_id.is_none() {
        for item in data.manifest.iter() {
            if item.has_property("cover-image") {
                data.metadata.cover_image_id = Some(item.id.clone());
                break;
            }
        }
    }

    Ok(data)
}

/// Tracks which OPF section the parser is currently inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Metadata,
    Manifest,
    Spine,
    Guide,
}

/// Reads attributes from a `<spine>` element.
fn parse_spine_attrs(e: &quick_xml::events::BytesStart<'_>, data: &mut OpfData) {
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"toc" => data.ncx_id = Some(xml_utils::bytes_to_string(&attr.value)),
            b"page-progression-direction" => {
                data.spine.page_progression_direction = match attr.value.as_ref() {
                    b"rtl" => Some(PageProgression::Rtl),
                    b"ltr" => Some(PageProgression::Ltr),
                    _ => None,
                };
            },
            _ => {},
        }
    }
}

/// Parses a `<manifest><item .../>` element into a `ManifestItem`.
fn parse_manifest_item(e: &quick_xml::events::BytesStart<'_>, manifest: &mut Manifest) {
    let mut id = String::new();
    let mut href = String::new();
    let mut media_type = String::new();
    let mut fallback = None;
    let mut properties = Vec::new();

    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"id" => id = xml_utils::bytes_to_string(&attr.value),
            b"href" => {
                let raw = xml_utils::bytes_to_string(&attr.value);
                href = percent_decode(&raw);
            },
            b"media-type" => media_type = xml_utils::bytes_to_string(&attr.value),
            b"fallback" => fallback = Some(xml_utils::bytes_to_string(&attr.value)),
            b"properties" => {
                let val = xml_utils::bytes_to_string(&attr.value);
                properties = val.split_whitespace().map(String::from).collect();
            },
            _ => {},
        }
    }

    if !id.is_empty() && !href.is_empty() {
        let mut item = ManifestItem::new(id, href, media_type);
        item.fallback = fallback;
        if !properties.is_empty() {
            item = item.with_properties(properties);
        }
        manifest.insert(item);
    }
}

/// Parses a `<spine><itemref .../>` element into a `SpineItem`.
fn parse_spine_itemref(e: &quick_xml::events::BytesStart<'_>, spine: &mut Spine) {
    let mut idref = String::new();
    let mut linear = true;

    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"idref" => idref = xml_utils::bytes_to_string(&attr.value),
            b"linear" => linear = attr.value.as_ref() != b"no",
            _ => {},
        }
    }

    if !idref.is_empty() {
        let item = if linear {
            SpineItem::new(idref)
        } else {
            SpineItem::non_linear(idref)
        };
        spine.push(item);
    }
}

/// Parses a `<guide><reference .../>` element into a `GuideReference`.
fn parse_guide_ref(e: &quick_xml::events::BytesStart<'_>, guide: &mut Guide) {
    let mut ref_type = String::new();
    let mut title = String::new();
    let mut href = String::new();

    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"type" => ref_type = xml_utils::bytes_to_string(&attr.value),
            b"title" => title = xml_utils::bytes_to_string(&attr.value),
            b"href" => {
                let raw = xml_utils::bytes_to_string(&attr.value);
                href = percent_decode(&raw);
            },
            _ => {},
        }
    }

    if !ref_type.is_empty() && !href.is_empty() {
        guide.push(GuideReference {
            ref_type: ref_type.parse().unwrap_or(GuideType::Other(ref_type)),
            title,
            href,
        });
    }
}

/// Processes an EPUB2 `<meta name="..." content="..."/>` element.
fn parse_meta_element(
    e: &quick_xml::events::BytesStart<'_>,
    metadata: &mut Metadata,
    cover_meta_id: &mut Option<String>,
) {
    let mut name = String::new();
    let mut content = String::new();

    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"name" => name = xml_utils::bytes_to_string(&attr.value),
            b"content" => content = xml_utils::bytes_to_string(&attr.value),
            _ => {},
        }
    }

    if name.is_empty() || content.is_empty() {
        return;
    }

    match name.as_str() {
        "cover" => *cover_meta_id = Some(content),
        "calibre:series" => metadata.series = Some(content),
        "calibre:series_index" => {
            metadata.series_index = content.parse::<f64>().ok();
        },
        "calibre:title_sort" => metadata.title_sort = Some(content),
        "calibre:author_link_map" | "calibre:timestamp" => {
            metadata.extended.insert(name, content);
        },
        _ => {
            metadata.extended.insert(name, content);
        },
    }
}

/// Maps a Dublin Core tag name + text value to `Metadata` fields.
fn apply_dc_metadata(tag: &str, text: &str, metadata: &mut Metadata) {
    if text.is_empty() {
        return;
    }
    match tag {
        "title" => metadata.title = Some(text.to_string()),
        "creator" => metadata.authors.push(text.to_string()),
        "language" => metadata.language = Some(text.to_string()),
        "publisher" => metadata.publisher = Some(text.to_string()),
        "identifier" => {
            metadata.identifier = Some(text.to_string());
            // Check for ISBN pattern (10 or 13 digits, optional hyphens).
            let stripped: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
            if stripped.len() == 10 || stripped.len() == 13 {
                metadata.isbn = Some(text.to_string());
            }
        },
        "description" => metadata.description = Some(text.to_string()),
        "subject" => metadata.subjects.push(text.to_string()),
        "rights" => metadata.rights = Some(text.to_string()),
        "date" => {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
                metadata.publication_date = Some(dt.with_timezone(&chrono::Utc));
            } else if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
                metadata.publication_date = date
                    .and_hms_opt(0, 0, 0)
                    .and_then(|ndt| ndt.and_local_timezone(chrono::Utc).single());
            } else if let Ok(year) = text.parse::<i32>() {
                metadata.publication_date = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .and_then(|ndt| ndt.and_local_timezone(chrono::Utc).single());
            }
        },
        _ => {},
    }
}

/// Simple percent-decoding for URL-encoded characters in href attributes.
fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.bytes();

    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo) {
                let hex = [h, l];
                if let Ok(s) = std::str::from_utf8(&hex)
                    && let Ok(byte) = u8::from_str_radix(s, 16)
                {
                    result.push(byte as char);
                    continue;
                }
                // Fallback: keep original sequence.
                result.push('%');
                result.push(h as char);
                result.push(l as char);
            }
        } else {
            result.push(b as char);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_opf() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Jane Author</dc:creator>
    <dc:language>en</dc:language>
    <dc:publisher>Test Press</dc:publisher>
    <dc:identifier id="uid">urn:isbn:9780123456789</dc:identifier>
    <dc:description>A test book for unit tests.</dc:description>
    <dc:subject>Testing</dc:subject>
    <dc:subject>Rust</dc:subject>
    <dc:rights>CC BY 4.0</dc:rights>
    <dc:date>2024-06-15</dc:date>
    <meta name="cover" content="cover-img"/>
    <meta name="calibre:series" content="Test Series"/>
    <meta name="calibre:series_index" content="2.5"/>
  </metadata>
  <manifest>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="chapter2.xhtml" media-type="application/xhtml+xml"/>
    <item id="cover-img" href="images/cover.jpg" media-type="image/jpeg" properties="cover-image"/>
    <item id="style" href="style.css" media-type="text/css"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
  </manifest>
  <spine toc="ncx" page-progression-direction="ltr">
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
    <itemref idref="nav" linear="no"/>
  </spine>
  <guide>
    <reference type="cover" title="Cover" href="chapter1.xhtml"/>
    <reference type="toc" title="Table of Contents" href="nav.xhtml"/>
  </guide>
</package>"#
    }

    #[test]
    fn parses_metadata() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(data.metadata.title.as_deref(), Some("Test Book"));
        assert_eq!(data.metadata.authors, vec!["Jane Author"]);
        assert_eq!(data.metadata.language.as_deref(), Some("en"));
        assert_eq!(data.metadata.publisher.as_deref(), Some("Test Press"));
        assert_eq!(
            data.metadata.description.as_deref(),
            Some("A test book for unit tests.")
        );
        assert_eq!(data.metadata.subjects, vec!["Testing", "Rust"]);
        assert_eq!(data.metadata.rights.as_deref(), Some("CC BY 4.0"));
        assert!(data.metadata.publication_date.is_some());
        assert_eq!(
            data.metadata.isbn.as_deref(),
            Some("urn:isbn:9780123456789")
        );
    }

    #[test]
    fn parses_calibre_metadata() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(data.metadata.series.as_deref(), Some("Test Series"));
        assert_eq!(data.metadata.series_index, Some(2.5));
    }

    #[test]
    fn parses_cover_meta() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(data.metadata.cover_image_id.as_deref(), Some("cover-img"));
    }

    #[test]
    fn parses_manifest_items() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(data.manifest.len(), 6);

        let ch1 = data.manifest.get("ch1").unwrap();
        assert_eq!(ch1.href, "chapter1.xhtml");
        assert_eq!(ch1.media_type, "application/xhtml+xml");

        let cover = data.manifest.get("cover-img").unwrap();
        assert_eq!(cover.href, "images/cover.jpg");
        assert!(cover.has_property("cover-image"));

        let nav = data.manifest.get("nav").unwrap();
        assert!(nav.has_property("nav"));
    }

    #[test]
    fn parses_spine() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(data.spine.len(), 3);
        assert_eq!(data.ncx_id.as_deref(), Some("ncx"));
        assert_eq!(
            data.spine.page_progression_direction,
            Some(PageProgression::Ltr)
        );

        let items: Vec<_> = data.spine.iter().collect();
        assert_eq!(items[0].manifest_id, "ch1");
        assert!(items[0].linear);
        assert_eq!(items[1].manifest_id, "ch2");
        assert!(items[1].linear);
        assert_eq!(items[2].manifest_id, "nav");
        assert!(!items[2].linear);
    }

    #[test]
    fn parses_guide() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(data.guide.references.len(), 2);

        let cover_ref = data.guide.find(&GuideType::Cover).unwrap();
        assert_eq!(cover_ref.title, "Cover");
        assert_eq!(cover_ref.href, "chapter1.xhtml");

        let toc_ref = data.guide.find(&GuideType::Toc).unwrap();
        assert_eq!(toc_ref.title, "Table of Contents");
    }

    #[test]
    fn percent_decode_works() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("no%2Fslash"), "no/slash");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn linear_items_count() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        let linear: Vec<_> = data.spine.linear_items().collect();
        assert_eq!(linear.len(), 2);
    }

    #[test]
    fn parses_opf_file_as_attribute() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Test</dc:title>
    <dc:creator opf:file-as="Author, Jane">Jane Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert_eq!(data.metadata.authors, vec!["Jane Author"]);
        assert_eq!(
            data.metadata.author_sort.as_deref(),
            Some("Author, Jane"),
            "author_sort should be parsed from opf:file-as"
        );
    }

    #[test]
    fn parses_bare_file_as_attribute() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title>
    <dc:creator file-as="Doe, John">John Doe</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert_eq!(data.metadata.authors, vec!["John Doe"]);
        assert_eq!(
            data.metadata.author_sort.as_deref(),
            Some("Doe, John"),
            "author_sort should be parsed from bare file-as attribute"
        );
    }

    #[test]
    fn no_author_sort_when_file_as_absent() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert!(
            data.metadata.author_sort.is_none(),
            "author_sort should be None when no file-as attribute exists"
        );
    }

    #[test]
    fn file_as_only_from_first_creator() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Test</dc:title>
    <dc:creator opf:file-as="Author, First">First Author</dc:creator>
    <dc:creator opf:file-as="Author, Second">Second Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert_eq!(data.metadata.authors.len(), 2);
        assert_eq!(
            data.metadata.author_sort.as_deref(),
            Some("Author, First"),
            "author_sort should come from the first creator only"
        );
    }

    #[test]
    fn publication_date_preferred_over_conversion_date() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:date opf:event="publication">2008-06-27</dc:date>
    <dc:date opf:event="conversion">2026-03-01T08:32:03.786809+00:00</dc:date>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        let date = data.metadata.publication_date.expect("publication_date should be set");
        assert_eq!(date.format("%Y-%m-%d").to_string(), "2008-06-27",
            "publication_date should be the publication date, not the conversion date");
    }

    #[test]
    fn conversion_only_date_is_ignored() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:date opf:event="conversion">2026-03-01T08:32:03.786809+00:00</dc:date>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert!(data.metadata.publication_date.is_none(),
            "publication_date should not be set when only a conversion date is present");
    }

    #[test]
    fn date_without_event_attribute_still_parsed() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:date>2024-01-15</dc:date>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        let date = data.metadata.publication_date.expect("publication_date should be set");
        assert_eq!(date.format("%Y-%m-%d").to_string(), "2024-01-15",
            "dc:date without opf:event should still be parsed as publication_date");
    }

    #[test]
    fn publication_date_wins_regardless_of_order() {
        // Conversion date appears BEFORE the publication date
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:date opf:event="conversion">2026-03-01T08:32:03.786809+00:00</dc:date>
    <dc:date opf:event="publication">2008-06-27</dc:date>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        let date = data.metadata.publication_date.expect("publication_date should be set");
        assert_eq!(date.format("%Y-%m-%d").to_string(), "2008-06-27",
            "publication_date should be set from event=publication regardless of element order");
    }

    #[test]
    fn parses_opf_version_attribute() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert_eq!(
            data.metadata.extended.get("opf:version").map(|s| s.as_str()),
            Some("2.0"),
            "OPF version should be captured from the <package> element"
        );
    }

    #[test]
    fn parses_opf_version_3() {
        let data = parse_opf_xml(sample_opf()).unwrap();
        assert_eq!(
            data.metadata.extended.get("opf:version").map(|s| s.as_str()),
            Some("3.0"),
            "OPF version 3.0 should be captured from the sample OPF"
        );
    }

    #[test]
    fn additional_dates_capture_all_dc_date_elements() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:date opf:event="publication">2008-06-27</dc:date>
    <dc:date opf:event="conversion">2026-03-01T08:32:03.786809+00:00</dc:date>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert_eq!(data.metadata.additional_dates.len(), 2,
            "Both dc:date elements should be stored in additional_dates");
        assert_eq!(data.metadata.additional_dates[0],
            (Some("publication".to_string()), "2008-06-27".to_string()));
        assert_eq!(data.metadata.additional_dates[1],
            (Some("conversion".to_string()), "2026-03-01T08:32:03.786809+00:00".to_string()));
    }

    #[test]
    fn additional_dates_captures_date_without_event() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:date>2024-01-15</dc:date>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;
        let data = parse_opf_xml(xml).unwrap();
        assert_eq!(data.metadata.additional_dates.len(), 1);
        assert_eq!(data.metadata.additional_dates[0],
            (None, "2024-01-15".to_string()));
    }
}
