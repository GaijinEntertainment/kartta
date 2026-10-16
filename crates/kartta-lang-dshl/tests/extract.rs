//! What a DSHL file extracts to from its bytes alone: its declarations, the macro invocations it makes, the names it
//! reads and the files it includes.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{Language, NO_CALLER_ID, OwnMembers, RawExtraction, Symbol, SymbolKind};
use kartta_lang_dshl::Dshl;

fn extract(source: &str) -> RawExtraction {
    Dshl::new().extract(source.as_bytes())
}

fn symbol<'a>(raw: &'a RawExtraction, qualified_name: &str) -> &'a Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("no symbol {qualified_name}"))
}

fn kinds(raw: &RawExtraction) -> Vec<(&str, SymbolKind)> {
    raw.symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect()
}

fn calls(raw: &RawExtraction) -> Vec<(Option<&str>, &str)> {
    raw.calls
        .iter()
        .map(|call| {
            let caller = (call.caller_id != NO_CALLER_ID)
                .then(|| raw.symbols[call.caller_id.0 as usize].name.as_str());
            (caller, call.callee.as_str())
        })
        .collect()
}

fn uses(raw: &RawExtraction) -> Vec<(Option<&str>, &str)> {
    raw.references
        .iter()
        .map(|site| {
            let holder = (site.site_of != NO_CALLER_ID)
                .then(|| raw.symbols[site.site_of.0 as usize].name.as_str());
            (holder, site.name.as_str())
        })
        .collect()
}

const SAMPLE: &str = r#"include "shader_global.dshl"

texture frame_tex;
float4 world_view_pos = (0, 0, 0, 1) always_referenced;
int quality = 1;
interval quality: low < 1, medium < 2, high;
sampler point_clamp { filter = point; address_u = clamp; };
bool use_fog = quality >= medium && !hardware.fsh_5_0;

macro USE_FOO(stage)
  (stage) {
    foo_tex@smp2d = frame_tex;
    foo_pos@f4 = (world_view_pos.x * 2, -world_view_pos.y, 1, 0);
  }
  hlsl(stage) {
    #include "foo_helpers.hlsl"
    float4 foo(float2 tc) { return tex2D(foo_tex, tc); }
  ##if use_fog
    #define FOO_FOG 1
  ##endif
  }
endmacro

shader foo_shader, foo_shader_alt
{
  supports global_frame;
  z_write = false;
  cull_mode = none;
  static int foo_mode = 0;
  local float4 scratch = (0, 0, 0, 0);
  if (shader == foo_shader_alt || maybe(use_fog)) {
    USE_FOO(ps)
  } else {
    dont_render;
  }
  compile("target_ps", "main_ps");
}
"#;

#[test]
fn every_declaration_is_a_symbol_qualified_by_what_declares_it() {
    let raw = extract(SAMPLE);

    assert_eq!(
        kinds(&raw),
        [
            ("frame_tex", SymbolKind::Variable),
            ("world_view_pos", SymbolKind::Variable),
            ("quality", SymbolKind::Variable),
            ("quality", SymbolKind::Enum),
            ("quality.low", SymbolKind::EnumVariant),
            ("quality.medium", SymbolKind::EnumVariant),
            ("quality.high", SymbolKind::EnumVariant),
            ("point_clamp", SymbolKind::Variable),
            ("use_fog", SymbolKind::Constant),
            ("USE_FOO", SymbolKind::Macro),
            ("USE_FOO.foo_tex", SymbolKind::Field),
            ("USE_FOO.foo_pos", SymbolKind::Field),
            ("foo_shader", SymbolKind::Class),
            ("foo_shader_alt", SymbolKind::Class),
            ("foo_shader.foo_mode", SymbolKind::Field),
        ]
    );
}

#[test]
fn a_declaration_keeps_its_lines_owner_and_signature() {
    let raw = extract(SAMPLE);

    let macro_ = symbol(&raw, "USE_FOO");
    let field = symbol(&raw, "foo_shader.foo_mode");

    assert_eq!((macro_.line, macro_.end_line), (10, 22));
    assert_eq!(macro_.signature.as_deref(), Some("macro USE_FOO(stage)"));
    assert_eq!(field.owner.as_deref(), Some("foo_shader"));
    assert_eq!(field.signature.as_deref(), Some("static int foo_mode = 0"));
    assert!(raw.symbols.iter().all(|symbol| symbol.exported));
}

#[test]
fn a_supported_block_is_a_parent_of_every_name_and_a_use() {
    let raw = extract(SAMPLE);

    assert_eq!(symbol(&raw, "foo_shader").parents, ["global_frame"]);
    assert_eq!(symbol(&raw, "foo_shader_alt").parents, ["global_frame"]);
    assert!(uses(&raw).contains(&(Some("foo_shader"), "global_frame")));
}

#[test]
fn a_macro_invocation_is_a_call_made_by_the_declaration_around_it() {
    let raw = extract(
        "macro A() endmacro\nmacro B() A() endmacro\nA()\nshader s { if (C(x)) { B() } }\n",
    );

    assert_eq!(
        calls(&raw),
        [
            (Some("B"), "A"),
            (None, "A"),
            (Some("s"), "C"),
            (Some("s"), "B")
        ]
    );
}

#[test]
fn names_a_condition_or_a_value_reads_are_uses_and_keywords_are_not() {
    let raw = extract(SAMPLE);

    let found = uses(&raw);
    for expected in [
        (Some("USE_FOO"), "frame_tex"),
        (Some("USE_FOO"), "world_view_pos"),
        (Some("USE_FOO"), "use_fog"),
        (Some("foo_shader"), "foo_shader_alt"),
        (Some("foo_shader"), "use_fog"),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
    for keyword in [
        "point", "clamp", "none", "fsh_5_0", "ps", "x", "y", "scratch",
    ] {
        assert!(
            found.iter().all(|(_, name)| *name != keyword),
            "{keyword} in {found:?}"
        );
    }
}

#[test]
fn a_use_at_file_scope_has_no_holder() {
    let raw = extract(SAMPLE);

    assert!(uses(&raw).contains(&(None, "quality")));
}

#[test]
fn the_range_a_comparison_tests_against_is_no_use() {
    let raw = extract("shader s { if (quality == high && mode != NULL) { dont_render; } }\n");

    assert_eq!(uses(&raw), [(Some("s"), "quality"), (Some("s"), "mode")]);
}

#[test]
fn a_local_variable_is_no_symbol() {
    let raw = extract(SAMPLE);

    assert!(raw.symbols.iter().all(|symbol| symbol.name != "scratch"));
}

#[test]
fn includes_of_the_script_and_of_its_hlsl_are_directives() {
    let raw = extract(SAMPLE);

    let directives: Vec<(&str, u32)> = raw
        .directives
        .iter()
        .map(|directive| (directive.spec.as_str(), directive.line))
        .collect();
    assert_eq!(
        directives,
        [("shader_global.dshl", 1), ("foo_helpers.hlsl", 16)]
    );
}

#[test]
fn render_passes_and_framebuffer_layouts_are_classes() {
    let raw = extract(
        "render_pass main_pass { color0 rt = R8G8B8A8; }\nframebuffer_layout gbuf { albedo = R8G8B8A8; }\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("main_pass", SymbolKind::Class),
            ("gbuf", SymbolKind::Class)
        ]
    );
}

#[test]
fn a_block_declares_an_interface_with_its_own_members() {
    let raw =
        extract("block(frame) global_frame { (vs) { globtm@f44 = globtm; } supports none; }\n");

    assert_eq!(
        kinds(&raw),
        [
            ("global_frame", SymbolKind::Interface),
            ("global_frame.globtm", SymbolKind::Field)
        ]
    );
    assert_eq!(
        symbol(&raw, "global_frame").signature.as_deref(),
        Some("block(frame) global_frame")
    );
    assert_eq!(symbol(&raw, "global_frame").parents, Vec::<String>::new());
}

#[test]
fn an_opaque_macro_body_extracts_nothing_but_the_macro() {
    let raw = extract("macro PAIR(a, b) a, b endmacro\n");

    assert_eq!(kinds(&raw), [("PAIR", SymbolKind::Macro)]);
    assert!(raw.calls.is_empty() && raw.references.is_empty());
}

#[test]
fn a_file_the_parser_cannot_read_keeps_what_it_recovered() {
    let raw = extract("texture ok_tex;\nshader broken { z_write = ; }\n");

    assert_eq!(symbol(&raw, "ok_tex").kind, SymbolKind::Variable);
}

#[test]
fn the_extension_is_claimed_in_any_case_and_nothing_else() {
    let dshl = Dshl::new();

    assert!(dshl.claims("prog/shaders/a.dshl"));
    assert!(dshl.claims("A.DSHL"));
    assert!(!dshl.claims("prog/shaders/compile_shaders.sh"));
    assert!(!dshl.claims("prog/shaders/common.hlsl"));
}

#[test]
fn only_a_macro_is_a_call_target() {
    let conventions = Dshl::new().conventions();

    assert_eq!(conventions.own_members, OwnMembers::Unreached);
    assert!(!conventions.values_are_call_targets);
}

#[test]
fn the_fingerprint_names_the_crate_and_is_computed_once() {
    let (one, other) = (Dshl::new(), Dshl::new());

    let first = one.fingerprint();
    let second = other.fingerprint();

    assert!(first.starts_with(b"kartta-lang-dshl\0"));
    assert!(std::ptr::eq(first, second));
}
