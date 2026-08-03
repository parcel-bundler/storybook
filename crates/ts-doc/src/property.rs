use swc_core::common::Spanned;
use swc_core::ecma::ast::*;
use swc_core::ecma::atoms::Atom as JsWord;

use crate::jsdoc::{JsDocs, parse_jsdoc};
use crate::parameter::ToParameter;
use crate::parse::Context;
use crate::ty::{ToType, define_type_params};
use crate::{Parameter, Type, TypeId};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Property {
  pub name: JsWord,
  pub index_type: Option<TypeId>,
  pub value: TypeId,
  pub optional: bool,
  pub is_method: bool,
  #[serde(rename = "static")]
  pub is_static: bool,
  #[serde(rename = "abstract")]
  pub is_abstract: bool,
  pub description: Option<JsWord>,
  pub access: Option<JsWord>,
  pub selector: Option<JsWord>,
  pub default: Option<JsWord>,
}

pub trait ToProperty {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property;
}

/// Applies `@param` descriptions (by name) to a signature's parameters.
fn apply_param_docs(mut params: Vec<Parameter>, jsdoc: &JsDocs) -> Vec<Parameter> {
  for param in params.iter_mut() {
    if let Some(desc) = jsdoc.params.get(&param.name) {
      param.description = Some(desc.clone());
    }
  }
  params
}

/// Builds a `Type::Function` for a signature/method with the given pieces.
fn function_type(
  type_params: &Option<Box<TsTypeParamDecl>>,
  params: Vec<Parameter>,
  return_type: TypeId,
  ctx: &mut Context<'_>,
) -> Type {
  Type::Function {
    id: None,
    name: None,
    type_parameters: define_type_params(type_params, ctx),
    parameters: params,
    return_type,
    description: None,
    return_description: None,
    access: None,
    examples: Vec::new(),
  }
}

impl ToProperty for TsTypeElement {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    match self {
      TsTypeElement::TsCallSignatureDecl(v) => {
        let return_type = optional_type_ann(v.type_ann.as_deref(), ctx);
        let value = function_type(
          &v.type_params,
          apply_param_docs(v.params.iter().map(|p| p.to_parameter(ctx)).collect(), &jsdoc),
          return_type,
          ctx,
        );
        Property {
          name: "".into(),
          index_type: None,
          value: ctx.add_type(value),
          optional: false,
          is_method: true,
          is_static: false,
          is_abstract: false,
          description: jsdoc.description,
          access: jsdoc.access,
          selector: jsdoc.selector,
          default: jsdoc.default,
        }
      }
      TsTypeElement::TsConstructSignatureDecl(v) => {
        let return_type = optional_type_ann(v.type_ann.as_deref(), ctx);
        let value = function_type(
          &v.type_params,
          apply_param_docs(v.params.iter().map(|p| p.to_parameter(ctx)).collect(), &jsdoc),
          return_type,
          ctx,
        );
        Property {
          name: "constructor".into(),
          index_type: None,
          value: ctx.add_type(value),
          optional: false,
          is_method: true,
          is_static: false,
          is_abstract: false,
          description: jsdoc.description,
          access: jsdoc.access,
          selector: jsdoc.selector,
          default: jsdoc.default,
        }
      }
      TsTypeElement::TsPropertySignature(v) => Property {
        name: expr_to_name(&v.key),
        index_type: None,
        value: optional_type_ann(v.type_ann.as_deref(), ctx),
        optional: v.optional,
        is_method: false,
        is_static: false,
        is_abstract: false,
        description: jsdoc.description,
        access: jsdoc.access,
        selector: jsdoc.selector,
        default: jsdoc.default,
      },
      TsTypeElement::TsGetterSignature(v) => Property {
        name: expr_to_name(&v.key),
        index_type: None,
        value: optional_type_ann(v.type_ann.as_deref(), ctx),
        optional: false,
        is_method: false,
        is_static: false,
        is_abstract: false,
        access: jsdoc.access,
        default: jsdoc.default,
        description: jsdoc.description,
        selector: jsdoc.selector,
      },
      TsTypeElement::TsSetterSignature(v) => Property {
        name: expr_to_name(&v.key),
        index_type: None,
        value: v.param.to_parameter(ctx).value,
        optional: false,
        is_method: false,
        is_static: false,
        is_abstract: false,
        access: jsdoc.access,
        default: jsdoc.default,
        description: jsdoc.description,
        selector: jsdoc.selector,
      },
      TsTypeElement::TsMethodSignature(v) => {
        let return_type = optional_type_ann(v.type_ann.as_deref(), ctx);
        let value = function_type(
          &v.type_params,
          apply_param_docs(v.params.iter().map(|p| p.to_parameter(ctx)).collect(), &jsdoc),
          return_type,
          ctx,
        );
        Property {
          name: expr_to_name(&v.key),
          index_type: None,
          value: ctx.add_type(value),
          optional: v.optional,
          is_method: true,
          is_static: false,
          is_abstract: false,
          description: jsdoc.description,
          access: jsdoc.access,
          selector: jsdoc.selector,
          default: jsdoc.default,
        }
      }
      TsTypeElement::TsIndexSignature(v) => v.to_property(ctx),
    }
  }
}

/// Converts an optional type annotation to a `TypeId`, defaulting to `any`.
fn optional_type_ann(type_ann: Option<&TsTypeAnn>, ctx: &mut Context<'_>) -> TypeId {
  let t = type_ann
    .map(|t| t.type_ann.to_type(ctx))
    .unwrap_or(Type::Any);
  ctx.add_type(t)
}

fn expr_to_name(expr: &Expr) -> JsWord {
  match &expr {
    Expr::Lit(Lit::Str(s)) => s.value.clone().try_into_atom().unwrap(),
    Expr::Lit(Lit::Num(n)) => n.value.to_string().into(),
    Expr::Ident(id) => id.sym.clone(),
    _ => "unknown".into(),
  }
}

impl ToProperty for TsIndexSignature {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property {
    let jsdoc = parse_jsdoc(self.span, ctx);
    let param = self.params.first().unwrap().to_parameter(ctx);
    Property {
      name: param.name,
      index_type: Some(param.value),
      value: optional_type_ann(self.type_ann.as_deref(), ctx),
      optional: false,
      is_method: false,
      is_static: false,
      is_abstract: false,
      default: jsdoc.default,
      description: jsdoc.description,
      access: jsdoc.access,
      selector: jsdoc.selector,
    }
  }
}

impl ToProperty for ClassProp {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let name = prop_name(&self.key);
    Property {
      name,
      index_type: None,
      value: optional_type_ann(self.type_ann.as_deref(), ctx),
      optional: self.is_optional,
      is_method: false,
      is_static: self.is_static,
      is_abstract: self.is_abstract,
      access: convert_access(&self.accessibility).or(jsdoc.access),
      default: jsdoc.default,
      description: jsdoc.description,
      selector: jsdoc.selector,
    }
  }
}

impl ToProperty for ClassMethod {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property {
    let jsdoc = parse_jsdoc(self.span(), ctx);
    let name = prop_name(&self.key);
    let access = convert_access(&self.accessibility).or(jsdoc.access);
    match &self.kind {
      MethodKind::Getter => Property {
        name,
        index_type: None,
        value: optional_type_ann(self.function.return_type.as_deref(), ctx),
        optional: self.is_optional,
        is_method: false,
        is_static: self.is_static,
        is_abstract: self.is_abstract,
        access,
        default: jsdoc.default,
        description: jsdoc.description,
        selector: jsdoc.selector,
      },
      MethodKind::Setter => Property {
        name,
        index_type: None,
        value: {
          let t = if let Some(t) = &self
            .function
            .params
            .first()
            .and_then(|p| p.pat.as_ident())
            .and_then(|i| i.type_ann.as_ref())
          {
            t.type_ann.to_type(ctx)
          } else {
            Type::Any
          };
          ctx.add_type(t)
        },
        optional: false,
        is_method: false,
        is_static: self.is_static,
        is_abstract: self.is_abstract,
        access,
        default: jsdoc.default,
        description: jsdoc.description,
        selector: jsdoc.selector,
      },
      MethodKind::Method => {
        let mut function = self.function.to_type(ctx);
        // The method's own description lives on the property node, not on its
        // inner function value (matches the JS implementation).
        if let Type::Function { description, .. } = &mut function {
          *description = None;
        }
        Property {
          name,
          index_type: None,
          value: ctx.add_type(function),
          optional: self.is_optional,
          is_method: true,
          is_static: self.is_static,
          is_abstract: self.is_abstract,
          access,
          default: jsdoc.default,
          description: jsdoc.description,
          selector: jsdoc.selector,
        }
      }
    }
  }
}

impl ToProperty for Constructor {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property {
    let jsdoc = parse_jsdoc(self.span, ctx);
    let name = if let Some(key) = self.key.as_ident() {
      key.sym.clone()
    } else {
      "constructor".into()
    };
    let return_type = ctx.add_type(Type::Void);
    let parameters = self
      .params
      .iter()
      .map(|p| match &p {
        ParamOrTsParamProp::Param(t) => t.to_parameter(ctx),
        ParamOrTsParamProp::TsParamProp(t) => t.param.to_parameter(ctx),
      })
      .collect();
    let parameters = apply_param_docs(parameters, &jsdoc);
    let value = function_type(&None, parameters, return_type, ctx);
    Property {
      name,
      index_type: None,
      value: ctx.add_type(value),
      optional: self.is_optional,
      is_method: true,
      is_static: false,
      is_abstract: false,
      access: convert_access(&self.accessibility).or(jsdoc.access),
      default: jsdoc.default,
      description: jsdoc.description,
      selector: jsdoc.selector,
    }
  }
}

/// Converts a class member's value type (property, method, or object property).
impl ToProperty for Prop {
  fn to_property(&self, ctx: &mut Context<'_>) -> Property {
    match self {
      Prop::Shorthand(id) => {
        let value = ctx.add_type(Type::Any);
        Property {
          name: id.sym.clone(),
          index_type: None,
          value,
          optional: false,
          is_method: false,
          is_static: false,
          is_abstract: false,
          description: None,
          access: None,
          selector: None,
          default: None,
        }
      }
      Prop::KeyValue(kv) => {
        let t = kv.value.to_type(ctx);
        Property {
          name: prop_name(&kv.key),
          index_type: None,
          value: ctx.add_type(t),
          optional: false,
          is_method: false,
          is_static: false,
          is_abstract: false,
          description: None,
          access: None,
          selector: None,
          default: None,
        }
      }
      Prop::Method(m) => {
        let f = m.function.to_type(ctx);
        Property {
          name: prop_name(&m.key),
          index_type: None,
          value: ctx.add_type(f),
          optional: false,
          is_method: true,
          is_static: false,
          is_abstract: false,
          description: None,
          access: None,
          selector: None,
          default: None,
        }
      }
      Prop::Getter(g) => Property {
        name: prop_name(&g.key),
        index_type: None,
        value: optional_type_ann(g.type_ann.as_deref(), ctx),
        optional: false,
        is_method: false,
        is_static: false,
        is_abstract: false,
        description: None,
        access: None,
        selector: None,
        default: None,
      },
      Prop::Setter(s) => {
        let value = ctx.add_type(Type::Any);
        Property {
          name: prop_name(&s.key),
          index_type: None,
          value,
          optional: false,
          is_method: false,
          is_static: false,
          is_abstract: false,
          description: None,
          access: None,
          selector: None,
          default: None,
        }
      }
      Prop::Assign(a) => {
        let value = ctx.add_type(Type::Any);
        Property {
          name: a.key.sym.clone(),
          index_type: None,
          value,
          optional: false,
          is_method: false,
          is_static: false,
          is_abstract: false,
          description: None,
          access: None,
          selector: None,
          default: None,
        }
      }
    }
  }
}

fn prop_name(name: &PropName) -> JsWord {
  match name {
    PropName::Ident(id) => id.sym.clone(),
    PropName::Str(s) => s.value.clone().try_into_atom().unwrap(),
    PropName::Num(n) => n.value.to_string().into(),
    PropName::BigInt(b) => b.value.to_string().into(),
    PropName::Computed(_) => "unknown".into(),
  }
}

fn convert_access(value: &Option<Accessibility>) -> Option<JsWord> {
  match value {
    Some(Accessibility::Public) => Some("public".into()),
    Some(Accessibility::Protected) => Some("protected".into()),
    Some(Accessibility::Private) => Some("private".into()),
    None => None,
  }
}
