//! The packaging phase: links and merges the per-module `API`s produced by the
//! transformer into the final `{ exports, links }` documentation artifact.
//!
//! This is a port of `parcel-packager-docs/DocsPackager.js`. See
//! `PACKAGER_PLAN.md` for the design and the mapping from the JS implementation.

mod bundle;
mod merge;
mod node;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use indexmap::IndexMap;
use serde::Serialize;
use swc_core::ecma::ast::TsTypeOperatorOp;
use swc_core::ecma::atoms::Atom as JsWord;

pub use bundle::{AssetId, BundleGraph};
pub use node::{Node, NodeKind, NodeRef};

use crate::property::Property;
use crate::ty::{Type, TypeId};

/// The final documentation artifact.
#[derive(Debug, Serialize)]
pub struct PackageOutput {
  pub exports: IndexMap<JsWord, NodeRef>,
  pub links: IndexMap<JsWord, NodeRef>,
}

/// Packages a bundle graph, starting from its entry module.
pub fn package<G: BundleGraph>(graph: &G) -> PackageOutput {
  let mut packager = Packager::new(graph);
  let entry = graph.entry();
  packager.ensure_processed(entry);
  let exports = packager.cache.get(&entry).cloned().unwrap_or_default();
  let links = packager.collect_links(&exports);
  PackageOutput { exports, links }
}

/// The "key" a node was reached through — controls interface merging and
/// type-parameter substitution. Only the variants below are distinguished; every
/// other key is [`Key::Other`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
  Root,
  Props,
  Extends,
  Base,
  /// A type-parameter *declaration* (in a `typeParameters` list), as opposed to
  /// a usage. Usages are substituted; declarations are preserved.
  TypeParamDecl,
  Other,
}

struct Packager<'a, G: BundleGraph> {
  graph: &'a G,
  /// Processed exports per asset (memoized; also guards export-all cycles).
  cache: HashMap<AssetId, IndexMap<JsWord, NodeRef>>,
  /// Registry of every interface/alias encountered, keyed by id.
  nodes: HashMap<JsWord, NodeRef>,
  /// Type-parameter substitution maps (stack).
  param_stack: Vec<HashMap<JsWord, NodeRef>>,
  /// Keys of the current recursion path (for `should_merge`).
  key_stack: Vec<Key>,
  /// Resolved type arguments of the current generic application.
  application: Option<Vec<NodeRef>>,
  /// Arena types currently being materialized, to break cycles.
  visiting: HashSet<(AssetId, u32)>,
}

impl<'a, G: BundleGraph> Packager<'a, G> {
  fn new(graph: &'a G) -> Packager<'a, G> {
    Packager {
      graph,
      cache: HashMap::new(),
      nodes: HashMap::new(),
      param_stack: Vec::new(),
      key_stack: Vec::new(),
      application: None,
      visiting: HashSet::new(),
    }
  }

  /// Processes an asset's exports into the `cache` (memoized). Idempotent and
  /// cheap to call repeatedly — subsequent calls are a single map lookup.
  fn ensure_processed(&mut self, asset: AssetId) {
    if self.cache.contains_key(&asset) {
      return;
    }
    // Reserve a placeholder to guard against export-all cycles.
    self.cache.insert(asset, IndexMap::new());

    // Each asset's exports are processed in a fresh root context, independent of
    // any in-progress walk that triggered this (mirrors the JS packager's
    // per-module `paramStack`/`keyStack`).
    let saved_keys = std::mem::take(&mut self.key_stack);
    let saved_params = std::mem::take(&mut self.param_stack);
    let saved_app = self.application.take();

    let mut res = IndexMap::new();
    let export_names: Vec<(JsWord, TypeId)> = self
      .graph
      .api(asset)
      .exports
      .iter()
      .map(|(k, v)| (k.clone(), *v))
      .collect();

    for (name, type_id) in export_names {
      let mut node = self.walk(asset, type_id, Key::Root);
      // The export name becomes the type's name (handles re-export aliases).
      // `Rc::make_mut` clones only if this node is shared (e.g. it's a cached
      // interface/alias link); a freshly-built node is mutated in place.
      set_name(Rc::make_mut(&mut node), &name);
      res.insert(name, node);
    }

    // `export * from './x'` merges the target's exports.
    let export_all: Vec<JsWord> = self.graph.api(asset).export_all.clone();
    for specifier in export_all {
      if let Some(target) = self.graph.resolve(asset, &specifier) {
        self.ensure_processed(target);
        if let Some(target_exports) = self.cache.get(&target) {
          for (name, node) in target_exports {
            res.entry(name.clone()).or_insert_with(|| node.clone());
          }
        }
      }
    }

    self.key_stack = saved_keys;
    self.param_stack = saved_params;
    self.application = saved_app;

    self.cache.insert(asset, res);
  }

  /// The transformation walk: resolves `(asset, type_id)` to an output `Node`,
  /// applying reference resolution, generic substitution, and interface merging.
  fn walk(&mut self, asset: AssetId, type_id: TypeId, key: Key) -> NodeRef {
    let (asset, type_id) = self.deref(asset, type_id);
    // Borrow the arena type for the graph's lifetime (which outlives the
    // packager), so it doesn't alias the `&mut self` used to mutate walk state —
    // avoiding a clone of every type on every visit.
    let graph = self.graph;
    let ty = &graph.api(asset).types[type_id.0 as usize];

    // 1. Resolve imported references (possibly switching asset).
    if let Type::Reference {
      specifier,
      imported,
      local,
    } = ty
    {
      return match self.resolve_reference(asset, specifier, imported.as_deref()) {
        Some((next_asset, next_id)) => {
          // Process the target module's exports at their own root first, so
          // shared `nodes` entries get root-level treatment (e.g. type-parameter
          // constraint replacement). `nodes` uses first-write-wins, so the
          // subsequent walk below won't override the root version.
          if next_asset != asset {
            self.ensure_processed(next_asset);
          }
          self.walk(next_asset, next_id, key)
        }
        None => Node::identifier(
          local
            .clone()
            .or_else(|| imported.clone())
            .unwrap_or_default(),
        ),
      };
    }

    // Fast path: a reference to an interface that is already registered and
    // won't be merged here becomes a link without re-materializing all of its
    // properties. This is equivalent to the full walk below (whose `finish`
    // would emit the same link) but avoids a lot of redundant work when an
    // interface is referenced from many places.
    if let Type::Interface {
      id,
      type_parameters,
      ..
    } = ty
    {
      if let Some(cached) = self.nodes.get(id) {
        if !self.should_merge(ty, key) {
          return Node::link(id.clone());
        }
        // Also fast-path the *merged* (inlined) form: `nodes[id]` already holds
        // it (registered below, in `finish`), so an interface reused as an
        // `extends` target — very common for shared mixins like `DOMProps` — or
        // reused as `props`/a re-exported root doesn't need a full re-walk and
        // re-merge each time. Restricted to non-generic interfaces, since a
        // generic one may be instantiated with different type arguments at each
        // use and so needs its own substitution pass. Cheap: just bumps `cached`'s
        // refcount instead of deep-cloning the merged interface.
        if type_parameters.is_empty() {
          return Rc::clone(cached);
        }
      }
    }

    // Same idea for aliases: `finish` always turns an alias into a link except
    // when it's being inlined as `props`, regardless of its value's contents or
    // the current generic-substitution context. So once registered, any further
    // reference (at any other key) can skip materializing its value entirely.
    // This matters a lot for type-heavy code (e.g. CSS-in-JS style macros) where
    // the same named alias is referenced from many places.
    if let Type::Alias {
      id,
      type_parameters,
      ..
    } = ty
    {
      if let Some(cached) = self.nodes.get(id) {
        if key != Key::Props {
          return Node::link(id.clone());
        }
        // As with interfaces, also fast-path the inlined (`props`) form for
        // non-generic aliases: `nodes[id]` already holds the fully-walked alias,
        // so its `value` can be reused directly instead of re-walking.
        if type_parameters.is_empty() {
          if let NodeKind::Alias { value, .. } = &cached.kind {
            return Rc::clone(value);
          }
        }
      }
    }

    // 2. Capture the resolved type arguments of a generic application.
    if let Type::Application {
      type_parameters, ..
    } = ty
    {
      let app = type_parameters
        .iter()
        .map(|p| self.walk(asset, *p, Key::Other))
        .collect();
      self.application = Some(app);
    }

    // 3. Push a type-parameter substitution map where appropriate.
    let pushed = self.maybe_push_params(asset, ty, key);

    // 4. Recurse into children.
    self.key_stack.push(key);
    let cycle = !self.visiting.insert((asset, type_id.0));
    let node = if cycle {
      // Circular reference: emit a link if the type has an id.
      cycle_link(ty).unwrap_or_else(|| self.visit_children(asset, ty))
    } else {
      self.visit_children(asset, ty)
    };
    if !cycle {
      self.visiting.remove(&(asset, type_id.0));
    }
    self.key_stack.pop();

    if pushed {
      self.param_stack.pop();
    }

    // 5. Post-recursion rewrites.
    self.finish(node, ty, key)
  }

  /// Steps 5+ of the JS `fn`: applications, Omit/Pick, param substitution,
  /// interface/alias links, keyof unions.
  fn finish(&mut self, node: Node, ty: &Type, key: Key) -> NodeRef {
    // Application: unwrap to base when used as `props`.
    if let NodeKind::Application { base, .. } = &node.kind {
      self.application = None;
      if key == Key::Props {
        return Rc::clone(base);
      }
    }

    if let NodeKind::Identifier { name } = &node.kind {
      if let Some(app) = self.application.clone() {
        if name == "Omit" && app.len() >= 2 {
          return Rc::new(merge::omit(&app[0], &app[1], &self.nodes));
        }
        if name == "Pick" && app.len() >= 2 {
          return Rc::new(merge::pick(&app[0], &app[1], &self.nodes));
        }
      }
      if let Some(sub) = self.param_stack.last().and_then(|p| p.get(name)) {
        return Rc::clone(sub);
      }
    }

    // A type-parameter *usage* (not a declaration) is substituted if bound, and
    // otherwise rendered as a plain identifier (matching the JS implementation,
    // where type-parameter usages are `identifier` nodes).
    if key != Key::TypeParamDecl {
      if let NodeKind::TypeParameter { name, .. } = &node.kind {
        if let Some(sub) = self.param_stack.last().and_then(|p| p.get(name)) {
          return Rc::clone(sub);
        }
        return Node::identifier(name.clone());
      }
    }

    if let NodeKind::Alias { id, value, .. } = &node.kind {
      if key == Key::Props {
        return Rc::clone(value);
      }
      let id = id.clone();
      let node = Rc::new(node);
      self.nodes.entry(id.clone()).or_insert_with(|| Rc::clone(&node));
      return Node::link(id);
    }

    if let NodeKind::Interface { id, .. } = &node.kind {
      let id = id.clone();
      let merged = Rc::new(merge::merge_interface(&node));
      self
        .nodes
        .entry(id.clone())
        .or_insert_with(|| Rc::clone(&merged));
      return if self.should_merge(ty, key) {
        merged
      } else {
        Node::link(id)
      };
    }

    Rc::new(node)
  }

  /// Step 3 of the JS `fn`: push a type-parameter substitution map when a generic
  /// application is being merged, or when replacing root-level params with their
  /// constraints. Returns whether a map was pushed.
  fn maybe_push_params(&mut self, asset: AssetId, ty: &Type, key: Key) -> bool {
    let (type_parameters, is_component) = match ty {
      Type::Alias {
        type_parameters, ..
      }
      | Type::Interface {
        type_parameters, ..
      } => (type_parameters, false),
      Type::Component {
        type_parameters, ..
      } => (type_parameters, true),
      _ => return false,
    };

    if type_parameters.is_empty() {
      return false;
    }

    // Generic application onto an alias/interface we are about to merge.
    if !is_component && self.application.is_some() && self.should_merge(ty, key) {
      let app = self.application.take().unwrap();
      let mut params = self.param_stack.last().cloned().unwrap_or_default();
      for (i, tp) in type_parameters.iter().enumerate() {
        let (name, default) = self.type_param_name_default(asset, *tp);
        let value = app
          .get(i)
          .cloned()
          .or_else(|| default.map(|d| self.walk(asset, d, Key::Other)));
        if let Some(value) = value {
          params.insert(name, value);
        }
      }
      self.param_stack.push(params);
      return true;
    }

    // Root export: replace unbound type params with their constraints.
    if self.key_stack.is_empty() {
      let mut params = self.param_stack.last().cloned().unwrap_or_default();
      for tp in type_parameters {
        let node = self.walk(asset, *tp, Key::TypeParamDecl);
        if let NodeKind::TypeParameter {
          name,
          constraint: Some(constraint),
          ..
        } = &node.kind
        {
          params
            .entry(name.clone())
            .or_insert_with(|| Rc::clone(constraint));
        }
      }
      self.param_stack.push(params);
      return true;
    }

    false
  }

  /// Ports `shouldMerge`: whether an alias/interface should be inlined here.
  fn should_merge(&self, ty: &Type, key: Key) -> bool {
    match ty {
      Type::Interface { .. } => {
        if matches!(key, Key::Root | Key::Props | Key::Extends) {
          return true;
        }
        key == Key::Base
          && matches!(self.key_stack.last(), Some(Key::Props | Key::Extends))
      }
      Type::Alias { .. } => {
        key == Key::Base
          && matches!(self.key_stack.last(), Some(Key::Props | Key::Extends))
      }
      _ => false,
    }
  }

  /// Materializes an arena type's children into a `Node` (ports `visitChildren`).
  fn visit_children(&mut self, asset: AssetId, ty: &Type) -> Node {
    let kind = match ty {
      Type::Any => NodeKind::Any,
      Type::Null => NodeKind::Null,
      Type::Undefined => NodeKind::Undefined,
      Type::Void => NodeKind::Void,
      Type::Unknown => NodeKind::Unknown,
      Type::Never => NodeKind::Never,
      Type::This => NodeKind::This,
      Type::Symbol => NodeKind::Symbol,
      Type::ObjectKeyword => NodeKind::Object {
        properties: None,
        access: None,
      },
      Type::Identifier { name } => NodeKind::Identifier { name: name.clone() },
      Type::String { value } => NodeKind::String {
        value: value.clone(),
      },
      Type::Number { value } => NodeKind::Number { value: *value },
      Type::Boolean { value } => NodeKind::Boolean { value: *value },
      Type::Union { elements } => NodeKind::Union {
        elements: self.walk_all(asset, elements, Key::Other),
      },
      Type::Intersection { types } => NodeKind::Intersection {
        types: self.walk_all(asset, types, Key::Other),
      },
      Type::Application {
        base,
        type_parameters,
      } => NodeKind::Application {
        base: self.walk(asset, *base, Key::Base),
        type_parameters: self.walk_all(asset, type_parameters, Key::Other),
      },
      Type::TypeOperator { operator, value } => NodeKind::TypeOperator {
        operator: operator_name(*operator),
        value: self.walk(asset, *value, Key::Other),
      },
      Type::Function {
        id,
        name,
        parameters,
        return_type,
        return_description,
        type_parameters,
        access,
        examples,
        ..
      } => {
        let parameters = parameters
          .iter()
          .map(|p| self.walk_parameter(asset, p))
          .collect();
        let mut return_node = self.walk(asset, *return_type, Key::Other);
        // Only primitive/container return types carry the `@returns`
        // description; for anything the packager rebuilds (applications, unions,
        // references→links/identifiers, …) it is dropped, matching JS.
        if return_carries_description(&return_node.kind) {
          Rc::make_mut(&mut return_node).set_description(return_description.clone());
        }
        NodeKind::Function {
          id: id.clone(),
          name: name.clone(),
          parameters,
          return_type: return_node,
          type_parameters: self.walk_all(asset, type_parameters, Key::TypeParamDecl),
          examples: examples.clone(),
          access: access.clone(),
        }
      }
      Type::Alias {
        id,
        name,
        value,
        type_parameters,
        access,
        ..
      } => NodeKind::Alias {
        id: id.clone(),
        name: name.clone(),
        value: self.walk(asset, *value, Key::Other),
        type_parameters: self.walk_all(asset, type_parameters, Key::TypeParamDecl),
        access: access.clone(),
      },
      Type::Interface {
        id,
        name,
        extends,
        properties,
        type_parameters,
        access,
        ..
      } => NodeKind::Interface {
        id: id.clone(),
        name: name.clone(),
        extends: self.walk_all(asset, extends, Key::Extends),
        properties: self.walk_properties(asset, properties),
        type_parameters: self.walk_all(asset, type_parameters, Key::TypeParamDecl),
        access: access.clone(),
      },
      Type::Object {
        properties, access, ..
      } => NodeKind::Object {
        properties: Some(self.walk_properties(asset, properties)),
        access: access.clone(),
      },
      Type::Array { element_type } => NodeKind::Array {
        element_type: self.walk(asset, *element_type, Key::Other),
      },
      Type::Tuple { elements } => NodeKind::Tuple {
        elements: self.walk_all(asset, elements, Key::Other),
      },
      Type::Template { elements } => NodeKind::Template {
        elements: self.walk_all(asset, elements, Key::Other),
      },
      Type::Component {
        id,
        name,
        props,
        type_parameters,
        ref_type,
        access,
        examples,
        ..
      } => NodeKind::Component {
        id: id.clone(),
        name: name.clone(),
        props: props.map(|p| self.walk(asset, p, Key::Props)),
        type_parameters: self.walk_all(asset, type_parameters, Key::TypeParamDecl),
        ref_type: ref_type.map(|r| self.walk(asset, r, Key::Other)),
        examples: examples.clone(),
        access: access.clone(),
      },
      Type::Conditional {
        check_type,
        extends_type,
        true_type,
        false_type,
      } => NodeKind::Conditional {
        check_type: self.walk(asset, *check_type, Key::Other),
        extends_type: self.walk(asset, *extends_type, Key::Other),
        true_type: self.walk(asset, *true_type, Key::Other),
        false_type: self.walk(asset, *false_type, Key::Other),
      },
      Type::IndexedAccess {
        object_type,
        index_type,
      } => NodeKind::IndexedAccess {
        object_type: self.walk(asset, *object_type, Key::Other),
        index_type: self.walk(asset, *index_type, Key::Other),
      },
      Type::Mapped {
        readonly,
        type_parameter,
        type_annotation,
      } => NodeKind::Mapped {
        readonly: readonly.clone(),
        // The mapped type parameter is a binding (kept as a `typeParameter`).
        type_parameter: self.walk(asset, *type_parameter, Key::TypeParamDecl),
        type_annotation: self.walk(asset, *type_annotation, Key::Other),
      },
      Type::TypeParameter {
        name,
        constraint,
        default,
        ..
      } => NodeKind::TypeParameter {
        name: name.clone(),
        constraint: constraint.map(|c| self.walk(asset, c, Key::Other)),
        default: default.map(|d| self.walk(asset, d, Key::Other)),
      },
      Type::Link { id, .. } => NodeKind::Link { id: id.clone() },
      // Reference is resolved before `visit_children`; Ref is dereffed.
      Type::Reference {
        specifier,
        imported,
        local,
      } => NodeKind::Reference {
        specifier: specifier.clone(),
        imported: imported.clone(),
        local: local.clone(),
      },
      Type::Ref(_) => NodeKind::Any,
    };

    let mut node = Node::new(kind);
    node.set_description(type_description(ty));
    node
  }

  fn walk_all(&mut self, asset: AssetId, ids: &[TypeId], key: Key) -> Vec<NodeRef> {
    ids.iter().map(|id| self.walk(asset, *id, key)).collect()
  }

  fn walk_properties(
    &mut self,
    asset: AssetId,
    properties: &IndexMap<JsWord, Property>,
  ) -> IndexMap<JsWord, NodeRef> {
    properties
      .iter()
      .map(|(name, prop)| (name.clone(), self.walk_property(asset, prop)))
      .collect()
  }

  fn walk_property(&mut self, asset: AssetId, prop: &Property) -> NodeRef {
    let value = self.walk(asset, prop.value, Key::Other);
    let kind = if prop.is_method {
      NodeKind::Method {
        name: prop.name.clone(),
        value,
        access: prop.access.clone(),
        default: prop.default.clone(),
        is_static: prop.is_static,
        is_abstract: prop.is_abstract,
      }
    } else {
      NodeKind::Property {
        name: prop.name.clone(),
        index_type: prop.index_type.map(|i| self.walk(asset, i, Key::Other)),
        value,
        optional: prop.optional,
        access: prop.access.clone(),
        selector: prop.selector.clone(),
        default: prop.default.clone(),
      }
    };
    let mut node = Node::new(kind);
    node.set_description(prop.description.clone());
    Rc::new(node)
  }

  fn walk_parameter(&mut self, asset: AssetId, param: &crate::Parameter) -> NodeRef {
    let mut node = Node::new(NodeKind::Parameter {
      name: param.name.clone(),
      value: self.walk(asset, param.value, Key::Other),
      optional: param.optional,
      rest: param.rest,
    });
    node.set_description(param.description.clone());
    Rc::new(node)
  }

  /// Follows `Type::Ref` chains to the concrete arena type.
  fn deref(&self, asset: AssetId, mut type_id: TypeId) -> (AssetId, TypeId) {
    loop {
      match &self.graph.api(asset).types[type_id.0 as usize] {
        Type::Ref(t) => type_id = *t,
        _ => return (asset, type_id),
      }
    }
  }

  /// Resolves an imported `Reference` to the `(asset, type_id)` it points at.
  fn resolve_reference(
    &self,
    asset: AssetId,
    specifier: &str,
    imported: Option<&str>,
  ) -> Option<(AssetId, TypeId)> {
    let target = self.graph.resolve(asset, specifier)?;
    self.resolve_export(target, imported?)
  }

  /// Resolves an export name in an asset to its defining `(asset, type_id)`,
  /// following re-export chains and `export *`.
  fn resolve_export(&self, asset: AssetId, name: &str) -> Option<(AssetId, TypeId)> {
    let key: JsWord = name.into();
    let api = self.graph.api(asset);
    if let Some(type_id) = api.exports.get(&key) {
      let (asset, type_id) = self.deref(asset, *type_id);
      if let Type::Reference {
        specifier,
        imported,
        ..
      } = &self.graph.api(asset).types[type_id.0 as usize]
      {
        let target = self.graph.resolve(asset, specifier)?;
        return self.resolve_export(target, imported.as_deref()?);
      }
      return Some((asset, type_id));
    }
    for specifier in &api.export_all {
      if let Some(target) = self.graph.resolve(asset, specifier) {
        if let Some(resolved) = self.resolve_export(target, name) {
          return Some(resolved);
        }
      }
    }
    None
  }

  /// Reads a type parameter's name and default from the arena.
  fn type_param_name_default(&self, asset: AssetId, type_id: TypeId) -> (JsWord, Option<TypeId>) {
    let (asset, type_id) = self.deref(asset, type_id);
    match &self.graph.api(asset).types[type_id.0 as usize] {
      Type::TypeParameter { name, default, .. } => (name.clone(), *default),
      _ => ("".into(), None),
    }
  }

  /// Collects the `links` section by following every `link` node in `exports`.
  fn collect_links(&self, exports: &IndexMap<JsWord, NodeRef>) -> IndexMap<JsWord, NodeRef> {
    let mut links = IndexMap::new();
    for node in exports.values() {
      self.walk_links(node, &mut links);
    }
    links
  }

  fn walk_links(&self, node: &Node, links: &mut IndexMap<JsWord, NodeRef>) {
    if let NodeKind::Link { id } = &node.kind {
      if !links.contains_key(id) {
        if let Some(target) = self.nodes.get(id) {
          links.insert(id.clone(), Rc::clone(target));
          self.walk_links(target, links);
        }
      }
    }
    for child in child_nodes(node) {
      self.walk_links(child, links);
    }
  }
}

/// Sets the top-level `name` of a declaration node (used for re-export aliases).
fn set_name(node: &mut Node, new_name: &JsWord) {
  match &mut node.kind {
    NodeKind::Function { name, .. } | NodeKind::Component { name, .. } => {
      *name = Some(new_name.clone())
    }
    NodeKind::Interface { name, .. } | NodeKind::Alias { name, .. } => *name = new_name.clone(),
    _ => {}
  }
}

/// Returns a link node for a circular reference, if the type has an id.
fn cycle_link(ty: &Type) -> Option<Node> {
  match ty {
    Type::Interface { id, .. } | Type::Alias { id, .. } => {
      Some(Node::new(NodeKind::Link { id: id.clone() }))
    }
    _ => None,
  }
}

fn operator_name(op: TsTypeOperatorOp) -> JsWord {
  match op {
    TsTypeOperatorOp::KeyOf => "keyof".into(),
    TsTypeOperatorOp::Unique => "unique".into(),
    TsTypeOperatorOp::ReadOnly => "readonly".into(),
  }
}

/// Whether a return node of this kind preserves the `@returns` description in
/// the JS packager (leaf/primitive and container types), as opposed to nodes the
/// packager rebuilds fresh (applications, unions, references, …).
fn return_carries_description(kind: &NodeKind) -> bool {
  matches!(
    kind,
    NodeKind::Any
      | NodeKind::Null
      | NodeKind::Undefined
      | NodeKind::Void
      | NodeKind::Unknown
      | NodeKind::Never
      | NodeKind::This
      | NodeKind::Symbol
      | NodeKind::String { .. }
      | NodeKind::Number { .. }
      | NodeKind::Boolean { .. }
      | NodeKind::Object { .. }
      | NodeKind::Alias { .. }
      | NodeKind::Interface { .. }
      | NodeKind::Function { .. }
      | NodeKind::Component { .. }
  )
}

/// The description carried directly on an arena type, if any.
fn type_description(ty: &Type) -> Option<JsWord> {
  match ty {
    Type::Function { description, .. }
    | Type::Alias { description, .. }
    | Type::Interface { description, .. }
    | Type::Object { description, .. }
    | Type::Component { description, .. } => description.clone(),
    _ => None,
  }
}

/// Yields the immediate child `Node`s of a node (for link collection).
fn child_nodes(node: &Node) -> Vec<&Node> {
  let mut out: Vec<&Node> = Vec::new();
  match &node.kind {
    NodeKind::Union { elements }
    | NodeKind::Tuple { elements }
    | NodeKind::Template { elements } => out.extend(elements.iter().map(|n| n.as_ref())),
    NodeKind::Intersection { types } => out.extend(types.iter().map(|n| n.as_ref())),
    NodeKind::Application {
      base,
      type_parameters,
    } => {
      out.push(base);
      out.extend(type_parameters.iter().map(|n| n.as_ref()));
    }
    NodeKind::TypeOperator { value, .. } => out.push(value),
    NodeKind::Function {
      parameters,
      return_type,
      type_parameters,
      ..
    } => {
      out.extend(parameters.iter().map(|n| n.as_ref()));
      out.push(return_type);
      out.extend(type_parameters.iter().map(|n| n.as_ref()));
    }
    NodeKind::Parameter { value, .. } => out.push(value),
    NodeKind::Property {
      index_type, value, ..
    } => {
      if let Some(index_type) = index_type {
        out.push(index_type);
      }
      out.push(value);
    }
    NodeKind::Method { value, .. } => out.push(value),
    NodeKind::Alias {
      value,
      type_parameters,
      ..
    } => {
      out.push(value);
      out.extend(type_parameters.iter().map(|n| n.as_ref()));
    }
    NodeKind::Interface {
      extends,
      properties,
      type_parameters,
      ..
    } => {
      out.extend(extends.iter().map(|n| n.as_ref()));
      out.extend(properties.values().map(|n| n.as_ref()));
      out.extend(type_parameters.iter().map(|n| n.as_ref()));
    }
    NodeKind::Object {
      properties: Some(properties),
      ..
    } => out.extend(properties.values().map(|n| n.as_ref())),
    NodeKind::Array { element_type } => out.push(element_type),
    NodeKind::TypeParameter {
      constraint,
      default,
      ..
    } => {
      out.extend(constraint.iter().map(|b| b.as_ref()));
      out.extend(default.iter().map(|b| b.as_ref()));
    }
    NodeKind::Component {
      props,
      type_parameters,
      ref_type,
      ..
    } => {
      out.extend(props.iter().map(|b| b.as_ref()));
      out.extend(type_parameters.iter().map(|n| n.as_ref()));
      out.extend(ref_type.iter().map(|b| b.as_ref()));
    }
    NodeKind::Conditional {
      check_type,
      extends_type,
      true_type,
      false_type,
    } => {
      out.push(check_type);
      out.push(extends_type);
      out.push(true_type);
      out.push(false_type);
    }
    NodeKind::IndexedAccess {
      object_type,
      index_type,
    } => {
      out.push(object_type);
      out.push(index_type);
    }
    NodeKind::Mapped {
      type_parameter,
      type_annotation,
      ..
    } => {
      out.push(type_parameter);
      out.push(type_annotation);
    }
    _ => {}
  }
  out
}
