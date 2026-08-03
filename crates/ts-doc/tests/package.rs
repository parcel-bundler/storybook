//! End-to-end packaging tests.
//!
//! These port the cases from the JS `DocsTransformer.parceltest.tsx`, whose
//! snapshots capture the *packaged* `{exports, links}` output (transform +
//! package). They are the parity oracle for the Rust packager.

use insta::assert_json_snapshot;
use ts_doc::{PackageOutput, mock::MockBundleGraph, package};

/// Package a single-module bundle whose entry is `/test/src/index.tsx`.
fn package_one(code: &str) -> PackageOutput {
  let graph = MockBundleGraph::from_sources(&[("/test/src/index.tsx", code)], "/test/src/index.tsx");
  package(&graph)
}

/// Package a multi-module bundle; the first source is the entry.
fn package_many(sources: &[(&str, &str)]) -> PackageOutput {
  let entry = sources[0].0;
  let graph = MockBundleGraph::from_sources(sources, entry);
  package(&graph)
}

mod builtins {
  use super::*;

  #[test]
  fn static_number() {
    assert_json_snapshot!(package_one("export let a: number = 4;"));
  }

  #[test]
  fn static_string() {
    assert_json_snapshot!(package_one(r#"export let b: string = "foo";"#));
  }

  #[test]
  fn referenced_string() {
    assert_json_snapshot!(package_one("let name = 'foo';\nexport let c = name;"));
  }

  #[test]
  fn referenced_function() {
    assert_json_snapshot!(package_one(
      "function foo() {\n  return 'foo';\n}\nexport let d = foo();"
    ));
  }
}

mod components {
  use super::*;

  #[test]
  fn react_component() {
    assert_json_snapshot!(package_one(
      "import React from 'react';\n\nexport function App1(props) {\n  return <div />;\n}"
    ));
  }

  #[test]
  fn local_name_react_component() {
    assert_json_snapshot!(package_one(
      "import React from 'react';\n\nfunction App2(props) {\n  return <div />;\n}\nexport {App2 as AppReal};"
    ));
  }
}

mod types {
  use super::*;

  #[test]
  fn type_alias() {
    assert_json_snapshot!(package_one("export type Foo = number;"));
  }

  #[test]
  fn type_union() {
    assert_json_snapshot!(package_one("export type Foo = number | string;"));
  }

  #[test]
  fn type_template() {
    assert_json_snapshot!(package_one("export type Foo = `${number}%`;"));
  }

  #[test]
  fn complex_type_template() {
    assert_json_snapshot!(package_one(
      "export type Foo = `${number}.${number} ${string}`;"
    ));
  }
}

mod interfaces {
  use super::*;

  #[test]
  fn interface() {
    assert_json_snapshot!(package_one("export interface Foo {\n  a: number\n};"));
  }

  #[test]
  fn follows_imported_interfaces() {
    assert_json_snapshot!(package_many(&[
      (
        "/test/src/index.tsx",
        "import {Foo} from './component';\nexport function Bar(props: Foo) {\n  return null;\n}"
      ),
      ("/test/src/component.tsx", "export interface Foo {\n  a: number\n};"),
    ]));
  }
}

/// Exercises the merging / generics / special-form paths of the packager.
mod advanced {
  use super::*;

  #[test]
  fn interface_extends_merges_properties() {
    assert_json_snapshot!(package_one(
      "interface Base { a: number; }\nexport interface Foo extends Base { b: string; }"
    ));
  }

  #[test]
  fn generic_interface_application() {
    assert_json_snapshot!(package_one(
      "interface Base<T> { value: T; }\nexport interface Foo extends Base<string> { b: number; }"
    ));
  }

  #[test]
  fn omit() {
    assert_json_snapshot!(package_one(
      "interface Full { a: number; b: string; c: boolean; }\nexport type Sub = Omit<Full, 'a' | 'b'>;"
    ));
  }

  #[test]
  fn pick() {
    assert_json_snapshot!(package_one(
      "interface Full { a: number; b: string; c: boolean; }\nexport type Sub = Pick<Full, 'a'>;"
    ));
  }

  #[test]
  fn return_description_on_primitive() {
    assert_json_snapshot!(package_one(
      "/**\n * Does a thing.\n * @returns whether it worked\n */\nexport function f(): boolean {\n  return true;\n}"
    ));
  }

  #[test]
  fn typed_component_props() {
    assert_json_snapshot!(package_one(
      "export function C(props: {x: number}): JSX.Element {\n  return null;\n}"
    ));
  }

  #[test]
  fn circular_interface() {
    assert_json_snapshot!(package_one(
      "export interface Tree {\n  value: number;\n  child: Tree;\n}"
    ));
  }
}

mod identifiers {
  use super::*;

  #[test]
  fn identifiers() {
    assert_json_snapshot!(package_many(&[
      (
        "/test/src/index.tsx",
        "import {Column, SpectrumColumnProps} from './column';\nconst SpectrumColumn = Column as <T>(props: SpectrumColumnProps<T>) => React.JSX.Element;\nexport {SpectrumColumn as Column};"
      ),
      (
        "/test/src/column.tsx",
        "export interface SpectrumColumnProps<T> {id: string};\nexport let Column = (props: {id: string}) => null;"
      ),
    ]));
  }
}
