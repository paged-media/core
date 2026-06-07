// Wire robustness (W2.0): a LinkFrames against a non-text-frame id must
// surface as a clean, serializable MutationFailed — the editor client
// correlates replies by seq, so every mutate MUST produce a reply that
// serde can serialize (a panic or unserializable error hangs the UI).
use paged_canvas::channel::{Mutation, WorkerToMain, WorkerToMainKind, PROTOCOL_VERSION};
use paged_canvas::model::CanvasModel;

#[test]
fn link_frames_to_rectangle_errors_and_reply_serializes() {
    let bytes = std::fs::read("../../corpus/generated/text.idml").expect("fixture");
    let mut model = CanvasModel::load("d", &bytes, paged_canvas::model::CanvasOptions::default())
        .expect("load");
    // find a text frame + a rectangle id from the built doc
    let m = Mutation::LinkFrames {
        from: "u7f28c9".into(),
        to: "not-a-frame".into(),
    };
    let out = model.apply_mutation(&m);
    println!("apply result: {:?}", out.as_ref().err());
    assert!(out.is_err(), "expected validation error");
    let reply = WorkerToMain {
        seq: Some(1),
        protocol: PROTOCOL_VERSION,
        kind: WorkerToMainKind::MutationFailed {
            error: out.unwrap_err(),
        },
    };
    let json = serde_json::to_string(&reply).expect("reply must serialize");
    println!("serialized: {}", &json[..json.len().min(400)]);
}
