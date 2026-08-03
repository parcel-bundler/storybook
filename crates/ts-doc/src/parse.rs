use std::collections::HashMap;
use std::path::Path;
use swc_core::common::comments::SingleThreadedComments;
use swc_core::ecma::atoms::Atom as JsWord;
use swc_core::ecma::visit::{Visit, VisitWith};
use swc_core::ecma::{ast::*, parser::TsSyntax};
use swc_core::{
  common::{FileName, Globals, Mark, SourceMap, sync::Lrc},
  ecma::{parser::parse_file_as_module, transforms::base::resolver, visit::VisitMutWith},
};

use crate::jsdoc::{JsDocs, parse_jsdoc};
use crate::ty::ToType;
use crate::{API, Type, TypeId};

pub fn parse(path: &Path, code: String) -> API {
  let source_map = Lrc::new(SourceMap::default());
  let source_file = source_map.new_source_file(Lrc::new(FileName::Real(path.to_owned())), code);
  let mut recovered_errors = std::vec::Vec::new();
  let comments = SingleThreadedComments::default();
  let tsx = path
    .extension()
    .map(|e| e == "tsx" || e == "jsx")
    .unwrap_or(false);
  let mut module = match parse_file_as_module(
    &source_file,
    swc_core::ecma::parser::Syntax::Typescript(TsSyntax {
      dts: path.to_str().map(|p| p.ends_with(".d.ts")).unwrap_or(false),
      tsx,
      ..Default::default()
    }),
    Default::default(),
    Some(&comments),
    &mut recovered_errors,
  ) {
    Ok(module) => module,
    Err(_) => {
      return API::default();
    }
  };

  let mut decls = HashMap::new();
  let mut api = API::default();

  let mut ctx = Context {
    api: &mut api,
    comments: &comments,
    decls: &mut decls,
    path,
  };

  swc_core::common::GLOBALS.set(&Globals::new(), || {
    let unresolved_mark = Mark::fresh(Mark::root());
    let global_mark = Mark::fresh(Mark::root());
    module.visit_mut_with(&mut resolver(unresolved_mark, global_mark, true));
    module.visit_with(&mut ctx);
  });

  api
}

pub struct Context<'a> {
  pub path: &'a Path,
  pub decls: &'a mut HashMap<Id, TypeId>,
  pub api: &'a mut API,
  pub comments: &'a SingleThreadedComments,
}

impl<'a> Context<'a> {
  pub fn add_decl(&mut self, id: Id, ty: Type) -> Type {
    let type_id = self.define(id, ty);
    Type::Ref(type_id)
  }

  pub fn define(&mut self, id: Id, ty: Type) -> TypeId {
    if let Type::Ref(t) = ty {
      return t;
    }

    if let Some(&existing) = self.decls.get(&id) {
      let mut ty = ty;
      // Preserve a previous overload's description if this declaration (e.g. a
      // function implementation signature) lacks its own.
      if ty.description().is_none() {
        if let Some(prev) = self.api.types[existing.0 as usize].description().cloned() {
          ty.set_description(Some(prev));
        }
      }
      self.api.types[existing.0 as usize] = ty;
      existing
    } else {
      let t = self.add_type(ty);
      self.decls.insert(id.clone(), t);
      t
    }
  }

  pub fn add_type(&mut self, ty: Type) -> TypeId {
    self.api.add_type(ty)
  }

  /// Applies JSDoc parsed from an `export` statement to the exported type.
  pub fn apply_export_docs(&mut self, ty: TypeId, jsdoc: &JsDocs) {
    if jsdoc.is_empty() {
      return;
    }
    // Temporarily move the type out so `add_docs` can also borrow `self` (it
    // may reach into the arena to annotate a function's return type).
    let mut t = std::mem::replace(&mut self.api.types[ty.0 as usize], Type::Any);
    t.add_docs(jsdoc.clone(), None, Some(self));
    self.api.types[ty.0 as usize] = t;
  }

  pub fn references_import(&self, expr: &Expr, import: &str, name: &str) -> bool {
    match expr {
      Expr::Ident(id) => {
        if let Some(decl) = self.decls.get(&id.to_id()) {
          if let Type::Reference {
            imported,
            specifier,
            ..
          } = &self.api.types[decl.0 as usize]
          {
            return specifier == import && matches!(imported, Some(imported) if name == imported);
          }
        }
        false
      }
      Expr::Member(member) => {
        if let Some(id) = member.prop.as_ident() {
          if self.references_import(&member.obj, import, "default")
            || self.references_import(&member.obj, import, "*")
          {
            return id.sym == name;
          }
        }
        false
      }
      _ => false,
    }
  }
}

impl<'a> Visit for Context<'a> {
  fn visit_import_decl(&mut self, node: &ImportDecl) {
    for specifier in &node.specifiers {
      match specifier {
        ImportSpecifier::Named(named) => {
          let t = Type::Reference {
            imported: Some(match &named.imported {
              Some(ModuleExportName::Ident(id)) => id.sym.clone(),
              Some(ModuleExportName::Str(s)) => s.value.clone().try_into_atom().unwrap(),
              None => named.local.sym.clone(),
            }),
            specifier: node.src.value.clone().try_into_atom().unwrap(),
            local: Some(named.local.sym.clone()),
          };
          self.add_decl(named.local.to_id(), t);
        }
        ImportSpecifier::Default(default) => {
          let t = Type::Reference {
            imported: Some("default".into()),
            specifier: node.src.value.clone().try_into_atom().unwrap(),
            local: Some(default.local.sym.clone()),
          };
          self.add_decl(default.local.to_id(), t);
        }
        ImportSpecifier::Namespace(ns) => {
          let t = Type::Reference {
            imported: None,
            specifier: node.src.value.clone().try_into_atom().unwrap(),
            local: Some(ns.local.sym.clone()),
          };
          self.add_decl(ns.local.to_id(), t);
        }
      }
    }
  }

  fn visit_export_decl(&mut self, node: &ExportDecl) {
    // JSDoc comments on an exported declaration attach to the `export`
    // statement, not the inner declaration, so parse them here and apply.
    let jsdoc = parse_jsdoc(node.span, self);
    match &node.decl {
      Decl::Class(class) => {
        if let Type::Ref(ty) = class.to_type(self) {
          self.apply_export_docs(ty, &jsdoc);
          self.api.exports.insert(class.ident.sym.clone(), ty);
        }
      }
      Decl::Fn(f) => {
        if let Type::Ref(ty) = f.to_type(self) {
          self.apply_export_docs(ty, &jsdoc);
          self.api.exports.insert(f.ident.sym.clone(), ty);
        }
      }
      Decl::TsInterface(i) => {
        if let Type::Ref(ty) = i.to_type(self) {
          self.apply_export_docs(ty, &jsdoc);
          self.api.exports.insert(i.id.sym.clone(), ty);
        }
      }
      Decl::TsTypeAlias(i) => {
        if let Type::Ref(ty) = i.to_type(self) {
          self.apply_export_docs(ty, &jsdoc);
          self.api.exports.insert(i.id.sym.clone(), ty);
        }
      }
      Decl::Var(v) => {
        for decl in &v.decls {
          if let Some(name) = decl.name.as_ident() {
            if let Type::Ref(ty) = decl.to_type(self) {
              self.apply_export_docs(ty, &jsdoc);
              self.api.exports.insert(name.sym.clone(), ty);
            }
          } else {
            // Destructured export, e.g. `export const {a, b} = ...`.
            decl.to_type(self);
            let mut names = Vec::new();
            collect_binding_idents(&decl.name, &mut names);
            for name in names {
              let ty = self.add_type(Type::Any);
              self.api.exports.insert(name, ty);
            }
          }
        }
      }
      _ => {}
    }
  }

  fn visit_export_default_decl(&mut self, node: &ExportDefaultDecl) {
    match &node.decl {
      DefaultDecl::Class(class_expr) => {
        let ty = class_expr.to_type(self);
        let t = self.add_type(ty);
        self.api.exports.insert("default".into(), t);
      }
      DefaultDecl::Fn(fn_expr) => {
        let ty = fn_expr.to_type(self);
        let t = self.add_type(ty);
        self.api.exports.insert("default".into(), t);
      }
      DefaultDecl::TsInterfaceDecl(ts_interface_decl) => {
        let ty = ts_interface_decl.to_type(self);
        let t = self.add_type(ty);
        self.api.exports.insert("default".into(), t);
      }
    }
  }

  fn visit_named_export(&mut self, node: &NamedExport) {
    if let Some(src) = &node.src {
      self
        .api
        .dependencies
        .insert(src.value.clone().try_into_atom().unwrap());
      for specifier in &node.specifiers {
        match specifier {
          ExportSpecifier::Named(s) => {
            let orig = match &s.orig {
              ModuleExportName::Ident(id) => id.sym.clone(),
              ModuleExportName::Str(s) => s.value.clone().try_into_atom().unwrap(),
            };
            let exported = match &s.exported {
              Some(ModuleExportName::Ident(id)) => id.sym.clone(),
              Some(ModuleExportName::Str(s)) => s.value.clone().try_into_atom().unwrap(),
              None => orig.clone(),
            };
            let ty = Type::Reference {
              imported: Some(orig),
              specifier: src.value.clone().try_into_atom().unwrap(),
              local: None,
            };
            let t = self.add_type(ty);
            self.api.exports.insert(exported, t);
          }
          ExportSpecifier::Namespace(_) => {}
          ExportSpecifier::Default(_) => {}
        }
      }
    } else {
      for specifier in &node.specifiers {
        match specifier {
          ExportSpecifier::Named(s) => {
            let id = match &s.orig {
              ModuleExportName::Ident(id) => id,
              ModuleExportName::Str(_) => unreachable!(),
            };
            let exported = match &s.exported {
              Some(ModuleExportName::Ident(id)) => id.sym.clone(),
              Some(ModuleExportName::Str(s)) => s.value.clone().try_into_atom().unwrap(),
              None => id.sym.clone(),
            };
            let ty = id.to_type(self);
            if let Type::Ref(t) = ty {
              // The exported alias becomes the type's name (e.g. `X as Y`).
              self.api.types[t.0 as usize].set_name(exported.clone());
              self.api.exports.insert(exported, t);
            }
          }
          ExportSpecifier::Namespace(_) => {}
          ExportSpecifier::Default(_) => {}
        }
      }
    }
  }

  fn visit_export_all(&mut self, node: &ExportAll) {
    self
      .api
      .dependencies
      .insert(node.src.value.clone().try_into_atom().unwrap());
    self
      .api
      .export_all
      .push(node.src.value.clone().try_into_atom().unwrap());
  }

  fn visit_decl(&mut self, node: &Decl) {
    node.to_type(self);
  }
}

/// Collects the names bound by a (possibly destructuring) pattern.
fn collect_binding_idents(pat: &Pat, out: &mut Vec<JsWord>) {
  match pat {
    Pat::Ident(id) => out.push(id.sym.clone()),
    Pat::Array(a) => {
      for el in a.elems.iter().flatten() {
        collect_binding_idents(el, out);
      }
    }
    Pat::Object(o) => {
      for prop in &o.props {
        match prop {
          ObjectPatProp::KeyValue(kv) => collect_binding_idents(&kv.value, out),
          ObjectPatProp::Assign(a) => out.push(a.key.sym.clone()),
          ObjectPatProp::Rest(r) => collect_binding_idents(&r.arg, out),
        }
      }
    }
    Pat::Rest(r) => collect_binding_idents(&r.arg, out),
    Pat::Assign(a) => collect_binding_idents(&a.left, out),
    _ => {}
  }
}
