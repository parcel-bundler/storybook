use indexmap::IndexMap;
use swc_core::common::Span;
use swc_core::common::comments::CommentKind;
use swc_core::ecma::atoms::Atom as JsWord;

use crate::parse::Context;

#[derive(Default, Clone)]
pub struct JsDocs {
  pub description: Option<JsWord>,
  pub default: Option<JsWord>,
  pub access: Option<JsWord>,
  pub return_description: Option<JsWord>,
  /// Parameter descriptions keyed by parameter name (matches JSDoc `@param`).
  pub params: IndexMap<JsWord, JsWord>,
  pub selector: Option<JsWord>,
  pub examples: Vec<JsWord>,
}

impl JsDocs {
  /// Whether this doc block carried any content (i.e. there was a JSDoc comment).
  pub fn is_empty(&self) -> bool {
    self.description.is_none()
      && self.default.is_none()
      && self.access.is_none()
      && self.return_description.is_none()
      && self.params.is_empty()
      && self.selector.is_none()
      && self.examples.is_empty()
  }
}

pub fn parse_jsdoc(span: Span, ctx: &Context) -> JsDocs {
  let mut docs = JsDocs::default();
  ctx.comments.with_leading(span.lo, |comments| {
    for c in comments {
      // JSDoc comments are block comments beginning with an extra `*` (`/** */`).
      if c.kind == CommentKind::Block && c.text.starts_with('*') {
        parse_comment(&c.text, &mut docs);
      }
    }
  });
  docs
}

/// Parses a single JSDoc block. The description is everything up to the first
/// line-leading block tag (`@x`); each tag then consumes its own line plus any
/// following continuation lines. This mirrors doctrine (used by the JS
/// implementation) rather than relying on a stricter parser.
fn parse_comment(text: &str, docs: &mut JsDocs) {
  let lines: Vec<&str> = text.lines().map(strip_comment_prefix).collect();

  let mut i = 0;
  let mut desc_lines = Vec::new();
  while i < lines.len() && !is_tag_line(lines[i]) {
    desc_lines.push(lines[i].trim_end());
    i += 1;
  }
  let description = desc_lines.join("\n");
  let description = description.trim();
  if !description.is_empty() {
    docs.description = Some(description.into());
  }

  while i < lines.len() {
    let mut block = vec![lines[i].trim_end()];
    i += 1;
    while i < lines.len() && !is_tag_line(lines[i]) {
      block.push(lines[i].trim_end());
      i += 1;
    }
    parse_tag(&block.join("\n"), docs);
  }
}

fn is_tag_line(line: &str) -> bool {
  line.trim_start().starts_with('@')
}

fn parse_tag(block: &str, docs: &mut JsDocs) {
  let block = block.trim_start();
  let (tag, rest) = split_first_token(block);
  let rest = rest.trim_start_matches([' ', '\t']);
  match tag {
    "@param" | "@arg" | "@argument" => {
      if let Some((name, desc)) = parse_param(rest) {
        let desc = strip_separator(desc);
        if !desc.is_empty() {
          docs.params.insert(name.into(), desc.into());
        }
      }
    }
    "@returns" | "@return" => {
      let desc = strip_separator(skip_type(rest));
      if !desc.is_empty() {
        docs.return_description = Some(desc.into());
      }
    }
    "@default" => {
      let value = rest.trim();
      if !value.is_empty() {
        docs.default = Some(value.into());
      }
    }
    "@access" => {
      let (value, _) = split_first_token(rest);
      if !value.is_empty() {
        docs.access = Some(value.into());
      }
    }
    "@private" | "@deprecated" => docs.access = Some("private".into()),
    "@public" => docs.access = Some("public".into()),
    "@protected" => docs.access = Some("protected".into()),
    "@selector" => {
      let value = rest.trim();
      if !value.is_empty() {
        docs.selector = Some(value.into());
      }
    }
    "@example" => {
      let example = normalize_example(rest);
      if !example.is_empty() && !docs.examples.contains(&example) {
        docs.examples.push(example);
      }
    }
    _ => {}
  }
}

/// Trims a description and drops a leading `-` separator (`@param x - desc`).
fn strip_separator(desc: &str) -> &str {
  let desc = desc.trim();
  match desc.strip_prefix('-') {
    Some(rest) => rest.trim_start_matches([' ', '\t']),
    None => desc,
  }
}

/// Splits off the first whitespace-delimited token, returning `(token, rest)`.
fn split_first_token(s: &str) -> (&str, &str) {
  let s = s.trim_start();
  match s.find(char::is_whitespace) {
    Some(idx) => (&s[..idx], &s[idx + 1..]),
    None => (s, ""),
  }
}

/// Skips a leading `{type}` annotation, returning the remainder.
fn skip_type(s: &str) -> &str {
  let s = s.trim_start();
  if s.starts_with('{') {
    if let Some(end) = s.find('}') {
      return s[end + 1..].trim_start();
    }
  }
  s
}

/// Parses a `@param` body into `(name, description)`, tolerating an optional
/// `{type}` and `[name]` / `[name=default]` bracket syntax.
fn parse_param(rest: &str) -> Option<(String, &str)> {
  let rest = skip_type(rest);
  let (name, desc) = split_first_token(rest);
  if name.is_empty() {
    return None;
  }
  // `[name]` (optional) and `[name=default]`.
  let name = name.trim_start_matches('[').trim_end_matches(']');
  let name = name.split('=').next().unwrap_or(name);
  Some((name.to_string(), desc))
}

/// Strips a leading JSDoc comment prefix (` * `) from a single line.
fn strip_comment_prefix(line: &str) -> &str {
  let line = line.trim_start();
  let line = line.strip_prefix('*').unwrap_or(line);
  line.strip_prefix(' ').unwrap_or(line)
}

/// Normalizes an `@example` body: drops a leading two-space indent per line
/// (matching the JS implementation's output) and trims.
fn normalize_example(text: &str) -> JsWord {
  text
    .lines()
    .map(|line| line.strip_prefix("  ").unwrap_or(line))
    .collect::<Vec<_>>()
    .join("\n")
    .trim()
    .into()
}
