//! The lines a unified diff changes, read from its file and hunk headers.

use std::ops::RangeInclusive;

/// The lines of one file a change touches, in the tree the change is applied to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Changed {
    pub(crate) path: String,
    pub(crate) lines: RangeInclusive<u32>,
}

/// What `git diff` writes for the side of a file that does not exist: the old side of an added file, the new side of a
/// deleted one.
const NO_FILE: &str = "/dev/null";

/// Appends to `regions` the regions of `diff`, a unified diff as `git diff` writes it: each hunk's lines on its `+`
/// side, under the path of its `+++` header less git's `b/` prefix, and the whole of a file the diff deletes, under the
/// path of its `---` header less `a/`. A hunk that only deletes lines covers the line on each side of the deletion.
/// Every other line outside a hunk is skipped, and a hunk's body is read by the line counts its header gives, so a
/// changed line that reads like a header is not taken for one.
///
/// # Errors
///
/// A hunk header that does not read, a hunk with no `+++` header after the last `---` one, a hunk body line that is
/// no context, removed or added line, and a hunk whose body ends before its header's counts do, each named by its line
/// in `diff`.
pub(crate) fn regions(diff: &str, regions: &mut Vec<Changed>) -> Result<(), String> {
    let mut old_path = None;
    let mut new_path = None;
    let mut lines = diff.lines().enumerate();
    while let Some((at, line)) = lines.next() {
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = Some(header_path(path, "a/"));
            new_path = None;
        } else if let Some(path) = line.strip_prefix("+++ ") {
            new_path = Some(header_path(path, "b/"));
            if new_path == Some(NO_FILE)
                && let Some(old) = old_path.filter(|&old| old != NO_FILE)
            {
                regions.push(Changed {
                    path: old.to_owned(),
                    lines: 1..=u32::MAX,
                });
            }
        } else if line.starts_with("@@ ") {
            let hunk = Hunk::read(line).ok_or_else(|| {
                format!("line {} of the diff is not a hunk header: {line:?}", at + 1)
            })?;
            skip_body(&mut lines, hunk, at)?;
            match new_path {
                None => {
                    return Err(format!(
                        "the hunk at line {} of the diff has no +++ file header before it - pass the diff unedited",
                        at + 1
                    ));
                }
                Some(NO_FILE) => {}
                Some(path) => regions.push(Changed {
                    path: path.to_owned(),
                    lines: hunk.new_lines(),
                }),
            }
        }
    }
    Ok(())
}

/// A file header's path: the text before a tab, which `diff -u` follows with a timestamp, less git's `prefix`.
fn header_path<'d>(text: &'d str, prefix: &str) -> &'d str {
    let path = text.split('\t').next().unwrap_or(text);
    if path == NO_FILE {
        return path;
    }
    path.strip_prefix(prefix).unwrap_or(path)
}

/// The line counts of a hunk header, `@@ -start,count +start,count @@`, where a count left out is 1.
#[derive(Debug, Clone, Copy)]
struct Hunk {
    old_count: u32,
    new_start: u32,
    new_count: u32,
}

impl Hunk {
    fn read(header: &str) -> Option<Self> {
        let mut words = header.strip_prefix("@@ -")?.split(' ');
        let (_, old_count) = range(words.next()?)?;
        let (new_start, new_count) = range(words.next()?.strip_prefix('+')?)?;
        (words.next()? == "@@").then_some(Self {
            old_count,
            new_start,
            new_count,
        })
    }

    /// The hunk's lines on its `+` side, or, for a hunk with none, the lines either side of where it deleted.
    fn new_lines(self) -> RangeInclusive<u32> {
        if self.new_count == 0 {
            self.new_start.max(1)..=self.new_start.saturating_add(1)
        } else {
            self.new_start..=self.new_start.saturating_add(self.new_count - 1)
        }
    }
}

fn range(text: &str) -> Option<(u32, u32)> {
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

/// Consumes the body of `hunk`, whose header is line `at`: the lines its counts announce on each side, a line past a
/// side's count failing. An empty line is a context line whose leading space an editor stripped.
fn skip_body<'d>(
    lines: &mut impl Iterator<Item = (usize, &'d str)>,
    hunk: Hunk,
    at: usize,
) -> Result<(), String> {
    let (mut old, mut new) = (hunk.old_count, hunk.new_count);
    while old > 0 || new > 0 {
        let Some((body_at, line)) = lines.next() else {
            return Err(format!(
                "the hunk at line {} of the diff ends before the lines its header counts - pass the diff unedited",
                at + 1
            ));
        };
        let counted = match line.as_bytes().first() {
            None | Some(b' ') if old > 0 && new > 0 => {
                old -= 1;
                new -= 1;
                true
            }
            Some(b'-') if old > 0 => {
                old -= 1;
                true
            }
            Some(b'+') if new > 0 => {
                new -= 1;
                true
            }
            Some(b'\\') => true,
            _ => false,
        };
        if !counted {
            return Err(format!(
                "line {} of the diff is no line of the hunk at line {}: {line:?} - pass the diff unedited",
                body_at + 1,
                at + 1
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, lines: RangeInclusive<u32>) -> Changed {
        Changed {
            path: path.to_owned(),
            lines,
        }
    }

    fn read(diff: &str) -> Result<Vec<Changed>, String> {
        let mut read = Vec::new();
        regions(diff, &mut read).map(|()| read)
    }

    #[test]
    fn each_hunk_answers_its_new_lines_and_a_deleted_file_answers_whole() {
        let diff = "\
diff --git a/pkg/a.go b/pkg/a.go
index 1111111..2222222 100644
--- a/pkg/a.go
+++ b/pkg/a.go
@@ -10,3 +10,4 @@ func A() {
 \tx := 1
-\ty := 2
+\ty := 3
+\tz := 4
 \treturn
@@ -40,2 +41,0 @@ func B() {
-\tdead()
-\tdead()
diff --git a/pkg/new.go b/pkg/new.go
new file mode 100644
--- /dev/null
+++ b/pkg/new.go
@@ -0,0 +1 @@
+package pkg
diff --git a/pkg/gone.go b/pkg/gone.go
deleted file mode 100644
--- a/pkg/gone.go
+++ /dev/null
@@ -1,2 +0,0 @@
-package pkg
-func Gone() {}
";

        assert_eq!(
            read(diff),
            Ok(vec![
                changed("pkg/a.go", 10..=13),
                changed("pkg/a.go", 41..=42),
                changed("pkg/new.go", 1..=1),
                changed("pkg/gone.go", 1..=u32::MAX),
            ])
        );
    }

    #[test]
    fn a_changed_line_that_reads_like_a_header_stays_in_its_hunk() {
        let diff = "--- a/x.txt\t2026-10-18\n+++ b/x.txt\t2026-10-18\n@@ -1 +1,2 @@\n-old\n+++ b/y.txt\n+@@ -1 +1 @@\n";

        assert_eq!(read(diff), Ok(vec![changed("x.txt", 1..=2)]));
    }

    #[test]
    fn an_empty_line_in_a_hunk_is_context() {
        assert_eq!(
            read("+++ b/x\n@@ -1,3 +1,3 @@\n a\n\n-b\n+c\n"),
            Ok(vec![changed("x", 1..=3)])
        );
    }

    #[test]
    fn a_hunk_without_its_file_or_a_line_that_does_not_read_is_named_by_its_line() {
        let no_file = "the hunk at line 1 of the diff has no +++ file header before it - pass the diff unedited";
        assert_eq!(read("@@ -1 +1 @@\n-a\n+b\n"), Err(no_file.to_owned()));
        assert_eq!(
            read("+++ b/x\n@@ -1 +1 @@\n-a\n+b\n--- a/y\n@@ -1 +1 @@\n-a\n+b\n"),
            Err(no_file.replace("line 1", "line 6"))
        );
        assert_eq!(
            read("+++ b/x\n@@ -1 +one @@\n"),
            Err(r#"line 2 of the diff is not a hunk header: "@@ -1 +one @@""#.to_owned())
        );
        assert_eq!(
            read("+++ b/x\n@@ -1,2 +1,2 @@\n a\nb\n"),
            Err(r#"line 4 of the diff is no line of the hunk at line 2: "b" - pass the diff unedited"#.to_owned())
        );
        assert!(read("+++ b/x\n@@ -1,3 +1,3 @@\n a\n").is_err());
        assert_eq!(
            read("+++ b/x\n@@ -0,0 +1 @@\n-a\n+b\n"),
            Err(r#"line 3 of the diff is no line of the hunk at line 2: "-a" - pass the diff unedited"#.to_owned())
        );
    }
}
