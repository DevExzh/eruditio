use crate::domain::TocItem;
use crate::error::{EruditioError, Result};
use crate::formats::common::xml_utils;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;

/// Parses an EPUB3 navigation document, extracting the TOC from
/// `<nav epub:type="toc">`.
pub fn parse_nav(xhtml: &str) -> Result<Vec<TocItem>> {
    let mut reader = XmlReader::from_str(xhtml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(256);

    let mut in_toc_nav = false;

    // Stack for nested <ol> lists. Each level is a Vec<TocItem>.
    let mut list_stack: Vec<Vec<TocItem>> = Vec::new();

    // Current item being built from <a> element.
    let mut current_href = String::new();
    let mut current_title = String::new();
    let mut in_anchor = false;
    let mut has_current_item = false;
    let mut item_pushed = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "nav" if !in_toc_nav => {
                        if is_toc_nav(e) {
                            in_toc_nav = true;
                        }
                    },
                    "ol" if in_toc_nav => {
                        // If there's a pending item, push it before opening a child list.
                        if has_current_item && !item_pushed {
                            let item = TocItem::new(&current_title, &current_href);
                            if let Some(current_list) = list_stack.last_mut() {
                                current_list.push(item);
                            }
                            item_pushed = true;
                        }
                        list_stack.push(Vec::new());
                    },
                    "a" if in_toc_nav => {
                        in_anchor = true;
                        current_title.clear();
                        current_href.clear();
                        has_current_item = false;
                        item_pushed = false;
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"href" {
                                current_href = xml_utils::bytes_to_string(&attr.value);
                            }
                        }
                    },
                    _ => {},
                }
            },
            Ok(Event::Text(ref e)) => {
                if in_anchor {
                    match std::str::from_utf8(e.as_ref()) {
                        Ok(s) => current_title.push_str(s),
                        Err(_) => current_title.push_str(&String::from_utf8_lossy(e.as_ref())),
                    }
                }
            },
            Ok(Event::End(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                match tag {
                    "nav" if in_toc_nav => {
                        in_toc_nav = false;
                    },
                    "a" if in_toc_nav => {
                        in_anchor = false;
                        has_current_item = true;
                    },
                    "li" if in_toc_nav => {
                        // Push item if it wasn't already pushed (no nested <ol>).
                        if has_current_item && !item_pushed {
                            let item = TocItem::new(&current_title, &current_href);
                            if let Some(current_list) = list_stack.last_mut() {
                                current_list.push(item);
                            }
                        }
                        has_current_item = false;
                        item_pushed = false;
                        current_title.clear();
                        current_href.clear();
                    },
                    "ol" if in_toc_nav => {
                        // Close the current list level.
                        if let Some(children) = list_stack.pop() {
                            if let Some(parent_list) = list_stack.last_mut() {
                                // Attach children to the last item in the parent list.
                                if let Some(parent_item) = parent_list.last_mut() {
                                    parent_item.children = children;
                                }
                            } else {
                                // Top-level list — these are the root items.
                                return Ok(children);
                            }
                        }
                    },
                    _ => {},
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(EruditioError::Parse(format!("Nav XHTML error: {}", e)));
            },
            _ => {},
        }
        buf.clear();
    }

    // If we get here without returning from the <ol> close, collect remaining items.
    if let Some(items) = list_stack.pop() {
        Ok(items)
    } else {
        Ok(Vec::new())
    }
}

/// Checks if a `<nav>` element has `epub:type="toc"`.
fn is_toc_nav(e: &quick_xml::events::BytesStart<'_>) -> bool {
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref();
        // Match both "epub:type" and any namespaced ":type" suffix.
        if (key == b"epub:type" || key.ends_with(b":type"))
            && attr.value.as_ref().windows(3).any(|w| w == b"toc")
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_nav() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Navigation</title></head>
<body>
  <nav epub:type="toc">
    <h2>Table of Contents</h2>
    <ol>
      <li><a href="chapter1.xhtml">Chapter 1</a></li>
      <li><a href="chapter2.xhtml">Chapter 2</a>
        <ol>
          <li><a href="chapter2.xhtml#s1">Section 2.1</a></li>
          <li><a href="chapter2.xhtml#s2">Section 2.2</a></li>
        </ol>
      </li>
      <li><a href="chapter3.xhtml">Chapter 3</a></li>
    </ol>
  </nav>
  <nav epub:type="landmarks">
    <ol>
      <li><a epub:type="bodymatter" href="chapter1.xhtml">Start</a></li>
    </ol>
  </nav>
</body>
</html>"#
    }

    #[test]
    fn parses_flat_entries() {
        let toc = parse_nav(sample_nav()).unwrap();
        assert_eq!(toc.len(), 3);
        assert_eq!(toc[0].title, "Chapter 1");
        assert_eq!(toc[0].href, "chapter1.xhtml");
        assert_eq!(toc[2].title, "Chapter 3");
    }

    #[test]
    fn parses_nested_entries() {
        let toc = parse_nav(sample_nav()).unwrap();
        assert_eq!(toc[1].title, "Chapter 2");
        assert_eq!(toc[1].children.len(), 2);
        assert_eq!(toc[1].children[0].title, "Section 2.1");
        assert_eq!(toc[1].children[0].href, "chapter2.xhtml#s1");
        assert_eq!(toc[1].children[1].title, "Section 2.2");
    }

    #[test]
    fn ignores_non_toc_nav() {
        let toc = parse_nav(sample_nav()).unwrap();
        let total: usize = toc.iter().map(|t| t.count()).sum();
        assert_eq!(total, 5); // 3 top-level + 2 children
    }

    #[test]
    fn empty_nav_returns_empty() {
        let xhtml = r#"<html><body><nav epub:type="toc"><ol></ol></nav></body></html>"#;
        let toc = parse_nav(xhtml).unwrap();
        assert!(toc.is_empty());
    }

    #[test]
    fn missing_toc_nav_returns_empty() {
        let xhtml = r#"<html><body><p>No nav here</p></body></html>"#;
        let toc = parse_nav(xhtml).unwrap();
        assert!(toc.is_empty());
    }
}
