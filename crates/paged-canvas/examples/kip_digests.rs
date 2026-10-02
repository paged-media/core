// SCRATCH (not committed): page digests of every corpus/generated fixture,
// cold and with every story grown, plus the widow-controlled long stories.
use paged_canvas::{CanvasModel, CanvasOptions, Mutation};

fn corpus(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus").join(rel)
}
fn options() -> CanvasOptions {
    CanvasOptions {
        fonts: vec![std::fs::read(corpus("fonts/Inter.ttf")).expect("Inter.ttf")],
        font_registry: paged_canvas::font_registry_from_paths(&[corpus("fonts")]),
        cmyk_icc_profile: std::fs::read(corpus("profiles/default_cmyk.icc")).ok(),
        ..CanvasOptions::default()
    }
}
fn dump(name: &str, m: &CanvasModel) {
    let d: Vec<String> = m.built().pages.iter().map(|p| format!("{:016x}", p.list.digest())).collect();
    println!("{name} {} {}", d.len(), d.join(","));
}
fn widow_story(copies: usize) -> Vec<u8> {
    let idml = paged_gen::write_idml(&paged_gen::samples::keeps_reflow::build()).expect("idml");
    let mut doc = idml_import::import_idml_doc(&idml).expect("import");
    let id = paged_gen::ids::self_id("keeps-reflow", "BodyStory", 0);
    let s = doc.stories.iter_mut().find(|s| s.self_id == id).expect("body story");
    s.story.grow = Some(paged_model::FlowGrowRule { copy_frame_options: true, ..Default::default() });
    let one = s.story.paragraphs.clone();
    for _ in 1..copies {
        s.story.paragraphs.extend(one.iter().cloned());
    }
    paged_store::package::wrap_document(&doc, "Widow.docx", 172.0, 318.0).expect("wrap")
}
fn main() {
    let mut files: Vec<_> = std::fs::read_dir(corpus("generated")).unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "idml"))
        .collect();
    files.sort();
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(&f).unwrap();
        let Ok(mut m) = CanvasModel::load("d", &bytes, options()) else { println!("{name} LOADFAIL"); continue };
        dump(&name, &m);
        let ids: Vec<String> = m.scene().stories.iter().map(|s| s.self_id.clone()).collect();
        for id in ids {
            let _ = m.apply_mutation(&Mutation::SetFlowGrowRule { story_id: id, grow: true, max_pages: None, copy_frame_options: Some(true) });
        }
        dump(&format!("{name}+grow"), &m);
    }
    if let Ok(list) = std::env::var("KIP_EXTRA") {
        for f in std::fs::read_to_string(list).unwrap().lines() {
            let Ok(bytes) = std::fs::read(f) else { continue };
            match std::panic::catch_unwind(|| CanvasModel::load("d", &bytes, options())) {
                Ok(Ok(m)) => dump(f, &m),
                _ => println!("{f} LOADFAIL"),
            }
        }
    }
    for copies in [1usize, 5, 30] {
        let bytes = widow_story(copies);
        let mut m = CanvasModel::load("w", &bytes, options()).unwrap();
        dump(&format!("widow{copies}"), &m);
        let id = paged_gen::ids::self_id("keeps-reflow", "BodyStory", 0);
        // pastes like edit_latency, applied cumulatively; dump after each
        for page in [8usize, 117, 226] {
            let Some(idx) = m.built().pages.get(page - 1).and_then(|p| p.story_layout.first()).map(|l| l.paragraph_idx as usize + 1) else { continue };
            let s = m.scene().stories.iter().find(|s| s.self_id == id).unwrap();
            let at: u32 = s.story.paragraphs[..idx + 1].iter().map(|p| p.runs.iter().map(|r| r.text.len() as u32).sum::<u32>() + 1).sum::<u32>() - 1;
            m.apply_mutation(&Mutation::InsertText { story_id: id.clone(), offset: at, text: "\nNEWlineAmmmm NEWlineBmmmm NEWlineCmmmm".into(), cell: None }).unwrap();
            dump(&format!("widow{copies}+paste{page}"), &m);
        }
    }
}
