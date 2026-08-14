use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

const MAX_LINES: usize = 300;
const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    "build",
    "data",
    "dist",
    "target",
];
const MAINTAINED_SUFFIXES: &[&str] = &["md", "py", "rs", "toml", "yaml", "yml", "json"];

#[test]
fn maintained_files_stay_within_the_loc_budget() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    visit(&root, &mut files).unwrap_or_else(|error| panic!("repository scan failed: {error}"));
    files.sort();
    let mut violations = Vec::new();
    for path in files {
        let lines = count_lines(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        if lines > MAX_LINES {
            let relative = path.strip_prefix(&root).unwrap_or(&path);
            violations.push(format!("{}: {lines} > {MAX_LINES}", relative.display()));
        }
    }
    assert!(
        violations.is_empty(),
        "LOC budget exceeded:\n{}",
        violations.join("\n")
    );
}

#[test]
fn repository_has_no_python_sources() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    visit(&root, &mut files).unwrap_or_else(|error| panic!("repository scan failed: {error}"));
    let mut python = files
        .into_iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("py"))
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();
    python.sort();
    assert!(
        python.is_empty(),
        "Python sources are not allowed; use Rust equivalents:\n{}",
        python.join("\n")
    );
}

fn visit(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !EXCLUDED_DIRECTORIES.contains(&name.as_ref()) {
                visit(&path, files)?;
            }
        } else if file_type.is_file() && is_maintained(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_maintained(path: &Path) -> bool {
    if path.file_name().and_then(|name| name.to_str()) == Some("Dockerfile") {
        return true;
    }
    path.extension()
        .and_then(|suffix| suffix.to_str())
        .is_some_and(|suffix| MAINTAINED_SUFFIXES.contains(&suffix))
}

fn count_lines(path: &Path) -> std::io::Result<usize> {
    let file = fs::File::open(path)?;
    BufReader::new(file)
        .lines()
        .try_fold(0_usize, |count, line| line.map(|_| count + 1))
}
