use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};
use webdock_quality::{
    architecture, migrations,
    runner::{self, Scope},
    serde_contracts,
};

fn source_findings(root: &Path) -> Result<Vec<String>, String> {
    let mut findings = Vec::new();
    for folder in ["crates", "plugins"] {
        for entry in walkdir::WalkDir::new(root.join(folder))
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !matches!(
                    entry.file_name().to_str(),
                    Some("target" | "node_modules" | ".git")
                )
            })
        {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().is_file()
                || entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(entry.path())
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
            let violations = serde_contracts::violations(&source).map_err(|error| {
                format!("{}: Rust parse failed: {error}", entry.path().display())
            })?;
            for violation in violations {
                findings.push(format!(
                    "{}:{}: {}",
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap_or(entry.path())
                        .display(),
                    violation.line,
                    violation.message
                ));
            }
        }
    }
    Ok(findings)
}
fn check(root: &Path) -> Result<Vec<String>, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let manifests = architecture::workspace_manifests(&root).map_err(|error| error.to_string())?;
    let graph = architecture::graph(&root, &manifests).map_err(|error| error.to_string())?;
    let mut findings = architecture::violations(&graph);
    let mut lint_manifests = manifests;
    lint_manifests.push(root.join("tools/quality/Cargo.toml"));
    findings.extend(
        architecture::lint_policy(&root, &lint_manifests).map_err(|error| error.to_string())?,
    );
    findings.extend(source_findings(&root)?);
    findings.extend(migrations::violations(&root)?);
    Ok(findings)
}
fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let default_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (scope, root) = match runner::arguments(&arguments, default_root) {
        Ok(values) => values,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let result = if scope == Scope::Frontend {
        Ok(Vec::new())
    } else {
        check(&root)
    };
    match result {
        Ok(findings) if findings.is_empty() => {
            if scope != Scope::Frontend {
                println!(
                    "Rust architecture, lint policy, strict Serde contracts and frozen migrations passed."
                );
            }
            let result = run_plan(&root, scope);
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(findings) => {
            for finding in &findings {
                eprintln!("{finding}");
            }
            eprintln!("{} quality violations", findings.len());
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("Quality checks could not complete: {error}");
            ExitCode::FAILURE
        }
    }
}
fn run_plan(root: &Path, scope: Scope) -> Result<(), String> {
    let mut standalone =
        architecture::workspace_manifests(root).map_err(|error| error.to_string())?;
    standalone.retain(|manifest| manifest != &root.join("Cargo.toml"));
    standalone.push(root.join("tools/quality/Cargo.toml"));
    runner::execute(root, &runner::plan(scope, &standalone))
}
