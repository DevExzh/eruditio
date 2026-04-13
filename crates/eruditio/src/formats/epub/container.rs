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

    let size_hint = container_file.size() as usize;
    let mut contents = String::with_capacity(size_hint.min(1 << 20));
    container_file.read_to_string(&mut contents)?;

    let mut xml_reader = XmlReader::from_str(&contents);
    xml_reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(256);
    let mut opf_path = None;

    loop {
        match xml_reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"rootfile"
                    && let Some(attr) = e.try_get_attribute(b"full-path").ok().flatten()
                {
                    opf_path = Some(crate::formats::common::text_utils::bytes_to_string(
                        &attr.value,
                    ));
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
