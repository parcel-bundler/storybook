//! The owned, self-contained output type produced by the packager.
//!
//! Unlike the transform's [`crate::Type`], which references other types by arena
//! index (`TypeId`), a `Node` is a fully materialized tree that serializes
//! directly to the final documentation JSON. Shared and circular
//! interfaces/aliases are represented as [`NodeKind::Link`] and collected into a
//! separate `links` section.
//!
//! Child fields are [`NodeRef`] (`Rc<Node>`) rather than owned `Node`/`Box<Node>`.
//! The packager frequently needs to return a previously-computed node again (a
//! shared interface/alias, a generic-parameter substitution, an unwrapped
//! `props` value, …); with `Rc`, doing so is a cheap refcount bump instead of a
//! deep clone of the whole subtree. Serde serializes `Rc<Node>` exactly like
//! `Node` (via the `rc` feature), so the JSON output is unaffected.

use std::rc::Rc;

use indexmap::IndexMap;
use serde::{Serialize, Serializer};
use swc_core::ecma::atoms::Atom as JsWord;

/// A reference-counted, shareable `Node`. See the module docs for why.
pub type NodeRef = Rc<Node>;

/// Serializes a mapped type's `readonly` modifier: `"true"` becomes the boolean
/// `true`, while `"+"`/`"-"` are emitted as strings (matching the JS output).
fn serialize_readonly<S: Serializer>(value: &Option<JsWord>, s: S) -> Result<S::Ok, S::Error> {
  match value.as_deref() {
    Some("true") => s.serialize_bool(true),
    Some(other) => s.serialize_str(other),
    None => s.serialize_none(),
  }
}

/// Serializes a whole number as an integer (e.g. `12` not `12.0`), matching the
/// JS `JSON.stringify` output.
fn serialize_number_value<S: Serializer>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
  match value {
    Some(n) if n.fract() == 0.0 && n.is_finite() && n.abs() < 9e15 => s.serialize_i64(*n as i64),
    Some(n) => s.serialize_f64(*n),
    None => s.serialize_none(),
  }
}

/// A materialized output type: its structural kind plus a cross-cutting optional
/// `description` (JSDoc can attach a description to a node of any kind, e.g. a
/// function's `@returns` on a primitive return type).
#[derive(Clone, Debug, Serialize)]
pub struct Node {
  #[serde(flatten)]
  pub kind: NodeKind,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub description: Option<JsWord>,
}

impl Node {
  pub fn new(kind: NodeKind) -> Node {
    Node {
      kind,
      description: None,
    }
  }

  /// Sets the description if one is provided and not already present.
  pub fn set_description(&mut self, description: Option<JsWord>) {
    if self.description.is_none() && description.is_some() {
      self.description = description;
    }
  }

  pub fn link(id: JsWord) -> NodeRef {
    Rc::new(Node::new(NodeKind::Link { id }))
  }

  pub fn identifier(name: JsWord) -> NodeRef {
    Rc::new(Node::new(NodeKind::Identifier { name }))
  }
}

impl From<NodeKind> for Node {
  fn from(kind: NodeKind) -> Node {
    Node::new(kind)
  }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NodeKind {
  Any,
  Null,
  Undefined,
  Void,
  Unknown,
  Never,
  This,
  Symbol,
  Identifier {
    name: JsWord,
  },
  String {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<JsWord>,
  },
  Number {
    #[serde(
      skip_serializing_if = "Option::is_none",
      serialize_with = "serialize_number_value"
    )]
    value: Option<f64>,
  },
  Boolean {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<bool>,
  },
  Union {
    elements: Vec<NodeRef>,
  },
  Intersection {
    types: Vec<NodeRef>,
  },
  #[serde(rename_all = "camelCase")]
  Application {
    base: NodeRef,
    type_parameters: Vec<NodeRef>,
  },
  TypeOperator {
    operator: JsWord,
    value: NodeRef,
  },
  #[serde(rename_all = "camelCase")]
  Function {
    id: Option<JsWord>,
    name: Option<JsWord>,
    parameters: Vec<NodeRef>,
    #[serde(rename = "return")]
    return_type: NodeRef,
    type_parameters: Vec<NodeRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    examples: Vec<JsWord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
  },
  Parameter {
    name: JsWord,
    value: NodeRef,
    optional: bool,
    rest: bool,
  },
  #[serde(rename_all = "camelCase")]
  Property {
    name: JsWord,
    index_type: Option<NodeRef>,
    value: NodeRef,
    optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<JsWord>,
    default: Option<JsWord>,
  },
  Method {
    name: JsWord,
    value: NodeRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
    default: Option<JsWord>,
    #[serde(rename = "static")]
    is_static: bool,
    #[serde(rename = "abstract")]
    is_abstract: bool,
  },
  #[serde(rename_all = "camelCase")]
  Alias {
    id: JsWord,
    name: JsWord,
    value: NodeRef,
    type_parameters: Vec<NodeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Interface {
    id: JsWord,
    name: JsWord,
    extends: Vec<NodeRef>,
    properties: IndexMap<JsWord, NodeRef>,
    type_parameters: Vec<NodeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
  },
  Object {
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<IndexMap<JsWord, NodeRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Array {
    element_type: NodeRef,
  },
  Tuple {
    elements: Vec<NodeRef>,
  },
  Template {
    elements: Vec<NodeRef>,
  },
  TypeParameter {
    name: JsWord,
    constraint: Option<NodeRef>,
    default: Option<NodeRef>,
  },
  #[serde(rename_all = "camelCase")]
  Component {
    id: Option<JsWord>,
    name: Option<JsWord>,
    props: Option<NodeRef>,
    type_parameters: Vec<NodeRef>,
    #[serde(rename = "ref")]
    ref_type: Option<NodeRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    examples: Vec<JsWord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Conditional {
    check_type: NodeRef,
    extends_type: NodeRef,
    true_type: NodeRef,
    false_type: NodeRef,
  },
  #[serde(rename_all = "camelCase")]
  IndexedAccess {
    object_type: NodeRef,
    index_type: NodeRef,
  },
  #[serde(rename_all = "camelCase")]
  Mapped {
    #[serde(
      skip_serializing_if = "Option::is_none",
      serialize_with = "serialize_readonly"
    )]
    readonly: Option<JsWord>,
    type_parameter: NodeRef,
    type_annotation: NodeRef,
  },
  Link {
    id: JsWord,
  },
  /// An unresolved reference to an imported type (external/skipped module).
  Reference {
    specifier: JsWord,
    #[serde(skip_serializing_if = "Option::is_none")]
    imported: Option<JsWord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local: Option<JsWord>,
  },
}
