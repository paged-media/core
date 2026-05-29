//! Scripting Stage 2 — embedded Boa (pure-Rust JS) bridge.
//!
//! Hosts a Boa JS context inside the canvas worker so user scripts
//! can mutate the document through the same Operation channel the
//! Inspector + REPL already use. Per `docs/verso/scripting-layer.md`
//! every write goes through `idml_mutate::apply`; the host functions
//! installed here are the only path JS can take to reach it.
//!
//! Boa is pure Rust, so the wasm build needs nothing more than
//! `cargo build --target wasm32-unknown-unknown` — no libc sysroot,
//! no wasm-capable clang, no WASI polyfill (which were all required
//! by the previous rquickjs/QuickJS-in-C path).
//!
//! v1 surface (function-style + Proxy sugar via the bootstrap JS):
//!   verso.set(idStr, pathStr, value)
//!   verso.get(idStr, pathStr) -> value | null
//!   verso.inspect(idStr) -> ElementProperties JSON
//!   verso.layers() -> LayerSummary[]
//!   verso.tree() -> SceneTreeNode[]
//!   verso.selection() -> ElementId[] JSON (current element selection)
//!   verso.contentSelection() -> ContentSelection JSON | null
//!   verso.undo() / verso.redo()
//!   verso.frame(idStr) -> Proxy whose `prop = value` writes go
//!                          through verso.set
//!   console.log(...) -> captured into the output log

use std::cell::RefCell;

use boa_engine::{
    js_string,
    object::ObjectInitializer,
    property::Attribute,
    Context, JsArgs, JsResult, JsValue, NativeFunction, Source,
};
use idml_canvas::channel::Mutation;
use idml_canvas::CanvasModel;
use serde::{Deserialize, Serialize};

/// Result of one `execute_script` call. Output is the accumulated
/// `console.log` / `console.warn` / etc. lines (in emission order).
/// `error` is set when the script threw an unhandled exception.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptResult {
    pub output: Vec<String>,
    pub error: Option<String>,
}

// SAFETY pattern: Boa runs host functions synchronously inside the
// `ctx.eval(...)` call from `execute_script`. We stash a raw pointer
// to the CanvasModel in thread-local storage on entry and clear it
// on exit. The host functions read it via `with_model`. Wasm is
// single-threaded so the thread-local pattern is safe by
// construction; native (test-only) usage is also serial.
thread_local! {
    static MODEL_PTR: RefCell<Option<*mut CanvasModel>> = const { RefCell::new(None) };
    static OUTPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn with_model<R>(f: impl FnOnce(&mut CanvasModel) -> R) -> R {
    MODEL_PTR.with(|p| {
        let ptr = p
            .borrow()
            .expect("idml-script: host fn called outside execute_script");
        // SAFETY: pointer valid for the duration of the enclosing
        // execute_script call (set/cleared by execute_script itself).
        unsafe { f(&mut *ptr) }
    })
}

fn push_output(line: String) {
    OUTPUT.with(|o| o.borrow_mut().push(line));
}

/// Run `source` against the given `CanvasModel`. Every write the
/// script issues lands as an `Operation` via `apply_mutation`, so
/// undo/redo work identically to any UI-driven change.
pub fn execute_script(model: &mut CanvasModel, source: &str) -> ScriptResult {
    let ptr = model as *mut CanvasModel;
    MODEL_PTR.with(|p| *p.borrow_mut() = Some(ptr));
    OUTPUT.with(|o| o.borrow_mut().clear());

    let mut ctx = Context::default();

    let error = run(&mut ctx, source);

    let output = OUTPUT.with(|o| std::mem::take(&mut *o.borrow_mut()));
    MODEL_PTR.with(|p| *p.borrow_mut() = None);

    ScriptResult { output, error }
}

fn run(ctx: &mut Context, source: &str) -> Option<String> {
    if let Err(e) = install_bridge(ctx) {
        return Some(format!("bridge install: {}", format_error(&e, ctx)));
    }
    let bootstrap = r#"
        (function () {
            const baseSet = verso.set;
            const baseGet = verso.get;
            verso.frame = function (id) {
                return new Proxy({}, {
                    set(_t, prop, value) {
                        baseSet(id, String(prop), value);
                        return true;
                    },
                    get(_t, prop) {
                        const raw = baseGet(id, String(prop));
                        if (raw === null || raw === undefined) return null;
                        try { return JSON.parse(raw); } catch (_) { return raw; }
                    },
                });
            };
        })();
    "#;
    if let Err(e) = ctx.eval(Source::from_bytes(bootstrap.as_bytes())) {
        return Some(format!("bootstrap: {}", format_error(&e, ctx)));
    }
    match ctx.eval(Source::from_bytes(source.as_bytes())) {
        Ok(v) => {
            if !v.is_undefined() {
                let line = format_value(&v, ctx);
                push_output(line);
            }
            None
        }
        Err(e) => Some(format_error(&e, ctx)),
    }
}

fn install_bridge(ctx: &mut Context) -> JsResult<()> {
    let verso = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(verso_set), js_string!("set"), 3)
        .function(NativeFunction::from_fn_ptr(verso_get), js_string!("get"), 2)
        .function(NativeFunction::from_fn_ptr(verso_undo), js_string!("undo"), 0)
        .function(NativeFunction::from_fn_ptr(verso_redo), js_string!("redo"), 0)
        .function(
            NativeFunction::from_fn_ptr(verso_inspect),
            js_string!("inspect"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(verso_layers),
            js_string!("layers"),
            0,
        )
        .function(NativeFunction::from_fn_ptr(verso_tree), js_string!("tree"), 0)
        .function(
            NativeFunction::from_fn_ptr(verso_selection),
            js_string!("selection"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(verso_content_selection),
            js_string!("contentSelection"),
            0,
        )
        .build();
    ctx.register_global_property(js_string!("verso"), verso, Attribute::all())?;

    let console = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(console_log), js_string!("log"), 0)
        .function(NativeFunction::from_fn_ptr(console_warn), js_string!("warn"), 0)
        .function(
            NativeFunction::from_fn_ptr(console_error),
            js_string!("error"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(console_info),
            js_string!("info"),
            0,
        )
        .build();
    ctx.register_global_property(js_string!("console"), console, Attribute::all())?;

    Ok(())
}

// ---------------------------------------------------------------- verso.*

fn verso_set(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args.get_or_undefined(0).to_string(ctx)?.to_std_string_escaped();
    let path = args.get_or_undefined(1).to_string(ctx)?.to_std_string_escaped();
    let value_arg = args.get_or_undefined(2).clone();

    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let Some(wire_path) = parse_property_path(&path) else {
        return Ok(JsValue::from(false));
    };
    let Some(wire_value) = js_value_to_wire(&value_arg, ctx) else {
        return Ok(JsValue::from(false));
    };

    let mutation = Mutation::SetElementProperty {
        element_id,
        path: wire_path,
        value: wire_value,
    };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

fn verso_get(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args.get_or_undefined(0).to_string(ctx)?.to_std_string_escaped();
    let path = args.get_or_undefined(1).to_string(ctx)?.to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::null());
    };
    let entry = with_model(|m| {
        m.element_properties(&element_id).and_then(|props| {
            props
                .entries
                .into_iter()
                .find(|e| property_path_label(e.path) == path)
        })
    });
    match entry {
        Some(e) => Ok(JsValue::from(
            js_string!(serde_json::to_string(&e.value).unwrap_or_default()),
        )),
        None => Ok(JsValue::null()),
    }
}

fn verso_undo(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(with_model(|m| m.undo().is_some())))
}

fn verso_redo(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(with_model(|m| m.redo().is_some())))
}

fn verso_inspect(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args.get_or_undefined(0).to_string(ctx)?.to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::null());
    };
    let payload = with_model(|m| {
        m.element_properties(&element_id)
            .map(|p| serde_json::to_string(&p).unwrap_or_default())
    });
    match payload {
        Some(s) => Ok(JsValue::from(js_string!(s))),
        None => Ok(JsValue::null()),
    }
}

fn verso_layers(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.layers()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

fn verso_tree(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.scene_tree()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// Returns the current element-selection set as a JSON-encoded
/// `ElementId[]`. Empty selection yields `"[]"` — never `null` —
/// to mirror the always-present array shape the UI consumes via
/// `useElementSelection()`. Application state, not document state:
/// reads do not enter the Operation log, and the caller is expected
/// to re-poll on `mutationApplied` if it wants to react to changes.
fn verso_selection(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.element_selection.ids).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// Returns the current text-side selection (caret or range) as a
/// JSON-encoded `ContentSelection`, or JS `null` when there is none.
/// Same shape `client.setSelection` accepts on the way in.
fn verso_content_selection(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let payload = with_model(|m| {
        m.current_selection
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default())
    });
    match payload {
        Some(s) => Ok(JsValue::from(js_string!(s))),
        None => Ok(JsValue::null()),
    }
}

// ---------------------------------------------------------------- console.*

fn console_log(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("log", args, ctx);
    Ok(JsValue::undefined())
}
fn console_warn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("warn", args, ctx);
    Ok(JsValue::undefined())
}
fn console_error(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("error", args, ctx);
    Ok(JsValue::undefined())
}
fn console_info(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("info", args, ctx);
    Ok(JsValue::undefined())
}

fn emit_console(level: &str, args: &[JsValue], ctx: &mut Context) {
    let parts: Vec<String> = args.iter().map(|v| format_value(v, ctx)).collect();
    push_output(format!("[{level}] {}", parts.join(" ")));
}

// ---------------------------------------------------------------- parsing

fn parse_element_id(s: &str) -> Option<idml_canvas::element_selection::ElementId> {
    use idml_canvas::element_selection::ElementId;
    let (kind, id) = s.split_once(':')?;
    if id.is_empty() {
        return None;
    }
    let id = id.to_string();
    Some(match kind {
        "textFrame" | "textframe" => ElementId::TextFrame(id),
        "rectangle" | "rect" => ElementId::Rectangle(id),
        "oval" => ElementId::Oval(id),
        "polygon" => ElementId::Polygon(id),
        "graphicLine" | "graphicline" => ElementId::GraphicLine(id),
        "group" => ElementId::Group(id),
        _ => return None,
    })
}

fn parse_property_path(s: &str) -> Option<idml_mutate::PropertyPath> {
    use idml_mutate::PropertyPath::*;
    Some(match s {
        "frameBounds" => FrameBounds,
        "frameFillColor" => FrameFillColor,
        "frameStrokeColor" => FrameStrokeColor,
        "frameStrokeWeight" => FrameStrokeWeight,
        "frameOpacity" => FrameOpacity,
        "frameTransform" => FrameTransform,
        "imageContentTransform" => ImageContentTransform,
        "framePathPoint" => FramePathPoint,
        "pathPointInsert" => PathPointInsert,
        "pathPointRemove" => PathPointRemove,
        "pathPointCurveType" => PathPointCurveType,
        "layerVisible" => LayerVisible,
        "layerLocked" => LayerLocked,
        "layerPrintable" => LayerPrintable,
        "layerName" => LayerName,
        "characterFontSize" => CharacterFontSize,
        "characterLeading" => CharacterLeading,
        "characterTracking" => CharacterTracking,
        "characterFillColor" => CharacterFillColor,
        _ => return None,
    })
}

fn property_path_label(path: idml_mutate::PropertyPath) -> &'static str {
    use idml_mutate::PropertyPath::*;
    match path {
        FrameBounds => "frameBounds",
        FrameFillColor => "frameFillColor",
        FrameStrokeColor => "frameStrokeColor",
        FrameStrokeWeight => "frameStrokeWeight",
        FrameOpacity => "frameOpacity",
        FrameTransform => "frameTransform",
        ImageContentTransform => "imageContentTransform",
        FramePathPoint => "framePathPoint",
        PathPointInsert => "pathPointInsert",
        PathPointRemove => "pathPointRemove",
        PathPointCurveType => "pathPointCurveType",
        LayerVisible => "layerVisible",
        LayerLocked => "layerLocked",
        LayerPrintable => "layerPrintable",
        LayerName => "layerName",
        CharacterFontSize => "characterFontSize",
        CharacterLeading => "characterLeading",
        CharacterTracking => "characterTracking",
        CharacterFillColor => "characterFillColor",
    }
}

fn js_value_to_wire(value: &JsValue, ctx: &mut Context) -> Option<idml_mutate::Value> {
    use idml_mutate::Value as W;
    if let Some(b) = value.as_boolean() {
        return Some(W::Bool(b));
    }
    if let Some(n) = value.as_number() {
        return Some(W::Length(Some(n as f32)));
    }
    if value.is_null() {
        return Some(W::Length(None));
    }
    if let Some(s) = value.as_string() {
        return Some(W::ColorRef(Some(s.to_std_string_escaped())));
    }
    if let Some(obj) = value.as_object() {
        if obj.is_array() {
            let len = obj
                .get(js_string!("length"), ctx)
                .ok()?
                .to_length(ctx)
                .ok()? as usize;
            if len == 4 {
                let mut out = [0.0f32; 4];
                for i in 0..4 {
                    let v = obj.get(i as u32, ctx).ok()?;
                    out[i] = v.as_number()? as f32;
                }
                return Some(W::Bounds(out));
            }
            if len == 6 {
                let mut out = [0.0f32; 6];
                for i in 0..6 {
                    let v = obj.get(i as u32, ctx).ok()?;
                    out[i] = v.as_number()? as f32;
                }
                return Some(W::Transform(Some(out)));
            }
        }
        // `{ type, value }` shape — round-trip via JSON.
        let json = value.to_json(ctx).ok()??;
        return serde_json::from_value::<idml_mutate::Value>(json).ok();
    }
    None
}

// ---------------------------------------------------------------- formatting

fn format_value(value: &JsValue, ctx: &mut Context) -> String {
    if value.is_undefined() {
        return "undefined".to_string();
    }
    if value.is_null() {
        return "null".to_string();
    }
    if let Some(b) = value.as_boolean() {
        return b.to_string();
    }
    if let Some(n) = value.as_number() {
        if n.is_finite() && n == n.trunc() && n.abs() < 1e15 {
            return (n as i64).to_string();
        }
        return n.to_string();
    }
    if let Some(s) = value.as_string() {
        return s.to_std_string_escaped();
    }
    if let Some(obj) = value.as_object() {
        // Error-shaped objects: pull `.name`, `.message`, `.stack`.
        let name_v = obj.get(js_string!("name"), ctx).ok();
        let msg_v = obj.get(js_string!("message"), ctx).ok();
        let has_msg = msg_v.as_ref().is_some_and(|v| !v.is_undefined() && !v.is_null());
        if has_msg {
            let mut head = String::new();
            if let Some(nv) = name_v {
                if !nv.is_undefined() && !nv.is_null() {
                    head.push_str(&format_value(&nv, ctx));
                }
            }
            let msg_s = format_value(msg_v.as_ref().unwrap(), ctx);
            if !head.is_empty() {
                head.push_str(": ");
            }
            head.push_str(&msg_s);
            if let Ok(stack) = obj.get(js_string!("stack"), ctx) {
                if !stack.is_undefined() && !stack.is_null() {
                    return format!("{head}\n{}", format_value(&stack, ctx));
                }
            }
            return head;
        }
    }
    // Fallback: JSON.stringify equivalent via Boa's to_json.
    if let Ok(Some(json)) = value.to_json(ctx) {
        return serde_json::to_string(&json).unwrap_or_else(|_| "[opaque]".to_string());
    }
    "[opaque]".to_string()
}

fn format_error(err: &boa_engine::JsError, ctx: &mut Context) -> String {
    // Boa exposes the thrown value via `to_opaque(ctx)`; reuse the
    // value formatter so Error objects come out as "Name: message".
    let opaque = err.to_opaque(ctx);
    format_value(&opaque, ctx)
}
