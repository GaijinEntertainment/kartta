use kartta_graph::Stats;

/// The files one language claimed during the build, and what became of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct LanguageFiles<'e> {
    /// The language's [name](kartta_lang::Language::name).
    pub name: &'e str,
    /// The family whose graph holds its files.
    pub family: &'e str,
    /// The files it was the first language to claim, of those the ignore rules kept.
    pub claimed: usize,
    /// The claimed files the walk read as text. The others are binary, by their extension or their content, or could
    /// not be read and are listed in [`EngineOutcome::errors`](crate::EngineOutcome::errors).
    pub read: usize,
    /// The read files whose extraction the content cache answered. An answer in a form the build cannot decode counts
    /// here too, and its file is extracted again from a second read.
    pub cached: usize,
    /// The read files its family's graph holds. The others had their extraction cached in a form the build could not
    /// decode, could not be read again, and are listed in [`EngineOutcome::errors`](crate::EngineOutcome::errors).
    pub held: usize,
}

/// What an engine's build made of its languages' files, and what each family's graph holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EngineStats<'e> {
    /// Each language's files, in the order the languages were given to [`Engine::build`](crate::Engine::build).
    pub languages: Vec<LanguageFiles<'e>>,
    /// Each family's name and what its graph holds, in the order of [`Engine::graphs`](crate::Engine::graphs).
    pub families: Vec<(&'e str, Stats)>,
}
