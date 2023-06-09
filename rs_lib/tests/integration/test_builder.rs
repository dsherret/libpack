use anyhow::Result;
use deno_ast::ModuleSpecifier;
use rs_lib::rs_pack;
use rs_lib::PackOptions;

use super::InMemoryLoader;

pub struct TestBuilder {
  loader: InMemoryLoader,
  entry_point: String,
}

impl TestBuilder {
  pub fn new() -> Self {
    let loader = InMemoryLoader::default();
    Self {
      loader,
      entry_point: "file:///mod.ts".to_string(),
    }
  }

  pub fn with_loader(
    &mut self,
    mut action: impl FnMut(&mut InMemoryLoader),
  ) -> &mut Self {
    action(&mut self.loader);
    self
  }

  pub fn entry_point(&mut self, value: impl AsRef<str>) -> &mut Self {
    self.entry_point = value.as_ref().to_string();
    self
  }

  pub async fn pack(&self) -> Result<String> {
    rs_pack(
      &PackOptions {
        entry_points: vec![self.entry_point.clone()],
        import_map: None,
      },
      &mut self.loader.clone(),
    )
    .await
    .map(|output| output.js)
  }

  pub async fn pack_dts(&self) -> Result<String> {
    rs_pack(
      &PackOptions {
        entry_points: vec![self.entry_point.clone()],
        import_map: None,
      },
      &mut self.loader.clone(),
    )
    .await
    .map(|output| output.dts)
  }
}
