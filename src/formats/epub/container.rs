use crate::error::{EruditioError, Result};
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use std::io::{Read, Seek};
use zip::ZipArchive;

/// Finds the path to the root OPF file from META-INF/container.xml.
pub(crate) fn find_opf_path<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<String> {
    let mut container_file = archive
        .by_name("META-INF/container.xml")
        .map_err(|_| EruditioError::Format("Missing META-INF/container.xml".to_string()))?;

    let mut contents = String::new();
    container_file
        .read_to_string(&mut contents)
        .map_err(EruditioError::Io)?;

    let mut xml_reader = XmlReader::from_str(&contents);
    xml_reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut opf_path = None;

    loop {
        match xml_reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"rootfile" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"full-path" {
                            opf_path = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            break;
                        }
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(EruditioError::Parse(format!("XML error: {}", e))),
            _ => (),
        }
        buf.clear();
    }

    opf_path.ok_or_else(|| EruditioError::Format("No OPF path found in container.xml".to_string()))
}
