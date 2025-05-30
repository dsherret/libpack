use std::{path::PathBuf, sync::Arc};

use deno_cache_dir::file_fetcher::{
  CacheSetting, HeaderMap, NullBlobStore, SendError, SendResponse,
};
use deno_error::JsErrorBox;
use deno_graph::{CapturingEsParser, CapturingModuleAnalyzer, DefaultModuleAnalyzer, MediaType, Module, ModuleGraph};
use deno_npm_cache::{NpmCacheHttpClientBytesResponse, NpmCacheHttpClientResponse};
use deno_npm_installer::{
  NpmInstallerFactory, NpmInstallerFactoryOptions, lifecycle_scripts::NullLifecycleScriptsExecutor,
};
use deno_resolver::{
  factory::{ResolverFactory, WorkspaceFactory},
  file_fetcher::{
    DenoGraphLoader, DenoGraphLoaderOptions, PermissionedFileFetcher,
    PermissionedFileFetcherOptions,
  },
  graph::DefaultDenoResolverRc,
  workspace::ScopedJsxImportSourceConfig,
};
use reqwest::{StatusCode, header};
use sys_traits::{EnvCurrentDir, impls::RealSys};
use url::Url;

mod bundler;
mod helpers;

#[derive(Debug)]
struct DenoBundleHttpClient {
  client: reqwest::Client,
}

#[async_trait::async_trait(?Send)]
impl deno_cache_dir::file_fetcher::HttpClient for DenoBundleHttpClient {
  async fn send_no_follow(&self, url: &Url, headers: HeaderMap) -> Result<SendResponse, SendError> {
    eprintln!("Downloading: {}", url);
    let response = self
      .client
      .get(url.clone())
      .headers(headers)
      .send()
      .await
      .map_err(|err| SendError::Failed(Box::new(err)))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
      Ok(SendResponse::NotModified)
    } else if response.status().is_redirection() {
      // todo: how to not clone?
      let headers = response.headers().clone();
      Ok(SendResponse::Redirect(headers))
    } else if response.status() == reqwest::StatusCode::NOT_FOUND {
      Err(SendError::NotFound)
    } else if response.status().is_server_error() {
      Err(SendError::StatusCode(response.status()))
    } else {
      // todo: how to not clone?
      let headers = response.headers().clone();
      let bytes = response.bytes().await.map_err(|err| SendError::Failed(Box::new(err)))?;
      Ok(SendResponse::Success(headers, bytes.into()))
    }
  }
}

#[async_trait::async_trait(?Send)]
impl deno_npm_cache::NpmCacheHttpClient for DenoBundleHttpClient {
  // todo: implement retrying
  async fn download_with_retries_on_any_tokio_runtime(
    &self,
    url: Url,
    maybe_auth: Option<String>,
    maybe_etag: Option<String>,
  ) -> Result<NpmCacheHttpClientResponse, deno_npm_cache::DownloadError> {
    eprintln!("Downloading: {}", url);
    let mut headers = HeaderMap::new();
    if let Some(auth) = maybe_auth {
      headers.append(header::AUTHORIZATION, header::HeaderValue::try_from(auth).unwrap());
    }
    if let Some(etag) = maybe_etag {
      headers.append(header::IF_NONE_MATCH, header::HeaderValue::try_from(etag).unwrap());
    }
    let response = self.client.get(url.clone()).headers(headers).send().await.map_err(|err| {
      deno_npm_cache::DownloadError {
        status_code: err.status().map(|s| s.as_u16()),
        error: JsErrorBox::generic(err.to_string()),
      }
    })?;
    if response.status() == StatusCode::NOT_FOUND {
      Ok(NpmCacheHttpClientResponse::NotFound)
    } else if response.status() == StatusCode::NOT_MODIFIED {
      Ok(NpmCacheHttpClientResponse::NotModified)
    } else if response.status().is_success() {
      let headers = response.headers().clone(); // todo: do not clone here
      let body = response.bytes().await.map_err(|err| deno_npm_cache::DownloadError {
        status_code: err.status().map(|s| s.as_u16()),
        error: JsErrorBox::generic(err.to_string()),
      })?;
      Ok(NpmCacheHttpClientResponse::Bytes(NpmCacheHttpClientBytesResponse {
        etag: headers.get(header::ETAG).and_then(|e| e.to_str().map(|t| t.to_string()).ok()),
        bytes: body.into(),
      }))
    } else {
      Err(deno_npm_cache::DownloadError {
        status_code: Some(response.status().as_u16()),
        error: JsErrorBox::generic(response.status().canonical_reason().unwrap_or("unknown error")),
      })
    }
  }
}

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::print_stdout)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let sys = RealSys;
  let cwd = sys.env_current_dir()?;
  let entrypoint =
    deno_path_util::url_from_file_path(&cwd.join(std::env::args().collect::<Vec<_>>().remove(1)))
      .unwrap();
  let workspace_factory = Arc::new(WorkspaceFactory::new(sys.clone(), cwd, Default::default()));
  let cwd = workspace_factory.initial_cwd();
  let resolver_factory =
    Arc::new(ResolverFactory::new(workspace_factory.clone(), Default::default()));
  let rolldown_client = Arc::new(DenoBundleHttpClient { client: reqwest::Client::new() });
  let npm_installer_factory = NpmInstallerFactory::new(
    resolver_factory.clone(),
    rolldown_client,
    Arc::new(NullLifecycleScriptsExecutor),
    deno_npm_installer::LogReporter,
    NpmInstallerFactoryOptions {
      cache_setting: deno_npm_cache::NpmCacheSetting::Use,
      caching_strategy: deno_npm_installer::graph::NpmCachingStrategy::Eager,
      lifecycle_scripts_config: deno_npm_installer::LifecycleScriptsConfig {
        allowed: deno_npm_installer::PackagesAllowedScripts::None,
        initial_cwd: cwd.clone(),
        root_dir: workspace_factory.workspace_directory()?.workspace.root_dir_path(),
        explicit_install: false,
      },
      resolve_npm_resolution_snapshot: Box::new(|| Ok(None)),
    },
  );
  let npm_package_info_provider = npm_installer_factory.lockfile_npm_package_info_provider()?;
  let lockfile = workspace_factory.maybe_lockfile(npm_package_info_provider).await?;
  let resolver = resolver_factory.deno_resolver().await?;
  let cjs_tracker = resolver_factory.cjs_tracker()?;
  let jsx_config =
    ScopedJsxImportSourceConfig::from_workspace_dir(workspace_factory.workspace_directory()?)?;

  let file_fetcher = Arc::new(PermissionedFileFetcher::new(
    NullBlobStore,
    Arc::new(workspace_factory.http_cache()?.clone()),
    DenoBundleHttpClient { client: reqwest::Client::new() },
    sys.clone(),
    PermissionedFileFetcherOptions { allow_remote: true, cache_setting: CacheSetting::Use },
  ));
  let roots = Vec::from([entrypoint.clone()]);
  let graph_resolver = resolver.as_graph_resolver(cjs_tracker, &jsx_config);
  let module_analyzer = CapturingModuleAnalyzer::default();
  let loader = DenoGraphLoader::new(
    file_fetcher.clone(),
    workspace_factory.global_http_cache()?.clone(),
    resolver_factory.in_npm_package_checker()?.clone(),
    workspace_factory.sys().clone(),
    DenoGraphLoaderOptions { file_header_overrides: Default::default(), permissions: None },
  );

  let mut locker = lockfile.as_ref().map(|l| l.as_deno_graph_locker());
  let mut graph = deno_graph::ModuleGraph::new(deno_graph::GraphKind::CodeOnly);
  let npm_resolver = npm_installer_factory.npm_deno_graph_resolver().await?;
  graph
    .build(
      roots,
      Vec::new(),
      &loader,
      deno_graph::BuildOptions {
        is_dynamic: false,
        skip_dynamic_deps: false,
        module_info_cacher: Default::default(),
        executor: Default::default(),
        locker: locker.as_mut().map(|l| l as _),
        file_system: &sys,
        jsr_url_provider: Default::default(),
        passthrough_jsr_specifiers: false,
        module_analyzer: &module_analyzer,
        npm_resolver: Some(npm_resolver.as_ref()),
        reporter: None,
        resolver: Some(&graph_resolver),
      },
    )
    .await;
  graph.valid().unwrap();

  let packed = bundler::pack(&graph, module_analyzer.as_capturing_parser(), bundler::PackOptions { include_remote: true }).unwrap();

  println!("{}", packed);

//   let plugin = HttpImportPlugin { file_fetcher, resolver: resolver.clone(), graph };
//   let mut bundler = Bundler::with_plugins(
//     BundlerOptions {
//       input: Some(vec![InputItem {
//         name: Some(
//           // todo: improve lol
//           PathBuf::from(entrypoint.as_str().split('/').last().unwrap().to_string())
//             .file_stem()
//             .unwrap()
//             .to_string_lossy()
//             .to_string(),
//         ),
//         import: entrypoint.to_string(),
//       }]),
//       entry_filenames: Some(ChunkFilenamesOutputOption::String("[name].bundle.js".to_string())),
//       cwd: Some(cwd.clone()),
//       sourcemap: Some(SourceMapType::File),
//       ..Default::default()
//     },
//     vec![Arc::new(plugin)],
//   );
//   bundler.options().input.iter().for_each(|input| println!("Bundle {}", input.import));

//   let result = match bundler.write().await {
//     Ok(result) => result,
//     Err(err) => {
//       panic!("{:?}", err);
//     }
//   };

//   for asset in result.assets {
//     eprintln!("Writing: {}", asset.filename());
//     std::fs::write(cwd.join(asset.filename()), asset.content_as_bytes()).unwrap();
//   }

//   for err in result.warnings {
//     eprintln!("{}", err);
//   }

  Ok(())
}
