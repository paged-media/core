fn main() {
    use std::time::Instant;
    use blitz_dom::DocumentConfig;
    use blitz_html::HtmlDocument;
    use blitz_traits::shell::{ColorScheme, Viewport};

    // Cold: parse+style+layout+paint incl. font discovery.
    let t = Instant::now();
    let stats = spike_blitz_wasm::render_fragment(spike_blitz_wasm::FRAGMENT, 480, 320);
    println!("cold full pipeline: {:?} ({} commands)", t.elapsed(), stats.total());

    // Fresh-document loop (worst case: source replaced wholesale).
    let n = 50u32;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(spike_blitz_wasm::render_fragment(
            spike_blitz_wasm::FRAGMENT, 480, 320,
        ));
    }
    println!("fresh-doc: {:?}/render", t.elapsed() / n);

    // Persistent-document loop (editor flow: re-resolve + repaint).
    let mut doc = HtmlDocument::from_html(spike_blitz_wasm::FRAGMENT, DocumentConfig::default());
    doc.set_viewport(Viewport::new(480, 320, 1.0, ColorScheme::Light));
    doc.resolve(0.0);
    let n = 500u32;
    let t = Instant::now();
    for i in 0..n {
        // Alternate viewport width to force real restyle+relayout work.
        let w = if i % 2 == 0 { 480 } else { 481 };
        doc.set_viewport(Viewport::new(w, 320, 1.0, ColorScheme::Light));
        doc.resolve(0.0);
        std::hint::black_box(spike_blitz_wasm::paint_count(&mut doc, w, 320));
    }
    println!("persistent-doc relayout+repaint: {:?}/frame", t.elapsed() / n);

    // Fresh documents sharing ONE Parley FontContext (the
    // catalog-automation path: N records -> N fragments).
    let font_ctx = parley::FontContext::default();
    let n = 100u32;
    let t = Instant::now();
    for _ in 0..n {
        let mut doc = HtmlDocument::from_html(
            spike_blitz_wasm::FRAGMENT,
            DocumentConfig {
                font_ctx: Some(font_ctx.clone()),
                ..Default::default()
            },
        );
        doc.set_viewport(Viewport::new(480, 320, 1.0, ColorScheme::Light));
        doc.resolve(0.0);
        std::hint::black_box(spike_blitz_wasm::paint_count(&mut doc, 480, 320));
    }
    println!("fresh-doc shared-font-ctx: {:?}/render", t.elapsed() / n);
}
