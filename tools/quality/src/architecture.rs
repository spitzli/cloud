//! Cargo package identities and manifest policy, not source-text dependency guesses.
use crate::{Error, Result};
use cargo_metadata::{CargoOpt, DependencyKind, Metadata, MetadataCommand, PackageId};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Zone {
    Platform,
    Provider,
    Shared,
    Other,
}
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub zone: Zone,
    pub dependencies: BTreeSet<String>,
}
pub fn violations(graph: &BTreeMap<String, Package>) -> Vec<String> {
    let mut findings = Vec::new();
    for (start, package) in graph {
        if !matches!(package.zone, Zone::Platform | Zone::Provider | Zone::Shared) {
            continue;
        }
        let mut queue = VecDeque::from([(start.clone(), vec![package.name.clone()])]);
        let mut seen = BTreeSet::new();
        while let Some((id, path)) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(node) = graph.get(&id) else {
                continue;
            };
            let forbidden = matches!(
                (&package.zone, &node.zone),
                (Zone::Platform, Zone::Provider)
                    | (Zone::Provider, Zone::Platform)
                    | (Zone::Shared, Zone::Platform | Zone::Provider)
            );
            if forbidden {
                findings.push(format!("provider boundary: {}", path.join(" -> ")));
                continue;
            }
            for next in &node.dependencies {
                let mut path = path.clone();
                path.push(
                    graph
                        .get(next)
                        .map_or(next.as_str(), |p| p.name.as_str())
                        .into(),
                );
                queue.push_back((next.clone(), path));
            }
        }
    }
    findings.sort();
    findings.dedup();
    findings
}
fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.into(),
        source,
    })
}
fn manifest(path: &Path) -> Result<toml::Value> {
    toml::from_str(&read(path)?).map_err(|source| Error::Manifest {
        path: path.into(),
        source,
    })
}
fn canonical(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|source| Error::Io {
        path: path.into(),
        source,
    })
}
pub fn workspace_manifests(root: &Path) -> Result<Vec<PathBuf>> {
    let mut manifests = vec![root.join("Cargo.toml")];
    let entries = std::fs::read_dir(root.join("plugins")).map_err(|source| Error::Io {
        path: root.join("plugins"),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: root.join("plugins"),
            source,
        })?;
        let path = entry.path().join("Cargo.toml");
        if path.is_file() {
            manifests.push(path);
        }
    }
    manifests.sort();
    Ok(manifests)
}
const SHARED: &[&str] = &[
    "webdock-core",
    "webdock-ids",
    "webdock-agent-protocol",
    "webdock-plugin-protocol",
    "webdock-plugin-runtime",
    "webdock-plugin-http",
    "webdock-plugin-kit",
    "webdock-repository-protocol",
    "webdock-export-protocol",
    "webdock-object-storage",
    "webdock-mail-protocol",
    "webdock-mail-message",
    "webdock-mail-runtime",
];
fn zone(root: &Path, path: &Path, name: &str) -> Zone {
    // This vendor SDK is currently used only by the separately installed repository provider.
    if name == "octocrab" {
        return Zone::Provider;
    }
    if path.starts_with(root.join("plugins")) {
        return Zone::Provider;
    }
    if path.starts_with(root.join("crates")) {
        return if SHARED.contains(&name) {
            Zone::Shared
        } else {
            Zone::Platform
        };
    }
    Zone::Other
}
fn metadata(path: &Path) -> Result<Metadata> {
    MetadataCommand::new()
        .manifest_path(path)
        .features(CargoOpt::AllFeatures)
        .other_options(vec!["--locked".into()])
        .exec()
        .map_err(|_| Error::Metadata(path.into()))
}
pub fn graph(root: &Path, manifests: &[PathBuf]) -> Result<BTreeMap<String, Package>> {
    let root = canonical(root)?;
    let mut graph = BTreeMap::new();
    let mut pending = VecDeque::from(manifests.to_vec());
    let mut visited = BTreeSet::new();
    let mut identities = BTreeMap::new();
    let mut declared = Vec::new();
    while let Some(path) = pending.pop_front() {
        let path = canonical(&path)?;
        if !visited.insert(path.clone()) {
            continue;
        }
        let data = metadata(&path)?;
        for package in &data.packages {
            let path = canonical(package.manifest_path.as_std_path())?;
            identities.insert(path.clone(), package.id.clone());
            graph
                .entry(package.id.to_string())
                .or_insert_with(|| Package {
                    name: package.name.to_string(),
                    zone: zone(&root, &path, package.name.as_str()),
                    dependencies: BTreeSet::new(),
                });
            // Local optional dependencies can be hidden behind an unselected transitive feature.
            // Read their own metadata too; a target/feature flag must not bypass this boundary.
            for dependency in &package.dependencies {
                if dependency.kind != DependencyKind::Development
                    && let Some(directory) = &dependency.path
                {
                    let target = canonical(&directory.as_std_path().join("Cargo.toml"))?;
                    declared.push((package.id.clone(), target.clone()));
                    pending.push_back(target);
                }
            }
        }
        let resolve = data
            .resolve
            .as_ref()
            .ok_or_else(|| Error::Metadata(path.clone()))?;
        for node in &resolve.nodes {
            for dependency in &node.deps {
                if dependency.dep_kinds.is_empty()
                    || dependency
                        .dep_kinds
                        .iter()
                        .any(|kind| kind.kind != DependencyKind::Development)
                {
                    add_edge(&mut graph, &node.id, &dependency.pkg)?;
                }
            }
        }
    }
    // Match canonical package paths, never aliases, after even inactive optional dependencies
    // have been inspected. This also covers target-specific and build-only dependency edges.
    for (from, path) in declared {
        let target = identities
            .get(&path)
            .ok_or_else(|| Error::MissingPackage(path.display().to_string()))?;
        add_edge(&mut graph, &from, target)?;
    }
    Ok(graph)
}
fn add_edge(graph: &mut BTreeMap<String, Package>, from: &PackageId, to: &PackageId) -> Result<()> {
    graph
        .get_mut(&from.to_string())
        .ok_or_else(|| Error::MissingPackage(from.to_string()))?
        .dependencies
        .insert(to.to_string());
    Ok(())
}
pub fn lint_policy(root: &Path, manifests: &[PathBuf]) -> Result<Vec<String>> {
    let root_manifest = manifest(&root.join("Cargo.toml"))?;
    let Some(policy) = root_manifest.get("workspace").and_then(|v| v.get("lints")) else {
        return Ok(vec!["root workspace must define lints".into()]);
    };
    let mut findings = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).map_err(|source| Error::Io {
        path: root.join("crates"),
        source,
    })?;
    for entry in crates {
        let path = entry
            .map_err(|source| Error::Io {
                path: root.join("crates"),
                source,
            })?
            .path()
            .join("Cargo.toml");
        if !path.is_file() {
            continue;
        }
        let value = manifest(&path)?;
        if value
            .get("lints")
            .and_then(|v| v.get("workspace"))
            .and_then(toml::Value::as_bool)
            != Some(true)
        {
            findings.push(format!("{} must inherit workspace lints", path.display()));
        }
    }
    for path in manifests
        .iter()
        .filter(|path| *path != &root.join("Cargo.toml"))
    {
        if manifest(path)?.get("lints") != Some(policy) {
            findings.push(format!("{} must use the root lint policy", path.display()));
        }
    }
    Ok(findings)
}
