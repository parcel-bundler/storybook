//! Interface merging and the `Omit`/`Pick`/`resolveValue` helpers, ported from
//! the JS packager. These operate on materialized [`Node`] trees.

use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;
use swc_core::ecma::atoms::Atom as JsWord;

use super::node::{Node, NodeKind, NodeRef};

/// Flattens an interface (plus its `extends` chain) into a single interface with
/// all inherited properties inlined. Non-interface `extends` are preserved.
pub fn merge_interface(node: &Node) -> Node {
  // Unwrap generic applications and aliases to reach the interface.
  let obj = match &node.kind {
    NodeKind::Application { base, .. } => base.as_ref(),
    NodeKind::Alias { value, .. } => value.as_ref(),
    _ => node,
  };

  let NodeKind::Interface {
    id,
    name,
    extends,
    properties,
    type_parameters,
    access,
  } = &obj.kind
  else {
    return obj.clone();
  };

  // Cheap: cloning an `IndexMap<_, NodeRef>` only bumps refcounts, not a deep
  // copy of every property's value tree.
  let mut merged: IndexMap<JsWord, NodeRef> = properties.clone();
  let mut exts = Vec::new();
  for ext in extends {
    let m = merge_interface(ext);
    if let NodeKind::Interface { properties, .. } = &m.kind {
      merge_props(&mut merged, properties);
    } else {
      exts.push(Rc::new(m));
    }
  }

  Node {
    kind: NodeKind::Interface {
      id: id.clone(),
      name: name.clone(),
      extends: exts,
      properties: merged,
      type_parameters: type_parameters.clone(),
      access: access.clone(),
    },
    description: obj.description.clone(),
  }
}

/// Copies keys from `b` into `a` that `a` does not already have.
fn merge_props(a: &mut IndexMap<JsWord, NodeRef>, b: &IndexMap<JsWord, NodeRef>) {
  for (key, value) in b {
    if !a.contains_key(key) {
      a.insert(key.clone(), value.clone());
    }
  }
}

/// `Omit<T, K>`: drops the string keys named by `K` from `T`.
pub fn omit(obj: &Node, to_omit: &Node, nodes: &HashMap<JsWord, NodeRef>) -> Node {
  filter_keys(obj, to_omit, nodes, false)
}

/// `Pick<T, K>`: keeps only the string keys named by `K` from `T`.
pub fn pick(obj: &Node, to_pick: &Node, nodes: &HashMap<JsWord, NodeRef>) -> Node {
  filter_keys(obj, to_pick, nodes, true)
}

fn filter_keys(obj: &Node, keys_node: &Node, nodes: &HashMap<JsWord, NodeRef>, keep: bool) -> Node {
  let resolved = resolve_value(obj, nodes);

  let keys = collect_string_keys(keys_node);
  if keys.is_empty() {
    return resolved;
  }

  match &resolved.kind {
    NodeKind::Interface {
      id,
      name,
      extends,
      properties,
      type_parameters,
      access,
    } => {
      let properties = properties
        .iter()
        .filter(|(k, _)| keys.contains(*k) == keep)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
      Node {
        kind: NodeKind::Interface {
          id: id.clone(),
          name: name.clone(),
          extends: extends.clone(),
          properties,
          type_parameters: type_parameters.clone(),
          access: access.clone(),
        },
        description: resolved.description.clone(),
      }
    }
    NodeKind::Object { properties, access } => {
      let properties = properties.as_ref().map(|props| {
        props
          .iter()
          .filter(|(k, _)| keys.contains(*k) == keep)
          .map(|(k, v)| (k.clone(), v.clone()))
          .collect()
      });
      Node {
        kind: NodeKind::Object {
          properties,
          access: access.clone(),
        },
        description: resolved.description.clone(),
      }
    }
    _ => resolved,
  }
}

/// Collects string-literal values from a `string` node or a `union` of them.
fn collect_string_keys(node: &Node) -> std::collections::HashSet<JsWord> {
  let mut keys = std::collections::HashSet::new();
  match &node.kind {
    NodeKind::String { value: Some(v) } => {
      keys.insert(v.clone());
    }
    NodeKind::Union { elements } => {
      for e in elements {
        if let NodeKind::String { value: Some(v) } = &e.kind {
          keys.insert(v.clone());
        }
      }
    }
    _ => {}
  }
  keys
}

/// Follows `link`/`application`/`alias` nodes to the underlying value node.
pub fn resolve_value(obj: &Node, nodes: &HashMap<JsWord, NodeRef>) -> Node {
  match &obj.kind {
    NodeKind::Link { id } => match nodes.get(id) {
      Some(node) => resolve_value(node, nodes),
      None => obj.clone(),
    },
    NodeKind::Application { base, .. } => resolve_value(base, nodes),
    NodeKind::Alias { value, .. } => resolve_value(value, nodes),
    _ => obj.clone(),
  }
}
