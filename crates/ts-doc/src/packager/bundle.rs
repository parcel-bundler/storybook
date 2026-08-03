//! The abstraction over the module graph that the packager operates on.
//!
//! In production this is backed by Parcel's bundle graph; in tests by
//! [`crate::mock::MockBundleGraph`]. The packager only needs to (a) find the
//! entry module, (b) read each module's parsed [`API`], and (c) resolve a
//! dependency specifier from one module to another. Following re-exports,
//! aliases, and inline imported types is done by the packager itself using the
//! `Reference` types recorded during transformation.

use crate::API;

/// Identifies a module within the bundle. Opaque to the packager.
pub type AssetId = usize;

pub trait BundleGraph {
  /// The entry module whose exports drive the output.
  fn entry(&self) -> AssetId;

  /// The parsed transform output for a module.
  fn api(&self, asset: AssetId) -> &API;

  /// Resolves a dependency `specifier` (as written in `asset`) to another
  /// module. Returns `None` for external or unresolvable modules (e.g.
  /// `react`), in which case references to them are left as identifiers.
  fn resolve(&self, asset: AssetId, specifier: &str) -> Option<AssetId>;
}
