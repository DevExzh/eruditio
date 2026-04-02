fn main() {
    let text = quick_xml::events::BytesText::new("test");
    println!("{:?}", text.unescape());
}
