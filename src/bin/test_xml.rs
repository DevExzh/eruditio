use quick_xml::events::BytesText;

fn main() {
    let text = BytesText::new("test");
    let unescaped = text.into_inner();
    println!("{:?}", unescaped);
}
