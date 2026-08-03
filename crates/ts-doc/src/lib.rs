use indexmap::{IndexMap, IndexSet};
use serde::Serialize;
use swc_core::ecma::atoms::Atom as JsWord;

pub mod fs_graph;
mod jsdoc;
pub mod mock;
mod packager;
mod parameter;
mod parse;
mod property;
mod ty;

pub use packager::{AssetId, BundleGraph, Node, NodeKind, PackageOutput, package};
pub use parameter::Parameter;
pub use parse::parse;
pub use property::Property;
pub use ty::{Type, TypeId};

/// Represents an individual module API.
#[derive(Clone, Default, Debug, Serialize)]
pub struct API {
  /// Specifiers for imported types.
  pub dependencies: IndexSet<JsWord>,
  /// Arena of types defined in this module.
  pub types: Vec<Type>,
  /// Map of exported names to types.
  pub exports: IndexMap<JsWord, TypeId>,
  /// Re-exports.
  pub export_all: Vec<JsWord>,
}

impl API {
  pub fn add_type(&mut self, ty: Type) -> TypeId {
    if let Type::Ref(t) = ty {
      return t;
    }

    let type_id = TypeId(self.types.len() as u32);
    self.types.push(ty);
    type_id
  }
}
