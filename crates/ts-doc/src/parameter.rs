use swc_core::ecma::ast::*;
use swc_core::ecma::atoms::Atom as JsWord;

use crate::parse::Context;
use crate::ty::ToType;
use crate::{Type, TypeId};

#[derive(Clone, Debug, serde::Serialize)]
pub struct Parameter {
  pub name: JsWord,
  pub value: TypeId,
  pub optional: bool,
  pub rest: bool,
  pub description: Option<JsWord>,
}

pub trait ToParameter {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter;
}

impl ToParameter for Param {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    self.pat.to_parameter(ctx)
  }
}

impl ToParameter for Pat {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    match self {
      Pat::Ident(id) => {
        let name = id.sym.clone();
        Parameter {
          name,
          value: {
            let t = id
              .type_ann
              .as_ref()
              .map(|t| t.type_ann.to_type(ctx))
              .unwrap_or(Type::Any);
            ctx.add_type(t)
          },
          optional: id.id.optional,
          rest: false,
          description: None,
        }
      }
      Pat::Rest(r) => r.to_parameter(ctx),
      Pat::Array(a) => a.to_parameter(ctx),
      Pat::Object(o) => o.to_parameter(ctx),
      Pat::Assign(a) => a.to_parameter(ctx),
      _ => Parameter {
        name: "unknown".into(),
        value: ctx.add_type(Type::Any),
        optional: false,
        rest: false,
        description: None,
      },
    }
  }
}

impl ToParameter for TsFnParam {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    match self {
      TsFnParam::Ident(id) => Parameter {
        name: id.sym.clone(),
        value: {
          let t = id
            .type_ann
            .as_ref()
            .map(|t| t.type_ann.to_type(ctx))
            .unwrap_or(Type::Any);
          ctx.add_type(t)
        },
        optional: id.optional,
        rest: false,
        description: None,
      },
      TsFnParam::Array(a) => a.to_parameter(ctx),
      TsFnParam::Object(o) => o.to_parameter(ctx),
      TsFnParam::Rest(r) => r.to_parameter(ctx),
    }
  }
}

impl ToParameter for ArrayPat {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    Parameter {
      name: "unknown".into(),
      value: {
        let t = self
          .type_ann
          .as_ref()
          .map(|t| t.type_ann.to_type(ctx))
          .unwrap_or(Type::Any);
        ctx.add_type(t)
      },
      optional: self.optional,
      rest: false,
      description: None,
    }
  }
}

impl ToParameter for ObjectPat {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    Parameter {
      name: "unknown".into(),
      value: {
        let t = self
          .type_ann
          .as_ref()
          .map(|t| t.type_ann.to_type(ctx))
          .unwrap_or(Type::Any);
        ctx.add_type(t)
      },
      optional: self.optional,
      rest: false,
      description: None,
    }
  }
}

impl ToParameter for RestPat {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    let name = self
      .arg
      .as_ident()
      .map(|p| p.sym.clone())
      .unwrap_or("unknown".into());
    Parameter {
      name,
      value: {
        let t = self
          .type_ann
          .as_ref()
          .map(|t| t.type_ann.to_type(ctx))
          .unwrap_or(Type::Any);
        ctx.add_type(t)
      },
      optional: false,
      rest: true,
      description: None,
    }
  }
}

impl ToParameter for AssignPat {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    self.left.to_parameter(ctx)
  }
}

impl ToParameter for TsParamPropParam {
  fn to_parameter(&self, ctx: &mut Context<'_>) -> Parameter {
    match self {
      TsParamPropParam::Ident(i) => {
        let name = i.sym.clone();
        Parameter {
          name,
          value: ctx.add_type(Type::Any),
          optional: false,
          rest: false,
          description: None,
        }
      }
      TsParamPropParam::Assign(a) => a.to_parameter(ctx),
    }
  }
}
