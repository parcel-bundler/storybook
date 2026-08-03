//! Transformation-phase tests.
//!
//! These are ported from the JS implementation's `DocsTransformer` tests
//! (`packages/dev/parcel-transformer-docs/__tests__`). The original tests run
//! the full Parcel pipeline (transform + package) and snapshot the final
//! `{exports, links}` artifact. The packaging phase is not yet ported to Rust,
//! so these snapshot the raw per-module `API` produced by transformation. When
//! packaging lands, matching end-to-end tests can assert the merged output.

use std::path::Path;

use insta::assert_json_snapshot;
use ts_doc::{API, parse};

/// Transform a single source file. `name` is used to build a stable, machine
/// independent module path (e.g. `index` -> `/test/src/index.tsx`) so snapshots
/// don't depend on the local filesystem.
fn transform(name: &str, code: &str) -> API {
  let path = format!("/test/src/{name}.tsx");
  parse(Path::new(&path), code.to_string())
}

mod builtins {
  use super::*;

  #[test]
  fn static_number() {
    assert_json_snapshot!(transform("index", "export let a: number = 4;"));
  }

  #[test]
  fn static_string() {
    assert_json_snapshot!(transform("index", r#"export let b: string = "foo";"#));
  }

  #[test]
  fn referenced_string() {
    assert_json_snapshot!(transform(
      "index",
      "let name = 'foo';\nexport let c = name;"
    ));
  }

  #[test]
  fn referenced_function() {
    assert_json_snapshot!(transform(
      "index",
      "function foo() {\n  return 'foo';\n}\nexport let d = foo();"
    ));
  }
}

mod components {
  use super::*;

  #[test]
  fn react_component() {
    assert_json_snapshot!(transform(
      "index",
      "import React from 'react';\n\nexport function App1(props) {\n  return <div />;\n}"
    ));
  }

  #[test]
  fn local_name_react_component() {
    assert_json_snapshot!(transform(
      "index",
      "import React from 'react';\n\nfunction App2(props) {\n  return <div />;\n}\nexport {App2 as AppReal};"
    ));
  }
}

mod types {
  use super::*;

  #[test]
  fn type_alias() {
    assert_json_snapshot!(transform("index", "export type Foo = number;"));
  }

  #[test]
  fn type_union() {
    assert_json_snapshot!(transform("index", "export type Foo = number | string;"));
  }

  #[test]
  fn type_template() {
    assert_json_snapshot!(transform("index", "export type Foo = `${number}%`;"));
  }

  #[test]
  fn complex_type_template() {
    assert_json_snapshot!(transform(
      "index",
      "export type Foo = `${number}.${number} ${string}`;"
    ));
  }
}

mod interfaces {
  use super::*;

  #[test]
  fn interface() {
    assert_json_snapshot!(transform(
      "index",
      "export interface Foo {\n  a: number\n};"
    ));
  }

  #[test]
  fn follows_imported_interfaces() {
    // The original test spans two modules. Transformation processes each
    // module independently; linking happens during packaging.
    assert_json_snapshot!("follows_imported_interfaces__component", transform(
      "component",
      "export interface Foo {\n  a: number\n};"
    ));
    assert_json_snapshot!("follows_imported_interfaces__index", transform(
      "index",
      "import {Foo} from './component';\nexport function Bar(props: Foo) {\n  return null;\n}"
    ));
  }
}

/// Regression tests for parity fixes against the JS `DocsTransformer`.
mod parity {
  use super::*;

  #[test]
  fn class_id_name_extends_and_members() {
    assert_json_snapshot!(transform(
      "index",
      "export class Foo extends Bar<T> {\n  x: number;\n  static y = 1;\n  private z() {}\n}"
    ));
  }

  #[test]
  fn jsdoc_on_exported_declaration() {
    assert_json_snapshot!(transform(
      "index",
      "/** My docs */\nexport function foo(a) { return a; }"
    ));
  }

  #[test]
  fn constructor_type() {
    assert_json_snapshot!(transform("index", "export type F = new () => string;"));
  }

  #[test]
  fn object_expression() {
    assert_json_snapshot!(transform("index", "export const config = {a: 1, b: 'x'};"));
  }

  #[test]
  fn param_descriptions_by_name() {
    assert_json_snapshot!(transform(
      "index",
      "/**\n * Does a thing.\n * @param first The first argument.\n * @param second The second argument.\n */\nexport function foo(first, second) { return first; }"
    ));
  }

  #[test]
  fn examples() {
    assert_json_snapshot!(transform(
      "index",
      "/**\n * Docs.\n * @example\n * foo();\n */\nexport function Comp(props): JSX.Element {\n  return null;\n}"
    ));
  }

  #[test]
  fn forward_ref_component_naming() {
    assert_json_snapshot!(transform(
      "index",
      "import {forwardRef} from 'react';\nexport const Button = forwardRef((props: {x: number}, ref) => <div />);"
    ));
  }

  #[test]
  fn destructured_export() {
    assert_json_snapshot!(transform("index", "export const {a, b} = obj;"));
  }

  #[test]
  fn private_interface_propagates_access() {
    assert_json_snapshot!(transform(
      "index",
      "/** @private */\nexport interface Secret {\n  a: number;\n  b: string;\n}"
    ));
  }
}

mod identifiers {
  use super::*;

  #[test]
  fn identifiers() {
    assert_json_snapshot!("identifiers__column", transform(
      "column",
      "export interface SpectrumColumnProps<T> {id: string};\nexport let Column = (props: {id: string}) => null;"
    ));
    assert_json_snapshot!("identifiers__index", transform(
      "index",
      "import {Column, SpectrumColumnProps} from './column';\nconst SpectrumColumn = Column as <T>(props: SpectrumColumnProps<T>) => React.JSX.Element;\nexport {SpectrumColumn as Column};"
    ));
  }
}
