//! A filesystem-backed [`BundleGraph`] for local testing.
//!
//! Starting from an entry file, it parses assets and resolves their
//! dependencies with `parcel_resolver`, following the same source-first
//! resolution the JS `DocsResolver` uses. Only workspace source files are
//! processed; anything resolving into `node_modules` (external packages like
//! `react`) is treated as external. This is single-threaded and intended for
//! testing — the real Parcel orchestrator will provide its own implementation.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use parcel_resolver::{
  Cache, ExportsCondition, Extensions, Fields, OsFileSystem, ResolveOptions, Resolution, Resolver,
  SpecifierType,
};

use crate::packager::{AssetId, BundleGraph};
use crate::{API, parse};

pub struct FsBundleGraph {
  apis: Vec<API>,
  paths: Vec<PathBuf>,
  path_to_id: HashMap<PathBuf, AssetId>,
  resolutions: HashMap<(AssetId, String), Option<AssetId>>,
  entry: AssetId,
}

impl FsBundleGraph {
  /// Builds the graph by parsing `entry` and transitively resolving/parsing all
  /// of its workspace dependencies. `project_root` is used for node_modules
  /// resolution.
  pub fn build(entry: &Path, project_root: &Path) -> FsBundleGraph {
    let cache = Cache::new(Arc::new(OsFileSystem));
    let mut resolver = Resolver::parcel(project_root, cache);
    // Prefer TypeScript source over built output, matching the JS DocsResolver.
    resolver.extensions = Extensions::Owned(
      ["ts", "tsx", "d.ts", "mjs", "js", "jsx", "cjs", "json"]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    resolver.entries = Fields::SOURCE | Fields::TYPES | Fields::MAIN | Fields::MODULE;
    resolver.conditions = ExportsCondition::SOURCE | ExportsCondition::TYPES;

    let mut graph = FsBundleGraph {
      apis: Vec::new(),
      paths: Vec::new(),
      path_to_id: HashMap::new(),
      resolutions: HashMap::new(),
      entry: 0,
    };

    let entry = std::fs::canonicalize(entry).unwrap_or_else(|_| entry.to_owned());
    graph.entry = graph.load(&entry);

    // Breadth-first: resolve and parse every workspace dependency.
    let mut queue = vec![graph.entry];
    let mut i = 0;
    while i < queue.len() {
      let asset = queue[i];
      i += 1;

      let mut specifiers: Vec<String> = graph.apis[asset]
        .dependencies
        .iter()
        .map(|s| s.to_string())
        .collect();
      specifiers.extend(graph.apis[asset].export_all.iter().map(|s| s.to_string()));

      let from = graph.paths[asset].clone();
      for specifier in specifiers {
        if graph.resolutions.contains_key(&(asset, specifier.clone())) {
          continue;
        }
        let resolved = resolve_source(&resolver, &specifier, &from);
        let id = resolved.map(|path| {
          let existed = graph.path_to_id.contains_key(&path);
          let id = graph.load(&path);
          if !existed {
            queue.push(id);
          }
          id
        });
        graph.resolutions.insert((asset, specifier), id);
      }
    }

    graph
  }

  fn load(&mut self, path: &Path) -> AssetId {
    if let Some(&id) = self.path_to_id.get(path) {
      return id;
    }
    let code = std::fs::read_to_string(path).unwrap_or_default();
    let api = parse(path, code);
    let id = self.apis.len();
    self.apis.push(api);
    self.paths.push(path.to_owned());
    self.path_to_id.insert(path.to_owned(), id);
    id
  }
}

impl BundleGraph for FsBundleGraph {
  fn entry(&self) -> AssetId {
    self.entry
  }

  fn api(&self, asset: AssetId) -> &API {
    &self.apis[asset]
  }

  fn resolve(&self, asset: AssetId, specifier: &str) -> Option<AssetId> {
    *self
      .resolutions
      .get(&(asset, specifier.to_string()))
      .unwrap_or(&None)
  }
}

/// Resolves a specifier to a workspace source file, or `None` if it is external
/// (resolves into `node_modules`) or not a source file we can process.
fn resolve_source(resolver: &Resolver, specifier: &str, from: &Path) -> Option<PathBuf> {
  let result = resolver.resolve_with_options(
    specifier,
    from,
    SpecifierType::Esm,
    ResolveOptions {
      conditions: ExportsCondition::SOURCE | ExportsCondition::TYPES,
      custom_conditions: Vec::new(),
    },
  );

  let Resolution::Path(path) = result.result.ok()?.resolution else {
    return None;
  };

  // Resolve symlinks (workspace packages are symlinked into node_modules).
  let path = std::fs::canonicalize(&path).unwrap_or(path);

  // External packages live in node_modules; skip them.
  if path.components().any(|c| c.as_os_str() == "node_modules") {
    return None;
  }

  if !is_source_file(&path) {
    return None;
  }

  Some(path)
}

fn is_source_file(path: &Path) -> bool {
  matches!(
    path.extension().and_then(|e| e.to_str()),
    Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs")
  )
}

/// Strips everything up to and including `packages/` from a path, for
/// machine-independent id comparison against pre-built api.json files.
pub fn short_path(path: &Path) -> String {
  let mut components = path.components();
  let mut kept: Vec<Component> = Vec::new();
  let mut found = false;
  for c in components.by_ref() {
    if found {
      kept.push(c);
    } else if c.as_os_str() == "packages" {
      found = true;
    }
  }
  if found {
    let mut p = PathBuf::from("packages");
    for c in kept {
      p.push(c);
    }
    p.display().to_string()
  } else {
    path.display().to_string()
  }
}
