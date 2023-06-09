use anyhow::Context;
use deno_ast::ModuleSpecifier;
use deno_graph::source::Loader;
use deno_graph::CapturingModuleAnalyzer;
use deno_graph::DefaultModuleParser;
use serde::Deserialize;
use serde::Serialize;

mod dts;
mod helpers;
mod pack_js;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(module = "/helpers.js")]
extern "C" {
  async fn fetch_specifier(specifier: String) -> JsValue;
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
  #[wasm_bindgen(js_namespace = console, js_name = error)]
  pub fn log(s: &str);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn log(s: &str) {
  eprintln!("{}", s);
}

#[macro_export]
macro_rules! console_log {
  ($($t:tt)*) => ($crate::log(&format_args!($($t)*).to_string()))
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct JsLoader;

#[cfg(target_arch = "wasm32")]
impl Loader for JsLoader {
  fn load(
    &mut self,
    specifier: &ModuleSpecifier,
    _is_dynamic: bool,
  ) -> deno_graph::source::LoadFuture {
    let specifier = specifier.to_string();
    Box::pin(async move {
      let resp = fetch_specifier(specifier).await;
      if resp.is_null() || resp.is_undefined() {
        return Ok(None);
      }
      if !resp.is_object() {
        anyhow::bail!("fetch response wasn't an object");
      }
      let load_response = serde_wasm_bindgen::from_value(resp).unwrap();
      Ok(Some(load_response))
    })
  }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub async fn pack(options: JsValue) -> Result<JsValue, JsValue> {
  console_error_panic_hook::set_once();

  let options: PackOptions = serde_wasm_bindgen::from_value(options).unwrap();
  let mut loader = JsLoader::default();
  match rs_pack(&options, &mut loader).await {
    Ok(output) => Ok(serde_wasm_bindgen::to_value(&output).unwrap()),
    Err(err) => Err(format!("{:#}", err))?,
  }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackOptions {
  pub entry_points: Vec<String>,
  pub import_map: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackOutput {
  pub js: String,
  pub dts: String,
  pub import_map: Option<String>,
}

pub async fn rs_pack(
  options: &PackOptions,
  loader: &mut dyn Loader,
) -> Result<PackOutput, anyhow::Error> {
  let mut graph = deno_graph::ModuleGraph::new(deno_graph::GraphKind::All);
  let entry_points = parse_module_specifiers(&options.entry_points)?;
  let source_parser = DefaultModuleParser::new_for_analysis();
  let capturing_analyzer =
    CapturingModuleAnalyzer::new(Some(Box::new(source_parser)), None);
  let maybe_import_map = match &options.import_map {
    Some(import_map_url) => Some(
      ImportMapResolver::load(
        &ModuleSpecifier::parse(&import_map_url)?,
        loader,
      )
      .await
      .context("Error loading import map.")?,
    ),
    None => None,
  };
  graph
    .build(
      entry_points,
      loader,
      deno_graph::BuildOptions {
        is_dynamic: false,
        imports: vec![],
        resolver: maybe_import_map.as_ref().map(|r| r.as_resolver()),
        module_analyzer: Some(&capturing_analyzer),
        reporter: None,
        npm_resolver: None,
      },
    )
    .await;
  graph.valid()?;
  let parser = capturing_analyzer.as_capturing_parser();
  let js = pack_js::pack(
    &graph,
    &parser,
    pack_js::PackOptions {
      include_remote: false,
    },
  )?;
  let dts = dts::pack_dts(&graph, &parser)?;

  Ok(PackOutput {
    js,
    dts,
    import_map: maybe_import_map.map(|r| r.0.to_json()),
  })
}

fn parse_module_specifiers(
  values: &[String],
) -> Result<Vec<ModuleSpecifier>, anyhow::Error> {
  let mut specifiers = Vec::new();
  for value in values {
    let entry_point = ModuleSpecifier::parse(&value)?;
    specifiers.push(entry_point);
  }
  Ok(specifiers)
}

#[derive(Debug)]
struct ImportMapResolver(import_map::ImportMap);

impl ImportMapResolver {
  pub async fn load(
    import_map_url: &ModuleSpecifier,
    loader: &mut dyn Loader,
  ) -> anyhow::Result<Self> {
    let response = loader
      .load(import_map_url, false)
      .await?
      .ok_or_else(|| anyhow::anyhow!("Could not find {}", import_map_url))?;
    match response {
      deno_graph::source::LoadResponse::External { specifier } => {
        anyhow::bail!("Did not expect external import map {}", specifier)
      }
      deno_graph::source::LoadResponse::Module {
        content, specifier, ..
      } => {
        let value = jsonc_parser::parse_to_serde_value(
          &content,
          &jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_loose_object_property_names: true,
            allow_trailing_commas: true,
          },
        )?
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
        let result = import_map::parse_from_value(&specifier, value)?;
        Ok(ImportMapResolver(result.import_map))
      }
    }
  }

  pub fn as_resolver(&self) -> &dyn deno_graph::source::Resolver {
    self
  }
}

impl deno_graph::source::Resolver for ImportMapResolver {
  fn resolve(
    &self,
    specifier: &str,
    referrer: &ModuleSpecifier,
  ) -> Result<ModuleSpecifier, anyhow::Error> {
    self
      .0
      .resolve(specifier, referrer)
      .map_err(|err| err.into())
  }
}
