use std::collections::VecDeque;

use anyhow::Result;
use deno_ast::ModuleSpecifier;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;
use deno_graph::ModuleParser;

use super::binder::ModuleAnalyzer;

enum ExportsToTrace {
  All,
  Named(Vec<String>),
}

impl ExportsToTrace {
  pub fn add(&mut self, name: &str) {
    if let ExportsToTrace::Named(names) = self {
      names.push(name.to_string());
    }
  }
}

struct Context<'a> {
  graph: &'a ModuleGraph,
  parser: &'a CapturingModuleParser<'a>,
  analyzer: ModuleAnalyzer,
}

pub fn trace<'a>(
  graph: &'a ModuleGraph,
  parser: &'a CapturingModuleParser<'a>,
) -> Result<ModuleAnalyzer> {
  let mut context = Context {
    graph,
    parser,
    analyzer: ModuleAnalyzer::default(),
  };
  assert_eq!(graph.roots.len(), 1);
  let root = &graph.roots[0];

  trace_module(root, &mut context, &ExportsToTrace::All)?;

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
    ExportsToTrace::All => {
      module_symbol.export_symbols()
    }
    ExportsToTrace::Named(names) => {
      names.iter().filter_map(|name| {
        module_symbol.export_symbol_id(name)
      }).collect::<Vec<_>>()
    },
  };

  while let Some(symbol_id) = pending_symbol_ids.pop() {
    let symbol = module_symbol.symbol(symbol_id).unwrap();
    if symbol.mark_public() {
      let ids = symbol.swc_dep_ids().map(ToOwned::to_owned).collect::<Vec<_>>();
      pending_symbol_ids.extend(
        ids.iter().map(|id| module_symbol.symbol_id_from_swc(id).unwrap())
      );
    }
  }

  Ok(())
}