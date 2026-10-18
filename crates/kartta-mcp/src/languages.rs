use kartta_languages::NAMES;

/// The names a `languages` parameter takes: every language kartta ships, and each family key that names several of
/// them at once, as `jsts` names TypeScript, TSX and JavaScript.
#[derive(Debug)]
pub(crate) struct Catalog {
    families: Vec<String>,
}

impl Catalog {
    pub(crate) fn new() -> Self {
        Self {
            families: kartta_languages::all()
                .iter()
                .map(|language| language.family().to_owned())
                .collect(),
        }
    }

    /// Fails on the first name that is neither a language nor a family key, naming every name it takes.
    pub(crate) fn check(&self, requested: Option<&[String]>) -> Result<(), String> {
        let Some(unknown) = requested
            .unwrap_or_default()
            .iter()
            .find(|name| !NAMES.contains(&name.as_str()) && !self.families.contains(name))
        else {
            return Ok(());
        };
        let mut valid: Vec<&str> = NAMES.to_vec();
        for family in &self.families {
            if !valid.contains(&family.as_str()) {
                valid.push(family);
            }
        }
        Err(format!(
            "unknown language {unknown:?}; valid: {}",
            valid.join(", ")
        ))
    }

    /// The languages to build, every one when `requested` is `None`: each named one, and each of a named family.
    pub(crate) fn to_build(&self, requested: Option<&[String]>) -> Vec<&'static str> {
        NAMES
            .iter()
            .zip(&self.families)
            .filter(|(name, family)| {
                requested.is_none_or(|requested| {
                    requested
                        .iter()
                        .any(|wanted| wanted == *name || wanted == *family)
                })
            })
            .map(|(name, _)| *name)
            .collect()
    }

    /// Whether a query filtered by `requested`, `None` filtering nothing, asks about the graph of `family`.
    pub(crate) fn selects(&self, requested: Option<&[String]>, family: &str) -> bool {
        requested.is_none_or(|requested| {
            requested.iter().any(|wanted| {
                wanted == family
                    || NAMES
                        .iter()
                        .zip(&self.families)
                        .any(|(name, of)| wanted == name && of == family)
            })
        })
    }
}

/// How an answer echoes the `languages` filter it was asked with: nothing when there was none, `[]` for an empty one.
pub(crate) fn echo(requested: Option<&[String]>) -> String {
    match requested {
        None => String::new(),
        Some([]) => crate::render::EMPTY_LANGUAGES.to_owned(),
        Some(names) => names.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    #[test]
    fn a_family_key_names_its_languages() {
        let catalog = Catalog::new();

        assert_eq!(
            catalog.to_build(Some(&names(&["jsts", "go"]))),
            ["go", "typescript", "tsx", "javascript"]
        );
        assert_eq!(catalog.to_build(None), NAMES);
        assert_eq!(catalog.to_build(Some(&[])), Vec::<&str>::new());
    }

    #[test]
    fn a_language_of_a_family_selects_the_family_graph() {
        let catalog = Catalog::new();
        let tsx = names(&["tsx"]);

        assert!(catalog.selects(Some(&tsx), "jsts"));
        assert!(!catalog.selects(Some(&tsx), "go"));
        assert!(catalog.selects(None, "go"));
        assert!(!catalog.selects(Some(&[]), "go"));
    }

    #[test]
    fn an_unknown_name_lists_every_name_taken() {
        let catalog = Catalog::new();

        assert_eq!(catalog.check(Some(&names(&["go", "jsts"]))), Ok(()));
        let error = catalog.check(Some(&names(&["nut"]))).unwrap_err();
        assert!(
            error.starts_with("unknown language \"nut\"; valid: go, cpp,"),
            "{error}"
        );
        assert!(error.ends_with(", dshl, jsts"), "{error}");
    }

    #[test]
    fn the_echo_tells_an_empty_filter_from_none() {
        assert_eq!(echo(None), "");
        assert_eq!(echo(Some(&[])), "[]");
        assert_eq!(echo(Some(&names(&["go", "cpp"]))), "go, cpp");
    }
}
