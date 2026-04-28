//! Tiny debug helper: scan stories in an IDML and print any auto-page-
//! number markers that surface from the parser. Used to verify the
//! ACE 18 / ACE 19 PI handler reaches downstream consumers.
use std::path::PathBuf;

fn main() {
    let path: PathBuf = std::env::args().nth(1).expect("usage: dump_markers <idml>").into();
    let bytes = std::fs::read(&path).unwrap();
    let container = idml_parse::Container::open(&bytes).unwrap();
    for (name, raw) in container.entries.iter() {
        if !name.starts_with("Stories/") || !name.ends_with(".xml") {
            continue;
        }
        let story = idml_parse::Story::parse(raw).unwrap();
        for p in &story.paragraphs {
            for r in &p.runs {
                if r.text.contains(idml_parse::AUTO_PAGE_NUMBER_MARKER)
                    || r.text.contains(idml_parse::NEXT_PAGE_NUMBER_MARKER)
                {
                    println!("{name}  text={:?}", r.text);
                }
            }
        }
    }
}
