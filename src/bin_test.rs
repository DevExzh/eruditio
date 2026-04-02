use quick_xml::events::Event;
fn main() {
    let mut reader = quick_xml::Reader::from_str("<test>  hello  </test>");
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    if let Ok(Event::Text(e)) = reader.read_event_into(&mut buf) {
        println!("{:?}", e.unescape());
    }
}
