use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use deno_graph::symbols::Definition;
use deno_graph::symbols::DefinitionPath;
use deno_graph::symbols::FileDep;
use deno_graph::symbols::FileDepName;
use deno_graph::symbols::ModuleId;
use deno_graph::symbols::ModuleInfo;
use deno_graph::symbols::ModuleInfoRef;
use deno_graph::symbols::ResolvedExportOrReExportAllPath;
use deno_graph::symbols::ResolvedSymbolDepEntry;
use deno_graph::symbols::RootSymbol;
use deno_graph::symbols::Symbol;
use deno_graph::symbols::SymbolDeclKind;
use deno_graph::symbols::UniqueSymbolId;
use deno_graph::ModuleError;
use deno_graph::ModuleGraph;
use deno_graph::ModuleSpecifier;
use indexmap::IndexMap;
use indexmap::IndexSet;

use crate::helpers::is_remote_specifier;

#[derive(Debug)]
pub enum SymbolOrRemoteDep {
  Symbol(UniqueSymbolId),
  RemoteDepName {
    referrer: ModuleId,
    specifier: String,
  },
}

#[derive(Debug)]
pub struct ExportAnalysis {
  exports_to_dep: IndexMap<String, SymbolOrRemoteDep>,
}

impl ExportAnalysis {
  pub fn build(root_symbol: &RootSymbol, graph: &ModuleGraph) -> Self {
    fn fill_exports_to_dep(
      root_symbol: &RootSymbol,
      exports_to_dep: &mut IndexMap<String, SymbolOrRemoteDep>,
      export_name: String,
      export_or_re_export_all_path: ResolvedExportOrReExportAllPath,
    ) {
      match export_or_re_export_all_path {
        ResolvedExportOrReExportAllPath::Export(export) => {
          if let Some(dep) = resolve_export_to_definition(root_symbol, &export)
          {
            exports_to_dep.insert(export_name, dep);
          } else {
            todo!("Export: {:#?}", export);
          }
        }
        ResolvedExportOrReExportAllPath::ReExportAllPath(path) => {
          if is_remote_specifier(path.resolved_module().specifier()) {
            exports_to_dep.insert(
              export_name,
              SymbolOrRemoteDep::RemoteDepName {
                referrer: path.referrer_module.module_id(),
                specifier: path.specifier.to_string(),
              },
            );
          } else {
            fill_exports_to_dep(
              root_symbol,
              exports_to_dep,
              export_name,
              *path.next,
            );
          }
        }
      }
    }

    assert_eq!(graph.roots.len(), 1);
    let mut exports_to_dep = IndexMap::new();
    let root = &graph.roots[0];
    let module_symbol = root_symbol.get_module_from_specifier(root).unwrap();
    let exports = module_symbol.exports(root_symbol);
    for (export_name, export_or_re_export_all_path) in exports.resolved {
      eprintln!("EXPORT NAME: {}", export_name);
      fill_exports_to_dep(
        root_symbol,
        &mut exports_to_dep,
        export_name,
        export_or_re_export_all_path,
      );
    }

    // todo: surface a diagnostic for this. It could be something like an npm module
    debug_assert!(exports.unresolved_specifiers.is_empty());

    Self { exports_to_dep }
  }

  pub fn export_symbols(
    &self,
  ) -> impl Iterator<Item = (&str, UniqueSymbolId)> + '_ {
    self
      .exports_to_dep
      .iter()
      .filter_map(|(name, dep)| match dep {
        SymbolOrRemoteDep::Symbol(id) => Some((name.as_str(), *id)),
        SymbolOrRemoteDep::RemoteDepName { .. } => None,
      })
  }
}

fn resolve_export_to_definition(
  root_symbol: &RootSymbol<'_>,
  export: &deno_graph::symbols::ResolvedExport<'_>,
) -> Option<SymbolOrRemoteDep> {
  let paths = root_symbol.find_definition_paths(export.module, export.symbol());
  let mut pending_paths = paths.into_iter().collect::<VecDeque<_>>();
  while let Some(path) = pending_paths.pop_front() {
    debug_assert!(!is_remote_specifier(path.module().specifier()));
    match path {
      DefinitionPath::Path {
        module,
        symbol,
        symbol_decl,
        parts,
        next,
      } => {
        match &symbol_decl.kind {
          SymbolDeclKind::FileRef(file_ref) => {
            // resolve the file ref specifier because the next path node might be an
            // unresolved specifier node, which wouldn't have the correct specifier
            if let Some(specifier) = root_symbol
              .resolve_types_dependency(&file_ref.specifier, module.specifier())
            {
              if is_remote_specifier(&specifier) {
                return Some(SymbolOrRemoteDep::RemoteDepName {
                  referrer: module.module_id(),
                  specifier: file_ref.specifier.to_string(),
                });
              }
            }
            pending_paths.extend(next);
          }
          SymbolDeclKind::Target(_) | SymbolDeclKind::QualifiedTarget(_, _) => {
            pending_paths.extend(next);
          }
          SymbolDeclKind::Definition(_) => unreachable!(),
        }
      }
      DefinitionPath::Definition(d) => {
        return Some(SymbolOrRemoteDep::Symbol(d.symbol.unique_id()));
      }
      DefinitionPath::Unresolved(d) => {
        return Some(SymbolOrRemoteDep::Symbol(d.symbol.unique_id()));
      }
    }
  }
  None
}

fn get_module_info<'a>(
  root_symbol: &'a RootSymbol,
  specifier: &ModuleSpecifier,
) -> ModuleInfoRef<'a> {
  root_symbol.get_module_from_specifier(specifier).unwrap()
}

fn resolve_deno_graph_module<'a>(
  graph: &'a ModuleGraph,
  specifier: &ModuleSpecifier,
) -> Result<&'a deno_graph::Module, &'a ModuleError> {
  Ok(graph.try_get_prefer_types(specifier)?.unwrap())
}

pub enum DefinitionOrRemoteRef<'a> {
  Definition(Definition<'a>),
  RemoteRef {
    specifier: &'a ModuleSpecifier,
    file_dep: &'a FileDep,
    symbol: &'a Symbol,
    parts: Vec<String>,
  },
}

fn go_to_definition_or_remote_ref<'a>(
  root_symbol: &'a RootSymbol,
  module: ModuleInfoRef<'a>,
  symbol: &'a Symbol,
) -> impl Iterator<Item = DefinitionOrRemoteRef<'a>> {
  struct IntoDefinitionIterator<'a> {
    queue: VecDeque<DefinitionPath<'a>>,
  }

  impl<'a> Iterator for IntoDefinitionIterator<'a> {
    type Item = DefinitionOrRemoteRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
      while let Some(path) = self.queue.pop_front() {
        match path {
          DefinitionPath::Path {
            symbol,
            next,
            parts,
            ..
          } => {
            if let Some(file_dep) = symbol.file_dep() {
              if let Some(next) = next.first() {
                if is_remote_specifier(next.module().specifier()) {
                  return Some(DefinitionOrRemoteRef::RemoteRef {
                    specifier: next.module().specifier(),
                    file_dep,
                    symbol: next.symbol(),
                    parts,
                  });
                }
              }
            }
            for child_path in next.into_iter().rev() {
              self.queue.push_front(child_path);
            }
          }
          DefinitionPath::Definition(def) => {
            return Some(DefinitionOrRemoteRef::Definition(def));
          }
          DefinitionPath::Unresolved(_) => todo!(),
        }
      }

      None
    }
  }

  let paths = root_symbol.find_definition_paths(module, symbol);
  IntoDefinitionIterator {
    queue: VecDeque::from(paths),
  }
}
