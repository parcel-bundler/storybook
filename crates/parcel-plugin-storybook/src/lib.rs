use std::{collections::HashMap, path::Path};

use parcel_plugin::{Asset, Diagnostic, Plugin, register_plugin};
use swc_core::{
  common::{
    DUMMY_SP, FileName, SourceMap, Span, Spanned, SyntaxContext,
    comments::{Comment, CommentKind, Comments, SingleThreadedComments},
    sync::Lrc,
  },
  ecma::{
    ast::*,
    codegen::{Emitter, Node, text_writer::JsWriter},
    parser::{
      EsSyntax, Parser, StringInput, Syntax, TsSyntax, error::Error as ParserError, lexer::Lexer,
    },
  },
  quote,
};

struct StorybookPlugin;

const HMR_HELPER: &str = "@parcel/transformer-storybook-v3/csf-hmr.js";
const DOCGEN_HELPER: &str = "@parcel/transformer-storybook-v3/to-react-docgen.js";

impl Plugin for StorybookPlugin {
  fn new(_config: &[u8]) -> Result<Self, Diagnostic> {
    Ok(StorybookPlugin)
  }

  fn transform(
    &self,
    asset: &mut Asset,
    options: &parcel_plugin::Options,
  ) -> Result<(), Diagnostic> {
    let code = asset.content();
    let file_path = asset.file_path();
    let refresh_name = (options.env("NODE_ENV").as_deref() != Some("production")).then(|| {
      format!(
        "$parcel$ReactRefresh${}",
        &md5_hex(file_path.as_bytes())[28..]
      )
    });

    let transformed = process_csf(&code, Path::new(&file_path), refresh_name.as_deref())?;
    asset.set_content(transformed);
    Ok(())
  }
}

register_plugin!(StorybookPlugin);

#[derive(Clone, Debug)]
struct Story {
  export_name: String,
  target_name: String,
  statement_span: Span,
  source_span: Span,
  args_span: Option<Span>,
  parameters_span: Option<Span>,
}

#[derive(Clone, Debug)]
struct Meta {
  name: String,
  statement_span: Span,
  object_span: Span,
}

struct DocgenTarget {
  component: Expr,
  source: String,
  export_name: String,
}

fn process_csf(
  code: &str,
  file_path: &Path,
  refresh_name: Option<&str>,
) -> Result<String, Diagnostic> {
  let cm: Lrc<SourceMap> = Default::default();
  let comments = SingleThreadedComments::default();
  let source_file = cm.new_source_file(
    Lrc::new(FileName::Real(file_path.to_path_buf())),
    code.to_owned(),
  );
  let extension = file_path
    .extension()
    .and_then(|extension| extension.to_str());
  let lexer = Lexer::new(
    syntax_for_extension(extension),
    EsVersion::EsNext,
    StringInput::from(&*source_file),
    Some(&comments),
  );
  let mut parser = Parser::new_from(lexer);
  let parse_error = |error: ParserError| {
    Diagnostic::new(format!(
      "Unable to parse {}: {}",
      file_path.display(),
      error.kind().msg()
    ))
  };
  let mut module = parser.parse_module().map_err(parse_error)?;
  if let Some(error) = parser.take_errors().into_iter().next() {
    return Err(parse_error(error));
  }

  let meta = normalize_meta(&mut module, file_path)?;

  let mut stories = collect_stories(&module);
  filter_stories(meta_object(&module, &meta), &mut stories);
  collect_assigned_annotations(&module, &mut stories);

  enrich_meta(&mut module, &meta, &comments);
  enrich_stories(
    &mut module,
    &stories,
    &comments,
    code,
    source_file.start_pos,
  );

  let docgen = matches!(extension, Some("ts" | "tsx"))
    .then(|| resolve_docgen_target(&module, &meta))
    .flatten();
  if let Some(docgen) = &docgen {
    attach_docgen_info(&mut module, docgen);
    let mut docs_import: ModuleItem = quote!("import __docgenInfo from \"\";" as ModuleItem);
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import_decl)) = &mut docs_import else {
      unreachable!();
    };
    *import_decl.src = string_literal(format!("docs:{}", docgen.source));
    module.body.insert(0, docs_import);

    let helper = ident("$parcel$storybook$docgen");
    let mut helper_import: ModuleItem = quote!(
      "import * as $helper from \"\";" as ModuleItem,
      helper = helper,
    );
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import_decl)) = &mut helper_import else {
      unreachable!();
    };
    *import_decl.src = string_literal(DOCGEN_HELPER);
    module.body.insert(0, helper_import);
  }

  if let Some(refresh_name) = refresh_name {
    let original_meta = slice_span(code, source_file.start_pos, meta.object_span);
    let mut component_count =
      transform_refresh_stories(&mut module, &stories, code, source_file.start_pos);

    let mut meta_additions = Vec::new();
    if let Some(meta_object) = meta_object_mut(&mut module, &meta) {
      handle_render_property(meta_object, &mut meta_additions, &mut component_count);
      let source_hash = string_expr(md5_hex(original_meta.as_bytes()));
      let meta_hash = if let Some(docgen) = &docgen {
        let docs = docgen_export_expr(&docgen.export_name);
        quote!(
          "$source_hash + JSON.stringify($docs)" as Expr,
          source_hash: Expr = source_hash,
          docs: Expr = docs,
        )
      } else {
        source_hash
      };
      push_object_property(meta_object, "_hash", meta_hash);
    }
    module.body.extend(meta_additions);
    wrap_refresh(&mut module, refresh_name);
  }

  emit_module(&module, cm, &comments)
}

fn syntax_for_extension(extension: Option<&str>) -> Syntax {
  match extension {
    Some(extension @ ("ts" | "tsx")) => Syntax::Typescript(TsSyntax {
      tsx: extension == "tsx",
      decorators: true,
      no_early_errors: true,
      ..Default::default()
    }),
    extension => Syntax::Es(EsSyntax {
      jsx: extension == Some("jsx"),
      decorators: true,
      decorators_before_export: true,
      export_default_from: true,
      ..Default::default()
    }),
  }
}

fn collect_stories(module: &Module) -> Vec<Story> {
  let mut stories = Vec::new();
  for item in &module.body {
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) = item else {
      continue;
    };
    match &export.decl {
      Decl::Var(var) => {
        for declaration in &var.decls {
          let Some(name) = binding_name(&declaration.name) else {
            continue;
          };
          if name == "__namedExportsOrder" {
            continue;
          }
          let Some(expression) = declaration.init.as_deref().map(unwrap_ts_expr) else {
            continue;
          };
          let (args_span, parameters_span) = match expression {
            Expr::Object(object) => annotation_spans(object),
            _ => Default::default(),
          };
          let (target_name, source_span) = resolve_bound_story(module, name)
            .and_then(|target| binding_source_span(module, &target).map(|span| (target, span)))
            .unwrap_or_else(|| (name.to_owned(), expression.span()));
          stories.push(Story {
            export_name: name.to_owned(),
            target_name,
            statement_span: export.span,
            source_span,
            args_span,
            parameters_span,
          });
        }
      }
      Decl::Fn(function) => stories.push(Story {
        export_name: function.ident.sym.to_string(),
        target_name: function.ident.sym.to_string(),
        statement_span: export.span,
        source_span: function.span(),
        args_span: None,
        parameters_span: None,
      }),
      _ => {}
    }
  }
  stories
}

fn filter_stories(meta: Option<&ObjectLit>, stories: &mut Vec<Story>) {
  let Some(meta) = meta else {
    return;
  };
  let includes = string_array_property(meta, "includeStories");
  let excludes = string_array_property(meta, "excludeStories");
  stories.retain(|story| {
    !story.export_name.starts_with('_')
      && includes
        .as_ref()
        .is_none_or(|names| names.iter().any(|name| name == &story.export_name))
      && excludes
        .as_ref()
        .is_none_or(|names| !names.iter().any(|name| name == &story.export_name))
  });
}

fn collect_assigned_annotations(module: &Module, stories: &mut [Story]) {
  for item in &module.body {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
      continue;
    };
    let Expr::Assign(assignment) = &*statement.expr else {
      continue;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
      continue;
    };
    let (Expr::Ident(story), MemberProp::Ident(annotation)) = (&*member.obj, &member.prop) else {
      continue;
    };
    let Some(story) = stories
      .iter_mut()
      .find(|item| story.sym == item.export_name.as_str())
    else {
      continue;
    };
    match annotation.sym.as_ref() {
      "args" => story.args_span = Some(assignment.right.span()),
      "parameters" => story.parameters_span = Some(assignment.right.span()),
      _ => {}
    }
  }
}

fn annotation_spans(object: &ObjectLit) -> (Option<Span>, Option<Span>) {
  let mut args = None;
  let mut parameters = None;
  for property in &object.props {
    let PropOrSpread::Prop(property) = property else {
      continue;
    };
    if let Prop::KeyValue(property) = &**property {
      match property_name(&property.key) {
        Some("args") => args = Some(property.value.span()),
        Some("parameters") => parameters = Some(property.value.span()),
        _ => {}
      }
    }
  }
  (args, parameters)
}

fn enrich_meta(module: &mut Module, meta: &Meta, comments: &SingleThreadedComments) {
  let Some(description) = extract_description(comments, meta.statement_span.lo()) else {
    return;
  };
  let meta = ident(&meta.name);
  let description = string_expr(description);
  let statement: Stmt = quote!(
    "$meta.parameters = {...$meta.parameters, docs: {...$meta.parameters?.docs, description: {component: $description, ...$meta.parameters?.docs?.description}}};" as Stmt,
    meta = meta,
    description: Expr = description,
  );
  module.body.push(statement.into());
}

fn enrich_stories(
  module: &mut Module,
  stories: &[Story],
  comments: &SingleThreadedComments,
  code: &str,
  source_start: swc_core::common::BytePos,
) {
  for story in stories {
    let story_ident = ident(&story.export_name);
    let source = string_expr(slice_span(code, source_start, story.source_span));
    let statement: Stmt = quote!(
      "$story.parameters = {...$story.parameters, docs: {...$story.parameters?.docs, source: {originalSource: $source, ...$story.parameters?.docs?.source}}};" as Stmt,
      story = story_ident.clone(),
      source: Expr = source,
    );
    module.body.push(statement.into());
    if let Some(description) = extract_description(comments, story.statement_span.lo()) {
      let description = string_expr(description);
      let statement: Stmt = quote!(
        "$story.parameters = {...$story.parameters, docs: {...$story.parameters?.docs, description: {story: $description, ...$story.parameters?.docs?.description}}};" as Stmt,
        story = story_ident,
        description: Expr = description,
      );
      module.body.push(statement.into());
    }
  }
}

fn resolve_docgen_target(module: &Module, meta: &Meta) -> Option<DocgenTarget> {
  let component = unwrap_ts_expr(object_property(meta_object(module, meta)?, "component")?).clone();
  let (local_name, namespace_export) = component_import_reference(&component)?;

  for item in &module.body {
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
      continue;
    };
    if import.type_only {
      continue;
    }
    for specifier in &import.specifiers {
      let export_name = match specifier {
        ImportSpecifier::Default(specifier) if specifier.local.sym == local_name => {
          Some("default".to_owned())
        }
        ImportSpecifier::Named(specifier)
          if !specifier.is_type_only && specifier.local.sym == local_name =>
        {
          match &specifier.imported {
            Some(ModuleExportName::Ident(imported)) => Some(imported.sym.to_string()),
            Some(ModuleExportName::Str(_)) => None,
            None => Some(specifier.local.sym.to_string()),
          }
        }
        ImportSpecifier::Namespace(specifier) if specifier.local.sym == local_name => {
          namespace_export.clone()
        }
        _ => None,
      };
      if let Some(export_name) = export_name {
        return Some(DocgenTarget {
          component,
          source: import.src.value.as_str()?.to_owned(),
          export_name,
        });
      }
    }
  }
  None
}

fn component_import_reference(expression: &Expr) -> Option<(String, Option<String>)> {
  match unwrap_ts_expr(expression) {
    Expr::Ident(identifier) => Some((identifier.sym.to_string(), None)),
    Expr::Member(member) => {
      let Expr::Ident(identifier) = unwrap_ts_expr(&member.obj) else {
        return None;
      };
      let export_name = match &member.prop {
        MemberProp::Ident(property) => Some(property.sym.to_string()),
        MemberProp::Computed(property) => match unwrap_ts_expr(&property.expr) {
          Expr::Lit(Lit::Str(value)) => value.value.as_str().map(str::to_owned),
          _ => None,
        },
        MemberProp::PrivateName(_) => None,
      };
      Some((identifier.sym.to_string(), export_name))
    }
    _ => None,
  }
}

fn attach_docgen_info(module: &mut Module, target: &DocgenTarget) {
  let docs = docgen_export_expr(&target.export_name);
  let helper = ident("$parcel$storybook$docgen");
  let converted: Expr = quote!(
    "$helper.toReactDocgen($docs)" as Expr,
    helper = helper,
    docs: Expr = docs,
  );
  let statement: Stmt = quote!(
    "$component.__docgenInfo = $docs;" as Stmt,
    component: Expr = target.component.clone(),
    docs: Expr = converted,
  );
  module.body.push(statement.into());
}

fn docgen_export_expr(export_name: &str) -> Expr {
  let exports = Expr::Member(MemberExpr {
    span: DUMMY_SP,
    obj: Box::new(Expr::Ident(ident("__docgenInfo"))),
    prop: MemberProp::Ident(IdentName::new("exports".into(), DUMMY_SP)),
  });
  Expr::Member(MemberExpr {
    span: DUMMY_SP,
    obj: Box::new(exports),
    prop: MemberProp::Ident(IdentName::new(export_name.into(), DUMMY_SP)),
  })
}

fn transform_refresh_stories(
  module: &mut Module,
  stories: &[Story],
  code: &str,
  source_start: swc_core::common::BytePos,
) -> usize {
  let mut additions = Vec::new();
  let mut count = 0;
  let mut transformed_targets: HashMap<String, String> = HashMap::new();
  for story in stories {
    let mut hash_input = String::new();
    if let Some(span) = story.args_span {
      hash_input.push_str(slice_span(code, source_start, span));
    }
    if let Some(span) = story.parameters_span {
      hash_input.push_str(slice_span(code, source_start, span));
    }
    // CsfFile creates an annotations object for every story, so the Babel
    // implementation also emits the MD5 of an empty string when a story has
    // no args or parameters.
    let story_hash = md5_hex(hash_input.as_bytes());

    // Several stories can share one template, which is extracted only once.
    let component = match transformed_targets.get(&story.target_name) {
      Some(component) => Some(component.clone()),
      None => {
        let component = transform_story_target(module, &story.target_name, count);
        if let Some(component) = &component {
          transformed_targets.insert(story.target_name.clone(), component.clone());
          count += 1;
        }
        component
      }
    };
    if let Some(component) = component {
      let story_ident = ident(&story.export_name);
      let component_ident = ident(&component);
      let internal_component: Stmt = quote!(
        "$story._internalComponent = $component;" as Stmt,
        story = story_ident.clone(),
        component = component_ident,
      );
      additions.push(internal_component.into());
      let story_hash = string_expr(story_hash);
      let hash: Stmt = quote!(
        "$story._hash = $hash;" as Stmt,
        story = story_ident,
        hash: Expr = story_hash,
      );
      additions.push(hash.into());
    } else if let Some(object) = find_story_object_mut(module, &story.target_name) {
      handle_render_property(object, &mut additions, &mut count);
      push_object_property(object, "_hash", string_expr(story_hash));
    }
  }
  module.body.extend(additions);
  count
}

fn transform_story_target(module: &mut Module, name: &str, count: usize) -> Option<String> {
  let component_name = format!("Story{count}");
  let component = if let Some(declaration) = find_function_mut(module, name) {
    extract_component_from_function(&mut declaration.function, &component_name)
  } else {
    extract_component(
      unwrap_ts_expr_mut(find_var_init_mut(module, name)?),
      &component_name,
    )?
  };
  module.body.push(component);
  Some(component_name)
}

/// Moves the body of a story function or arrow into a standalone component
/// declaration, leaving behind a wrapper that renders it. React Refresh can
/// then track the component across edits.
fn extract_component(expression: &mut Expr, component_name: &str) -> Option<ModuleItem> {
  match expression {
    Expr::Arrow(arrow) => {
      let params = arrow
        .params
        .iter()
        .cloned()
        .map(Param::from)
        .collect::<Vec<_>>();
      let body = match &*arrow.body {
        BlockStmtOrExpr::BlockStmt(block) => block.clone(),
        BlockStmtOrExpr::Expr(expression) => return_block(expression.clone()),
      };
      *arrow.body = BlockStmtOrExpr::BlockStmt(wrapper_body(component_name, &params));
      Some(extracted_function(component_name, &params, Some(&body)))
    }
    Expr::Fn(function) => Some(extract_component_from_function(
      &mut function.function,
      component_name,
    )),
    _ => None,
  }
}

fn extract_component_from_function(function: &mut Function, component_name: &str) -> ModuleItem {
  let params = function.params.clone();
  let body = function.body.replace(wrapper_body(component_name, &params));
  extracted_function(component_name, &params, body.as_ref())
}

fn extracted_function(name: &str, params: &[Param], body: Option<&BlockStmt>) -> ModuleItem {
  let function_name = ident(name);
  let mut statement: Stmt = quote!("function $name() {}" as Stmt, name = function_name,);
  let Stmt::Decl(Decl::Fn(function)) = &mut statement else {
    unreachable!();
  };
  function.function.params = params.to_vec();
  function.function.body = body.cloned();
  statement.into()
}

fn wrapper_body(name: &str, params: &[Param]) -> BlockStmt {
  let spread = params
    .first()
    .and_then(|param| binding_name(&param.pat))
    .map(ident);
  return_block(jsx_element(name, spread))
}

fn return_block(expression: Box<Expr>) -> BlockStmt {
  BlockStmt {
    span: DUMMY_SP,
    ctxt: SyntaxContext::empty(),
    stmts: vec![Stmt::Return(ReturnStmt {
      span: DUMMY_SP,
      arg: Some(expression),
    })],
  }
}

fn handle_render_property(
  object: &mut ObjectLit,
  additions: &mut Vec<ModuleItem>,
  component_count: &mut usize,
) {
  let Some(index) = object
    .props
    .iter()
    .position(|property| prop_key_name(property) == Some("render"))
  else {
    return;
  };

  let component_name = format!("Story{component_count}");
  let component = match &mut object.props[index] {
    PropOrSpread::Prop(property) => match &mut **property {
      Prop::KeyValue(property) => match component_reference(&property.value) {
        // A render that just points at a component doesn't need extracting;
        // wrapping it keeps args flowing through to the component itself.
        Some(name) => {
          let args = ident("args");
          *property.value = Expr::Arrow(ArrowExpr {
            params: vec![Pat::Ident(args.clone().into())],
            body: Box::new(BlockStmtOrExpr::Expr(jsx_element(&name, Some(args)))),
            ..Default::default()
          });
          None
        }
        None => extract_component(unwrap_ts_expr_mut(&mut property.value), &component_name),
      },
      Prop::Method(method) => Some(extract_component_from_function(
        &mut method.function,
        &component_name,
      )),
      _ => None,
    },
    PropOrSpread::Spread(_) => None,
  };

  if let Some(component) = component {
    additions.push(component);
    push_object_property(
      object,
      "_internalComponent",
      Expr::Ident(ident(&component_name)),
    );
    *component_count += 1;
  }
}

fn wrap_refresh(module: &mut Module, refresh_name: &str) {
  let old_body = std::mem::take(&mut module.body);
  let mut output = Vec::new();
  let mut statements = Vec::new();
  let mut export_vars = Vec::new();
  let mut exports = Vec::new();

  for item in old_body {
    match item {
      ModuleItem::ModuleDecl(ModuleDecl::Import(_))
      | ModuleItem::ModuleDecl(ModuleDecl::ExportAll(_)) => output.push(item),
      ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export))
        if export.type_only || export.src.is_some() =>
      {
        output.push(ModuleDecl::ExportNamed(export).into());
      }
      ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
        let names = outer_binding_names(&export.decl);
        statements.push(Stmt::Decl(export.decl));
        for exported in names {
          add_refresh_export(
            refresh_name,
            Expr::Ident(ident(&exported)),
            &exported,
            &mut export_vars,
            &mut exports,
            &mut statements,
          );
        }
      }
      ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) => {
        for specifier in export.specifiers {
          let ExportSpecifier::Named(specifier) = specifier else {
            continue;
          };
          if specifier.is_type_only {
            continue;
          }
          let ModuleExportName::Ident(local) = specifier.orig else {
            continue;
          };
          let exported = specifier
            .exported
            .unwrap_or_else(|| ModuleExportName::Ident(local.clone()));
          let exported = exported.atom().to_string();
          add_refresh_export(
            refresh_name,
            Expr::Ident(local),
            &exported,
            &mut export_vars,
            &mut exports,
            &mut statements,
          );
        }
      }
      ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
        add_refresh_export(
          refresh_name,
          *export.expr,
          "default",
          &mut export_vars,
          &mut exports,
          &mut statements,
        );
      }
      ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
        // A named default declaration stays a declaration so the rest of the
        // module can refer to it; an anonymous one becomes a plain expression.
        let value = match export.decl {
          DefaultDecl::Fn(function) => match function.ident.clone() {
            Some(identifier) => {
              statements.push(Stmt::Decl(Decl::Fn(FnDecl {
                ident: identifier.clone(),
                declare: false,
                function: function.function,
              })));
              Expr::Ident(identifier)
            }
            None => Expr::Fn(function),
          },
          DefaultDecl::Class(class) => match class.ident.clone() {
            Some(identifier) => {
              statements.push(Stmt::Decl(Decl::Class(ClassDecl {
                ident: identifier.clone(),
                declare: false,
                class: class.class,
              })));
              Expr::Ident(identifier)
            }
            None => Expr::Class(class),
          },
          DefaultDecl::TsInterfaceDecl(interface) => {
            statements.push(Stmt::Decl(Decl::TsInterface(interface)));
            continue;
          }
        };
        add_refresh_export(
          refresh_name,
          value,
          "default",
          &mut export_vars,
          &mut exports,
          &mut statements,
        );
      }
      ModuleItem::Stmt(statement) => statements.push(statement),
      // TypeScript-only declarations (`import x = require(...)`, `export =`)
      // hold no runtime value to track, so they stay at the module level.
      ModuleItem::ModuleDecl(declaration) => output.push(declaration.into()),
    }
  }

  let helpers = ident(format!("{refresh_name}$Helpers"));
  let mut helper_import: ModuleItem = quote!(
    "import * as $helpers from \"\";" as ModuleItem,
    helpers = helpers.clone(),
  );
  let ModuleItem::ModuleDecl(ModuleDecl::Import(import_decl)) = &mut helper_import else {
    unreachable!();
  };
  *import_decl.src = string_literal(HMR_HELPER);
  output.push(helper_import);

  if !export_vars.is_empty() {
    output.push(
      VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: export_vars
          .iter()
          .map(|name| VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(ident(name).into()),
            init: None,
            definite: false,
          })
          .collect(),
      }
      .into(),
    );
  }

  let prev_reg = ident(format!("{refresh_name}$PrevRefreshReg"));
  let prev_sig = ident(format!("{refresh_name}$PrevRefreshSig"));
  let window = ident("window");
  let refresh_reg = string_expr("$RefreshReg$");
  let refresh_sig = string_expr("$RefreshSig$");
  let previous_refresh: Stmt = quote!(
    "var $prev_reg = $window[$refresh_reg], $prev_sig = $window[$refresh_sig];" as Stmt,
    prev_reg = prev_reg.clone(),
    window = window.clone(),
    refresh_reg: Expr = refresh_reg.clone(),
    prev_sig = prev_sig.clone(),
    refresh_sig: Expr = refresh_sig.clone(),
  );
  output.push(previous_refresh.into());
  let prelude: Stmt = quote!(
    "$helpers.prelude(module);" as Stmt,
    helpers = helpers.clone(),
  );
  output.push(prelude.into());

  let mut try_statement: Stmt = quote!(
    "try {$helpers.postlude(module);} finally {$window[$refresh_reg] = $prev_reg; $window[$refresh_sig] = $prev_sig;}" as Stmt,
    helpers = helpers,
    window = window,
    refresh_reg: Expr = refresh_reg,
    prev_reg = prev_reg,
    refresh_sig: Expr = refresh_sig,
    prev_sig = prev_sig,
  );
  let Stmt::Try(try_statement) = &mut try_statement else {
    unreachable!();
  };
  let postlude = try_statement.block.stmts.pop().unwrap();
  statements.push(postlude);
  try_statement.block.stmts = statements;
  output.push(Stmt::Try(try_statement.clone()).into());

  if !exports.is_empty() {
    output.push(
      NamedExport {
        span: DUMMY_SP,
        specifiers: exports
          .into_iter()
          .map(|(local, exported)| {
            ExportSpecifier::Named(ExportNamedSpecifier {
              span: DUMMY_SP,
              orig: ModuleExportName::Ident(ident(local)),
              exported: Some(ModuleExportName::Ident(ident(exported))),
              is_type_only: false,
            })
          })
          .collect(),
        src: None,
        type_only: false,
        with: None,
      }
      .into(),
    );
  }

  module.body = output;
}

/// Assigns an exported value to a module-level variable that the export list
/// points at, so React Refresh can swap the value on reload.
fn add_refresh_export(
  refresh_name: &str,
  value: Expr,
  exported: &str,
  export_vars: &mut Vec<String>,
  exports: &mut Vec<(String, String)>,
  statements: &mut Vec<Stmt>,
) {
  let temp = format!("{refresh_name}$Export{}", export_vars.len());
  export_vars.push(temp.clone());
  exports.push((temp.clone(), exported.to_owned()));
  statements.push(assignment_statement(ident(temp), value));
}

fn outer_binding_names(declaration: &Decl) -> Vec<String> {
  match declaration {
    Decl::Fn(function) => vec![function.ident.sym.to_string()],
    Decl::Class(class) => vec![class.ident.sym.to_string()],
    Decl::Var(var) => var
      .decls
      .iter()
      .flat_map(|declaration| binding_names(&declaration.name))
      .collect(),
    _ => Vec::new(),
  }
}

fn binding_names(pattern: &Pat) -> Vec<String> {
  match pattern {
    Pat::Ident(identifier) => vec![identifier.sym.to_string()],
    Pat::Array(array) => array
      .elems
      .iter()
      .flatten()
      .flat_map(binding_names)
      .collect(),
    Pat::Object(object) => object
      .props
      .iter()
      .flat_map(|property| match property {
        ObjectPatProp::KeyValue(property) => binding_names(&property.value),
        ObjectPatProp::Assign(property) => vec![property.key.sym.to_string()],
        ObjectPatProp::Rest(property) => binding_names(&property.arg),
      })
      .collect(),
    Pat::Assign(assign) => binding_names(&assign.left),
    Pat::Rest(rest) => binding_names(&rest.arg),
    _ => Vec::new(),
  }
}

fn binding_source_span(module: &Module, name: &str) -> Option<Span> {
  if let Some(function) = find_function(module, name) {
    return Some(function.span());
  }
  find_var_init(module, name).map(|expression| unwrap_ts_expr(expression).span())
}

fn resolve_bound_story(module: &Module, name: &str) -> Option<String> {
  let expression = unwrap_ts_expr(find_var_init(module, name)?);
  let Expr::Call(call) = expression else {
    return None;
  };
  let Callee::Expr(callee) = &call.callee else {
    return None;
  };
  let Expr::Member(member) = &**callee else {
    return None;
  };
  if !member.prop.is_ident_with("bind") || call.args.len() > 1 {
    return None;
  }
  if let Some(argument) = call.args.first()
    && !matches!(&*argument.expr, Expr::Object(object) if object.props.is_empty())
  {
    return None;
  }
  let Expr::Ident(template) = &*member.obj else {
    return None;
  };
  Some(template.sym.to_string())
}

fn find_function<'a>(module: &'a Module, name: &str) -> Option<&'a FnDecl> {
  module.body.iter().find_map(|item| match item {
    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
      decl: Decl::Fn(function),
      ..
    }))
    | ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function)))
      if function.ident.sym == name =>
    {
      Some(function)
    }
    _ => None,
  })
}

fn find_function_mut<'a>(module: &'a mut Module, name: &str) -> Option<&'a mut FnDecl> {
  module.body.iter_mut().find_map(|item| match item {
    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
      decl: Decl::Fn(function),
      ..
    }))
    | ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function)))
      if function.ident.sym == name =>
    {
      Some(function)
    }
    _ => None,
  })
}

fn find_var_init<'a>(module: &'a Module, name: &str) -> Option<&'a Expr> {
  module
    .body
    .iter()
    .filter_map(|item| match item {
      ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        decl: Decl::Var(var),
        ..
      }))
      | ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => Some(var),
      _ => None,
    })
    .flat_map(|var| &var.decls)
    .find(|declarator| binding_name(&declarator.name) == Some(name))?
    .init
    .as_deref()
}

fn find_var_init_mut<'a>(module: &'a mut Module, name: &str) -> Option<&'a mut Expr> {
  module
    .body
    .iter_mut()
    .filter_map(|item| match item {
      ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        decl: Decl::Var(var),
        ..
      }))
      | ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => Some(var),
      _ => None,
    })
    .flat_map(|var| &mut var.decls)
    .find(|declarator| binding_name(&declarator.name) == Some(name))?
    .init
    .as_deref_mut()
}

fn find_story_object_mut<'a>(module: &'a mut Module, name: &str) -> Option<&'a mut ObjectLit> {
  match unwrap_ts_expr_mut(find_var_init_mut(module, name)?) {
    Expr::Object(object) => Some(object),
    _ => None,
  }
}

fn normalize_meta(module: &mut Module, file_path: &Path) -> Result<Meta, Diagnostic> {
  let Some(index) = module.body.iter().position(|item| {
    matches!(
      item,
      ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(_))
    )
  }) else {
    return Err(Diagnostic::new(format!(
      "CSF: missing default export in {}",
      file_path.display()
    )));
  };

  let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = &module.body[index] else {
    unreachable!();
  };
  let statement_span = export.span;
  if let Expr::Ident(identifier) = unwrap_ts_expr(&export.expr) {
    let name = identifier.sym.to_string();
    let expression = find_var_init(module, &name);
    let Some(Expr::Object(object)) = expression.map(unwrap_ts_expr) else {
      return Err(Diagnostic::new(format!(
        "CSF: default export must be an object in {}",
        file_path.display()
      )));
    };
    return Ok(Meta {
      statement_span: find_var_statement_span(module, &name).unwrap_or(statement_span),
      object_span: object.span,
      name,
    });
  }

  let Expr::Object(object) = unwrap_ts_expr(&export.expr) else {
    return Err(Diagnostic::new(format!(
      "CSF: default export must be an object in {}",
      file_path.display()
    )));
  };
  let object_span = object.span;
  let expression = export.expr.clone();
  let mut name = "$parcel$storybook$meta".to_owned();
  let mut suffix = 0;
  while find_var_init(module, &name).is_some() || find_function(module, &name).is_some() {
    suffix += 1;
    name = format!("$parcel$storybook$meta{suffix}");
  }
  let identifier = ident(&name);
  let declaration: ModuleItem = quote!(
    "const $name = $expression;" as ModuleItem,
    name = identifier.clone(),
    expression: Expr = *expression,
  );
  let default_export: ModuleItem =
    quote!("export default $name;" as ModuleItem, name = identifier,);
  module
    .body
    .splice(index..=index, [declaration, default_export]);
  Ok(Meta {
    name,
    statement_span,
    object_span,
  })
}

fn find_var_statement_span(module: &Module, name: &str) -> Option<Span> {
  module.body.iter().find_map(|item| {
    let (span, var) = match item {
      ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => (var.span, var),
      ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
        Decl::Var(var) => (export.span, var),
        _ => return None,
      },
      _ => return None,
    };
    var
      .decls
      .iter()
      .any(|declaration| binding_name(&declaration.name) == Some(name))
      .then_some(span)
  })
}

fn meta_object<'a>(module: &'a Module, meta: &Meta) -> Option<&'a ObjectLit> {
  match unwrap_ts_expr(find_var_init(module, &meta.name)?) {
    Expr::Object(object) => Some(object),
    _ => None,
  }
}

fn meta_object_mut<'a>(module: &'a mut Module, meta: &Meta) -> Option<&'a mut ObjectLit> {
  match unwrap_ts_expr_mut(find_var_init_mut(module, &meta.name)?) {
    Expr::Object(object) => Some(object),
    _ => None,
  }
}

fn binding_name(pattern: &Pat) -> Option<&str> {
  match pattern {
    Pat::Ident(identifier) => Some(&identifier.sym),
    _ => None,
  }
}

fn unwrap_ts_expr(mut expression: &Expr) -> &Expr {
  loop {
    expression = match expression {
      Expr::TsAs(expression) => &expression.expr,
      Expr::TsSatisfies(expression) => &expression.expr,
      Expr::TsTypeAssertion(expression) => &expression.expr,
      Expr::TsConstAssertion(expression) => &expression.expr,
      Expr::TsNonNull(expression) => &expression.expr,
      Expr::TsInstantiation(expression) => &expression.expr,
      Expr::Paren(expression) => &expression.expr,
      _ => return expression,
    };
  }
}

fn unwrap_ts_expr_mut(expression: &mut Expr) -> &mut Expr {
  match expression {
    Expr::TsAs(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    Expr::TsSatisfies(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    Expr::TsTypeAssertion(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    Expr::TsConstAssertion(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    Expr::TsNonNull(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    Expr::TsInstantiation(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    Expr::Paren(expression) => unwrap_ts_expr_mut(&mut expression.expr),
    expression => expression,
  }
}

/// The name of the identifier an expression refers to, ignoring type wrappers.
fn component_reference(expression: &Expr) -> Option<String> {
  match unwrap_ts_expr(expression) {
    Expr::Ident(identifier) => Some(identifier.sym.to_string()),
    _ => None,
  }
}

fn prop_key_name(property: &PropOrSpread) -> Option<&str> {
  let PropOrSpread::Prop(property) = property else {
    return None;
  };
  match &**property {
    Prop::KeyValue(property) => property_name(&property.key),
    Prop::Method(method) => property_name(&method.key),
    _ => None,
  }
}

/// The value of a plain `key: value` property on an object literal.
fn object_property<'a>(object: &'a ObjectLit, name: &str) -> Option<&'a Expr> {
  object.props.iter().find_map(|property| {
    let PropOrSpread::Prop(property) = property else {
      return None;
    };
    let Prop::KeyValue(property) = &**property else {
      return None;
    };
    (property_name(&property.key) == Some(name)).then_some(&*property.value)
  })
}

fn property_name(property: &PropName) -> Option<&str> {
  match property {
    PropName::Ident(identifier) => Some(&identifier.sym),
    PropName::Str(string) => string.value.as_str(),
    _ => None,
  }
}

fn string_array_property(object: &ObjectLit, name: &str) -> Option<Vec<String>> {
  let Expr::Array(array) = unwrap_ts_expr(object_property(object, name)?) else {
    return None;
  };
  Some(
    array
      .elems
      .iter()
      .flatten()
      .filter_map(|element| match unwrap_ts_expr(&element.expr) {
        Expr::Lit(Lit::Str(value)) => value.value.as_str().map(str::to_owned),
        _ => None,
      })
      .collect(),
  )
}

fn extract_description(
  comments: &SingleThreadedComments,
  position: swc_core::common::BytePos,
) -> Option<String> {
  let descriptions = comments
    .get_leading(position)
    .unwrap_or_default()
    .into_iter()
    .filter_map(jsdoc_description)
    .collect::<Vec<_>>();
  (!descriptions.is_empty()).then(|| descriptions.join("\n"))
}

fn jsdoc_description(comment: Comment) -> Option<String> {
  if comment.kind != CommentKind::Block || !comment.text.starts_with('*') {
    return None;
  }
  let description = comment
    .text
    .lines()
    .map(|line| {
      let line = line.trim_start();
      let line = line.trim_start_matches('*');
      line.strip_prefix(' ').unwrap_or(line)
    })
    .collect::<Vec<_>>()
    .join("\n")
    .trim()
    .to_owned();
  (!description.is_empty()).then_some(description)
}

fn push_object_property(object: &mut ObjectLit, name: &str, expression: Expr) {
  object
    .props
    .push(PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
      key: PropName::Ident(IdentName::new(name.into(), DUMMY_SP)),
      value: Box::new(expression),
    }))));
}

fn ident(value: impl AsRef<str>) -> Ident {
  Ident::new(value.as_ref().into(), DUMMY_SP, SyntaxContext::empty())
}

fn string_literal(value: impl Into<String>) -> Str {
  Str {
    span: DUMMY_SP,
    value: value.into().into(),
    raw: None,
  }
}

fn string_expr(value: impl Into<String>) -> Expr {
  Expr::Lit(Lit::Str(string_literal(value)))
}

fn assignment_statement(left: Ident, right: Expr) -> Stmt {
  quote!(
    "$left = $right;" as Stmt,
    left = left,
    right: Expr = right,
  )
}

fn jsx_element(name: &str, spread: Option<Ident>) -> Box<Expr> {
  Box::new(Expr::JSXElement(Box::new(JSXElement {
    span: DUMMY_SP,
    opening: JSXOpeningElement {
      name: JSXElementName::Ident(ident(name)),
      span: DUMMY_SP,
      attrs: spread
        .into_iter()
        .map(|identifier| {
          JSXAttrOrSpread::SpreadElement(SpreadElement {
            dot3_token: DUMMY_SP,
            expr: Box::new(Expr::Ident(identifier)),
          })
        })
        .collect(),
      self_closing: true,
      type_args: None,
    },
    children: Vec::new(),
    closing: None,
  })))
}

fn emit_module(
  module: &Module,
  cm: Lrc<SourceMap>,
  comments: &SingleThreadedComments,
) -> Result<String, Diagnostic> {
  let mut output = Vec::new();
  {
    let writer = JsWriter::new(cm.clone(), "\n", &mut output, None);
    let mut emitter = Emitter {
      cfg: Default::default(),
      cm,
      comments: Some(comments),
      wr: writer,
    };
    module
      .emit_with(&mut emitter)
      .map_err(|error| Diagnostic::new(format!("Unable to print Storybook transform: {error}")))?;
  }
  String::from_utf8(output).map_err(|error| {
    Diagnostic::new(format!(
      "Storybook transform produced invalid UTF-8: {error}"
    ))
  })
}

fn slice_span(source: &str, start: swc_core::common::BytePos, span: Span) -> &str {
  let lo = span.lo().0.saturating_sub(start.0) as usize;
  let hi = span.hi().0.saturating_sub(start.0) as usize;
  source.get(lo..hi).unwrap_or_default()
}

fn md5_hex(value: &[u8]) -> String {
  format!("{:x}", md5::compute(value))
}

#[cfg(test)]
mod tests {
  use super::{md5_hex, process_csf, syntax_for_extension};
  use std::path::Path;
  use swc_core::{
    common::{FileName, SourceMap, sync::Lrc},
    ecma::{
      ast::EsVersion,
      parser::{Parser, StringInput, lexer::Lexer},
    },
  };

  fn assert_parses(source: &str, path: &Path) {
    let cm: Lrc<SourceMap> = Default::default();
    let file = cm.new_source_file(
      Lrc::new(FileName::Real(path.to_path_buf())),
      source.to_owned(),
    );
    let lexer = Lexer::new(
      syntax_for_extension(path.extension().and_then(|extension| extension.to_str())),
      EsVersion::EsNext,
      StringInput::from(&*file),
      None,
    );
    let mut parser = Parser::new_from(lexer);
    if let Err(error) = parser.parse_module() {
      panic!("{}\n\n{source}", error.kind().msg());
    }
    if let Some(error) = parser.take_errors().into_iter().next() {
      panic!("{}\n\n{source}", error.kind().msg());
    }
  }

  #[test]
  fn adds_docgen_and_enriches_csf_without_refresh() {
    let source = r#"
      import {Button} from './Button';
      /** Button docs. */
      export default {component: Button} satisfies Meta<typeof Button>;
      /** Primary docs. */
      export const Primary = {args: {label: 'Save'}};
    "#;
    let result = process_csf(source, Path::new("/src/Button.story.tsx"), None).unwrap();
    assert!(result.contains("import __docgenInfo from \"docs:./Button\""));
    assert!(result.contains("from \"@parcel/transformer-storybook/to-react-docgen.js\""));
    assert!(result.contains("toReactDocgen(__docgenInfo.exports.Button)"));
    assert!(result.contains("originalSource"));
    assert!(result.contains("Primary docs."));
    assert!(result.contains("Button docs."));
  }

  #[test]
  fn extracts_function_and_object_renders_for_refresh() {
    let source = r#"
      import {Button} from './Button';
      export default {component: Button};
      export const Primary = args => <Button {...args} />;
      export const Secondary = {render: args => <Button {...args} />, parameters: {layout: 'centered'}};
    "#;
    let result = process_csf(
      source,
      Path::new("/src/Button.story.jsx"),
      Some("$parcel$ReactRefresh$abcd"),
    )
    .unwrap();
    assert!(result.contains("function Story0"));
    assert!(result.contains("_internalComponent"));
    assert!(result.contains("_hash"));
    assert!(result.contains("from \"@parcel/transformer-storybook/csf-hmr.js\""));
    assert!(result.contains("$parcel$ReactRefresh$abcd$Helpers.prelude(module)"));
    assert!(result.contains("export { $parcel$ReactRefresh$abcd$Export0 as default"));
  }

  #[test]
  fn wraps_identifier_render_without_extracting_it() {
    let source = r#"
      import {Button} from './Button';
      export default {component: Button};
      export const Primary = {render: Button};
    "#;
    let result = process_csf(
      source,
      Path::new("/src/Button.story.jsx"),
      Some("$parcel$ReactRefresh$abcd"),
    )
    .unwrap();
    assert!(result.contains("render: (args)=><Button {...args}/>"));
  }

  #[test]
  fn supports_static_csf2_annotations_and_narrow_bind() {
    let source = r#"
      import {Button} from './Button';
      const meta = {component: Button};
      export default meta;
      const Template = args => <Button {...args} />;
      export const Primary = Template.bind({});
      Primary.args = {label: 'Save'};
      Primary.parameters = {layout: 'centered'};
      Primary.story = {args: {ignored: true}};
      export const Secondary = Template.bind();
    "#;
    let result = process_csf(
      source,
      Path::new("/src/Button.story.jsx"),
      Some("$parcel$ReactRefresh$abcd"),
    )
    .unwrap();
    let hash = md5_hex("{label: 'Save'}{layout: 'centered'}".as_bytes());

    assert!(result.contains("Primary._internalComponent = Story0"));
    assert!(result.contains("Secondary._internalComponent = Story0"));
    assert!(result.contains(&format!("Primary._hash = \"{hash}\"")));
    assert!(result.contains("Secondary._hash = \"d41d8cd98f00b204e9800998ecf8427e\""));
    assert_eq!(result.matches("function Story0").count(), 1);
    assert!(!result.contains("function Story1"));
  }

  #[test]
  fn transforms_typed_csf3_with_import_attributes() {
    let source = r#"
      import AlertIcon from './Alert.svg';
      import {FunctionComponent} from 'react';
      import {IconProps} from './Icon';
      import {iconStyle} from './style' with {type: 'macro'};
      import type {Meta, StoryObj} from '@storybook/react';
      import NewIcon from './New.svg';

      const Alert = AlertIcon as FunctionComponent<IconProps>;
      const meta: Meta<FunctionComponent<IconProps>> = {
        component: NewIcon as FunctionComponent<IconProps>,
        parameters: {layout: 'centered'},
        title: 'Icon'
      };
      export default meta;

      type Story = StoryObj<typeof NewIcon>;
      export const Example: Story = {};
      export const ColorAndSize: Story = {
        render: args => <Alert {...args} styles={iconStyle({color: 'negative', size: 'XL'})} />
      };
    "#;
    let result = process_csf(
      source,
      Path::new("/src/Icon.stories.tsx"),
      Some("$parcel$ReactRefresh$abcd"),
    )
    .unwrap();
    assert!(result.contains("import __docgenInfo from \"docs:./New.svg\""));
    assert!(result.contains("toReactDocgen(__docgenInfo.exports.default)"));
    assert_parses(&result, Path::new("/src/Icon.stories.tsx"));
  }

  #[test]
  fn resolves_docgen_from_the_original_named_export() {
    let source = r#"
      import {Button as StoryButton} from './Button';
      export default {component: StoryButton};
      export const Primary = {};
    "#;
    let result = process_csf(source, Path::new("/src/Button.stories.tsx"), None).unwrap();

    assert!(result.contains("import __docgenInfo from \"docs:./Button\""));
    assert!(result.contains("toReactDocgen(__docgenInfo.exports.Button)"));
  }

  #[test]
  fn omits_docgen_for_a_locally_declared_component() {
    let source = r#"
      const Button = () => <button />;
      export default {component: Button};
      export const Primary = {};
    "#;
    let result = process_csf(
      source,
      Path::new("/src/Button.stories.tsx"),
      Some("$parcel$ReactRefresh$abcd"),
    )
    .unwrap();

    assert!(!result.contains("__docgenInfo"));
    assert_parses(&result, Path::new("/src/Button.stories.tsx"));
  }
}
