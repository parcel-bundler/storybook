use swc_core::common::Spanned;
use swc_core::ecma::ast::*;
use swc_core::ecma::atoms::Atom as JsWord;

use indexmap::IndexMap;

use serde::Serialize;
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::jsdoc::{JsDocs, parse_jsdoc};
use crate::parameter::ToParameter;
use crate::parse::Context;
use crate::property::ToProperty;
use crate::{Parameter, Property};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub struct TypeId(pub u32);

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Type {
  Any,
  Null,
  Undefined,
  Void,
  Unknown,
  Never,
  This,
  Symbol,
  #[serde(rename = "object")]
  ObjectKeyword,
  Identifier {
    name: JsWord,
  },
  String {
    value: Option<JsWord>,
  },
  Number {
    value: Option<f64>,
  },
  Boolean {
    value: Option<bool>,
  },
  Union {
    elements: Vec<TypeId>,
  },
  Intersection {
    types: Vec<TypeId>,
  },
  #[serde(rename_all = "camelCase")]
  Application {
    base: TypeId,
    type_parameters: Vec<TypeId>,
  },
  TypeOperator {
    #[serde(serialize_with = "serialize_op")]
    operator: TsTypeOperatorOp,
    value: TypeId,
  },
  #[serde(rename_all = "camelCase")]
  Function {
    id: Option<JsWord>,
    name: Option<JsWord>,
    parameters: Vec<Parameter>,
    #[serde(rename = "return")]
    return_type: TypeId,
    type_parameters: Vec<TypeId>,
    description: Option<JsWord>,
    #[serde(skip)]
    return_description: Option<JsWord>,
    access: Option<JsWord>,
    examples: Vec<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Alias {
    id: JsWord,
    name: JsWord,
    value: TypeId,
    type_parameters: Vec<TypeId>,
    description: Option<JsWord>,
    access: Option<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Interface {
    id: JsWord,
    name: JsWord,
    extends: Vec<TypeId>,
    properties: IndexMap<JsWord, Property>,
    type_parameters: Vec<TypeId>,
    description: Option<JsWord>,
    access: Option<JsWord>,
  },
  Object {
    properties: IndexMap<JsWord, Property>,
    description: Option<JsWord>,
    access: Option<JsWord>,
  },
  Array {
    element_type: TypeId,
  },
  Tuple {
    elements: Vec<TypeId>,
  },
  Template {
    elements: Vec<TypeId>,
  },
  #[serde(rename_all = "camelCase")]
  Component {
    id: Option<JsWord>,
    name: Option<JsWord>,
    props: Option<TypeId>,
    type_parameters: Vec<TypeId>,
    #[serde(rename = "ref")]
    ref_type: Option<TypeId>,
    description: Option<JsWord>,
    access: Option<JsWord>,
    examples: Vec<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Conditional {
    check_type: TypeId,
    extends_type: TypeId,
    true_type: TypeId,
    false_type: TypeId,
  },
  #[serde(rename_all = "camelCase")]
  IndexedAccess {
    object_type: TypeId,
    index_type: TypeId,
  },
  Link {
    id: JsWord,
    #[serde(skip)]
    value: TypeId,
  },
  /// Reference to an imported type.
  Reference {
    specifier: JsWord,
    imported: Option<JsWord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local: Option<JsWord>,
  },
  #[serde(rename_all = "camelCase")]
  Mapped {
    /// The `readonly` modifier: `None`, or `"true"`/`"+"`/`"-"`.
    readonly: Option<JsWord>,
    type_parameter: TypeId,
    type_annotation: TypeId,
  },
  TypeParameter {
    name: JsWord,
    constraint: Option<TypeId>,
    default: Option<TypeId>,
  },
  #[serde(untagged)]
  Ref(TypeId),
}

impl Type {
  pub fn is_jsx(&self, ctx: &Context) -> bool {
    match self {
      Type::Identifier { name } => name == "JSX.Element",
      Type::Union { elements } => elements
        .iter()
        .any(|t| ctx.api.types[t.0 as usize].is_jsx(ctx)),
      Type::Ref(r) => ctx.api.types[r.0 as usize].is_jsx(ctx),
      _ => false,
    }
  }

  /// Whether this type is a React element return type (`JSX.Element`,
  /// `ReactElement`, etc.). Used to recognize a component from a cast type
  /// annotation such as `forwardRef(fn) as <T>(props: P<T>) => ReactElement`.
  pub fn is_react_element(&self, ctx: &Context) -> bool {
    match self {
      Type::Identifier { name } => matches!(
        name.as_str(),
        "JSX.Element" | "React.JSX.Element" | "ReactElement" | "React.ReactElement"
      ),
      // `import {ReactElement} from 'react'` before the reference is resolved.
      Type::Reference { imported, .. } => {
        matches!(imported.as_deref(), Some("ReactElement"))
      }
      Type::Application { base, .. } => ctx.api.types[base.0 as usize].is_react_element(ctx),
      Type::Union { elements } => elements
        .iter()
        .any(|t| ctx.api.types[t.0 as usize].is_react_element(ctx)),
      Type::Ref(r) => ctx.api.types[r.0 as usize].is_react_element(ctx),
      _ => false,
    }
  }

  pub fn to_component(self) -> Type {
    if let Type::Function {
      id,
      name,
      parameters,
      type_parameters,
      description,
      access,
      examples,
      ..
    } = self
    {
      Type::Component {
        id,
        name,
        props: parameters.first().map(|p| p.value),
        type_parameters,
        ref_type: parameters.get(1).map(|p| p.value),
        description,
        access,
        examples,
      }
    } else {
      self
    }
  }

  /// Sets the `id` on named type kinds (function, component, interface, alias).
  pub fn set_id(&mut self, new_id: JsWord) {
    match self {
      Type::Function { id, .. } | Type::Component { id, .. } => *id = Some(new_id),
      Type::Interface { id, .. } | Type::Alias { id, .. } => *id = new_id,
      _ => {}
    }
  }

  /// Sets the `name` on named type kinds (function, component, interface, alias).
  pub fn set_name(&mut self, new_name: JsWord) {
    match self {
      Type::Function { name, .. } | Type::Component { name, .. } => *name = Some(new_name),
      Type::Interface { name, .. } | Type::Alias { name, .. } => *name = new_name,
      _ => {}
    }
  }

  /// The description carried directly on this type, if any.
  pub fn description(&self) -> Option<&JsWord> {
    match self {
      Type::Function { description, .. }
      | Type::Alias { description, .. }
      | Type::Interface { description, .. }
      | Type::Object { description, .. }
      | Type::Component { description, .. } => description.as_ref(),
      _ => None,
    }
  }

  /// Sets the description on type kinds that carry one.
  pub fn set_description(&mut self, desc: Option<JsWord>) {
    match self {
      Type::Function { description, .. }
      | Type::Alias { description, .. }
      | Type::Interface { description, .. }
      | Type::Object { description, .. }
      | Type::Component { description, .. } => *description = desc,
      _ => {}
    }
  }

  /// Whether this is an interface or component (used for variable-declarator naming).
  fn is_interface_or_component(&self, ctx: &Context) -> bool {
    match self {
      Type::Interface { .. } | Type::Component { .. } => true,
      Type::Ref(r) => ctx.api.types[r.0 as usize].is_interface_or_component(ctx),
      _ => false,
    }
  }

  /// Applies parsed JSDoc to this type. Only overwrites fields that the doc
  /// block actually provides, so it can be applied more than once (e.g. once
  /// for the declaration and again for the wrapping `export`).
  pub fn add_docs(&mut self, jsdoc: JsDocs, value_name: Option<JsWord>, ctx: Option<&mut Context>) {
    match self {
      Type::Function {
        name,
        parameters,
        description,
        return_description,
        access,
        examples,
        ..
      } => {
        if let Some(value_name) = value_name {
          *name = Some(value_name);
        }
        for param in parameters.iter_mut() {
          if let Some(desc) = jsdoc.params.get(&param.name) {
            param.description = Some(desc.clone());
          }
        }
        if jsdoc.description.is_some() {
          *description = jsdoc.description;
        }
        if jsdoc.access.is_some() {
          *access = jsdoc.access;
        }
        if !jsdoc.examples.is_empty() {
          *examples = jsdoc.examples;
        }
        if jsdoc.return_description.is_some() {
          *return_description = jsdoc.return_description;
        }
      }
      Type::Component {
        name,
        description,
        access,
        examples,
        ..
      } => {
        if let Some(value_name) = value_name {
          *name = Some(value_name);
        }
        if jsdoc.description.is_some() {
          *description = jsdoc.description;
        }
        if jsdoc.access.is_some() {
          *access = jsdoc.access;
        }
        if !jsdoc.examples.is_empty() {
          *examples = jsdoc.examples;
        }
      }
      Type::Alias {
        name,
        description,
        access,
        ..
      } => {
        if let Some(value_name) = value_name {
          *name = value_name;
        }
        if jsdoc.description.is_some() {
          *description = jsdoc.description;
        }
        if jsdoc.access.is_some() {
          *access = jsdoc.access;
        }
      }
      Type::Interface {
        name,
        properties,
        description,
        access,
        ..
      } => {
        if let Some(value_name) = value_name {
          *name = value_name;
        }
        if jsdoc.description.is_some() {
          *description = jsdoc.description;
        }
        if jsdoc.access.is_some() {
          *access = jsdoc.access;
        }
        // A `@private` interface marks all of its properties private too.
        if access.as_deref() == Some("private") {
          for prop in properties.values_mut() {
            prop.access = Some("private".into());
          }
        }
      }
      Type::Object {
        description,
        access,
        ..
      } => {
        if jsdoc.description.is_some() {
          *description = jsdoc.description;
        }
        if jsdoc.access.is_some() {
          *access = jsdoc.access;
        }
      }
      Type::Ref(r) => {
        if let Some(ctx) = ctx {
          ctx.api.types[r.0 as usize].add_docs(jsdoc, value_name, None);
        }
      }
      _ => {}
    }
  }
}

fn serialize_op<S: serde::ser::Serializer>(op: &TsTypeOperatorOp, s: S) -> Result<S::Ok, S::Error> {
  match op {
    TsTypeOperatorOp::KeyOf => "keyof".serialize(s),
    TsTypeOperatorOp::Unique => "unique".serialize(s),
    TsTypeOperatorOp::ReadOnly => "readonly".serialize(s),
  }
}

pub trait ToType {
  fn to_type(&self, ctx: &mut Context) -> Type;
}

/// Builds the fully-qualified id for a top-level declaration (`<path>:<name>`).
fn qualified_id(ctx: &Context, name: &JsWord) -> JsWord {
  format!("{}:{}", ctx.path.to_str().unwrap(), name).into()
}

impl ToType for Decl {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let mut ty = match self {
      Decl::Class(c) => c.to_type(ctx),
      Decl::Fn(f) => f.to_type(ctx),
      Decl::TsInterface(i) => i.to_type(ctx),
      Decl::TsTypeAlias(t) => t.to_type(ctx),
      Decl::Var(v) => v.to_type(ctx),
      _ => Type::Any,
    };
    ty.add_docs(jsdoc, None, Some(ctx));
    ty
  }
}

impl ToType for ClassDecl {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let mut ty = self.class.to_type(ctx);
    ty.set_id(qualified_id(ctx, &self.ident.sym));
    ty.set_name(self.ident.sym.clone());
    ty.add_docs(jsdoc, None, Some(ctx));
    ctx.add_decl(self.ident.to_id(), ty)
  }
}

impl ToType for ClassExpr {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let mut ty = self.class.to_type(ctx);
    if let Some(ident) = &self.ident {
      ty.set_id(qualified_id(ctx, &ident.sym));
      ty.set_name(ident.sym.clone());
      ty.add_docs(jsdoc, None, Some(ctx));
      ctx.add_decl(ident.to_id(), ty)
    } else {
      ty.add_docs(jsdoc, None, Some(ctx));
      ty
    }
  }
}

impl ToType for Class {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    // `id`/`name` are placeholders; named classes have them set by the
    // declaration/expression, and anonymous ones by the variable declarator.
    Type::Interface {
      id: "".into(),
      name: "".into(),
      extends: self
        .super_class
        .iter()
        .map(|e| {
          let base = e.to_type(ctx);
          if let Some(args) = &self.super_type_params {
            let type_parameters = args
              .params
              .iter()
              .map(|p| {
                let t = p.to_type(ctx);
                ctx.add_type(t)
              })
              .collect();
            let base = ctx.add_type(base);
            ctx.add_type(Type::Application {
              base,
              type_parameters,
            })
          } else {
            ctx.add_type(base)
          }
        })
        .collect(),
      properties: self
        .body
        .iter()
        .filter_map(|p| {
          let p = match p {
            ClassMember::ClassProp(p) => p.to_property(ctx),
            ClassMember::Method(m) => m.to_property(ctx),
            ClassMember::TsIndexSignature(t) => t.to_property(ctx),
            ClassMember::Constructor(c) => c.to_property(ctx),
            _ => return None,
          };
          Some((p.name.clone(), p))
        })
        .collect(),
      type_parameters: define_type_params(&self.type_params, ctx),
      description: jsdoc.description,
      access: jsdoc.access,
    }
  }
}

impl ToType for FnDecl {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let mut f = self.function.to_type(ctx);
    f.set_id(qualified_id(ctx, &self.ident.sym));
    f.set_name(self.ident.sym.clone());
    f.add_docs(jsdoc, None, Some(ctx));
    ctx.add_decl(self.ident.to_id(), f)
  }
}

impl ToType for FnExpr {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let mut f = self.function.to_type(ctx);
    if let Some(ident) = &self.ident {
      f.set_id(qualified_id(ctx, &ident.sym));
      f.set_name(ident.sym.clone());
      f.add_docs(jsdoc, None, Some(ctx));
      ctx.add_decl(ident.to_id(), f)
    } else {
      f.add_docs(jsdoc, None, Some(ctx));
      f
    }
  }
}

impl ToType for TsInterfaceDecl {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let ty = Type::Interface {
      id: format!("{}:{}", ctx.path.to_str().unwrap(), self.id.sym).into(),
      name: self.id.sym.clone(),
      type_parameters: define_type_params(&self.type_params, ctx),
      extends: self
        .extends
        .iter()
        .map(|p| {
          let t = p.to_type(ctx);
          ctx.add_type(t)
        })
        .collect(),
      properties: self
        .body
        .body
        .iter()
        .map(|p| {
          let p = p.to_property(ctx);
          (p.name.clone(), p)
        })
        .collect(),
      description: jsdoc.description,
      access: jsdoc.access,
    };
    ctx.add_decl(self.id.to_id(), ty)
  }
}

impl ToType for TsTypeAliasDecl {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let ty = Type::Alias {
      id: format!("{}:{}", ctx.path.to_str().unwrap(), self.id.sym).into(),
      name: self.id.sym.clone(),
      type_parameters: define_type_params(&self.type_params, ctx),
      value: {
        let t = self.type_ann.to_type(ctx);
        ctx.add_type(t)
      },
      description: jsdoc.description,
      access: jsdoc.access,
    };
    ctx.add_decl(self.id.to_id(), ty)
  }
}

impl ToType for VarDecl {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    for decl in &self.decls {
      let mut ty = decl.to_type(ctx);
      ty.add_docs(jsdoc.clone(), None, Some(ctx));
    }
    Type::Any
  }
}

impl ToType for VarDeclarator {
  fn to_type(&self, ctx: &mut Context) -> Type {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    if let Pat::Ident(binding) = &self.name {
      let mut t = if let Some(t) = &binding.type_ann {
        t.type_ann.to_type(ctx)
      } else if let Some(init) = &self.init {
        init.to_type(ctx)
      } else {
        Type::Any
      };
      // Interfaces (object literals/classes) and components take their id/name
      // from the variable they are assigned to.
      if t.is_interface_or_component(ctx) {
        let id = qualified_id(ctx, &binding.sym);
        let name = binding.sym.clone();
        if let Type::Ref(r) = t {
          ctx.api.types[r.0 as usize].set_id(id);
          ctx.api.types[r.0 as usize].set_name(name);
        } else {
          t.set_id(id);
          t.set_name(name);
        }
      }
      t.add_docs(jsdoc, None, Some(ctx));
      ctx.add_decl(binding.to_id(), t)
    } else {
      Type::Any
    }
  }
}

/// If `ty` is a function type whose return is a React element type, convert it
/// to a component. This lets a cast like `forwardRef(fn) as <T>(props) => JSX.Element`
/// keep the (more informative) annotated signature while still being classified
/// as a component.
fn component_if_element_return(ty: Type, ctx: &mut Context) -> Type {
  let is_component = match &ty {
    Type::Function { return_type, .. } => {
      ctx.api.types[return_type.0 as usize].is_react_element(ctx)
    }
    _ => false,
  };
  if is_component {
    split_component_ref(ty.to_component(), ctx)
  } else {
    ty
  }
}

/// The S2 cast pattern combines props and ref into one parameter:
/// `(props: SomeProps<T> & {ref?: RefType}) => ReactElement`. When a component's
/// props is such an intersection, split the `{ref?: …}` member out into `ref`
/// (matching what the JS implementation derives from `forwardRef`'s two params).
fn split_component_ref(mut ty: Type, ctx: &mut Context) -> Type {
  let props_id = match &ty {
    Type::Component {
      props: Some(props),
      ref_type: None,
      ..
    } => *props,
    _ => return ty,
  };

  let resolved = resolve_ref(ctx, props_id);
  let members = match &ctx.api.types[resolved.0 as usize] {
    Type::Intersection { types } => types.clone(),
    _ => return ty,
  };

  let mut ref_type = None;
  let mut rest = Vec::new();
  for member in members {
    if ref_type.is_none() {
      if let Some(r) = ref_object_value(ctx, member) {
        ref_type = Some(r);
        continue;
      }
    }
    rest.push(member);
  }

  let Some(ref_type) = ref_type else {
    return ty;
  };
  let new_props = match rest.len() {
    0 => None,
    1 => Some(rest[0]),
    _ => Some(ctx.add_type(Type::Intersection { types: rest })),
  };

  if let Type::Component {
    props, ref_type: r, ..
  } = &mut ty
  {
    *props = new_props;
    *r = Some(ref_type);
  }
  ty
}

/// If `id` resolves to an object type with a single `ref` property (`{ref?: X}`),
/// returns the type of that property (`X`).
fn ref_object_value(ctx: &Context, id: TypeId) -> Option<TypeId> {
  let id = resolve_ref(ctx, id);
  if let Type::Object { properties, .. } = &ctx.api.types[id.0 as usize] {
    if properties.len() == 1 {
      if let Some((name, prop)) = properties.iter().next() {
        if name == "ref" {
          return Some(prop.value);
        }
      }
    }
  }
  None
}

/// Follows `Type::Ref` chains to the concrete arena type id.
fn resolve_ref(ctx: &Context, mut id: TypeId) -> TypeId {
  while let Type::Ref(r) = &ctx.api.types[id.0 as usize] {
    id = *r;
  }
  id
}

impl ToType for Expr {
  fn to_type(&self, ctx: &mut Context) -> Type {
    match self {
      Expr::Lit(lit) => lit.to_type(ctx),
      Expr::Tpl(tpl) => tpl.to_type(ctx),
      Expr::Paren(paren_expr) => paren_expr.expr.to_type(ctx),
      Expr::TsTypeAssertion(ts_type_assertion) => {
        let t = ts_type_assertion.type_ann.to_type(ctx);
        component_if_element_return(t, ctx)
      }
      Expr::TsAs(ts_as_expr) => {
        let t = ts_as_expr.type_ann.to_type(ctx);
        component_if_element_return(t, ctx)
      }
      Expr::TsSatisfies(ts_satisfies_expr) => ts_satisfies_expr.type_ann.to_type(ctx),
      Expr::This(_) => Type::This,
      Expr::Fn(fn_expr) => fn_expr.to_type(ctx),
      Expr::Arrow(arrow_expr) => arrow_expr.to_type(ctx),
      Expr::Class(class_expr) => class_expr.to_type(ctx),
      Expr::Call(call_expr) => call_expr.to_type(ctx),
      Expr::Ident(ident) => ident.to_type(ctx),
      Expr::Object(object_lit) => object_lit.to_type(ctx),
      _ => Type::Any,
    }
  }
}

impl ToType for ObjectLit {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    // Object expressions are documented as interfaces, matching the JS
    // implementation. The id/name are filled in by the variable declarator.
    Type::Interface {
      id: "".into(),
      name: "".into(),
      extends: Vec::new(),
      properties: self
        .props
        .iter()
        .filter_map(|p| {
          let p = match p {
            PropOrSpread::Prop(p) => p.to_property(ctx),
            PropOrSpread::Spread(_) => return None,
          };
          Some((p.name.clone(), p))
        })
        .collect(),
      type_parameters: Vec::new(),
      description: None,
      access: None,
    }
  }
}

impl ToType for TsType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    match self {
      TsType::TsKeywordType(v) => v.to_type(ctx),
      TsType::TsThisType(_) => Type::This,
      TsType::TsFnOrConstructorType(v) => match v {
        TsFnOrConstructorType::TsFnType(v) => v.to_type(ctx),
        TsFnOrConstructorType::TsConstructorType(v) => v.to_type(ctx),
      },
      TsType::TsTypeRef(v) => v.to_type(ctx),
      TsType::TsTypeQuery(v) => v.to_type(ctx),
      TsType::TsTypeLit(v) => v.to_type(ctx),
      TsType::TsArrayType(v) => v.to_type(ctx),
      TsType::TsTupleType(v) => v.to_type(ctx),
      TsType::TsOptionalType(_) => Type::Any, // TODO
      TsType::TsRestType(_) => Type::Any,     // TODO
      TsType::TsUnionOrIntersectionType(v) => match v {
        TsUnionOrIntersectionType::TsUnionType(v) => v.to_type(ctx),
        TsUnionOrIntersectionType::TsIntersectionType(v) => v.to_type(ctx),
      },
      TsType::TsConditionalType(v) => v.to_type(ctx),
      TsType::TsInferType(_) => Type::Any, // TODO
      TsType::TsParenthesizedType(v) => v.type_ann.to_type(ctx),
      TsType::TsTypeOperator(v) => v.to_type(ctx),
      TsType::TsIndexedAccessType(v) => v.to_type(ctx),
      TsType::TsMappedType(v) => v.to_type(ctx),
      TsType::TsLitType(v) => v.lit.to_type(ctx),
      TsType::TsTypePredicate(_) => Type::Any, // TODO
      TsType::TsImportType(_) => Type::Any,    // TODO
    }
  }
}

impl ToType for TsKeywordType {
  fn to_type(&self, _ctx: &mut Context<'_>) -> Type {
    match self.kind {
      TsKeywordTypeKind::TsAnyKeyword => Type::Any,
      TsKeywordTypeKind::TsUnknownKeyword => Type::Unknown,
      TsKeywordTypeKind::TsNumberKeyword => Type::Number { value: None },
      TsKeywordTypeKind::TsObjectKeyword => Type::ObjectKeyword,
      TsKeywordTypeKind::TsBooleanKeyword => Type::Boolean { value: None },
      TsKeywordTypeKind::TsBigIntKeyword => Type::Any, // TODO
      TsKeywordTypeKind::TsStringKeyword => Type::String { value: None },
      TsKeywordTypeKind::TsSymbolKeyword => Type::Symbol,
      TsKeywordTypeKind::TsVoidKeyword => Type::Void,
      TsKeywordTypeKind::TsUndefinedKeyword => Type::Undefined,
      TsKeywordTypeKind::TsNullKeyword => Type::Null,
      TsKeywordTypeKind::TsNeverKeyword => Type::Never,
      TsKeywordTypeKind::TsIntrinsicKeyword => Type::Any, // TODO
    }
  }
}

impl ToType for TsLit {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    match self {
      TsLit::Number(v) => Type::Number {
        value: Some(v.value),
      },
      TsLit::Str(v) => Type::String {
        value: Some(v.value.clone().try_into_atom().unwrap()),
      },
      TsLit::Bool(v) => Type::Boolean {
        value: Some(v.value),
      },
      TsLit::BigInt(_) => Type::Any, // TODO
      TsLit::Tpl(v) => v.to_type(ctx),
    }
  }
}

impl ToType for Lit {
  fn to_type(&self, _ctx: &mut Context<'_>) -> Type {
    match self {
      Lit::Num(v) => Type::Number {
        value: Some(v.value),
      },
      Lit::Str(v) => Type::String {
        value: Some(v.value.clone().try_into_atom().unwrap()),
      },
      Lit::JSXText(v) => Type::String {
        value: Some(v.value.clone()),
      },
      Lit::Bool(v) => Type::Boolean {
        value: Some(v.value),
      },
      Lit::BigInt(_) => Type::Any, // TODO
      Lit::Null(_) => Type::Null,
      Lit::Regex(_) => Type::Any,
    }
  }
}

impl ToType for TsTplLitType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let mut elements = Vec::new();
    let mut i = 0;
    for q in &self.quasis {
      elements.push(ctx.add_type(Type::String {
        value: Some(q.raw.clone()),
      }));
      if !q.tail {
        let t = self.types[i].to_type(ctx);
        elements.push(ctx.add_type(t));
        i += 1;
      }
    }
    Type::Template { elements }
  }
}

impl ToType for Tpl {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let mut elements = Vec::new();
    let mut i = 0;
    for q in &self.quasis {
      elements.push(ctx.add_type(Type::String {
        value: Some(q.raw.clone()),
      }));
      if !q.tail {
        let t = self.exprs[i].to_type(ctx);
        elements.push(ctx.add_type(t));
        i += 1;
      }
    }
    Type::Template { elements }
  }
}

impl ToType for TsTypeLit {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Object {
      properties: self
        .members
        .iter()
        .map(|m| {
          let m = m.to_property(ctx);
          (m.name.clone(), m)
        })
        .collect(),
      description: None,
      access: None,
    }
  }
}

impl ToType for TsTypeParam {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let ty = Type::TypeParameter {
      name: self.name.sym.clone(),
      constraint: self.constraint.as_ref().map(|t| {
        let t = t.to_type(ctx);
        ctx.add_type(t)
      }),
      default: self.default.as_ref().map(|t| {
        let t = t.to_type(ctx);
        ctx.add_type(t)
      }),
    };
    ctx.add_decl(self.name.to_id(), ty)
  }
}

impl ToType for TsArrayType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Array {
      element_type: {
        let t = self.elem_type.to_type(ctx);
        ctx.add_type(t)
      },
    }
  }
}

impl ToType for TsTupleType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Tuple {
      elements: self
        .elem_types
        .iter()
        .map(|e| {
          let t = e.ty.to_type(ctx);
          ctx.add_type(t)
        })
        .collect(),
    }
  }
}

impl ToType for TsConditionalType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Conditional {
      check_type: {
        let t = self.check_type.to_type(ctx);
        ctx.add_type(t)
      },
      extends_type: {
        let t = self.extends_type.to_type(ctx);
        ctx.add_type(t)
      },
      true_type: {
        let t = self.true_type.to_type(ctx);
        ctx.add_type(t)
      },
      false_type: {
        let t = self.false_type.to_type(ctx);
        ctx.add_type(t)
      },
    }
  }
}

impl ToType for TsTypeOperator {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::TypeOperator {
      operator: self.op,
      value: {
        let t = self.type_ann.to_type(ctx);
        ctx.add_type(t)
      },
    }
  }
}

impl ToType for TsIndexedAccessType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::IndexedAccess {
      object_type: {
        let t = self.obj_type.to_type(ctx);
        ctx.add_type(t)
      },
      index_type: {
        let t = self.index_type.to_type(ctx);
        ctx.add_type(t)
      },
    }
  }
}

impl ToType for TsMappedType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Mapped {
      readonly: self.readonly.map(|r| match r {
        TruePlusMinus::True => "true".into(),
        TruePlusMinus::Plus => "+".into(),
        TruePlusMinus::Minus => "-".into(),
      }),
      type_parameter: {
        let t = self.type_param.to_type(ctx);
        ctx.define(self.type_param.name.to_id(), t)
      },
      type_annotation: {
        let t = self
          .type_ann
          .as_ref()
          .map(|t| t.to_type(ctx))
          .unwrap_or(Type::Any);
        ctx.add_type(t)
      },
    }
  }
}

impl ToType for TsUnionType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Union {
      elements: self
        .types
        .iter()
        .map(|t| {
          let t = t.to_type(ctx);
          ctx.add_type(t)
        })
        .collect(),
    }
  }
}

impl ToType for TsIntersectionType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Intersection {
      types: self
        .types
        .iter()
        .map(|t| {
          let t = t.to_type(ctx);
          ctx.add_type(t)
        })
        .collect(),
    }
  }
}

impl ToType for TsFnType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Function {
      id: None,
      name: None,
      type_parameters: define_type_params(&self.type_params, ctx),
      parameters: self.params.iter().map(|p| p.to_parameter(ctx)).collect(),
      return_type: {
        let t = self.type_ann.type_ann.to_type(ctx);
        ctx.add_type(t)
      },
      description: None,
      return_description: None,
      access: None,
      examples: Vec::new(),
    }
  }
}

impl ToType for TsConstructorType {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    Type::Function {
      id: None,
      name: None,
      type_parameters: define_type_params(&self.type_params, ctx),
      parameters: self.params.iter().map(|p| p.to_parameter(ctx)).collect(),
      return_type: {
        let t = self.type_ann.type_ann.to_type(ctx);
        ctx.add_type(t)
      },
      description: None,
      return_description: None,
      access: None,
      examples: Vec::new(),
    }
  }
}

impl ToType for TsTypeRef {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let base = self.type_name.to_type(ctx);
    if let Some(params) = &self.type_params {
      let params = params
        .params
        .iter()
        .map(|p| {
          let t = p.to_type(ctx);
          ctx.add_type(t)
        })
        .collect();
      Type::Application {
        base: ctx.add_type(base),
        type_parameters: params,
      }
    } else {
      base
    }
  }
}

impl ToType for TsTypeQuery {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let base = self.expr_name.to_type(ctx);
    if let Some(params) = &self.type_args {
      let params = params
        .params
        .iter()
        .map(|p| {
          let t = p.to_type(ctx);
          ctx.add_type(t)
        })
        .collect();
      Type::Application {
        base: ctx.add_type(base),
        type_parameters: params,
      }
    } else {
      base
    }
  }
}

impl ToType for TsTypeQueryExpr {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    match self {
      TsTypeQueryExpr::TsEntityName(v) => v.to_type(ctx),
      TsTypeQueryExpr::Import(_) => Type::Any, // TODO
    }
  }
}

/// The dotted name of an identifier/member-access expression used in a type
/// position (e.g. `Intl.NumberFormatOptions`), if it is a plain chain of names.
fn expr_dotted_name(expr: &Expr) -> Option<String> {
  match expr {
    Expr::Ident(id) => Some(id.sym.to_string()),
    Expr::Member(m) => {
      let obj = expr_dotted_name(&m.obj)?;
      let prop = m.prop.as_ident()?.sym.to_string();
      Some(format!("{obj}.{prop}"))
    }
    _ => None,
  }
}

impl ToType for TsExprWithTypeArgs {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    // Member expressions in a type position (e.g. `extends Intl.Foo`) become
    // dotted identifiers, matching the JS implementation.
    let base = match &*self.expr {
      Expr::Member(_) => match expr_dotted_name(&self.expr) {
        Some(name) => Type::Identifier { name: name.into() },
        None => self.expr.to_type(ctx),
      },
      _ => self.expr.to_type(ctx),
    };
    if let Some(params) = &self.type_args {
      let params = params
        .params
        .iter()
        .map(|p| {
          let t = p.to_type(ctx);
          ctx.add_type(t)
        })
        .collect();
      Type::Application {
        base: ctx.add_type(base),
        type_parameters: params,
      }
    } else {
      base
    }
  }
}

/// Builds the dotted textual name of an entity name (e.g. `React.JSX.Element`).
fn entity_name_str(name: &TsEntityName) -> String {
  match name {
    TsEntityName::Ident(id) => id.sym.to_string(),
    TsEntityName::TsQualifiedName(q) => {
      format!("{}.{}", entity_name_str(&q.left), q.right.sym)
    }
  }
}

impl ToType for TsEntityName {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    match self {
      TsEntityName::TsQualifiedName(name) => {
        // If the left side resolves to an interface/object in scope, the
        // qualified name refers to one of its members.
        let left = name.left.to_type(ctx);
        let resolved = match &left {
          Type::Ref(r) => &ctx.api.types[r.0 as usize],
          other => other,
        };
        if let Type::Interface { properties, .. } | Type::Object { properties, .. } = resolved {
          if let Some(p) = properties.get(&name.right.sym) {
            return Type::Ref(p.value);
          }
        }
        // Otherwise treat it as a dotted identifier (e.g. `JSX.Element`).
        Type::Identifier {
          name: entity_name_str(self).into(),
        }
      }
      TsEntityName::Ident(id) => id.to_type(ctx),
    }
  }
}

impl ToType for Ident {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    if let Some(decl) = ctx.decls.get(&self.to_id()) {
      if let Type::Reference { specifier, .. } = &ctx.api.types[decl.0 as usize] {
        ctx.api.dependencies.insert(specifier.clone());
      }
      Type::Ref(*decl)
    } else {
      ctx.add_decl(
        self.to_id(),
        Type::Identifier {
          name: self.sym.clone(),
        },
      )
    }
  }
}

impl ToType for Function {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let jsdoc = parse_jsdoc(self.span, ctx);
    let mut parameters: Vec<Parameter> = self.params.iter().map(|p| p.to_parameter(ctx)).collect();
    for param in parameters.iter_mut() {
      if let Some(desc) = jsdoc.params.get(&param.name) {
        param.description = Some(desc.clone());
      }
    }

    let return_type = self
      .return_type
      .as_ref()
      .map(|t| t.type_ann.to_type(ctx))
      .unwrap_or(Type::Any);

    let mut is_component = return_type.is_jsx(ctx);
    if !is_component {
      is_component = is_jsx(&self.body, ctx);
    }

    let f = Type::Function {
      id: None,
      name: None,
      type_parameters: define_type_params(&self.type_params, ctx),
      parameters,
      return_type: ctx.add_type(return_type),
      description: jsdoc.description,
      return_description: jsdoc.return_description,
      access: jsdoc.access,
      examples: jsdoc.examples,
    };

    if is_component { f.to_component() } else { f }
  }
}

impl ToType for ArrowExpr {
  fn to_type(&self, ctx: &mut Context<'_>) -> Type {
    let return_type = self
      .return_type
      .as_ref()
      .map(|t| t.type_ann.to_type(ctx))
      .unwrap_or(Type::Any);

    let mut is_component = return_type.is_jsx(ctx);
    if !is_component {
      is_component = match &*self.body {
        BlockStmtOrExpr::Expr(e) => is_jsx_expr(e, ctx),
        BlockStmtOrExpr::BlockStmt(b) => is_jsx(b, ctx),
      };
    }

    let f = Type::Function {
      id: None,
      name: None,
      type_parameters: define_type_params(&self.type_params, ctx),
      parameters: self.params.iter().map(|p| p.to_parameter(ctx)).collect(),
      return_type: ctx.add_type(return_type),
      description: None,
      return_description: None,
      access: None,
      examples: Vec::new(),
    };

    if is_component { f.to_component() } else { f }
  }
}

fn is_jsx<'a, 'b, V: VisitWith<JSXVisitor<'a, 'b>>>(v: &V, ctx: &'a Context<'b>) -> bool {
  let mut visitor = JSXVisitor { is_jsx: false, ctx };
  v.visit_with(&mut visitor);
  visitor.is_jsx
}

struct JSXVisitor<'a, 'b> {
  is_jsx: bool,
  ctx: &'a Context<'b>,
}

impl<'a, 'b> Visit for JSXVisitor<'a, 'b> {
  fn visit_return_stmt(&mut self, node: &ReturnStmt) {
    if node
      .arg
      .as_ref()
      .map(|arg| is_jsx_expr(arg, self.ctx))
      .unwrap_or_default()
    {
      self.is_jsx = true;
    }
  }

  fn visit_function(&mut self, _node: &Function) {
    // skip children
  }

  fn visit_class(&mut self, _node: &Class) {
    // skip children
  }

  fn visit_arrow_expr(&mut self, _node: &ArrowExpr) {
    // skip children
  }
}

/// Unwraps parenthesized and type-cast expressions to reach the underlying
/// expression, e.g. `(forwardRef as forwardRefType)` -> `forwardRef`. Needed to
/// recognize calls like `(forwardRef as forwardRefType)(fn)`, a common pattern
/// for typing a `forwardRef` component's generic parameters.
fn unwrap_as(expr: &Expr) -> &Expr {
  match expr {
    Expr::Paren(p) => unwrap_as(&p.expr),
    Expr::TsAs(a) => unwrap_as(&a.expr),
    Expr::TsTypeAssertion(a) => unwrap_as(&a.expr),
    Expr::TsSatisfies(a) => unwrap_as(&a.expr),
    _ => expr,
  }
}

fn is_jsx_expr(expr: &Expr, ctx: &Context) -> bool {
  match expr.unwrap_parens() {
    Expr::JSXElement(_) | Expr::JSXFragment(_) => true,
    Expr::Call(call) => {
      if let Callee::Expr(callee) = &call.callee {
        let callee = unwrap_as(callee);
        ctx.references_import(callee, "react", "cloneElement")
          || ctx.references_import(callee, "react-dom", "createPortal")
      } else {
        false
      }
    }
    _ => false,
  }
}

impl ToType for CallExpr {
  fn to_type(&self, ctx: &mut Context) -> Type {
    if let Callee::Expr(e) = &self.callee {
      let e = unwrap_as(e);
      if ctx.references_import(e, "react", "forwardRef")
        || matches!(e, Expr::Ident(id) if id.sym == "createHideableComponent")
      {
        self
          .args
          .first()
          .map(|a| a.expr.to_type(ctx))
          .unwrap_or(Type::Any)
      } else if matches!(e, Expr::Ident(id) if id.sym == "createLeafComponent" || id.sym == "createBranchComponent")
      {
        self
          .args
          .get(1)
          .map(|a| a.expr.to_type(ctx))
          .unwrap_or(Type::Any)
      } else {
        Type::Any
      }
    } else {
      Type::Any
    }
  }
}

pub fn define_type_params(
  type_params: &Option<Box<TsTypeParamDecl>>,
  ctx: &mut Context<'_>,
) -> Vec<TypeId> {
  type_params
    .as_ref()
    .map(|t| {
      t.params
        .iter()
        .map(|p| {
          let t = p.to_type(ctx);
          ctx.define(p.name.to_id(), t)
        })
        .collect()
    })
    .unwrap_or_default()
}
