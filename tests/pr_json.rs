use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

static FIXTURES: AtomicU64 = AtomicU64::new(0);

struct Repo {
    parent: PathBuf,
    repo: PathBuf,
}

impl Repo {
    fn init(branch: &str) -> Self {
        let id = FIXTURES.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let parent = std::env::temp_dir().join(format!("chunk-pr-json-{id}-{nanos}"));
        let repo = parent.join("repo");
        fs::create_dir_all(&repo).expect("create fixture");
        git(&repo, &["init", "-b", branch]);
        git(&repo, &["config", "user.name", "Chunk Test"]);
        git(&repo, &["config", "user.email", "chunk@example.test"]);
        Self { parent, repo }
    }

    fn path(&self) -> &Path {
        &self.repo
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.parent);
    }
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap_or_else(|error| panic!("spawn git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn write(dir: &Path, name: &str, contents: &str) {
    fs::write(dir.join(name), contents).unwrap_or_else(|error| panic!("write {name}: {error}"));
}

fn chunk(dir: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_chunk"))
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn chunk: {error}"));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("chunk still running after 5 seconds");
        }
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().expect("read chunk output"),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("wait for chunk: {error}"),
        }
    }
}

fn run_json(dir: &Path, args: &[&str]) -> (Vec<u8>, Value) {
    let output = chunk(dir, args);
    assert!(
        output.status.success(),
        "chunk {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is utf-8");
    assert!(
        stdout.ends_with('\n'),
        "stdout missing trailing newline: {stdout:?}"
    );
    let value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout is not one JSON value: {error}\n{stdout}"));
    (output.stdout, value)
}

fn paths(value: &Value) -> Vec<&str> {
    value["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("path"))
        .collect()
}

fn assert_committed_stages(value: &Value) {
    for file in value["files"].as_array().expect("files") {
        assert_eq!(file["stage"], "committed");
        for hunk in file["hunks"].as_array().expect("hunks") {
            assert_eq!(hunk["stage"], "committed");
        }
    }
}

fn assert_unstaged_stages(value: &Value) {
    for file in value["files"].as_array().expect("files") {
        assert_eq!(file["stage"], "unstaged");
        for hunk in file["hunks"].as_array().expect("hunks") {
            assert_eq!(hunk["stage"], "unstaged");
        }
    }
}

fn with_pr_identity(diff: &Value) -> Value {
    let mut expected = diff.clone();
    expected["title"] = json!("PR review feature into main");
    expected["source_label"] = json!("git diff main...HEAD");
    for file in expected["files"].as_array_mut().expect("files") {
        file["stage"] = json!("committed");
        for hunk in file["hunks"].as_array_mut().expect("hunks") {
            hunk["stage"] = json!("committed");
        }
    }
    expected
}

#[test]
fn pr_json_explicit_base_prints_committed_feature_file() {
    let repo = Repo::init("main");
    write(repo.path(), "base.txt", "base\n");
    git(repo.path(), &["add", "base.txt"]);
    git(repo.path(), &["commit", "-m", "main"]);
    git(repo.path(), &["checkout", "-b", "feature"]);
    write(repo.path(), "feature.txt", "feature\n");
    git(repo.path(), &["add", "feature.txt"]);
    git(repo.path(), &["commit", "-m", "feature"]);
    write(repo.path(), "base.txt", "dirty\n");
    write(repo.path(), "untracked.txt", "untracked\n");
    write(repo.path(), "staged.txt", "staged\n");
    git(repo.path(), &["add", "staged.txt"]);

    let (main_then_json, value) = run_json(repo.path(), &["pr", "main", "--json"]);
    let (json_then_main, _) = run_json(repo.path(), &["pr", "--json", "main"]);
    assert_eq!(main_then_json, json_then_main);
    assert_eq!(value["title"], "PR review feature into main");
    assert_eq!(value["source_label"], "git diff main...HEAD");
    assert_eq!(paths(&value), ["feature.txt"]);

    let file = &value["files"][0];
    assert_eq!(file["status"], "added");
    assert_eq!(file["stage"], "committed");
    assert_eq!(file["additions"], 1);
    assert_eq!(file["deletions"], 0);
    let hunk = &file["hunks"][0];
    assert_eq!(hunk["stage"], "committed");
    let lines = hunk["lines"].as_array().expect("lines");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["kind"], "added");
    assert_eq!(lines[0]["content"], "feature");
}

#[test]
fn pr_json_omitted_base_falls_back_to_origin_head_main_then_master() {
    let origin_repo = Repo::init("master");
    write(origin_repo.path(), "only-master.txt", "master\n");
    git(origin_repo.path(), &["add", "only-master.txt"]);
    git(origin_repo.path(), &["commit", "-m", "master"]);
    git(origin_repo.path(), &["checkout", "-b", "main"]);
    write(origin_repo.path(), "only-main.txt", "main\n");
    git(origin_repo.path(), &["add", "only-main.txt"]);
    git(origin_repo.path(), &["commit", "-m", "main"]);
    git(origin_repo.path(), &["checkout", "-b", "feature"]);
    write(origin_repo.path(), "only-feature.txt", "feature\n");
    git(origin_repo.path(), &["add", "only-feature.txt"]);
    git(origin_repo.path(), &["commit", "-m", "feature"]);
    let origin = origin_repo.parent.join("origin.git");
    git(
        origin_repo.path(),
        &["init", "--bare", origin.to_str().expect("origin path")],
    );
    git(
        origin_repo.path(),
        &[
            "remote",
            "add",
            "origin",
            origin.to_str().expect("origin path"),
        ],
    );
    git(origin_repo.path(), &["push", "origin", "main"]);
    git(
        origin_repo.path(),
        &["remote", "set-head", "origin", "main"],
    );

    let (_, origin_value) = run_json(origin_repo.path(), &["pr", "--json"]);
    assert_eq!(origin_value["title"], "PR review feature into origin/HEAD");
    assert_eq!(origin_value["source_label"], "git diff origin/HEAD...HEAD");
    assert_eq!(paths(&origin_value), ["only-feature.txt"]);
    assert_committed_stages(&origin_value);

    let main_repo = Repo::init("main");
    write(main_repo.path(), "only-main.txt", "main\n");
    git(main_repo.path(), &["add", "only-main.txt"]);
    git(main_repo.path(), &["commit", "-m", "main"]);
    git(main_repo.path(), &["checkout", "-b", "feature"]);
    write(main_repo.path(), "only-feature.txt", "feature\n");
    git(main_repo.path(), &["add", "only-feature.txt"]);
    git(main_repo.path(), &["commit", "-m", "feature"]);

    let (_, main_value) = run_json(main_repo.path(), &["pr", "--json"]);
    assert_eq!(main_value["title"], "PR review feature into main");
    assert_eq!(main_value["source_label"], "git diff main...HEAD");
    assert_eq!(paths(&main_value), ["only-feature.txt"]);
    assert_committed_stages(&main_value);

    let master_repo = Repo::init("master");
    write(master_repo.path(), "only-master.txt", "master\n");
    git(master_repo.path(), &["add", "only-master.txt"]);
    git(master_repo.path(), &["commit", "-m", "master"]);
    git(master_repo.path(), &["checkout", "-b", "feature"]);
    write(master_repo.path(), "only-feature.txt", "feature\n");
    git(master_repo.path(), &["add", "only-feature.txt"]);
    git(master_repo.path(), &["commit", "-m", "feature"]);

    let (omitted, master_value) = run_json(master_repo.path(), &["pr", "--json"]);
    let (whitespace, _) = run_json(master_repo.path(), &["pr", " \t ", "--json"]);
    assert_eq!(omitted, whitespace);
    assert_eq!(master_value["title"], "PR review feature into master");
    assert_eq!(master_value["source_label"], "git diff master...HEAD");
    assert_eq!(paths(&master_value), ["only-feature.txt"]);
    assert_committed_stages(&master_value);
}

#[test]
fn pr_json_matches_diff_json_except_identity_and_stage() {
    let repo = Repo::init("main");
    write(repo.path(), "a-modify.txt", "old\n");
    write(repo.path(), "b-delete.txt", "gone\n");
    git(repo.path(), &["add", "a-modify.txt", "b-delete.txt"]);
    git(repo.path(), &["commit", "-m", "main"]);
    git(repo.path(), &["checkout", "-b", "feature"]);
    write(repo.path(), "a-modify.txt", "new\n");
    fs::remove_file(repo.path().join("b-delete.txt")).expect("delete fixture file");
    write(repo.path(), "c-add.txt", "added\n");
    git(
        repo.path(),
        &["add", "a-modify.txt", "b-delete.txt", "c-add.txt"],
    );
    git(repo.path(), &["commit", "-m", "feature"]);

    let (_, pr) = run_json(repo.path(), &["pr", "main", "--json"]);
    git(repo.path(), &["reset", "-q", "main"]);
    let (_, diff) = run_json(repo.path(), &["diff", "--json"]);
    assert_unstaged_stages(&diff);
    assert_eq!(pr, with_pr_identity(&diff));
}

#[test]
fn pr_json_missing_base_prints_nothing() {
    let repo = Repo::init("main");
    git(repo.path(), &["commit", "--allow-empty", "-m", "main"]);

    let output = chunk(repo.path(), &["pr", "does-not-exist", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn pr_json_empty_range_prints_no_files() {
    let repo = Repo::init("main");
    git(repo.path(), &["commit", "--allow-empty", "-m", "main"]);

    let (_, value) = run_json(repo.path(), &["pr", "main", "--json"]);
    assert_eq!(value["files"], json!([]));
}
