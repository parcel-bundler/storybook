//! A test [`BundleGraph`] backed by in-memory source files.
//!
//! Build one from `(path, source)` pairs; each source is parsed with the
//! transformer. Dependency specifiers are resolved by relative-path joining plus
//! extension probing (`.tsx`, `.ts`, `.d.ts`), mirroring the JS `DocsResolver`.
//! Bare specifiers (e.g. `react`) resolve to `None` unless an explicit override
//! is supplied, modelling external modules.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::packager::{AssetId, BundleGraph};
use crate::{API, parse};

pub struct MockBundleGraph {
  apis: Vec<API>,
  paths: Vec<PathBuf>,
  entry: AssetId,
  /// Optional explicit `specifier -> path` overrides (e.g. for bare specifiers).
  overrides: HashMap<String, PathBuf>,
}

impl MockBundleGraph {
  /// Builds a bundle graph from `(path, source)` pairs. `entry` must match one
  /// of the provided paths.
  pub fn from_sources(sources: &[(&str, &str)], entry: &str) -> MockBundleGraph {
    let mut apis = Vec::new();
    let mut paths = Vec::new();
    for (path, code) in sources {
      let path = PathBuf::from(path);
      apis.push(parse(&path, code.to_string()));
      paths.push(path);
    }
    let entry = paths
      .iter()
      .position(|p| p == Path::new(entry))
      .expect("entry path must be one of the provided sources");
    MockBundleGraph {
      apis,
      paths,
      entry,
      overrides: HashMap::new(),
    }
  }

  /// Adds an explicit specifier override that resolves to the given path.
  pub fn with_override(mut self, specifier: &str, path: &str) -> MockBundleGraph {
    self.overrides.insert(specifier.to_string(), PathBuf::from(path));
    self
  }

  fn index_of(&self, path: &Path) -> Option<AssetId> {
    self.paths.iter().position(|p| p == path)
  }
}

impl BundleGraph for MockBundleGraph {
  fn entry(&self) -> AssetId {
    self.entry
  }

  fn api(&self, asset: AssetId) -> &API {
    &self.apis[asset]
  }

  fn resolve(&self, asset: AssetId, specifier: &str) -> Option<AssetId> {
    if let Some(path) = self.overrides.get(specifier) {
      return self.index_of(path);
    }

    if !specifier.starts_with('.') {
      // Bare specifier (external module) with no override.
      return None;
    }

    let dir = self.paths[asset].parent().unwrap_or_else(|| Path::new(""));
    let base = normalize(&dir.join(specifier));

    // Try the path as written, then with candidate extensions, then as a
    // directory with an index file.
    let exts = ["", ".tsx", ".ts", ".d.ts", ".js"];
    for ext in exts {
      let candidate = if ext.is_empty() {
        base.clone()
      } else {
        PathBuf::from(format!("{}{}", base.display(), ext))
      };
      if let Some(id) = self.index_of(&candidate) {
        return Some(id);
      }
    }
    for index in ["index.tsx", "index.ts", "index.d.ts", "index.js"] {
      if let Some(id) = self.index_of(&base.join(index)) {
        return Some(id);
      }
    }
    None
  }
}

/// Resolves `.`/`..` components without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
  let mut out = PathBuf::new();
  for comp in path.components() {
    match comp {
      Component::CurDir => {}
      Component::ParentDir => {
        out.pop();
      }
      other => out.push(other.as_os_str()),
    }
  }
  out
}
