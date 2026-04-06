use crate::domain::TocItem;
use crate::error::{EruditioError, Result};
use crate::formats::common::xml_utils;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;

/// Parses an NCX document's `<navMap>` into a hierarchical list of `TocItem`s.
pub fn parse_ncx(xml: &str) -> Result<Vec<TocItem>> {
    let mut reader = XmlReader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut in_nav_map = false;

    // Stack tracks the nesting of navPoint elements.
    // Each entry is a partially-built TocItem.
    let mut stack: Vec<TocItem> = Vec::new();
    let mut roots: Vec<TocItem> = Vec::new();

    // State within a single navPoint.
    let mut in_nav_label = false;
    let mut collecting_text = false;
    let mut current_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "navMap" => in_nav_map = true,
                    "navPoint" if in_nav_map => {
                        let mut id = None;
                        let mut play_order = None;

                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"id" => id = Some(xml_utils::bytes_to_string(&attr.value)),
                                b"playOrder" => {
                                    play_order = std::str::from_utf8(&attr.value)
                                        .ok()
                                        .and_then(|s| s.parse::<u32>().ok());
                                },
                                _ => {},
                            }
                        }

                        let mut item = TocItem::new("", "");
                        if let Some(id) = id {
                            item = item.with_id(id);
                        }
                        if let Some(order) = play_order {
                            item = item.with_play_order(order);
                        }
                        stack.push(item);
                    },
                    "navLabel" if in_nav_map => in_nav_label = true,
                    "text" if in_nav_label => {
                        collecting_text = true;
                        current_text.clear();
                    },
                    "content" if in_nav_map && !stack.is_empty() => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"src" {
                                let src = xml_utils::bytes_to_string(&attr.value);
                                if let Some(item) = stack.last_mut() {
                                    item.href = src;
                                }
                            }
                        }
                    },
                    _ => {},
                }
            },
            Ok(Event::Empty(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                if tag == "content" && in_nav_map && !stack.is_empty() {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"src" {
                            let src = xml_utils::bytes_to_string(&attr.value);
                            if let Some(item) = stack.last_mut() {
                                item.href = src;
                            }
                        }
                    }
                }
            },
            Ok(Event::Text(ref e)) => {
                if collecting_text {
                    match std::str::from_utf8(e.as_ref()) {
                        Ok(s) => current_text.push_str(s),
                        Err(_) => current_text.push_str(&String::from_utf8_lossy(e.as_ref())),
                    }
                }
            },
            Ok(Event::End(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "navMap" => in_nav_map = false,
                    "navPoint" if in_nav_map => {
                        if let Some(finished) = stack.pop() {
                            if let Some(parent) = stack.last_mut() {
                                parent.children.push(finished);
                            } else {
                                roots.push(finished);
                            }
                        }
                    },
                    "navLabel" => in_nav_label = false,
                    "text" if in_nav_label => {
                        collecting_text = false;
                        if let Some(item) = stack.last_mut() {
                            item.title = std::mem::take(&mut current_text);
                        }
                    },
                    _ => {},
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(EruditioError::Parse(format!("NCX XML error: {}", e))),
            _ => {},
        }
        buf.clear();
    }

    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ncx() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="test-uid"/>
    <meta name="dtb:depth" content="2"/>
  </head>
  <docTitle><text>Test Book</text></docTitle>
  <navMap>
    <navPoint id="np-1" playOrder="1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="chapter1.xhtml"/>
    </navPoint>
    <navPoint id="np-2" playOrder="2">
      <navLabel><text>Chapter 2</text></navLabel>
      <content src="chapter2.xhtml"/>
      <navPoint id="np-3" playOrder="3">
        <navLabel><text>Section 2.1</text></navLabel>
        <content src="chapter2.xhtml#s1"/>
      </navPoint>
      <navPoint id="np-4" playOrder="4">
        <navLabel><text>Section 2.2</text></navLabel>
        <content src="chapter2.xhtml#s2"/>
      </navPoint>
    </navPoint>
    <navPoint id="np-5" playOrder="5">
      <navLabel><text>Chapter 3</text></navLabel>
      <content src="chapter3.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#
    }

    #[test]
    fn parses_flat_nav_points() {
        let toc = parse_ncx(sample_ncx()).unwrap();
        assert_eq!(toc.len(), 3);
        assert_eq!(toc[0].title, "Chapter 1");
        assert_eq!(toc[0].href, "chapter1.xhtml");
        assert_eq!(toc[0].id.as_deref(), Some("np-1"));
        assert_eq!(toc[0].play_order, Some(1));
    }

    #[test]
    fn parses_nested_nav_points() {
        let toc = parse_ncx(sample_ncx()).unwrap();
        assert_eq!(toc[1].title, "Chapter 2");
        assert_eq!(toc[1].children.len(), 2);
        assert_eq!(toc[1].children[0].title, "Section 2.1");
        assert_eq!(toc[1].children[0].href, "chapter2.xhtml#s1");
        assert_eq!(toc[1].children[1].title, "Section 2.2");
    }

    #[test]
    fn play_order_is_parsed() {
        let toc = parse_ncx(sample_ncx()).unwrap();
        assert_eq!(toc[2].play_order, Some(5));
    }

    #[test]
    fn empty_ncx_returns_empty() {
        let xml = r#"<?xml version="1.0"?><ncx><navMap></navMap></ncx>"#;
        let toc = parse_ncx(xml).unwrap();
        assert!(toc.is_empty());
    }

    #[test]
    fn total_items_match() {
        let toc = parse_ncx(sample_ncx()).unwrap();
        let total: usize = toc.iter().map(|t| t.count()).sum();
        assert_eq!(total, 5);
    }
}
