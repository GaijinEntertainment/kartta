use kartta_lang::Repository;
use kartta_lang::path::dir;
use serde_json::Value;

const COMPOSER_JSON: &str = "composer.json";

// constraint: a longer prefix maps first, so `App\Tests` takes a name before `App`; one prefix may map several
// directories, tried in the order they are written
pub(crate) struct Psr4 {
    prefixes: Vec<(String, String)>,
}

impl Psr4 {
    // constraint: a vendored package's `composer.json` maps its own namespaces into a directory the repository does
    // not own, so only the project's configurations count
    pub(crate) fn read(repository: &dyn Repository) -> Self {
        let mut prefixes = Vec::new();
        for path in repository.files_named(COMPOSER_JSON) {
            if path
                .split('/')
                .any(|segment| matches!(segment, "vendor" | "node_modules"))
            {
                continue;
            }
            let Ok(contents) = repository.read(&path) else {
                continue;
            };
            let base = match dir(&path) {
                "." => "",
                base => base,
            };
            collect(&contents, base, &mut prefixes);
        }
        prefixes.sort_by(|(left, _), (right, _)| {
            right.len().cmp(&left.len()).then_with(|| left.cmp(right))
        });

        Self { prefixes }
    }

    // constraint: a function or a constant lives in the file of its namespace, so a name that maps to no file is tried
    // once more without its last segment
    pub(crate) fn function_or_const_file(
        &self,
        name: &str,
        repository: &dyn Repository,
    ) -> Option<String> {
        self.class_file(name, repository).or_else(|| {
            let (namespace, _) = name.rsplit_once('\\')?;
            self.class_file(namespace, repository)
        })
    }

    // constraint: a class's file is its whole name under the prefix's directory, and only that file; the empty prefix
    // maps every name, after every other prefix, as Composer's fallback directories do
    pub(crate) fn class_file(&self, name: &str, repository: &dyn Repository) -> Option<String> {
        let name = name.trim_start_matches('\\');

        self.prefixes.iter().find_map(|(prefix, directory)| {
            let rest = match name.strip_prefix(prefix.as_str())? {
                rest if prefix.is_empty() => rest,
                rest => rest.strip_prefix('\\')?,
            };
            if rest.is_empty() {
                return None;
            }

            let relative = rest.replace('\\', "/");
            let file = if directory.is_empty() {
                format!("{relative}.php")
            } else {
                format!("{directory}/{relative}.php")
            };
            repository.contains(&file).then_some(file)
        })
    }
}

fn collect(contents: &[u8], base: &str, prefixes: &mut Vec<(String, String)>) {
    let Ok(root) = serde_json::from_slice::<Value>(contents) else {
        return;
    };

    for section in ["autoload", "autoload-dev"] {
        let Some(mappings) = root
            .get(section)
            .and_then(|section| section.get("psr-4"))
            .and_then(Value::as_object)
        else {
            continue;
        };

        for (prefix, directories) in mappings {
            let prefix = prefix.trim_end_matches('\\');
            let directories: &[Value] = match directories {
                Value::String(_) => std::slice::from_ref(directories),
                Value::Array(directories) => directories,
                _ => continue,
            };
            for directory in directories.iter().filter_map(Value::as_str) {
                prefixes.push((
                    prefix.to_owned(),
                    rebased(base, directory.trim_end_matches('/')),
                ));
            }
        }
    }
}

fn rebased(base: &str, directory: &str) -> String {
    match (base.is_empty(), directory.is_empty()) {
        (true, _) => directory.to_owned(),
        (false, true) => base.to_owned(),
        (false, false) => format!("{base}/{directory}"),
    }
}
