//! The command line an app's menu entry (`.desktop` file) starts it with, so
//! an editor opened from GitKraft starts exactly as it does from the app menu:
//! with the environment, flags and executable (an AppImage, say) the user put
//! in its launcher.

use std::path::{Path, PathBuf};

/// The command for the first desktop entry found among `ids` (such as
/// `"lite-xl.desktop"`), opening `file`, and the working folder the entry asks
/// for. `None` when no entry exists, it is hidden, it has no usable `Exec`,
/// or its `TryExec` program is missing.
pub fn command_for(ids: &[&str], file: &Path) -> Option<(Vec<String>, Option<PathBuf>)> {
    command_in(&applications_dirs(), ids, file)
}

/// [`command_for`] over the given `applications` folders, searched in order.
fn command_in(
    dirs: &[PathBuf],
    ids: &[&str],
    file: &Path,
) -> Option<(Vec<String>, Option<PathBuf>)> {
    let path = ids
        .iter()
        .flat_map(|id| dirs.iter().map(move |d| d.join(id)))
        .find(|p| p.is_file())?;
    let contents = std::fs::read_to_string(path).ok()?;
    let entry = parse(&contents)?;
    if entry.hidden
        || entry
            .try_exec
            .as_deref()
            .is_some_and(|t| !program_exists(t))
    {
        return None;
    }
    let argv = exec_argv(&entry.exec, file)?;
    Some((argv, entry.path.map(PathBuf::from)))
}

/// Where desktop entries live, most specific first: `$XDG_DATA_HOME`
/// (`~/.local/share`) then each of `$XDG_DATA_DIRS`.
fn applications_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(std::env::split_paths(&system));
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// A program named in `TryExec`: an absolute path that exists, or a name on `PATH`.
fn program_exists(program: &str) -> bool {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// The keys of a `[Desktop Entry]` group that matter for launching.
#[derive(Debug, Default, PartialEq, Eq)]
struct Entry {
    exec: String,
    try_exec: Option<String>,
    path: Option<String>,
    hidden: bool,
}

/// Reads the `[Desktop Entry]` group (not the `[Desktop Action …]` ones, nor
/// localised keys). `None` without an `Exec`.
fn parse(contents: &str) -> Option<Entry> {
    let mut entry = Entry::default();
    let mut in_main = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unescape_string(value.trim());
        match key.trim() {
            "Exec" => entry.exec = value,
            "TryExec" if !value.is_empty() => entry.try_exec = Some(value),
            "Path" if !value.is_empty() => entry.path = Some(value),
            "Hidden" => entry.hidden = value == "true",
            _ => {}
        }
    }
    (!entry.exec.trim().is_empty()).then_some(entry)
}

/// The escapes of a desktop-entry string value: `\s`, `\n`, `\t`, `\r`, `\\`.
/// Others are left for the `Exec` quoting rules.
fn unescape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Splits an `Exec` value into arguments (double quotes group, and inside them
/// `\"`, `` \` ``, `\$` and `\\` escape) and puts `file` where the entry takes
/// its files (`%f %F %u %U`), or at the end when it names none. Other field
/// codes are dropped; `%%` is a `%`. `None` when nothing is left to run.
fn exec_argv(exec: &str, file: &Path) -> Option<Vec<String>> {
    let file = file.to_string_lossy();
    let mut argv = Vec::new();
    let mut took_file = false;
    for (word, quoted) in split_exec(exec) {
        if !quoted && matches!(word.as_str(), "%f" | "%F" | "%u" | "%U") {
            argv.push(file.to_string());
            took_file = true;
            continue;
        }
        let mut arg = String::with_capacity(word.len());
        let mut chars = word.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                arg.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => arg.push('%'),
                Some('f' | 'F' | 'u' | 'U') => {
                    arg.push_str(&file);
                    took_file = true;
                }
                // %i (icon), %c (name), %k (entry path) and the deprecated codes.
                Some(_) | None => {}
            }
        }
        if !arg.is_empty() || quoted {
            argv.push(arg);
        }
    }
    if argv.is_empty() {
        return None;
    }
    if !took_file {
        argv.push(file.to_string());
    }
    Some(argv)
}

/// The words of an `Exec` value, each with whether it was quoted.
fn split_exec(exec: &str) -> Vec<(String, bool)> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut started = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                quoted = true;
                started = true;
            }
            '\\' if in_quotes => match chars.next() {
                Some(e @ ('"' | '`' | '$' | '\\')) => word.push(e),
                Some(other) => {
                    word.push('\\');
                    word.push(other);
                }
                None => word.push('\\'),
            },
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    words.push((std::mem::take(&mut word), quoted));
                }
                quoted = false;
                started = false;
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push((word, quoted));
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(exec: &str, file: &str) -> Vec<String> {
        exec_argv(exec, Path::new(file)).expect("argv")
    }

    #[test]
    fn an_env_launcher_keeps_its_variables_and_takes_the_file_in_place_of_its_field_code() {
        assert_eq!(
            argv(
                "env SDL_VIDEODRIVER=x11 /home/u/Applications/LiteXL.AppImage %F",
                "/tmp/a b.rs"
            ),
            [
                "env",
                "SDL_VIDEODRIVER=x11",
                "/home/u/Applications/LiteXL.AppImage",
                "/tmp/a b.rs"
            ]
        );
    }

    #[test]
    fn quoted_words_keep_their_spaces_and_escapes() {
        assert_eq!(
            argv(r#""/opt/Lite XL/lite-xl" --title "say \"hi\"" %f"#, "/x"),
            ["/opt/Lite XL/lite-xl", "--title", "say \"hi\"", "/x"]
        );
    }

    #[test]
    fn without_a_file_code_the_file_goes_last_and_other_codes_are_dropped() {
        assert_eq!(
            argv("lite-xl --icon %i %c 100%%", "/x"),
            ["lite-xl", "--icon", "100%", "/x"]
        );
    }

    #[test]
    fn a_file_code_inside_a_word_is_filled_in() {
        assert_eq!(argv("ed --open=%u", "/x"), ["ed", "--open=/x"]);
    }

    #[test]
    fn an_exec_of_only_field_codes_runs_nothing() {
        assert_eq!(exec_argv("%i %c", Path::new("/x")), None);
    }

    #[test]
    fn only_the_main_group_counts() {
        let e = parse(
            "# a comment\n[Desktop Entry]\nName=Lite XL\nExec[de]=nope\n\
             Exec=env SDL_VIDEODRIVER=x11 /a/LiteXL.AppImage %F\n\
             TryExec=/a/LiteXL.AppImage\nPath=/work\\sdir\n\
             [Desktop Action new]\nExec=lite-xl --new\n",
        )
        .expect("entry");
        assert_eq!(e.exec, "env SDL_VIDEODRIVER=x11 /a/LiteXL.AppImage %F");
        assert_eq!(e.try_exec.as_deref(), Some("/a/LiteXL.AppImage"));
        assert_eq!(e.path.as_deref(), Some("/work dir"));
        assert!(!e.hidden);
        assert_eq!(parse("[Desktop Entry]\nName=x\n"), None);
    }

    #[test]
    fn an_entry_on_disk_is_found_and_a_missing_try_exec_skips_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let apps = dir.path().join("applications");
        std::fs::create_dir_all(&apps).expect("mkdir");
        let program = dir.path().join("editor");
        std::fs::write(&program, "").expect("program");
        std::fs::write(
            apps.join("good.desktop"),
            format!(
                "[Desktop Entry]\nExec=env A=1 {} %F\nTryExec={}\n",
                program.display(),
                program.display()
            ),
        )
        .expect("good");
        std::fs::write(
            apps.join("gone.desktop"),
            "[Desktop Entry]\nExec=/nowhere/editor %F\nTryExec=/nowhere/editor\n",
        )
        .expect("gone");

        let found = |id: &str| {
            command_in(std::slice::from_ref(&apps), &[id], Path::new("/f")).map(|(argv, _)| argv)
        };
        assert_eq!(found("missing.desktop"), None);
        // The first id that exists wins.
        assert_eq!(
            command_in(
                std::slice::from_ref(&apps),
                &["missing.desktop", "good.desktop"],
                Path::new("/f")
            )
            .map(|(argv, _)| argv.len()),
            Some(4)
        );
        assert_eq!(
            found("good.desktop"),
            Some(vec![
                "env".into(),
                "A=1".into(),
                program.display().to_string(),
                "/f".into()
            ])
        );
        assert_eq!(found("gone.desktop"), None);
    }
}
