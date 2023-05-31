use std::collections::HashMap;
use std::collections::VecDeque;

use anyhow::Result;
use deno_ast::ModuleSpecifier;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;
use deno_graph::ModuleParser;
use indexmap::IndexMap;

use super::analyzer::FileDepName;
use super::analyzer::ModuleAnalyzer;

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
  let graph_module = context.graph.get(specifier).unwrap();
  let graph_module = graph_module.esm().unwrap();
  let parsed_source = context.parser.parse_module(
    &graph_module.specifier,
    graph_module.source.clone(),
    graph_module.media_type,
  )?;

  let module_symbol = context.analyzer.get_or_analyze(&parsed_source);
  let mut pending_symbol_ids = match exports_to_trace {
    ExportsToTrace::All => module_symbol.export_symbols(),
    ExportsToTrace::Named(names) => names
      .iter()
      .filter_map(|name| module_symbol.export_symbol_id(name))
      .collect::<Vec<_>>(),
  };

  while let Some(symbol_id) = pending_symbol_ids.pop() {
    let symbol = module_symbol.symbol_mut(symbol_id).unwrap();
    if symbol.mark_public() {
      if let Some(file_dep) = symbol.file_dep() {
        let maybe_specifier = context.graph.resolve_dependency(
          &file_dep.specifier,
          specifier,
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
      pending_symbol_ids.extend(ids.iter().filter_map(|id| {
        match module_symbol.symbol_id_from_swc(id) {
          Some(id) => Some(id),
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
