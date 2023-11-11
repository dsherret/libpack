use std::borrow::Cow;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;

use deno_ast::swc::ast::*;
use deno_ast::swc::atoms::atom;
use deno_ast::swc::common::comments::CommentKind;
use deno_ast::swc::common::comments::SingleThreadedComments;
use deno_ast::swc::common::BytePos;
use deno_ast::swc::common::FileName;
use deno_ast::swc::common::SourceMap;
use deno_ast::swc::common::Span;
use deno_ast::swc::common::Spanned;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::visit::*;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_ast::SourcePos;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned;
use deno_graph::symbols::EsmModuleInfo;
use deno_graph::symbols::ModuleId;
use deno_graph::symbols::ModuleInfoRef;
use deno_graph::symbols::ResolvedSymbolDepEntry;
use deno_graph::symbols::RootSymbol;
use deno_graph::symbols::Symbol;
use deno_graph::symbols::SymbolNodeDep;
use deno_graph::symbols::UniqueSymbolId;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;
use indexmap::IndexMap;
use indexmap::IndexSet;

use crate::dts::analyzer::resolve_paths_to_remote_path;
use crate::helpers::adjust_spans;
use crate::helpers::fill_leading_comments;
use crate::helpers::ident;
use crate::helpers::is_remote_specifier;
use crate::helpers::print_program;
use crate::helpers::ts_keyword_type;
use crate::helpers::SpanAdjuster;
use crate::Diagnostic;
use crate::Reporter;

use self::analyzer::analyze_exports;
use self::analyzer::SymbolOrRemoteDep;

mod analyzer;

// struct LibPackTypeTraceHandler<'a, TReporter: Reporter>(&'a TReporter);

// impl<'a, TReporter: Reporter> deno_graph::type_tracer::TypeTraceHandler
//   for LibPackTypeTraceHandler<'a, TReporter>
// {
//   fn diagnostic(
//     &self,
//     diagnostic: deno_graph::type_tracer::TypeTraceDiagnostic,
//   ) {
//     self.0.diagnostic(Diagnostic {
//         message: match diagnostic.kind {
//             deno_graph::type_tracer::TypeTraceDiagnosticKind::UnsupportedDefaultExpr => concat!(
//               "Default expressions that are not identifiers are not supported. ",
//               "To work around this, extract out the expression to a variable, ",
//               "type the variable, and then default export the variable declaration.",
//             ).to_string(),
//         },
//         specifier: diagnostic.specifier,
//         line_and_column: diagnostic.line_and_column.map(|line_and_column| line_and_column.into()),
//       });
//   }
// }

pub fn pack_dts(
  graph: &ModuleGraph,
  parser: CapturingModuleParser,
  reporter: &impl Reporter,
) -> Result<String, anyhow::Error> {
  let source_map = Rc::new(SourceMap::default());
  let global_comments = SingleThreadedComments::default();
  // let mut remote_module_items = Vec::new();
  // let mut default_remote_module_items = Vec::new();
  let root_symbol = deno_graph::symbols::RootSymbol::new(graph, parser);
  // MAIN TODO:
  // - Now based on the exports, symbols, create a list of pending symbols to iterate over
  //   and start building up the declaration file. Add any symbols found along the way
  //   to the list of pending symbols.
  // - For every declaration, try to have it at the top level.
  // - If encountering a module or namespace, then create an appropriate typescript namespace that just re-exports the top level declarations.

  let analyzed_exports = analyze_exports(&root_symbol, graph);
  let pending_symbols = analyzed_exports
    .iter()
    .filter_map(|(export_name, d)| match d {
      SymbolOrRemoteDep::Symbol(dep) => Some((export_name, dep)),
      SymbolOrRemoteDep::RemoteDepName { .. } => None,
    })
    .map(|(export_name, id)| (Some(export_name.to_string()), *id))
    .collect::<VecDeque<_>>();
  let analyzed_symbols = pending_symbols
    .iter()
    .map(|(_, id)| id)
    .copied()
    .collect::<HashSet<_>>();

  let mut bundler = DtsBundler {
    graph,
    root_symbol: &root_symbol,
    reporter,
    output: OutputContainer {
      source_map,
      global_comments,
      modules: Default::default(),
      final_module: Module {
        span: DUMMY_SP,
        body: vec![],
        shebang: None,
      },
    },
    analyzed_symbols,
    emitted_symbols: Default::default(),
    pending_symbols,
    top_level_symbols: Default::default(),
    queued_named_exports: Default::default(),
  };
  bundler.bundle();

  print_program(
    &bundler.output.final_module,
    &bundler.output.source_map,
    &bundler.output.global_comments,
  )
}

#[derive(Debug, Default)]
struct TopLevelSymbols {
  name_to_id: IndexMap<String, UniqueSymbolId>,
  name_collision_count: IndexMap<String, usize>,
  id_to_name: IndexMap<UniqueSymbolId, String>,
  public_symbols: IndexSet<UniqueSymbolId>,
}

impl TopLevelSymbols {
  pub fn ensure_top_level_name(&mut self, symbol: &Symbol) -> String {
    let id = symbol.unique_id();
    if let Some(name) = self.id_to_name.get(&id) {
      return name.to_string();
    }
    let name = symbol.maybe_name().unwrap_or_else(|| {
      if symbol.decls().iter().all(|d| d.is_function()) {
        Cow::Borrowed("noName")
      } else {
        Cow::Borrowed("NoName")
      }
    });
    let name = if self.name_to_id.contains_key(name.as_ref()) {
      loop {
        let collision_count = *self
          .name_collision_count
          .entry(name.to_string())
          .and_modify(|count| *count += 1)
          .or_insert(1);
        let new_name = format!("{}{}", name, collision_count);
        if !self.name_to_id.contains_key(&new_name) {
          break new_name;
        }
      }
    } else {
      name.to_string()
    };
    self.name_to_id.insert(name.clone(), id);
    self.id_to_name.insert(id, name.clone());
    name
  }
}

struct OutputContainer {
  source_map: Rc<SourceMap>,
  global_comments: SingleThreadedComments,
  modules: IndexMap<ModuleId, BytePos>,
  final_module: Module,
}

impl OutputContainer {
  pub fn get_start_pos(&mut self, module: ModuleInfoRef) -> BytePos {
    if let Some(pos) = self.modules.get(&module.module_id()) {
      return *pos;
    }
    let source_file = self.source_map.new_source_file(
      FileName::Url(module.specifier().clone()),
      module.text_info().text_str().to_string(),
    );
    self
      .modules
      .insert(module.module_id(), source_file.start_pos);

    if let Some(module) = module.esm() {
      // Add the file's leading comments to the global comment map.
      // We don't have to deal with the trailing comments because
      // we're only interested in jsdocs
      fill_leading_comments(
        source_file.start_pos,
        &module.source(),
        &self.global_comments,
        // only include js docs
        |comment| {
          comment.kind == CommentKind::Block && comment.text.starts_with('*')
        },
      );
    }
    source_file.start_pos
  }

  pub fn adjust_spans(
    &mut self,
    module: ModuleInfoRef,
    node: &mut impl VisitMutWith<SpanAdjuster>,
  ) {
    let start_pos = self.get_start_pos(module);
    adjust_spans(start_pos, node)
  }

  fn add_module_item(
    &mut self,
    module: ModuleInfoRef,
    mut module_item: ModuleItem,
  ) {
    self.adjust_spans(module, &mut module_item);
    self.final_module.body.push(module_item);
  }

  fn add_class_decl<TReporter: Reporter>(
    &mut self,
    mut decl: ClassDecl,
    transformer: &mut DtsTransformer<'_, TReporter>,
    js_doc_span: Span,
    top_level_name: &String,
    maybe_export_name: &Option<String>,
    module: ModuleInfoRef<'_>,
  ) {
    decl.visit_mut_with(transformer);
    decl.ident.sym = top_level_name.clone().into();
    let module_item =
      if maybe_export_name.as_deref() == Some(top_level_name.as_str()) {
        decl.class.span = DUMMY_SP;
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
          span: js_doc_span,
          decl: Decl::Class(decl),
        }))
      } else {
        decl.class.span = js_doc_span;
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(decl)))
      };
    self.add_module_item(module, module_item);
  }

  fn add_enum<TReporter: Reporter>(
    &mut self,
    mut decl: TsEnumDecl,
    transformer: &mut DtsTransformer<'_, TReporter>,
    js_doc_span: Span,
    top_level_name: &String,
    maybe_export_name: &Option<String>,
    module: ModuleInfoRef<'_>,
  ) {
    decl.visit_mut_with(transformer);
    decl.id.sym = top_level_name.clone().into();
    let module_item =
      if maybe_export_name.as_deref() == Some(top_level_name.as_str()) {
        decl.span = DUMMY_SP;
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
          span: js_doc_span,
          decl: Decl::TsEnum(Box::new(decl)),
        }))
      } else {
        decl.span = js_doc_span;
        ModuleItem::Stmt(Stmt::Decl(Decl::TsEnum(Box::new(decl))))
      };
    self.add_module_item(module, module_item);
  }

  fn add_function<TReporter: Reporter>(
    &mut self,
    mut decl: FnDecl,
    transformer: &mut DtsTransformer<'_, TReporter>,
    js_doc_span: Span,
    top_level_name: &String,
    maybe_export_name: &Option<String>,
    module: ModuleInfoRef<'_>,
  ) {
    decl.visit_mut_with(transformer);
    decl.ident.sym = top_level_name.clone().into();
    let module_item =
      if maybe_export_name.as_deref() == Some(top_level_name.as_str()) {
        decl.function.span = DUMMY_SP;
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
          span: js_doc_span,
          decl: Decl::Fn(decl),
        }))
      } else {
        decl.function.span = js_doc_span;
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(decl)))
      };
    self.add_module_item(module, module_item);
  }

  fn add_interface<TReporter: Reporter>(
    &mut self,
    mut decl: TsInterfaceDecl,
    transformer: &mut DtsTransformer<'_, TReporter>,
    js_doc_span: Span,
    top_level_name: &String,
    maybe_export_name: &Option<String>,
    module: ModuleInfoRef<'_>,
  ) {
    decl.visit_mut_with(transformer);
    decl.id.sym = top_level_name.clone().into();
    let module_item =
      if maybe_export_name.as_deref() == Some(top_level_name.as_str()) {
        decl.span = DUMMY_SP;
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
          span: js_doc_span,
          decl: Decl::TsInterface(Box::new(decl)),
        }))
      } else {
        decl.span = js_doc_span;
        ModuleItem::Stmt(Stmt::Decl(Decl::TsInterface(Box::new(decl))))
      };
    self.add_module_item(module, module_item);
  }

  fn add_type_alias<TReporter: Reporter>(
    &mut self,
    mut decl: TsTypeAliasDecl,
    transformer: &mut DtsTransformer<'_, TReporter>,
    js_doc_span: Span,
    top_level_name: &String,
    maybe_export_name: &Option<String>,
    module: ModuleInfoRef<'_>,
  ) {
    decl.visit_mut_with(transformer);
    decl.id.sym = top_level_name.clone().into();
    let module_item =
      if maybe_export_name.as_deref() == Some(top_level_name.as_str()) {
        decl.span = DUMMY_SP;
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
          span: js_doc_span,
          decl: Decl::TsTypeAlias(Box::new(decl)),
        }))
      } else {
        decl.span = js_doc_span;
        ModuleItem::Stmt(Stmt::Decl(Decl::TsTypeAlias(Box::new(decl))))
      };
    self.add_module_item(module, module_item);
  }

  fn add_var_decl<TReporter: Reporter>(
    &mut self,
    mut decl: VarDecl,
    transformer: &mut DtsTransformer<'_, TReporter>,
    js_doc_span: Span,
    top_level_name: &String,
    maybe_export_name: &Option<String>,
    module: ModuleInfoRef<'_>,
  ) {
    if decl.decls.len() > 1 {
      todo!();
    }
    if !decl.decls[0].name.is_ident() {
      todo!();
    }
    decl.visit_mut_with(transformer);
    let declarator = &mut decl.decls.get_mut(0).unwrap();
    if let Pat::Ident(ident) = &mut declarator.name {
      ident.id.sym = top_level_name.clone().into();
    }
    let module_item =
      if maybe_export_name.as_deref() == Some(top_level_name.as_str()) {
        decl.span = DUMMY_SP;
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
          span: js_doc_span,
          decl: Decl::Var(Box::new(decl)),
        }))
      } else {
        decl.span = js_doc_span;
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(decl))))
      };
    self.add_module_item(module, module_item);
  }
}

struct DtsBundler<'a, TReporter: Reporter> {
  root_symbol: &'a RootSymbol<'a>,
  graph: &'a ModuleGraph,
  reporter: &'a TReporter,
  output: OutputContainer,
  pending_symbols: VecDeque<(Option<String>, UniqueSymbolId)>,
  analyzed_symbols: HashSet<UniqueSymbolId>,
  emitted_symbols: HashSet<UniqueSymbolId>,
  top_level_symbols: TopLevelSymbols,
  queued_named_exports: Vec<(String, String)>,
}

impl<'a, TReporter: Reporter> DtsBundler<'a, TReporter> {
  pub fn bundle(&mut self) {
    while let Some((maybe_export_name, symbol_id)) =
      self.pending_symbols.pop_front()
    {
      let module = self
        .root_symbol
        .get_module_from_id(symbol_id.module_id)
        .unwrap();
      let symbol = module.symbol(symbol_id.symbol_id).unwrap();
      let top_level_name = self.top_level_symbols.ensure_top_level_name(symbol);

      // this will be false when analyzing an export that goes to the same symbol
      if self.emitted_symbols.insert(symbol_id) {
        let mut transformer = DtsTransformer {
          reporter: self.reporter,
          root_symbol: &self.root_symbol,
          symbol,
          top_level_symbols: &mut self.top_level_symbols,
          graph: self.graph,
          module_info: module.esm().unwrap(), // todo: handle json
          found_symbols: Default::default(),
        };
        for decl in symbol.decls() {
          if decl.has_overloads() {
            continue; // ignore implementation signatures
          }
          match decl.maybe_node() {
            Some(node) => match node {
              deno_graph::symbols::SymbolNodeRef::Module(_) => todo!(),
              deno_graph::symbols::SymbolNodeRef::ExportDecl(
                export_decl,
                n,
              ) => match n {
                deno_graph::symbols::ExportDeclRef::Class(decl) => {
                  self.output.add_class_decl(
                    decl.clone(),
                    &mut transformer,
                    export_decl.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                deno_graph::symbols::ExportDeclRef::Fn(decl) => {
                  self.output.add_function(
                    decl.clone(),
                    &mut transformer,
                    export_decl.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                deno_graph::symbols::ExportDeclRef::Var(decl, _, _) => {
                  self.output.add_var_decl(
                    decl.clone(),
                    &mut transformer,
                    export_decl.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                deno_graph::symbols::ExportDeclRef::TsEnum(decl) => {
                  self.output.add_enum(
                    decl.clone(),
                    &mut transformer,
                    export_decl.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                deno_graph::symbols::ExportDeclRef::TsInterface(decl) => {
                  self.output.add_interface(
                    decl.clone(),
                    &mut transformer,
                    export_decl.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                deno_graph::symbols::ExportDeclRef::TsModule(_) => todo!(),
                deno_graph::symbols::ExportDeclRef::TsTypeAlias(decl) => {
                  self.output.add_type_alias(
                    decl.clone(),
                    &mut transformer,
                    export_decl.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
              },
              deno_graph::symbols::SymbolNodeRef::ExportDefaultDecl(
                export_default_expr,
              ) => match &export_default_expr.decl {
                DefaultDecl::Class(n) => {
                  let decl = ClassDecl {
                    ident: Ident::new(top_level_name.clone().into(), n.span()),
                    class: n.class.clone(),
                    declare: false,
                  };
                  self.output.add_class_decl(
                    decl,
                    &mut transformer,
                    export_default_expr.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                DefaultDecl::Fn(n) => {
                  let decl = FnDecl {
                    ident: Ident::new(top_level_name.clone().into(), n.span()),
                    function: n.function.clone(),
                    declare: false,
                  };
                  self.output.add_function(
                    decl,
                    &mut transformer,
                    export_default_expr.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
                DefaultDecl::TsInterfaceDecl(decl) => {
                  self.output.add_interface(
                    *decl.clone(),
                    &mut transformer,
                    export_default_expr.span(),
                    &top_level_name,
                    &maybe_export_name,
                    module,
                  );
                }
              },
              deno_graph::symbols::SymbolNodeRef::ExportDefaultExprLit(
                default_expr,
                lit,
              ) => {
                let decl = VarDecl {
                  span: default_expr.span,
                  kind: VarDeclKind::Const,
                  declare: false,
                  decls: Vec::from([VarDeclarator {
                    span: DUMMY_SP,
                    name: Pat::Ident(BindingIdent {
                      id: Ident::new(top_level_name.clone().into(), DUMMY_SP),
                      type_ann: maybe_infer_type_from_lit(lit).map(|t| {
                        Box::new(TsTypeAnn {
                          span: DUMMY_SP,
                          type_ann: Box::new(t),
                        })
                      }),
                    }),
                    init: None,
                    definite: false,
                  }]),
                };
                self.output.add_var_decl(
                  decl,
                  &mut transformer,
                  default_expr.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              deno_graph::symbols::SymbolNodeRef::ClassDecl(decl) => {
                self.output.add_class_decl(
                  decl.clone(),
                  &mut transformer,
                  decl.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              deno_graph::symbols::SymbolNodeRef::FnDecl(decl) => {
                self.output.add_function(
                  decl.clone(),
                  &mut transformer,
                  decl.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              deno_graph::symbols::SymbolNodeRef::TsEnum(decl) => {
                self.output.add_enum(
                  decl.clone(),
                  &mut transformer,
                  decl.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              deno_graph::symbols::SymbolNodeRef::TsInterface(decl) => {
                self.output.add_interface(
                  decl.clone(),
                  &mut transformer,
                  decl.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              deno_graph::symbols::SymbolNodeRef::TsNamespace(_) => {
                // do a message about how ts namespaces aren't supported at the moment
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsTypeAlias(decl) => {
                self.output.add_type_alias(
                  decl.clone(),
                  &mut transformer,
                  decl.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              deno_graph::symbols::SymbolNodeRef::Var(decl, _, _) => {
                self.output.add_var_decl(
                  decl.clone(),
                  &mut transformer,
                  decl.span(),
                  &top_level_name,
                  &maybe_export_name,
                  module,
                );
              }
              // members
              deno_graph::symbols::SymbolNodeRef::AutoAccessor(_) => todo!(),
              deno_graph::symbols::SymbolNodeRef::ClassMethod(_) => todo!(),
              deno_graph::symbols::SymbolNodeRef::ClassProp(_) => todo!(),
              deno_graph::symbols::SymbolNodeRef::Constructor(_) => todo!(),
              deno_graph::symbols::SymbolNodeRef::TsIndexSignature(_) => {
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsCallSignatureDecl(_) => {
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsConstructSignatureDecl(
                _,
              ) => {
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsPropertySignature(_) => {
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsGetterSignature(_) => {
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsSetterSignature(_) => {
                todo!()
              }
              deno_graph::symbols::SymbolNodeRef::TsMethodSignature(_) => {
                todo!()
              }
            },
            None => todo!(),
          }
        }
        for symbol_id in transformer.found_symbols {
          if self.analyzed_symbols.insert(symbol_id) {
            self.pending_symbols.push_back((None, symbol_id));
          }
        }
      }

      if let Some(export_name) = maybe_export_name {
        if export_name != top_level_name {
          self
            .queued_named_exports
            .push((top_level_name.clone(), export_name));
        }
      }
    }

    if !self.queued_named_exports.is_empty() {
      self.output.final_module.body.push(ModuleItem::ModuleDecl(
        ModuleDecl::ExportNamed(NamedExport {
          span: DUMMY_SP,
          specifiers: self
            .queued_named_exports
            .drain(..)
            .map(|(top_level_name, export_name)| {
              ExportSpecifier::Named(ExportNamedSpecifier {
                span: DUMMY_SP,
                orig: ModuleExportName::Ident(Ident::new(
                  top_level_name.into(),
                  DUMMY_SP,
                )),
                exported: Some(ModuleExportName::Ident(Ident::new(
                  export_name.into(),
                  DUMMY_SP,
                ))),
                is_type_only: false,
              })
            })
            .collect(),
          src: None,
          type_only: false,
          with: None,
        }),
      ));
    }
  }
}

struct DtsTransformer<'a, TReporter: Reporter> {
  reporter: &'a TReporter,
  root_symbol: &'a RootSymbol<'a>,
  symbol: &'a Symbol,
  top_level_symbols: &'a mut TopLevelSymbols,
  graph: &'a ModuleGraph,
  module_info: &'a EsmModuleInfo,
  found_symbols: Vec<UniqueSymbolId>,
}

impl<'a, TReporter: Reporter> DtsTransformer<'a, TReporter> {
  fn has_internal_jsdoc(&self, pos: SourcePos) -> bool {
    has_internal_jsdoc(self.module_info.source(), pos)
  }
}

impl<'a, TReporter: Reporter> VisitMut for DtsTransformer<'a, TReporter> {
  fn visit_mut_auto_accessor(&mut self, n: &mut AutoAccessor) {
    visit_mut_auto_accessor(self, n)
  }

  fn visit_mut_binding_ident(&mut self, n: &mut BindingIdent) {
    visit_mut_binding_ident(self, n)
  }

  fn visit_mut_block_stmt(&mut self, n: &mut BlockStmt) {
    visit_mut_block_stmt(self, n)
  }

  fn visit_mut_block_stmt_or_expr(&mut self, n: &mut BlockStmtOrExpr) {
    visit_mut_block_stmt_or_expr(self, n)
  }

  fn visit_mut_class(&mut self, n: &mut Class) {
    let had_private_prop = n.body.iter().any(|b| {
      matches!(
        b,
        ClassMember::PrivateProp(_) | ClassMember::PrivateMethod(_)
      )
    });
    let mut last_was_overload = false;
    n.body.retain(|member| {
      let is_overload = is_class_member_overload(member);
      let is_implementation_with_overloads = !is_overload && last_was_overload;
      last_was_overload = is_overload;
      let keep = match member {
        ClassMember::Constructor(_)
        | ClassMember::Method(_)
        | ClassMember::ClassProp(_)
        | ClassMember::TsIndexSignature(_) => true,
        ClassMember::PrivateProp(_)
        | ClassMember::PrivateMethod(_)
        | ClassMember::Empty(_)
        | ClassMember::StaticBlock(_) => false,
        ClassMember::AutoAccessor(_) => true,
      };
      keep
        && !self.has_internal_jsdoc(member.start())
        && !is_implementation_with_overloads
    });

    for member in n.body.iter_mut() {
      match member {
        ClassMember::Method(method) => {
          if method.accessibility == Some(Accessibility::Private) {
            *member = ClassMember::ClassProp(ClassProp {
              span: DUMMY_SP,
              key: method.key.clone(),
              value: None,
              type_ann: None,
              is_static: method.is_static,
              decorators: Vec::new(),
              accessibility: Some(Accessibility::Private),
              is_abstract: method.is_abstract,
              is_optional: method.is_optional,
              is_override: method.is_override,
              readonly: false,
              declare: false,
              definite: false,
            });
          }
        }
        ClassMember::ClassProp(prop) => {
          if prop.accessibility == Some(Accessibility::Private) {
            prop.type_ann = None;
          }
        }
        _ => {}
      }
    }

    let mut insert_props = Vec::new();
    if had_private_prop {
      insert_props.push(ClassMember::PrivateProp(PrivateProp {
        span: DUMMY_SP,
        key: PrivateName {
          span: DUMMY_SP,
          id: ident("private".into()),
        },
        value: None,
        type_ann: None,
        is_static: false,
        decorators: Vec::new(),
        accessibility: None,
        is_optional: false,
        is_override: false,
        readonly: false,
        definite: false,
      }))
    }
    for member in &n.body {
      if let ClassMember::Constructor(ctor) = member {
        for param in &ctor.params {
          if let ParamOrTsParamProp::TsParamProp(prop) = param {
            insert_props.push(ClassMember::ClassProp(ClassProp {
              span: DUMMY_SP,
              key: match &prop.param {
                TsParamPropParam::Ident(ident) => {
                  PropName::Ident(ident.id.clone())
                }
                TsParamPropParam::Assign(assign) => match &*assign.left {
                  Pat::Ident(ident) => PropName::Ident(ident.id.clone()),
                  Pat::Array(_) => todo!(),
                  Pat::Rest(_) => todo!(),
                  Pat::Object(_) => todo!(),
                  Pat::Assign(_) => todo!(),
                  Pat::Invalid(_) => todo!(),
                  Pat::Expr(_) => todo!(),
                },
              },
              value: None,
              type_ann: match &prop.param {
                TsParamPropParam::Ident(ident) => ident.type_ann.clone(),
                TsParamPropParam::Assign(assign) => {
                  let explicit_type_ann = match &*assign.left {
                    Pat::Ident(binding_ident) => binding_ident.type_ann.clone(),
                    _ => None,
                  };
                  explicit_type_ann.or_else(|| {
                    maybe_infer_type_from_expr(&*assign.right).map(|type_ann| {
                      Box::new(TsTypeAnn {
                        span: DUMMY_SP,
                        type_ann: Box::new(type_ann),
                      })
                    })
                  })
                }
              },
              is_static: false,
              decorators: Vec::new(),
              accessibility: match prop.accessibility {
                Some(Accessibility::Public) | None => None,
                Some(accessibility) => Some(accessibility),
              },
              is_abstract: false,
              is_optional: false,
              is_override: prop.is_override,
              readonly: prop.readonly,
              declare: false,
              definite: false,
            }))
          }
        }
      }
    }

    n.body.splice(0..0, insert_props);

    visit_mut_class(self, n)
  }

  fn visit_mut_class_decl(&mut self, n: &mut ClassDecl) {
    visit_mut_class_decl(self, n)
  }

  fn visit_mut_class_expr(&mut self, n: &mut ClassExpr) {
    visit_mut_class_expr(self, n)
  }

  fn visit_mut_class_member(&mut self, n: &mut ClassMember) {
    visit_mut_class_member(self, n)
  }

  fn visit_mut_class_method(&mut self, n: &mut ClassMethod) {
    visit_mut_class_method(self, n)
  }

  fn visit_mut_class_prop(&mut self, n: &mut ClassProp) {
    if n.type_ann.is_none() && n.accessibility != Some(Accessibility::Private) {
      let type_ann = n
        .value
        .as_ref()
        .and_then(|value| maybe_infer_type_from_expr(value))
        .unwrap_or_else(|| {
          ts_keyword_type(TsKeywordTypeKind::TsUnknownKeyword)
        });
      n.type_ann = Some(Box::new(TsTypeAnn {
        span: DUMMY_SP,
        type_ann: Box::new(type_ann),
      }))
    }
    n.value = None;
    visit_mut_class_prop(self, n)
  }

  fn visit_mut_computed_prop_name(&mut self, n: &mut ComputedPropName) {
    visit_mut_computed_prop_name(self, n)
  }

  fn visit_mut_constructor(&mut self, n: &mut Constructor) {
    n.body = None;
    for param in &mut n.params {
      match param {
        ParamOrTsParamProp::TsParamProp(param_prop) => {
          // convert to a parameter
          *param = ParamOrTsParamProp::Param(Param {
            span: param_prop.span,
            decorators: Vec::new(),
            pat: match &param_prop.param {
              TsParamPropParam::Ident(ident) => Pat::Ident(ident.clone()),
              TsParamPropParam::Assign(assign) => Pat::Assign(assign.clone()),
            },
          })
        }
        ParamOrTsParamProp::Param(_) => {
          // ignore
        }
      }
    }
    visit_mut_constructor(self, n)
  }

  fn visit_mut_decl(&mut self, n: &mut Decl) {
    visit_mut_decl(self, n)
  }

  fn visit_mut_decorators(&mut self, n: &mut Vec<Decorator>) {
    n.clear();
  }

  fn visit_mut_default_decl(&mut self, n: &mut DefaultDecl) {
    visit_mut_default_decl(self, n)
  }

  fn visit_mut_export_all(&mut self, n: &mut ExportAll) {
    visit_mut_export_all(self, n)
  }

  fn visit_mut_export_decl(&mut self, n: &mut ExportDecl) {
    visit_mut_export_decl(self, n)
  }

  fn visit_mut_export_default_decl(&mut self, n: &mut ExportDefaultDecl) {
    visit_mut_export_default_decl(self, n)
  }

  fn visit_mut_export_default_expr(&mut self, n: &mut ExportDefaultExpr) {
    todo!();
  }

  fn visit_mut_export_default_specifier(
    &mut self,
    n: &mut ExportDefaultSpecifier,
  ) {
    visit_mut_export_default_specifier(self, n)
  }

  fn visit_mut_export_named_specifier(&mut self, n: &mut ExportNamedSpecifier) {
    visit_mut_export_named_specifier(self, n)
  }

  fn visit_mut_export_namespace_specifier(
    &mut self,
    n: &mut ExportNamespaceSpecifier,
  ) {
    visit_mut_export_namespace_specifier(self, n)
  }

  fn visit_mut_export_specifier(&mut self, n: &mut ExportSpecifier) {
    visit_mut_export_specifier(self, n)
  }

  fn visit_mut_export_specifiers(&mut self, n: &mut Vec<ExportSpecifier>) {
    visit_mut_export_specifiers(self, n)
  }

  fn visit_mut_fn_decl(&mut self, n: &mut FnDecl) {
    visit_mut_fn_decl(self, n)
  }

  fn visit_mut_function(&mut self, n: &mut Function) {
    // insert a void type when there's no return type
    if n.return_type.is_none() {
      // todo: this should go into if statements and other things as well
      let has_return_stmt = get_return_stmt_from_function(n).is_some();

      if has_return_stmt {
        let line_and_column = self
          .module_info
          .source()
          .text_info()
          .line_and_column_display(n.start());
        self.reporter.diagnostic(Diagnostic {
          message:
            "Missing explicit return type for function with return statement."
              .to_string(),
          specifier: self.module_info.specifier().clone(),
          line_and_column: Some(line_and_column.into()),
        });
      }

      let return_type =
        Box::new(if has_return_stmt || n.is_generator || n.body.is_none() {
          ts_keyword_type(TsKeywordTypeKind::TsUnknownKeyword)
        } else {
          ts_keyword_type(TsKeywordTypeKind::TsVoidKeyword)
        });
      n.return_type = Some(Box::new(TsTypeAnn {
        span: DUMMY_SP,
        type_ann: if n.is_async {
          Box::new(TsType::TsTypeRef(TsTypeRef {
            span: DUMMY_SP,
            type_name: TsEntityName::Ident(Ident::new(
              "Promise".into(),
              DUMMY_SP,
            )),
            type_params: Some(Box::new(TsTypeParamInstantiation {
              span: DUMMY_SP,
              params: vec![return_type],
            })),
          }))
        } else if n.is_generator {
          Box::new(TsType::TsTypeRef(TsTypeRef {
            span: DUMMY_SP,
            type_name: TsEntityName::Ident(Ident::new(
              "Generator".into(),
              DUMMY_SP,
            )),
            type_params: Some(Box::new(TsTypeParamInstantiation {
              span: DUMMY_SP,
              params: vec![
                Box::new(ts_keyword_type(TsKeywordTypeKind::TsUnknownKeyword)),
                Box::new(ts_keyword_type(TsKeywordTypeKind::TsVoidKeyword)),
                Box::new(ts_keyword_type(TsKeywordTypeKind::TsUnknownKeyword)),
              ],
            })),
          }))
        } else {
          return_type
        },
      }));
    }
    n.body = None;
    n.is_async = false;
    n.is_generator = false;
    visit_mut_function(self, n)
  }

  fn visit_mut_getter_prop(&mut self, n: &mut GetterProp) {
    visit_mut_getter_prop(self, n)
  }

  fn visit_mut_ident(&mut self, n: &mut Ident) {
    // todo: get top level mark and don't rely on this
    if n.span.ctxt.as_u32() > 0 {
      let id = n.to_id();
      let entries = self.root_symbol.resolve_symbol_dep(
        ModuleInfoRef::Esm(self.module_info),
        self.symbol,
        &SymbolNodeDep::Id(id),
      );
      let paths = entries
        .into_iter()
        .filter_map(|entry| match entry {
          ResolvedSymbolDepEntry::Path(path) => Some(path),
          _ => None,
        })
        .collect::<Vec<_>>();
      if let Some(symbol_or_remote_dep) =
        resolve_paths_to_remote_path(self.root_symbol, paths)
      {
        match symbol_or_remote_dep {
          SymbolOrRemoteDep::Symbol(symbol_id) => {
            self.found_symbols.push(symbol_id);
            let module = self
              .root_symbol
              .get_module_from_id(symbol_id.module_id)
              .unwrap();
            let symbol = module.symbol(symbol_id.symbol_id).unwrap();
            let name = self.top_level_symbols.ensure_top_level_name(symbol);
            n.sym = name.into();
            n.span = DUMMY_SP;
          }
          SymbolOrRemoteDep::RemoteDepName { .. } => todo!(),
        }
      } else {
        todo!();
      }
    }

    visit_mut_ident(self, n)
  }

  fn visit_mut_import(&mut self, n: &mut Import) {
    visit_mut_import(self, n)
  }

  fn visit_mut_import_decl(&mut self, n: &mut ImportDecl) {
    visit_mut_import_decl(self, n)
  }

  fn visit_mut_import_default_specifier(
    &mut self,
    n: &mut ImportDefaultSpecifier,
  ) {
    visit_mut_import_default_specifier(self, n)
  }

  fn visit_mut_import_named_specifier(&mut self, n: &mut ImportNamedSpecifier) {
    visit_mut_import_named_specifier(self, n)
  }

  fn visit_mut_import_specifier(&mut self, n: &mut ImportSpecifier) {
    visit_mut_import_specifier(self, n)
  }

  fn visit_mut_import_specifiers(&mut self, n: &mut Vec<ImportSpecifier>) {
    visit_mut_import_specifiers(self, n)
  }

  fn visit_mut_import_star_as_specifier(
    &mut self,
    n: &mut ImportStarAsSpecifier,
  ) {
    visit_mut_import_star_as_specifier(self, n)
  }

  fn visit_mut_key(&mut self, n: &mut Key) {
    visit_mut_key(self, n)
  }

  fn visit_mut_key_value_pat_prop(&mut self, n: &mut KeyValuePatProp) {
    visit_mut_key_value_pat_prop(self, n)
  }

  fn visit_mut_key_value_prop(&mut self, n: &mut KeyValueProp) {
    visit_mut_key_value_prop(self, n)
  }

  fn visit_mut_method_prop(&mut self, n: &mut MethodProp) {
    visit_mut_method_prop(self, n)
  }

  fn visit_mut_module(&mut self, n: &mut Module) {
    todo!();
  }

  fn visit_mut_module_decl(&mut self, n: &mut ModuleDecl) {
    visit_mut_module_decl(self, n)
  }

  fn visit_mut_module_export_name(&mut self, n: &mut ModuleExportName) {
    visit_mut_module_export_name(self, n)
  }

  fn visit_mut_module_item(&mut self, n: &mut ModuleItem) {
    visit_mut_module_item(self, n)
  }

  fn visit_mut_module_items(&mut self, n: &mut Vec<ModuleItem>) {
    todo!();
  }

  fn visit_mut_named_export(&mut self, n: &mut NamedExport) {
    unreachable!();
  }

  fn visit_mut_opt_module_export_name(
    &mut self,
    n: &mut Option<ModuleExportName>,
  ) {
    visit_mut_opt_module_export_name(self, n)
  }

  fn visit_mut_opt_module_items(&mut self, n: &mut Option<Vec<ModuleItem>>) {
    visit_mut_opt_module_items(self, n)
  }

  fn visit_mut_param(&mut self, n: &mut Param) {
    visit_mut_param(self, n)
  }

  fn visit_mut_param_or_ts_param_prop(&mut self, n: &mut ParamOrTsParamProp) {
    visit_mut_param_or_ts_param_prop(self, n)
  }

  fn visit_mut_param_or_ts_param_props(
    &mut self,
    n: &mut Vec<ParamOrTsParamProp>,
  ) {
    visit_mut_param_or_ts_param_props(self, n)
  }

  fn visit_mut_params(&mut self, n: &mut Vec<Param>) {
    visit_mut_params(self, n)
  }

  fn visit_mut_program(&mut self, n: &mut Program) {
    visit_mut_program(self, n)
  }

  fn visit_mut_prop(&mut self, n: &mut Prop) {
    visit_mut_prop(self, n)
  }

  fn visit_mut_prop_name(&mut self, n: &mut PropName) {
    visit_mut_prop_name(self, n)
  }

  fn visit_mut_prop_or_spread(&mut self, n: &mut PropOrSpread) {
    visit_mut_prop_or_spread(self, n)
  }

  fn visit_mut_prop_or_spreads(&mut self, n: &mut Vec<PropOrSpread>) {
    visit_mut_prop_or_spreads(self, n)
  }

  fn visit_mut_setter_prop(&mut self, n: &mut SetterProp) {
    visit_mut_setter_prop(self, n)
  }

  fn visit_mut_static_block(&mut self, n: &mut StaticBlock) {
    visit_mut_static_block(self, n)
  }

  fn visit_mut_stmt(&mut self, n: &mut Stmt) {
    visit_mut_stmt(self, n)
  }

  fn visit_mut_stmts(&mut self, n: &mut Vec<Stmt>) {
    visit_mut_stmts(self, n)
  }

  fn visit_mut_ts_entity_name(&mut self, n: &mut TsEntityName) {
    visit_mut_ts_entity_name(self, n)
  }

  fn visit_mut_ts_enum_decl(&mut self, n: &mut TsEnumDecl) {
    visit_mut_ts_enum_decl(self, n)
  }

  fn visit_mut_ts_enum_member(&mut self, n: &mut TsEnumMember) {
    visit_mut_ts_enum_member(self, n)
  }

  fn visit_mut_ts_enum_member_id(&mut self, n: &mut TsEnumMemberId) {
    visit_mut_ts_enum_member_id(self, n)
  }

  fn visit_mut_ts_enum_members(&mut self, n: &mut Vec<TsEnumMember>) {
    visit_mut_ts_enum_members(self, n)
  }

  fn visit_mut_ts_export_assignment(&mut self, n: &mut TsExportAssignment) {
    visit_mut_ts_export_assignment(self, n)
  }

  fn visit_mut_ts_external_module_ref(&mut self, n: &mut TsExternalModuleRef) {
    visit_mut_ts_external_module_ref(self, n)
  }

  fn visit_mut_var_decl(&mut self, n: &mut VarDecl) {
    visit_mut_var_decl(self, n)
  }

  fn visit_mut_var_decl_kind(&mut self, n: &mut VarDeclKind) {
    visit_mut_var_decl_kind(self, n)
  }

  fn visit_mut_var_decl_or_expr(&mut self, n: &mut VarDeclOrExpr) {
    visit_mut_var_decl_or_expr(self, n)
  }

  fn visit_mut_for_head(&mut self, n: &mut ForHead) {
    visit_mut_for_head(self, n)
  }

  fn visit_mut_var_declarator(&mut self, n: &mut VarDeclarator) {
    n.definite = false;
    n.init = None;
    visit_mut_var_declarator(self, n)
  }

  fn visit_mut_var_declarators(&mut self, n: &mut Vec<VarDeclarator>) {
    visit_mut_var_declarators(self, n)
  }

  fn visit_mut_pat(&mut self, n: &mut Pat) {
    fn pat_type_ann(pat: &Pat) -> Option<Box<TsTypeAnn>> {
      match pat {
        Pat::Ident(left) => left.type_ann.clone(),
        Pat::Array(left) => left.type_ann.clone(),
        Pat::Rest(left) => left.type_ann.clone(),
        Pat::Object(left) => left.type_ann.clone(),
        Pat::Assign(left) => pat_type_ann(&left.left),
        Pat::Invalid(_) | Pat::Expr(_) => None,
      }
    }

    match &n {
      Pat::Assign(assign) => {
        let type_ann = pat_type_ann(&assign.left).clone().or_else(|| {
          maybe_infer_type_from_expr(&*assign.right).map(|type_ann| {
            Box::new(TsTypeAnn {
              span: DUMMY_SP,
              type_ann: Box::new(type_ann),
            })
          })
        });
        match &*assign.left {
          Pat::Ident(name) => {
            *n = Pat::Ident(BindingIdent {
              id: Ident {
                span: DUMMY_SP,
                sym: name.sym.to_string().into(),
                optional: true,
              },
              type_ann,
            });
          }
          Pat::Object(obj) => {
            *n = Pat::Object(ObjectPat {
              span: DUMMY_SP,
              optional: true,
              type_ann,
              props: obj.props.clone(),
            });
          }
          _ => {}
        }
      }
      _ => {}
    }

    visit_mut_pat(self, n)
  }

  fn visit_mut_object_pat(&mut self, n: &mut ObjectPat) {
    for prop in &mut n.props {
      match prop {
        ObjectPatProp::KeyValue(kv) => {
          *prop = ObjectPatProp::Assign(AssignPatProp {
            span: kv.span(),
            key: match &kv.key {
              PropName::Ident(ident) => ident.clone(),
              PropName::Str(_)
              | PropName::Num(_)
              | PropName::Computed(_)
              | PropName::BigInt(_) => todo!("Non ident prop name"),
            },
            value: None,
          });
        }
        ObjectPatProp::Assign(_) | ObjectPatProp::Rest(_) => {}
      }
    }
    visit_mut_object_pat(self, n)
  }

  fn visit_mut_assign_pat_prop(&mut self, n: &mut AssignPatProp) {
    n.value = None;
    visit_mut_assign_pat_prop(self, n)
  }
}

fn maybe_infer_type_from_lit(lit: &Lit) -> Option<TsType> {
  let keyword = match lit {
    Lit::Str(_) => Some(TsKeywordTypeKind::TsStringKeyword),
    Lit::Bool(_) => Some(TsKeywordTypeKind::TsBooleanKeyword),
    Lit::Null(_) => Some(TsKeywordTypeKind::TsNullKeyword),
    Lit::Num(_) => Some(TsKeywordTypeKind::TsNumberKeyword),
    Lit::BigInt(_) => Some(TsKeywordTypeKind::TsBigIntKeyword),
    Lit::Regex(_) => None,
    Lit::JSXText(_) => None,
  };
  keyword.map(|kind| {
    TsType::TsKeywordType(TsKeywordType {
      span: DUMMY_SP,
      kind,
    })
  })
}

fn maybe_infer_type_from_expr(expr: &Expr) -> Option<TsType> {
  match expr {
    Expr::TsTypeAssertion(n) => Some(*n.type_ann.clone()),
    Expr::TsAs(n) => Some(*n.type_ann.clone()),
    Expr::Lit(lit) => maybe_infer_type_from_lit(lit),
    Expr::This(_)
    | Expr::Array(_)
    | Expr::Object(_)
    | Expr::Fn(_)
    | Expr::Unary(_)
    | Expr::Update(_)
    | Expr::Bin(_)
    | Expr::Assign(_)
    | Expr::Member(_)
    | Expr::SuperProp(_)
    | Expr::Cond(_)
    | Expr::Call(_)
    | Expr::New(_)
    | Expr::Seq(_)
    | Expr::Ident(_)
    | Expr::Tpl(_)
    | Expr::TaggedTpl(_)
    | Expr::Arrow(_)
    | Expr::Class(_)
    | Expr::Yield(_)
    | Expr::MetaProp(_)
    | Expr::Await(_)
    | Expr::Paren(_)
    | Expr::JSXMember(_)
    | Expr::JSXNamespacedName(_)
    | Expr::JSXEmpty(_)
    | Expr::JSXElement(_)
    | Expr::JSXFragment(_)
    | Expr::TsConstAssertion(_)
    | Expr::TsNonNull(_)
    | Expr::TsInstantiation(_)
    | Expr::TsSatisfies(_)
    | Expr::PrivateName(_)
    | Expr::OptChain(_)
    | Expr::Invalid(_) => None,
  }
}

fn get_return_stmt_from_function<'a>(
  func: &'a Function,
) -> Option<&'a ReturnStmt> {
  let body = func.body.as_ref()?;
  get_return_stmt_from_stmts(&body.stmts)
}

fn get_return_stmt_from_stmts<'a>(stmts: &'a [Stmt]) -> Option<&'a ReturnStmt> {
  for stmt in stmts {
    if let Some(return_stmt) = get_return_stmt_from_stmt(stmt) {
      return Some(return_stmt);
    }
  }

  None
}

fn get_return_stmt_from_stmt<'a>(stmt: &'a Stmt) -> Option<&'a ReturnStmt> {
  match stmt {
    Stmt::Block(n) => get_return_stmt_from_stmts(&n.stmts),
    Stmt::With(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::Return(n) => Some(n),
    Stmt::Labeled(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::If(n) => get_return_stmt_from_stmt(&n.cons),
    Stmt::Switch(n) => n
      .cases
      .iter()
      .find_map(|case| get_return_stmt_from_stmts(&case.cons)),
    Stmt::Try(n) => get_return_stmt_from_stmts(&n.block.stmts)
      .or_else(|| {
        n.handler
          .as_ref()
          .and_then(|h| get_return_stmt_from_stmts(&h.body.stmts))
      })
      .or_else(|| {
        n.finalizer
          .as_ref()
          .and_then(|f| get_return_stmt_from_stmts(&f.stmts))
      }),
    Stmt::While(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::DoWhile(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::For(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::ForIn(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::ForOf(n) => get_return_stmt_from_stmt(&n.body),
    Stmt::Break(_)
    | Stmt::Continue(_)
    | Stmt::Throw(_)
    | Stmt::Debugger(_)
    | Stmt::Decl(_)
    | Stmt::Expr(_)
    | Stmt::Empty(_) => None,
  }
}

fn is_class_member_overload(member: &ClassMember) -> bool {
  match member {
    ClassMember::Constructor(ctor) => ctor.body.is_none(),
    ClassMember::Method(method) => method.function.body.is_none(),
    ClassMember::PrivateMethod(method) => method.function.body.is_none(),
    ClassMember::ClassProp(_)
    | ClassMember::PrivateProp(_)
    | ClassMember::TsIndexSignature(_)
    | ClassMember::AutoAccessor(_)
    | ClassMember::StaticBlock(_)
    | ClassMember::Empty(_) => false,
  }
}

fn has_internal_jsdoc(source: &ParsedSource, pos: SourcePos) -> bool {
  if let Some(comments) = source.comments().get_leading(pos) {
    comments.iter().any(|c| {
      c.kind == CommentKind::Block
        && c.text.starts_with("*")
        && c.text.contains("@internal")
    })
  } else {
    false
  }
}
