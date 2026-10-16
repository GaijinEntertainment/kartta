use kartta_lang::Repository;
use kartta_lang::path::base;

const MODULE_ROOTS: [&str; 5] = ["", "modules/", "site/", "site-modules/", "dist/"];

// constraint: a class name's segments match `^[a-z][a-z0-9_]*$` once its quotes and one leading `::` are dropped; an
// uppercase segment is a type reference and a `$` or `*` a computed name, and neither is a class
pub(crate) fn class_name(written: &str) -> Option<&str> {
    let unquoted = written.trim_matches(|quote| quote == '\'' || quote == '"');
    let name = unquoted.strip_prefix("::").unwrap_or(unquoted);
    name.split("::").all(is_class_segment).then_some(name)
}

fn is_class_segment(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

// constraint: the module autoloader maps `module` to `module/manifests/init.pp` and `module::a::b` to
// `module/manifests/a/b.pp` under the conventional module roots; elsewhere, a directory environment's copy is taken
// only when it is the one file of that tail, since several environments can hold the same module
pub(crate) fn manifest(written: &str, repository: &dyn Repository) -> Option<String> {
    let name = class_name(written)?;
    let suffix = match name.split_once("::") {
        None => format!("{name}/manifests/init.pp"),
        Some((module, nested)) => format!("{module}/manifests/{}.pp", nested.replace("::", "/")),
    };

    let mut candidate = String::with_capacity("site-modules/".len() + suffix.len());
    for root in MODULE_ROOTS {
        candidate.clear();
        candidate.push_str(root);
        candidate.push_str(&suffix);
        if repository.contains(&candidate) {
            return Some(candidate);
        }
    }

    let mut found = repository.files_named(base(&suffix)).filter(|path| {
        path.strip_suffix(suffix.as_str())
            .is_some_and(|head| head.ends_with('/'))
    });
    let only = found.next()?;
    found.next().is_none().then_some(only)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_name_is_lowercase_segments_after_one_leading_separator() {
        assert_eq!(class_name("::apache::vhost"), Some("apache::vhost"));
        assert_eq!(class_name("'apache'"), Some("apache"));
        for written in [
            "",
            "::",
            "apache::",
            "Apache",
            "Class['foo']",
            "$name",
            "*",
            "a::$b",
        ] {
            assert_eq!(class_name(written), None, "{written}");
        }
    }
}
