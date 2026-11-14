//! Cache of `cargo metadata` results, shared by all the crates of a workspace.

#[cfg(test)]
use std::sync::atomic::Ordering;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use cargo_metadata::{CargoOpt, Metadata, Node, Package, PackageId};

/// The result of a single `cargo metadata` call, with indexes for fast lookups.
pub struct WorkspaceMetadata {
    metadata: Metadata,
    /// Index into `metadata.packages`.
    packages: HashMap<PackageId, usize>,
    /// Index into `metadata.resolve.nodes`.
    nodes: HashMap<PackageId, usize>,
}

impl WorkspaceMetadata {
    fn new(metadata: Metadata) -> Self {
        let packages = metadata
            .packages
            .iter()
            .enumerate()
            .map(|(i, pkg)| (pkg.id.clone(), i))
            .collect();

        let nodes = metadata
            .resolve
            .iter()
            .flat_map(|resolve| resolve.nodes.iter().enumerate())
            .map(|(i, node)| (node.id.clone(), i))
            .collect();

        Self {
            metadata,
            packages,
            nodes,
        }
    }

    /// Returns the package description (including the original dependency tables).
    pub fn package(&self, id: &PackageId) -> Option<&Package> {
        self.packages.get(id).map(|&i| &self.metadata.packages[i])
    }

    /// Returns the node of the resolved dependency graph for the given package.
    pub fn node(&self, id: &PackageId) -> Option<&Node> {
        let resolve = self.metadata.resolve.as_ref()?;
        self.nodes.get(id).map(|&i| &resolve.nodes[i])
    }
}

/// A package, along with the metadata of the workspace it belongs to.
pub type CachedPackage = (PackageId, Arc<WorkspaceMetadata>);

#[derive(Default)]
pub struct MetadataCache {
    /// Maps the canonical path of a manifest to its package and workspace metadata.
    by_manifest: Mutex<HashMap<PathBuf, CachedPackage>>,

    /// One lock per probable workspace, so that crates of the same workspace analyzed in parallel
    /// wait for a single `cargo metadata` call, instead of each running their own.
    locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,

    /// Number of times `cargo-metadata` has been called, for testing purposes.
    #[cfg(test)]
    pub num_cargo_calls: std::sync::atomic::AtomicUsize,
}

/// Return the canonicalized path or the initial one, if canonicalization failed.
fn canonicalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

impl MetadataCache {
    fn lookup(&self, canonical_manifest_path: &Path) -> Option<CachedPackage> {
        self.by_manifest
            .lock()
            .unwrap()
            .get(canonical_manifest_path)
            .cloned()
    }

    /// Returns the metadata for the package described by the manifest at the given path, running
    /// `cargo metadata` only if no previous call covered this manifest.
    ///
    /// `probable_workspace` is the path to the manifest of the workspace this package likely
    /// belongs to (or the package's own manifest path, if there's none). It's only used to
    /// serialize calls for the same workspace: a wrong guess results in an extra `cargo metadata`
    /// call, not in a wrong result, since cargo is the one deciding about workspace membership.
    pub fn get(
        &self,
        manifest_path: &Path,
        probable_workspace: &Path,
    ) -> anyhow::Result<CachedPackage> {
        let manifest_path = canonicalize(manifest_path);

        if let Some(found) = self.lookup(&manifest_path) {
            return Ok(found);
        }

        let workspace_lock = self
            .locks
            .lock()
            .unwrap()
            .entry(canonicalize(probable_workspace))
            .or_default()
            .clone();
        let _guard = workspace_lock.lock().unwrap();

        // Another thread may have filled the cache while we were waiting for the lock.
        if let Some(found) = self.lookup(&manifest_path) {
            return Ok(found);
        }

        log::debug!("running cargo metadata for {}", manifest_path.display());

        #[cfg(test)]
        self.num_cargo_calls.fetch_add(1, Ordering::SeqCst);

        let metadata = cargo_metadata::MetadataCommand::new()
            .features(CargoOpt::AllFeatures)
            .manifest_path(&manifest_path)
            //.other_options(["--frozen".to_owned()]) // TODO causes errors in cargo-metadata
            .exec()?;

        let workspace = Arc::new(WorkspaceMetadata::new(metadata));

        // Register the result for every member of the workspace, as reported by cargo.
        let mut by_manifest = self.by_manifest.lock().unwrap();
        for id in &workspace.metadata.workspace_members {
            if let Some(pkg) = workspace.package(id) {
                by_manifest.insert(
                    canonicalize(pkg.manifest_path.as_std_path()),
                    (id.clone(), workspace.clone()),
                );
            }
        }

        by_manifest.get(&manifest_path).cloned().with_context(|| {
            format!(
                "cargo metadata didn't return a package for {}",
                manifest_path.display()
            )
        })
    }
}
