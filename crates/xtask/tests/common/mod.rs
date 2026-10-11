// Each test file uses its own part of this module.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn read_repository_file(name: &str) -> String {
    let path = repository_root().join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

pub fn read_workflow(name: &str) -> String {
    read_repository_file(&format!(".github/workflows/{name}"))
}

/// Every workflow file, as `(file name, text)`.
pub fn workflows() -> Vec<(String, String)> {
    let directory = repository_root().join(".github/workflows");
    let mut names: Vec<String> = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".yml"))
        .collect();
    names.sort();

    names
        .into_iter()
        .map(|name| {
            let text = read_workflow(&name);
            (name, text)
        })
        .collect()
}

/// Splits a workflow into the text of each job, in order, keyed by job id.
/// Comment lines are dropped so assertions only see what the workflow does.
pub fn jobs_of(workflow: &str) -> Vec<(String, String)> {
    const JOBS: &str = "\njobs:\n";
    let jobs_start = workflow.find(JOBS).expect("a workflow has jobs");

    let mut jobs: Vec<(String, String)> = Vec::new();
    for line in workflow[jobs_start + JOBS.len()..].split('\n') {
        if let Some(id) = job_header(line) {
            jobs.push((id.to_string(), String::new()));
        } else if let Some((_, text)) = jobs.last_mut()
            && !line.trim_start().starts_with('#')
        {
            text.push_str(line);
            text.push('\n');
        }
    }

    jobs
}

/// The id in a `  job-id:` line.
fn job_header(line: &str) -> Option<&str> {
    let id = line.strip_prefix("  ")?.strip_suffix(':')?;
    let mut characters = id.chars();
    let valid = characters.next()?.is_ascii_alphabetic()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character));
    valid.then_some(id)
}

pub fn job<'a>(jobs: &'a [(String, String)], id: &str) -> &'a str {
    jobs.iter()
        .find(|(job_id, _)| job_id == id)
        .map(|(_, text)| text.as_str())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| panic!("expected a \"{id}\" job"))
}

pub fn job_ids(jobs: &[(String, String)]) -> Vec<&str> {
    jobs.iter().map(|(id, _)| id.as_str()).collect()
}

pub fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// How many lines of `text` are exactly `line`.
pub fn count_lines(text: &str, line: &str) -> usize {
    text.lines().filter(|candidate| *candidate == line).count()
}

/// The steps of a job, each from its `      - ` line to the next one.
pub fn steps_of(job: &str) -> Vec<&str> {
    const STEP: &str = "\n      - ";
    let Some(first) = job.find(STEP) else {
        return Vec::new();
    };

    let mut steps = Vec::new();
    let mut start = first + 1;
    while let Some(next) = job[start..].find(STEP) {
        steps.push(&job[start..start + next + 1]);
        start += next + 1;
    }
    steps.push(&job[start..]);
    steps
}

/// A directory under the system temporary directory, removed when dropped.
pub struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "mymcps-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn write(&self, name: &str, contents: &str) {
        fs::write(self.0.join(name), contents).unwrap();
    }

    pub fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// True when `block` appears in `text` starting at the beginning of a line.
pub fn has_lines(text: &str, block: &str) -> bool {
    text.starts_with(block) || text.contains(&format!("\n{block}"))
}

/// The words of `text`: its runs of letters, digits and underscores.
pub fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
}

pub fn is_lowercase_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// `text` without its comment lines.
pub fn without_comments(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| [line, "\n"])
        .collect()
}
