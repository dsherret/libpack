use std::cell::RefCell;

use anyhow::Result;
use rs_lib::lib_pack;
use rs_lib::Diagnostic;
use rs_lib::PackOptions;
use rs_lib::PackOutput;
use rs_lib::Reporter;

use super::InMemoryLoader;

#[derive(Default)]
struct TestReporter {
  diagnostics: RefCell<Vec<Diagnostic>>,
}

impl TestReporter {
  pub fn diagnostics(self) -> Vec<Diagnostic> {
    self.diagnostics.take()
  }
}

impl Reporter for TestReporter {
  fn diagnostic(&self, diagnostic: Diagnostic) {
    self.diagnostics.borrow_mut().push(diagnostic);
  }
}

pub struct PackResult {
  pub output: PackOutput,
  pub diagnostics: Vec<Diagnostic>,
}

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

  pub async fn pack(&self) -> Result<PackResult> {
    let reporter = TestReporter::default();
    let output = lib_pack(
      &PackOptions {
        entry_points: vec![self.entry_point.clone()],
        import_map: None,
      },
      &mut self.loader.clone(),
      &reporter,
    )
    .await?;
    Ok(PackResult {
      output,
      diagnostics: reporter.diagnostics(),
    })
  }
}
