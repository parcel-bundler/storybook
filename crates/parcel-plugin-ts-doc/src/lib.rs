use std::path::Path;

use parcel_plugin::{
  Asset, AssetContent, BundleGraphDependencyResolution, ContentBuffer, DependencyFlags,
  DependencyOptions, Diagnostic, ExportsConditions, Plugin, register_plugin,
};
use ts_doc::{API, package, parse};

struct DocTransformer;

impl Plugin for DocTransformer {
  fn new(_config: &[u8]) -> Result<Self, Diagnostic> {
    Ok(DocTransformer)
  }

  fn transform(
    &self,
    asset: &mut Asset,
    _options: &parcel_plugin::Options,
  ) -> Result<(), Diagnostic> {
    let path = asset.file_path();
    let query = asset.query();
    let code = asset.content();
    let api = parse(Path::new(&path), code);

    for dep in &api.dependencies {
      asset.add_dependency(DependencyOptions {
        specifier: format!("docs:{}?inline=true", dep),
        specifier_type: parcel_plugin::SpecifierType::Esm,
        flags: DependencyFlags::SIDE_EFFECTS,
        conditions: ExportsConditions::TYPES,
        ..Default::default()
      });
    }

    asset.set_type("json");
    asset.set_custom_content(DocContent { api });
    asset.set_bundle_behavior(if query == "inline=true" {
      parcel_plugin::BundleBehavior::Inline
    } else {
      parcel_plugin::BundleBehavior::Isolated
    });

    Ok(())
  }
}

register_plugin!(DocTransformer);

struct DocContent {
  api: API,
}

impl AssetContent for DocContent {
  fn read(&self) -> Result<ContentBuffer, Diagnostic> {
    unreachable!()
  }

  fn package(
    &self,
    bundle_graph: &parcel_plugin::BundleGraph,
    bundle: &parcel_plugin::Bundle,
    _options: &parcel_plugin::Options,
  ) -> Result<ContentBuffer, Diagnostic> {
    let output = package(&DocGraph {
      bundle_graph,
      bundle,
    });
    let json = serde_json::to_string_pretty(&output).unwrap();
    Ok(ContentBuffer::String(json))
  }
}

struct DocGraph<'a> {
  bundle_graph: &'a parcel_plugin::BundleGraph,
  bundle: &'a parcel_plugin::Bundle,
}

impl<'a> ts_doc::BundleGraph for DocGraph<'a> {
  fn entry(&self) -> ts_doc::AssetId {
    self
      .bundle
      .main_entry_asset()
      .or_else(|| self.bundle.asset(0))
      .unwrap() as usize
  }

  fn api(&self, asset: ts_doc::AssetId) -> &API {
    let asset = self.bundle_graph.asset(asset as u32).unwrap();
    if let Some(content) = asset.custom_content::<DocContent>() {
      return &content.api;
    }

    unreachable!("expected DocContent")
  }

  fn resolve(&self, asset_id: ts_doc::AssetId, specifier: &str) -> Option<ts_doc::AssetId> {
    let asset = self.bundle_graph.asset(asset_id as u32).unwrap();
    for (dep_index, dep) in asset.dependencies().enumerate() {
      let s = dep.specifier();
      if &s[5..s.len() - 12] == specifier {
        match self
          .bundle_graph
          .dependency_resolution(asset_id as u32, dep_index)
        {
          BundleGraphDependencyResolution::Asset(resolution) => return Some(resolution as usize),
          BundleGraphDependencyResolution::Bundle(b) => {
            return self
              .bundle_graph
              .bundle(b)
              .unwrap()
              .main_entry_asset()
              .map(|b| b as usize);
          }
          _ => {}
        }
      }
    }

    None
  }
}
