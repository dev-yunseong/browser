use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{NodeData, RcDom, Handle};

pub fn parse_html(html: &str) -> RcDom {
    parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .unwrap()
}

pub fn print_dom_tree(handle: &Handle, indent: usize) {
    let node = handle;
    let indent_str = " ".repeat(indent * 2);

    match node.data {
        NodeData::Document => println!("{}Document", indent_str),
        NodeData::Doctype { ref name, .. } => println!("{}<!DOCTYPE {}>", indent_str, name),
        NodeData::Text { ref contents } => {
            let text = contents.borrow().to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                println!("{}Text: {:?}", indent_str, trimmed);
            }
        }
        NodeData::Comment { ref contents } => println!("{}<!-- {} -->", indent_str, contents),
        NodeData::Element { ref name, .. } => {
            println!("{}<{} ...>", indent_str, name.local);
        }
        NodeData::ProcessingInstruction { .. } => {}
    }

    for child in node.children.borrow().iter() {
        print_dom_tree(child, indent + 1);
    }
}
