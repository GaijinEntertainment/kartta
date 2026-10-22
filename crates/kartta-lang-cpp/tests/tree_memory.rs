//! How much parse tree an extraction holds at once, counted through tree-sitter's allocator.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the allocator \
              of an integration test crate is neither; a layout tree-sitter cannot have is the assertion"
)]

use std::alloc::{self, Layout};
use std::cell::Cell;
use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::{Barrier, Once};
use std::thread;

use kartta_lang::Language;
use kartta_lang_cpp::Cpp;
use tree_sitter::{Allocator, Parser};

const HEADER: usize = 16;

type SizeBytes = [u8; size_of::<usize>()];

static COUNTING: Once = Once::new();

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

fn layout(size: usize) -> Layout {
    Layout::from_size_align(size + HEADER, HEADER)
        .expect("a block tree-sitter asks for fits a layout")
}

fn counted(raw: *mut u8, size: usize) -> *mut c_void {
    if raw.is_null() {
        return raw.cast();
    }
    // SAFETY: `raw` is a fresh block of `HEADER + size` bytes, so its first word is writable.
    unsafe { raw.cast::<SizeBytes>().write(size.to_ne_bytes()) };
    let live = LIVE.get() + size;
    LIVE.set(live);
    PEAK.set(PEAK.get().max(live));
    // SAFETY: the block is `HEADER + size` bytes long, so the caller's part starts `HEADER` bytes in.
    unsafe { raw.add(HEADER) }.cast()
}

const fn header(block: *mut c_void) -> (*mut u8, usize) {
    // SAFETY: tree-sitter hands back only pointers `counted` returned, `HEADER` bytes into their block.
    let raw = unsafe { block.cast::<u8>().sub(HEADER) };
    // SAFETY: the block's first word holds the size `counted` wrote there.
    (
        raw,
        usize::from_ne_bytes(unsafe { raw.cast::<SizeBytes>().read() }),
    )
}

unsafe extern "C" fn counting_malloc(size: usize) -> *mut c_void {
    // SAFETY: the layout is never zero-sized, since it holds the header.
    counted(unsafe { alloc::alloc(layout(size)) }, size)
}

unsafe extern "C" fn counting_calloc(count: usize, size: usize) -> *mut c_void {
    let size = count * size;
    // SAFETY: the layout is never zero-sized, since it holds the header.
    counted(unsafe { alloc::alloc_zeroed(layout(size)) }, size)
}

unsafe extern "C" fn counting_realloc(block: *mut c_void, size: usize) -> *mut c_void {
    if block.is_null() {
        // SAFETY: a null block asks for a fresh one, as `malloc` does.
        return unsafe { counting_malloc(size) };
    }
    let (raw, old) = header(block);
    LIVE.set(LIVE.get() - old);
    // SAFETY: `raw` was allocated with `layout(old)`, and the new size keeps the header.
    counted(
        unsafe { alloc::realloc(raw, layout(old), size + HEADER) },
        size,
    )
}

unsafe extern "C" fn counting_free(block: *mut c_void) {
    if block.is_null() {
        return;
    }
    let (raw, size) = header(block);
    LIVE.set(LIVE.get() - size);
    // SAFETY: `raw` was allocated with `layout(size)` and is freed once.
    unsafe { alloc::dealloc(raw, layout(size)) };
}

fn peak_while(work: impl FnOnce()) -> usize {
    COUNTING.call_once(|| {
        // SAFETY: this runs before the first tree-sitter call of the process, and once.
        unsafe {
            tree_sitter::set_allocator(Some(Allocator {
                malloc: counting_malloc,
                calloc: counting_calloc,
                realloc: counting_realloc,
                free: counting_free,
            }));
        }
    });
    let before = LIVE.get();
    PEAK.set(before);
    work();
    PEAK.get() - before
}

#[test]
fn a_free_on_another_thread_does_not_move_a_measured_window() {
    let parse = || {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("the pinned grammar loads");
        parser.parse("int a;", None).expect("a tree");
    };
    let alone = peak_while(parse);

    let (held, window, freed) = (Barrier::new(2), Barrier::new(2), Barrier::new(2));
    let beside_a_free = thread::scope(|scope| {
        scope.spawn(|| {
            let parser = Parser::new();
            held.wait();
            window.wait();
            drop(parser);
            freed.wait();
        });
        held.wait();
        peak_while(|| {
            window.wait();
            freed.wait();
            parse();
        })
    });

    assert_eq!(
        beside_a_free, alone,
        "a parse peaked at {beside_a_free} bytes of trees beside a free on another thread, {alone} without one"
    );
}

#[test]
fn a_reparse_after_recovery_never_holds_the_first_tree_beside_the_second() {
    let mut source = String::new();
    for at in 0..400 {
        write!(
            source,
            "class Wrap{at} final : public Base\n{{\npublic:\n  void render(ContextId ctx, uint32_t mask,\n    \
             bool cuttings = true)\n#ifndef PROJECT_X\n    DAG_TS_REQUIRES(render_lock)\n#endif\n      ;\n  \
             void render(ContextId ctx, ContextId pilots, uint32_t mask,\n    bool cuttings = true)\n\
             #ifndef PROJECT_X\n    DAG_TS_REQUIRES(render_lock)\n#endif\n      ;\n  void close();\n}};\n\n"
        )
        .expect("a String takes any write");
    }

    let one_parse = peak_while(|| {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("the pinned grammar loads");
        let tree = parser.parse(&source, None).expect("a tree");
        assert!(tree.root_node().has_error());
    });
    let extraction = peak_while(|| {
        let raw = Cpp::new().extract(source.as_bytes());
        assert_eq!(raw.symbols.len(), 400 * 4);
    });

    assert!(
        extraction <= one_parse,
        "an extraction peaked at {extraction} bytes of trees, one parse at {one_parse}"
    );
}

#[test]
fn a_generated_parsers_tables_take_no_parse_tree() {
    let mut tables = String::new();
    for state in 0..2000 {
        write!(
            tables,
            "  [STATE({state})] = {{\n    [sym_a] = ACTIONS({state}),\n    [sym_b] = SHIFT({state}),\n  }},\n"
        )
        .expect("a String takes any write");
    }
    let parser = |define: &str| {
        format!(
            "#include \"tree_sitter/parser.h\"\n#define {define} 15\n\n\
             static const uint16_t ts_parse_table[2000][2] = {{\n{tables}}};\n\n\
             static bool ts_lex(TSLexer *lexer, TSStateId state) {{\n  START_LEXER();\n  ADVANCE(1);\n}}\n"
        )
    };
    let (generated, written) = (parser("LANGUAGE_VERSION"), parser("SCANNER_REVISION"));

    let written_peak = peak_while(|| {
        let raw = Cpp::new().extract(written.as_bytes());
        assert_eq!(raw.symbols.len(), 2);
    });
    let generated_peak = peak_while(|| {
        let raw = Cpp::new().extract(generated.as_bytes());
        assert_eq!(raw.symbols.len(), 2);
    });

    assert!(
        generated_peak * 10 < written_peak,
        "the parser peaked at {generated_peak} bytes of trees, the same tables outside a parser at {written_peak}"
    );
}
