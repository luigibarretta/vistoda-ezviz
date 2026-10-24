//! Every ci-local step runs in exactly one Gitea CI job (see
//! `tests/support/ci_local_jobs.rs`), plus unit tests of the checker.

#[path = "support/ci_local_jobs.rs"]
mod checker;

use std::{fs, path::PathBuf};

use checker::{AUDIT_STEP, Step, check_assignments, check_audit_step, workflow_calls};

const WORKFLOW: &str = ".gitea/workflows/ci.yaml";

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn every_manifest_step_runs_in_exactly_one_ci_job() {
    let manifest = serde_json::from_str(&read("ci-local.json"))
        .unwrap_or_else(|error| panic!("ci-local.json: {error}"));
    let steps = checker::manifest_steps(&manifest).unwrap_or_else(|error| panic!("{error}"));
    let problems = check_assignments(&steps, &read(WORKFLOW));
    assert!(
        problems.is_empty(),
        "ci-local jobs:\n{}",
        problems.join("\n")
    );
}

fn step(name: &str, needs: &[&str]) -> Step {
    Step {
        name: name.to_owned(),
        command: Some(format!("echo {name}")),
        uses: None,
        needs: needs.iter().map(|need| (*need).to_owned()).collect(),
        ecosystems: Vec::new(),
    }
}

fn audit() -> Step {
    Step {
        name: AUDIT_STEP.to_owned(),
        command: None,
        uses: Some("audit-all-lockfiles".to_owned()),
        needs: Vec::new(),
        ecosystems: vec!["cargo".to_owned()],
    }
}

fn check(mut steps: Vec<Step>, jobs: &str) -> Vec<String> {
    steps.push(audit());
    let workflow = format!(
        "jobs:\n  audit:\n      - run: node x/ci-local.mjs --ci --only {AUDIT_STEP}\n{jobs}"
    );
    check_assignments(&steps, &workflow)
}

#[test]
fn every_step_owned_by_exactly_one_job_passes() {
    let jobs = "  a:\n    steps:\n      - run: node x/ci-local.mjs --ci --only one,two\n";
    assert!(check(vec![step("one", &[]), step("two", &[])], jobs).is_empty());
}

#[test]
fn unassigned_duplicated_and_unknown_steps_fail() {
    let jobs = "  a:\n      - run: node x/ci-local.mjs --ci --only one,ghost\n  b:\n      - run: node x/ci-local.mjs --ci --only one\n";
    let problems = check(vec![step("one", &[]), step("lost", &[])], jobs);
    assert!(problems.contains(&"job a: unknown ci-local step ghost".to_owned()));
    assert!(problems.contains(&"step one runs in 2 CI assignments: a, b".to_owned()));
    assert!(problems.contains(&"step lost runs in no CI job".to_owned()));
}

#[test]
fn a_step_pulled_in_only_through_needs_is_not_an_assignment() {
    let steps = vec![step("install", &[]), step("build", &["install"])];
    let jobs = "  a:\n      - run: node x/ci-local.mjs --ci --only build\n";
    assert_eq!(check(steps, jobs), ["step install runs in no CI job"]);
}

#[test]
fn no_needs_requires_an_earlier_call_in_the_same_job() {
    let steps = || {
        vec![
            step("install", &[]),
            step("build", &["install"]),
            step("e2e", &["build"]),
        ]
    };
    let ok = "  a:\n      - run: node x/ci-local.mjs --ci --only install\n  b:\n      - run: node x/ci-local.mjs --ci --only build\n      - run: node x/ci-local.mjs --ci --no-needs --only e2e\n";
    assert!(check(steps(), ok).is_empty());
    let other_job = ok.replace("--only build\n", "--only build\n  c:\n");
    assert!(
        check(steps(), &other_job)
            .iter()
            .any(|problem| problem.contains("--no-needs step e2e needs build"))
    );
}

#[test]
fn workflow_calls_must_use_ci_and_only() {
    let (_, problems) = workflow_calls(
        "jobs:\n  a:\n      - run: node x/ci-local.mjs --quick\n      - run: node x/ci-local.mjs --only a\n",
    );
    assert_eq!(problems.len(), 2);
    let (calls, problems) =
        workflow_calls("jobs:\n  a:\n      # run `node x/ci-local.mjs --quick`\n");
    assert!(calls.is_empty() && problems.is_empty());
}

#[test]
fn the_workflow_must_not_repeat_a_manifest_command() {
    let mut clippy = step("clippy", &[]);
    clippy.command = Some("cargo clippy --locked -- -D warnings".to_owned());
    let jobs = "  a:\n      - run: node x/ci-local.mjs --ci --only clippy\n      - run: cargo clippy --locked -- -D warnings\n";
    assert_eq!(
        check(vec![clippy], jobs),
        [
            "workflow repeats the command of step clippy outside ci-local: \"cargo clippy --locked -- -D warnings\""
        ]
    );
}

#[test]
fn the_audit_step_must_audit_the_cargo_lockfile() {
    assert!(check_audit_step(Some(&audit())).is_empty());
    let mut npm_only = audit();
    npm_only.ecosystems = vec!["npm".to_owned()];
    assert_eq!(
        check_audit_step(Some(&npm_only)),
        ["step audit-lockfiles does not audit cargo"]
    );
    let mut plain = audit();
    plain.uses = None;
    assert_eq!(
        check_audit_step(Some(&plain)),
        ["step audit-lockfiles must use audit-all-lockfiles"]
    );
    assert_eq!(
        check_audit_step(None),
        ["step audit-lockfiles is missing from the manifest"]
    );
}
