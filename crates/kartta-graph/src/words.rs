// constraint: what a file is indexed under and what a query asks for split with this one function, so a word of a
// query meets the same word of a name
pub(crate) fn words(text: &str, word: &mut String, emit: &mut dyn FnMut(&str)) {
    word.clear();
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if is_combining_mark(c) {
            continue;
        }
        if !c.is_alphanumeric() {
            flush(word, emit);
            previous = None;
            continue;
        }
        if let Some(previous) = previous {
            let hump = (previous.is_lowercase() || previous.is_numeric()) && c.is_uppercase();
            let acronym_ends = previous.is_uppercase()
                && c.is_uppercase()
                && chars.peek().is_some_and(|next| next.is_lowercase());
            if hump || acronym_ends {
                flush(word, emit);
            }
        }
        if c.is_ascii() {
            word.push(c.to_ascii_lowercase());
        } else {
            word.extend(c.to_lowercase().filter(|&lower| !is_combining_mark(lower)));
        }
        previous = Some(c);
    }
    flush(word, emit);
}

// constraint: a combining mark is neither a letter nor a separator, so `İ`, whose lowercase is `i` and a combining dot,
// and a query that spells the dot or leaves it out all meet one word
pub(crate) const fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{300}'..='\u{36F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

// constraint: the last three segments of a path, the extension cut from the file's name alone, since a directory
// named `v1.2` has none
pub(crate) fn path_words(path: &str, word: &mut String, emit: &mut dyn FnMut(&str)) {
    let mut segments = path.rsplit('/');
    let file = segments.next().unwrap_or(path);
    let stem = match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    };
    for directory in segments.take(2) {
        words(directory, word, emit);
    }
    words(stem, word, emit);
}

fn flush(word: &mut String, emit: &mut dyn FnMut(&str)) {
    if !word.is_empty() {
        emit(word);
        word.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        words(text, &mut String::new(), &mut |word| {
            found.push(word.to_owned());
        });
        found
    }

    fn of_path(path: &str) -> Vec<String> {
        let mut found = Vec::new();
        path_words(path, &mut String::new(), &mut |word| {
            found.push(word.to_owned());
        });
        found
    }

    #[test]
    fn a_name_splits_at_separators_and_humps() {
        assert_eq!(split("parse_config"), ["parse", "config"]);
        assert_eq!(split("ns::Widget::draw"), ["ns", "widget", "draw"]);
        assert_eq!(split("parseConfigFile"), ["parse", "config", "file"]);
        assert_eq!(split("md5Sum"), ["md5", "sum"]);
    }

    #[test]
    fn an_acronym_is_one_word_and_ends_before_the_capital_of_the_next() {
        assert_eq!(split("HTTPServer"), ["http", "server"]);
        assert_eq!(split("ID"), ["id"]);
        assert_eq!(split("parseURL"), ["parse", "url"]);
        assert_eq!(split("HTTP2Server"), ["http2", "server"]);
    }

    #[test]
    fn a_letter_outside_ascii_is_lowered_and_belongs_to_its_word() {
        assert_eq!(split("ÜberCafé"), ["über", "café"]);
        assert_eq!(split("größe_Ärger"), ["größe", "ärger"]);
    }

    #[test]
    fn a_combining_mark_is_dropped_wherever_it_stands() {
        assert_eq!(split("İstanbul"), ["istanbul"]);
        assert_eq!(split("i\u{307}stanbul"), ["istanbul"]);
        assert_eq!(split("Cafe\u{301}Bar"), ["cafe", "bar"]);
    }

    #[test]
    fn a_path_gives_the_words_of_its_last_three_segments_and_its_file_stem() {
        assert_eq!(
            of_path("src/net/httpServer/config_loader.go"),
            ["http", "server", "net", "config", "loader"]
        );
        assert_eq!(of_path("lib/v1.2/api.ts"), ["v1", "2", "lib", "api"]);
        assert_eq!(of_path(".bashrc"), ["bashrc"]);
        assert_eq!(of_path("main.go"), ["main"]);
    }
}
