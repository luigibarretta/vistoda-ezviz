//! Asserts that every ci-local manifest step runs in exactly one CI job, so the
//! local gate (`ci-local.json`) and the Gitea workflow cannot drift apart. A
//! job owns a step when it calls `ci-local.mjs --ci [--no-needs] --only a,b`;
//! steps pulled in through `needs` are not assignments. It also fails when a
//! `--no-needs` call relies on a step no earlier call in the job ran, when the
//! workflow repeats a manifest command outside ci-local, and when the audit
//! step stops auditing the Cargo lockfile through `audit-all-lockfiles`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

pub const AUDIT_STEP: &str = "audit-lockfiles";
/// Shorter commands (for example `sh x.sh`) are too generic to flag.
const MIN_DUPLICATE_COMMAND: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub command: Option<String>,
    pub uses: Option<String>,
    pub needs: Vec<String>,
    pub ecosystems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub job: String,
    pub only: Vec<String>,
    pub no_needs: bool,
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the steps of a ci-local manifest.
pub fn manifest_steps(manifest: &Value) -> Result<Vec<Step>, String> {
    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("manifest has no steps array")?;
    steps
        .iter()
        .map(|step| {
            let name = step
                .get("name")
                .and_then(Value::as_str)
                .ok_or("manifest step without a name")?;
            let command = match step.get("run") {
                Some(Value::String(text)) => Some(text.clone()),
                Some(argv @ Value::Array(_)) => Some(strings(Some(argv)).join(" ")),
                _ => None,
            };
            let ecosystems = step
                .get("with")
                .and_then(|with| with.get("ecosystems"))
                .map_or_else(
                    || vec!["cargo".to_owned(), "npm".to_owned()],
                    |v| strings(Some(v)),
                );
            Ok(Step {
                name: name.to_owned(),
                command,
                uses: step.get("uses").and_then(Value::as_str).map(str::to_owned),
                needs: strings(step.get("needs")),
                ecosystems,
            })
        })
        .collect()
}

fn job_header(line: &str) -> Option<&str> {
    let name = line.strip_prefix("  ")?.strip_suffix(':')?;
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then_some(name)
}

fn parse_call(job: Option<&str>, args: &str, line: &str) -> Result<Call, String> {
    let mut tokens = args.split_whitespace();
    let (mut ci, mut no_needs, mut only) = (false, false, None);
    while let Some(token) = tokens.next() {
        match token {
            "--ci" => ci = true,
            "--no-needs" => no_needs = true,
            "--only" => only = tokens.next(),
            flag if flag.starts_with("--") => {}
            _ => break,
        }
    }
    let job = job.ok_or_else(|| format!("ci-local call outside a job: {line}"))?;
    if !ci {
        return Err(format!("job {job}: ci-local call without --ci: {line}"));
    }
    let only = only.ok_or_else(|| format!("job {job}: ci-local call without --only: {line}"))?;
    Ok(Call {
        job: job.to_owned(),
        only: only.split(',').map(str::to_owned).collect(),
        no_needs,
    })
}

/// Returns the ci-local calls in workflow order plus malformed-call problems.
pub fn workflow_calls(workflow: &str) -> (Vec<Call>, Vec<String>) {
    let (mut calls, mut problems) = (Vec::new(), Vec::new());
    let (mut in_jobs, mut job) = (false, None);
    for line in workflow.lines() {
        if line.trim_end() == "jobs:" {
            in_jobs = true;
        }
        if in_jobs && let Some(name) = job_header(line.trim_end()) {
            job = Some(name);
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((_, args)) = line.split_once("ci-local.mjs") else {
            continue;
        };
        match parse_call(job, args, line.trim()) {
            Ok(call) => calls.push(call),
            Err(problem) => problems.push(problem),
        }
    }
    (calls, problems)
}

fn closure(names: &[String], by_name: &BTreeMap<&str, &Step>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut queue: Vec<String> = names.to_vec();
    while let Some(name) = queue.pop() {
        if let Some(step) = by_name.get(name.as_str())
            && seen.insert(name.clone())
        {
            queue.extend(step.needs.iter().cloned());
        }
    }
    seen
}

fn missing_needs(
    call: &Call,
    by_name: &BTreeMap<&str, &Step>,
    ready: &BTreeSet<String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for step in call
        .only
        .iter()
        .filter_map(|name| by_name.get(name.as_str()))
    {
        for need in &step.needs {
            if !ready.contains(need) && !call.only.contains(need) {
                problems.push(format!(
                    "job {}: --no-needs step {} needs {need}, which no earlier ci-local call in the job ran",
                    call.job, step.name
                ));
            }
        }
    }
    problems
}

/// CI must run manifest commands through ci-local, not repeat them in YAML.
fn duplicated_commands(steps: &[Step], workflow: &str) -> Vec<String> {
    let lines = workflow
        .lines()
        .filter(|line| !line.contains("ci-local.mjs"));
    let yaml = lines.collect::<Vec<_>>().join("\n");
    steps
        .iter()
        .filter_map(|step| Some((step, step.command.as_deref()?)))
        .filter(|(_, command)| command.len() >= MIN_DUPLICATE_COMMAND && yaml.contains(command))
        .map(|(step, command)| {
            format!(
                "workflow repeats the command of step {} outside ci-local: \"{command}\"",
                step.name
            )
        })
        .collect()
}

/// ci-local fails closed on a lockfile no audit step covers; this keeps the
/// one audit step responsible for the Cargo lockfile.
pub fn check_audit_step(audit: Option<&Step>) -> Vec<String> {
    let Some(audit) = audit else {
        return vec![format!("step {AUDIT_STEP} is missing from the manifest")];
    };
    if audit.uses.as_deref() != Some("audit-all-lockfiles") {
        return vec![format!("step {AUDIT_STEP} must use audit-all-lockfiles")];
    }
    if audit.ecosystems.iter().any(|e| e == "cargo") {
        Vec::new()
    } else {
        vec![format!("step {AUDIT_STEP} does not audit cargo")]
    }
}

/// Returns every ownership problem; an empty list means the gate is aligned.
pub fn check_assignments(steps: &[Step], workflow: &str) -> Vec<String> {
    let by_name: BTreeMap<&str, &Step> = steps.iter().map(|s| (s.name.as_str(), s)).collect();
    let (calls, mut problems) = workflow_calls(workflow);
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut available: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for call in &calls {
        for name in &call.only {
            match by_name.get_key_value(name.as_str()) {
                Some((key, _)) => owners.entry(key).or_default().push(&call.job),
                None => problems.push(format!("job {}: unknown ci-local step {name}", call.job)),
            }
        }
        let ready = available.entry(&call.job).or_default();
        if call.no_needs {
            problems.extend(missing_needs(call, &by_name, ready));
            ready.extend(call.only.iter().cloned());
        } else {
            ready.extend(closure(&call.only, &by_name));
        }
    }
    for step in steps {
        let jobs = owners.get(step.name.as_str()).cloned().unwrap_or_default();
        match jobs.len() {
            0 => problems.push(format!("step {} runs in no CI job", step.name)),
            1 => {}
            count => problems.push(format!(
                "step {} runs in {count} CI assignments: {}",
                step.name,
                jobs.join(", ")
            )),
        }
    }
    problems.extend(duplicated_commands(steps, workflow));
    problems.extend(check_audit_step(by_name.get(AUDIT_STEP).copied()));
    problems
}
