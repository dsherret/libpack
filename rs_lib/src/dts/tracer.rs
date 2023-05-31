use std::collections::HashSet;

use anyhow::Result;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;
use deno_graph::ModuleParser;
use indexmap::IndexMap;

use super::analyzer::FileDepName;
use super::analyzer::ModuleAnalyzer;
use super::analyzer::ModuleSymbol;
use super::analyzer::SymbolId;

enum ExportsToTrace {
  All,
  Named(Vec<String>),
}

impl ExportsToTrace {
  pub fn from_file_dep_name(dep_name: &FileDepName) -> Self {
    match dep_name {
      FileDepName::All => Self::All,
      FileDepName::Name(value) => Self::Named(vec![value.clone()]),
    }
  }

  pub fn add(&mut self, name: &FileDepName) {
    match name {
      FileDepName::All => {
        *self = ExportsToTrace::All;
      }
      FileDepName::Name(name) => {
        if let ExportsToTrace::Named(names) = self {
          names.push(name.to_string());
        }
      }
    }
  }
}

struct Context<'a> {
  graph: &'a ModuleGraph,
  parser: &'a CapturingModuleParser<'a>,
  analyzer: ModuleAnalyzer,
  pending_traces: IndexMap<ModuleSpecifier, ExportsToTrace>,
}

impl<'a> Context<'a> {
  pub fn parsed_source(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Result<ParsedSource, deno_ast::Diagnostic> {
    let graph_module = self.graph.get(specifier).unwrap();
    let graph_module = graph_module.esm().unwrap();
    self.parser.parse_module(
      &graph_module.specifier,
      graph_module.source.clone(),
      graph_module.media_type,
    )
  }

  pub fn get_module_symbol(
    &mut self,
    specifier: &ModuleSpecifier,
  ) -> Result<&ModuleSymbol> {
    if self.analyzer.get(specifier).is_none() {
      let parsed_source = self.parsed_source(specifier)?;
      self.analyzer.analyze(&parsed_source);
    }
    Ok(self.analyzer.get(specifier).unwrap())
  }

  pub fn get_exports(
    &mut self,
    specifier: &ModuleSpecifier,
    exports_to_trace: &ExportsToTrace,
  ) -> Result<Vec<(ModuleSpecifier, SymbolId)>> {
    self.get_exports_inner(specifier, exports_to_trace, HashSet::new())
  }

  fn get_exports_inner(
    &mut self,
    specifier: &ModuleSpecifier,
    exports_to_trace: &ExportsToTrace,
    visited: HashSet<ModuleSpecifier>,
  ) -> Result<Vec<(ModuleSpecifier, SymbolId)>> {
    let mut result = Vec::new();
    let module_symbol = self.get_module_symbol(specifier)?;
    match exports_to_trace {
      ExportsToTrace::All => {
        result.extend(
          module_symbol
            .export_symbols()
            .into_iter()
            .map(|s| (specifier.clone(), s)),
        );
      }
      ExportsToTrace::Named(names) => {
        let exports = module_symbol.exports().clone();
        let re_exports = module_symbol.re_exports().clone();
        drop(module_symbol);
        for name in names {
          if let Some(symbol_id) = exports.get(name) {
            result.push((specifier.clone(), symbol_id.clone()));
          } else {
            for re_export_specifier in &re_exports {
              let maybe_specifier = self.graph.resolve_dependency(
                &re_export_specifier,
                specifier,
                /* prefer_types */ true,
              );
              if let Some(specifier) = maybe_specifier {
                let mut found = self.get_exports_inner(
                  &specifier,
                  &ExportsToTrace::Named(vec![name.clone()]),
                  {
                    let mut visited = visited.clone();
                    visited.insert(specifier.clone());
                    visited
                  },
                )?;
                if !found.is_empty() {
                  assert_eq!(found.len(), 1);
                  result.push(found.remove(0));
                  break;
                }
              }
            }
          }
        }
      }
    }
    Ok(result)
  }
}

pub fn trace<'a>(
  graph: &'a ModuleGraph,
  parser: &'a CapturingModuleParser<'a>,
) -> Result<ModuleAnalyzer> {
  assert_eq!(graph.roots.len(), 1);
  let mut context = Context {
    graph,
    parser,
    analyzer: ModuleAnalyzer::default(),
    pending_traces: IndexMap::from([(
      graph.roots[0].clone(),
      ExportsToTrace::All,
    )]),
  };
  while let Some((specifier, exports_to_trace)) = context.pending_traces.pop() {
    trace_module(&specifier, &mut context, &exports_to_trace)?;
  }

  Ok(context.analyzer)
}

fn trace_module<'a>(
  specifier: &ModuleSpecifier,
  context: &mut Context<'a>,
  exports_to_trace: &ExportsToTrace,
) -> Result<()> {
  let mut pending = context.get_exports(specifier, exports_to_trace)?;

  while let Some((specifier, symbol_id)) = pending.pop() {
    let module_symbol = context.analyzer.get_mut(&specifier).unwrap();
    let symbol = module_symbol.symbol_mut(symbol_id).unwrap();
    if symbol.mark_public() {
      if let Some(file_dep) = symbol.file_dep() {
        let maybe_specifier = context.graph.resolve_dependency(
          &file_dep.specifier,
          &specifier,
          /* prefer types */ true,
        );
        if let Some(specifier) = maybe_specifier {
          if let Some(exports_to_trace) =
            context.pending_traces.get_mut(&specifier)
          {
            exports_to_trace.add(&file_dep.name);
          } else {
            context.pending_traces.insert(
              specifier,
              ExportsToTrace::from_file_dep_name(&file_dep.name),
            );
          }
        }
      }
      let ids = symbol
        .swc_dep_ids()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
      pending.extend(ids.iter().filter_map(|id| {
        match module_symbol.symbol_id_from_swc(id) {
          Some(id) => Some((specifier.clone(), id)),
          None => {
            eprintln!("Failed to find symbol id for swc id: {:?}", id);
            None
          }
        }
      }));
    }
  }

  Ok(())
}
