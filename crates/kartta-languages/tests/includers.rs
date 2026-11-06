//! What each language of the set states it counts as a file's includer.

use kartta_languages::all;

#[test]
fn every_language_of_the_set_states_what_it_counts_as_an_includer() {
    let silent: Vec<String> = all()
        .iter()
        .filter(|language| language.conventions().includers.is_none_or(str::is_empty))
        .map(|language| language.name().to_owned())
        .collect();

    assert_eq!(silent, Vec::<String>::new());
}
