//! What a Go file's name alone says of the builds that compile it.

// constraint: go/build's `syslist.KnownOS`, whose names a file name constrains
const OPERATING_SYSTEMS: [&str; 18] = [
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];

// constraint: go/build's `syslist.KnownArch`, whose names a file name constrains
const ARCHITECTURES: [&str; 24] = [
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

fn is_system(segment: &str) -> bool {
    OPERATING_SYSTEMS.contains(&segment)
}

fn is_architecture(segment: &str) -> bool {
    ARCHITECTURES.contains(&segment)
}

// constraint: as go/build's `goodOSArchFile`: the name up to its first `.`, past its first `_`, less a last `_test`,
// ends in `_$GOOS_$GOARCH`, `_$GOOS` or `_$GOARCH`
/// The operating system and the architecture that the name of the file at `path` constrains it to.
fn constrained_to(path: &str) -> (Option<&str>, Option<&str>) {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.split('.').next().unwrap_or(name);
    let Some((_, constrained)) = stem.split_once('_') else {
        return (None, None);
    };
    let mut segments = constrained.rsplit('_').peekable();
    segments.next_if_eq(&"test");
    let last = segments.next();
    let before = segments.next();
    match (before, last) {
        (Some(system), Some(architecture))
            if is_system(system) && is_architecture(architecture) =>
        {
            (Some(system), Some(architecture))
        }
        (_, Some(system)) if is_system(system) => (Some(system), None),
        (_, Some(architecture)) if is_architecture(architecture) => (None, Some(architecture)),
        _ => (None, None),
    }
}

// constraint: go/build's `matchTag` takes `linux` as true under GOOS=android, `solaris` under illumos and `darwin`
// under ios, so a file of the first of a pair builds with a file of the second
fn share_a_system(one: &str, other: &str) -> bool {
    let builds_on = |general: &str, specific: &str| {
        matches!(
            (general, specific),
            ("linux", "android") | ("solaris", "illumos") | ("darwin", "ios")
        )
    };
    one == other || builds_on(one, other) || builds_on(other, one)
}

/// Whether the names of the files at `one` and `other` keep every build from compiling both.
pub(crate) fn built_apart(one: &str, other: &str) -> bool {
    let (one_system, one_architecture) = constrained_to(one);
    let (other_system, other_architecture) = constrained_to(other);
    let differ = |one: Option<&str>, other: Option<&str>, share: fn(&str, &str) -> bool| {
        one.zip(other)
            .is_some_and(|(one, other)| !share(one, other))
    };
    differ(one_system, other_system, share_a_system)
        || differ(one_architecture, other_architecture, str::eq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_constrains_a_file_as_go_build_reads_it() {
        for (path, constrained) in [
            ("a/b.go", (None, None)),
            ("a/linux.go", (None, None)),
            ("a/linux_amd64.go", (None, Some("amd64"))),
            ("a/b_linux.go", (Some("linux"), None)),
            ("a/b_amd64.go", (None, Some("amd64"))),
            ("a/b_linux_amd64.go", (Some("linux"), Some("amd64"))),
            ("a/b_linux_test.go", (Some("linux"), None)),
            ("a/b_linux_amd64_test.go", (Some("linux"), Some("amd64"))),
            ("a/b_test.go", (None, None)),
            ("a/b_linux_c.go", (None, None)),
            ("a/b_amd64_linux.go", (Some("linux"), None)),
            ("a/b_linux.pb.go", (Some("linux"), None)),
        ] {
            assert_eq!(constrained_to(path), constrained, "{path}");
        }
    }

    #[test]
    fn files_are_built_apart_only_where_their_names_exclude_each_other() {
        for (one, other, apart) in [
            ("a_linux.go", "a_windows.go", true),
            ("a_amd64.go", "a_arm64.go", true),
            ("a_linux_amd64.go", "a_darwin_amd64.go", true),
            ("a_linux_amd64.go", "a_linux_arm64.go", true),
            ("a_linux.go", "a_linux_amd64.go", false),
            ("a_linux.go", "a_amd64.go", false),
            ("a_linux.go", "a.go", false),
            ("a_linux.go", "a_android.go", false),
            ("a_android.go", "a_linux.go", false),
            ("a_solaris.go", "a_illumos.go", false),
            ("a_ios.go", "a_darwin.go", false),
            ("a_android.go", "a_ios.go", true),
        ] {
            assert_eq!(built_apart(one, other), apart, "{one} {other}");
        }
    }
}
