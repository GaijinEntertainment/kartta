//! What a C++ call of a function-like macro reaches where a second vendored copy of the library defines the name as a
//! function in a branch of its own header: the walk keeps that function and a macro is no symbol, so the call of the
//! first copy's macro binds nothing, while the second copy's own calls bind the function.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Selector, confidence};
use tempfile::TempDir;

const MACRO_HEADER: &str = "#pragma once

namespace physx
{
namespace Gu
{

#define PX_CP_TO_PCP(contactPoint) (reinterpret_cast<PersistentContact*>(contactPoint))

}
}
";

const CALLER_OF_THE_MACRO: &str = "#include \"h.h\"

namespace physx
{
namespace Gu
{

int use_a(ContactPoint *p)
{
\treturn PX_CP_TO_PCP(p)->x;
}

}
}
";

const FUNCTION_HEADER: &str = "#pragma once

namespace physx
{
namespace Gu
{

#ifdef PX_PS3
PX_FORCE_INLINE PersistentContact* PX_CP_TO_PCP(ContactPoint* contactPoint)
{
\treturn physx::PxUnionCast<PersistentContact*, ContactPoint*>(contactPoint);
}
#else
#define PX_CP_TO_PCP(contactPoint) (reinterpret_cast<PersistentContact*>(contactPoint))
#endif

}
}
";

const CALLER_THROUGH_THE_INCLUDE: &str = "#include \"h.h\"
#include \"pasted.inc\"

int use_b(ContactPoint *p)
{
\treturn PX_CP_TO_PCP(p)->x;
}
";

const CALLER_PASTED_IN: &str = "int use_pasted(ContactPoint *p)
{
\treturn PX_CP_TO_PCP(p)->x;
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let files = [
        ("copyA/h.h", MACRO_HEADER),
        ("copyA/a.cpp", CALLER_OF_THE_MACRO),
        ("copyB/h.h", FUNCTION_HEADER),
        ("copyB/b.cpp", CALLER_THROUGH_THE_INCLUDE),
        ("copyB/pasted.inc", CALLER_PASTED_IN),
    ];
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["cpp"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

#[test]
fn a_call_of_a_macro_binds_no_function_of_its_name_in_another_vendored_copy() {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of("copyA/a.cpp").expect("C++ claims the file");

    let walk = graph
        .callees("physx::Gu::use_a", Limits::new(1))
        .expect("use_a is defined");

    let callees: Vec<_> = (walk.reached.iter())
        .map(|reached| reached.symbol.qualified_name)
        .collect();

    assert_eq!(callees, Vec::<&str>::new());
    assert_eq!(walk.unresolved, 1);
}

#[test]
fn a_function_binds_the_calls_its_own_copy_includes_or_pastes_it_into() {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of("copyB/h.h").expect("C++ claims the file");

    let walk = graph
        .callers(
            Selector::new("physx::Gu::PX_CP_TO_PCP", None),
            Limits::new(1),
        )
        .expect("the function is defined");
    let callers: Vec<_> = (walk.reached.iter())
        .map(|reached| (reached.symbol.qualified_name, reached.confidence))
        .collect();

    assert_eq!(
        callers,
        [
            ("use_b", confidence::INCLUDE),
            ("use_pasted", confidence::UNIQUE),
        ]
    );
}
