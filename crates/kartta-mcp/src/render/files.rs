/// The `find_files` answer: the paths one per line, best first. A ranked answer needs no mark of a cap, since its order
/// says the paths it left out rank lower.
pub(crate) fn paths(pattern: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return format!("no files match {pattern:?}");
    }
    crate::llmxml::lines(paths)
}
