use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;

use deno_ast::swc::ast::Id;
use deno_ast::swc::ast::*;
use deno_ast::swc::common::comments::CommentKind;
use deno_ast::swc::common::comments::SingleThreadedComments;
use deno_ast::swc::common::util::take::Take;
use deno_ast::swc::common::FileName;
use deno_ast::swc::common::Mark;
use deno_ast::swc::common::SourceMap;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::visit::*;
use deno_ast::Diagnostic;
use deno_ast::EmitOptions;
use deno_ast::ModuleSpecifier;
use deno_graph::CapturingModuleParser;
use deno_graph::EsmModule;
use deno_graph::ModuleGraph;
use deno_graph::ModuleParser;
use deno_graph::WalkOptions;

use crate::helpers::adjust_spans;
use crate::helpers::const_var_decl;
use crate::helpers::export_x_as_y;
use crate::helpers::fill_leading_comments;
use crate::helpers::fill_trailing_comments;
use crate::helpers::ident;
use crate::helpers::member_x_y;
use crate::helpers::object_define_property;
use crate::helpers::print_program;

#[derive(Default)]
struct ModuleDataCollection {
  // todo: pre-allocate when upgrading deno_graph
  module_data: HashMap<ModuleSpecifier, ModuleData>,
}

impl ModuleDataCollection {
  pub fn get(&self, specifier: &ModuleSpecifier) -> Option<&ModuleData> {
    self.module_data.get(specifier)
  }

  pub fn get_mut(&mut self, specifier: &ModuleSpecifier) -> &mut ModuleData {
    let next_id = self.module_data.len();
    self
      .module_data
      .entry(specifier.clone())
      .or_insert_with(|| ModuleData {
        id: ModuleId(next_id),
        module: None,
        has_tla: false,
        exports: Default::default(),
        re_exports: Default::default(),
      })
  }

  pub fn get_export_names(&self, specifier: &ModuleSpecifier) -> Vec<String> {
    fn inner<'a>(
      collection: &'a ModuleDataCollection,
      specifier: &'a ModuleSpecifier,
      seen: &mut HashSet<&'a ModuleSpecifier>,
      result: &mut HashSet<&'a String>,
    ) {
      if seen.insert(specifier) {
        if let Some(module_data) = collection.module_data.get(&specifier) {
          result.extend(module_data.exports.iter().map(|e| e.export_name()));
          for re_export in &module_data.re_exports {
            match &re_export.name {
              ReExportName::Named(name) => {
                result.insert(name.export_name());
              }
              ReExportName::Namespace(namespace) => {
                result.insert(namespace);
              }
              ReExportName::All => {
                inner(collection, &re_export.specifier, seen, result);
              }
            }
          }
        }
      }
    }

    let mut result = HashSet::new();
    inner(self, specifier, &mut HashSet::new(), &mut result);
    let mut result = result
      .into_iter()
      .map(ToOwned::to_owned)
      .collect::<Vec<_>>();
    result.sort_unstable();
    result
  }
}

#[derive(Debug, Clone, Copy)]
struct ModuleId(usize);

impl ModuleId {
  pub fn to_code_string(&self) -> String {
    format!("pack{}", self.0)
  }
}

struct ExportName {
  // todo: I think these could all be &str
  local_name: String,
  export_name: Option<String>,
}

impl ExportName {
  pub fn export_name(&self) -> &String {
    self.export_name.as_ref().unwrap_or(&self.local_name)
  }
}

enum ReExportName {
  Named(ExportName),
  Namespace(String),
  All,
}

struct ReExport {
  name: ReExportName,
  specifier: ModuleSpecifier,
  module_id: ModuleId,
}

struct ModuleData {
  id: ModuleId,
  has_tla: bool,
  exports: Vec<ExportName>,
  re_exports: Vec<ReExport>,
  module: Option<Module>,
}

impl ModuleData {
  pub fn add_export_name(&mut self, name: String) {
    self.exports.push(ExportName {
      local_name: name,
      export_name: None,
    })
  }
}

struct Context<'a> {
  graph: &'a ModuleGraph,
  parser: &'a CapturingModuleParser<'a>,
  module_data: ModuleDataCollection,
}

pub struct PackOptions {
  /// If the packing should include remote modules or leave
  /// them as external.
  pub include_remote: bool,
}

pub fn pack(
  graph: &ModuleGraph,
  parser: &CapturingModuleParser,
  options: PackOptions,
) -> Result<String, anyhow::Error> {
  // TODO
  // - dynamic imports
  // - tla
  // - order modules properly (https://v8.dev/features/top-level-await#module-execution-order)
  // - keep remote the same

  let roots = &graph.roots;
  assert_eq!(roots.len(), 1);
  let mut context = Context {
    graph,
    parser,
    module_data: ModuleDataCollection::default(),
  };

  // todo: this is not correct. It should output by walking the graph
  // in the order that the loader does
  let mut remote_specifiers: Vec<(&ModuleSpecifier, &deno_graph::Module)> =
    Default::default();
  let mut local_specifiers: Vec<(&ModuleSpecifier, &deno_graph::Module)> =
    Default::default();

  let mut modules = graph.walk(
    roots,
    WalkOptions {
      check_js: true,
      follow_dynamic: true,
      follow_type_only: true,
    },
  );
  while let Some((specifier, _)) = modules.next() {
    let is_file = specifier.scheme() == "file";
    if !options.include_remote && !is_file {
      // don't analyze any dependenices of remote modules
      modules.skip_previous_dependencies();
    }
    let module = graph.get(specifier).unwrap();
    let specifier = module.specifier();
    if is_file {
      local_specifiers.push((specifier, module));
    } else {
      remote_specifiers.push((specifier, module));
    }
    match module {
      deno_graph::Module::Esm(esm) => {
        if options.include_remote || is_file {
          analyze_esm_module(esm, &mut context)?;
        }
      }
      deno_graph::Module::Json(_) => {}
      _ => {
        todo!("json modules");
      }
    }
  }

  let root_dir = get_root_dir(local_specifiers.iter().map(|(s, _)| *s));
  let global_comments = SingleThreadedComments::default();
  let source_map = Rc::new(SourceMap::default());
  let mut final_module = Module {
    span: DUMMY_SP,
    body: vec![],
    shebang: None,
  };
  let mut final_text = String::new();
  for (specifier, module) in
    remote_specifiers.iter().chain(local_specifiers.iter())
  {
    if specifier.scheme() != "file" {
      let module_data = context.module_data.get_mut(specifier);
      final_module
        .body
        .push(ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
          span: DUMMY_SP,
          specifiers: Vec::from([ImportSpecifier::Namespace(
            ImportStarAsSpecifier {
              span: DUMMY_SP,
              local: ident(module_data.id.to_code_string()),
            },
          )]),
          src: Box::new(Str {
            span: DUMMY_SP,
            value: specifier.to_string().into(),
            raw: None,
          }),
          type_only: false,
          asserts: None,
        })));
    } else {
      if let deno_graph::Module::Esm(_) = module {
        let export_names = context.module_data.get_export_names(specifier);
        let module_data = context.module_data.get_mut(specifier);
        if export_names.is_empty() || context.graph.roots[0] == **specifier {
          continue;
        }
        final_module
          .body
          .push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(
            const_var_decl(
              module_data.id.to_code_string(),
              Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props: export_names
                  .into_iter()
                  .map(|name| {
                    PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                      key: ident(name).into(),
                      value: ident("undefined".to_string()).into(),
                    })))
                  })
                  .collect(),
              }),
            ),
          )))));
      } else if let deno_graph::Module::Json(json) = module {
        let module_data = context.module_data.get_mut(specifier);
        let displayed_specifier = match root_dir {
          Some(prefix) => {
            if specifier.scheme() == "file" {
              let specifier = specifier.as_str();
              specifier.strip_prefix(prefix).unwrap_or(specifier)
            } else {
              specifier.as_str()
            }
          }
          None => specifier.as_str(),
        };
        // todo: use swc here too
        final_text.push_str(&format!(
          "// {}\nconst {} = {{\n  default: {}\n}};\n",
          displayed_specifier,
          module_data.id.to_code_string(),
          json.source.trim()
        ));
      }
    }
  }

  let globals = deno_ast::swc::common::Globals::new();
  deno_ast::swc::common::GLOBALS.set(&globals, || {
    for (specifier, module) in remote_specifiers
      .iter()
      .rev()
      .chain(local_specifiers.iter().rev())
    {
      if !options.include_remote && specifier.scheme() != "file" {
        continue;
      }

      if let deno_graph::Module::Esm(esm) = module {
        let source = &esm.source;
        // eprintln!("PACKING: {}", specifier);
        let module = {
          let parsed_source = context.parser.parse_module(
            &esm.specifier,
            esm.source.clone(),
            esm.media_type,
          )?;
          // todo: do a single transpile for everything
          let module_data = context.module_data.get_mut(specifier);
          let mut module = module_data.module.take().unwrap();
          let top_level_mark = Mark::fresh(Mark::root());
          let source_file = source_map.new_source_file(
            FileName::Url(esm.specifier.clone()),
            source.to_string(),
          );
          adjust_spans(source_file.start_pos, &mut module);
          fill_leading_comments(
            source_file.start_pos,
            &parsed_source,
            &global_comments,
            // remove any jsdoc comments from the js output as they will
            // appear in the dts output
            |c| c.kind != CommentKind::Block || !c.text.starts_with("*"),
          );
          fill_trailing_comments(
            source_file.start_pos,
            &parsed_source,
            &global_comments,
          );
          let program = deno_ast::fold_program(
            Program::Module(module),
            &EmitOptions::default(),
            source_map.clone(),
            &global_comments,
            top_level_mark,
            parsed_source.diagnostics(),
          )?;
          match program {
            Program::Module(module) => module,
            Program::Script(_) => unreachable!(),
          }
        };
        let module_data = context.module_data.get(specifier).unwrap();
        if !module.body.is_empty()
          || !module_data.exports.is_empty()
          || !module_data.re_exports.is_empty()
        {
          let displayed_specifier = match root_dir {
            Some(prefix) => {
              if specifier.scheme() == "file" {
                let specifier = specifier.as_str();
                specifier.strip_prefix(prefix).unwrap_or(specifier)
              } else {
                specifier.as_str()
              }
            }
            None => specifier.as_str(),
          };
          let specifier_id = displayed_specifier
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect::<String>();
          if *specifier == &roots[0] {
            final_module.body.extend(module.body);

            // re-exports
            let mut export_names = HashSet::with_capacity(
              module_data.exports.len() + module_data.re_exports.len(),
            );
            for export in &module_data.exports {
              export_names.insert(export.export_name());
            }
            // todo: lots of code duplication below
            let mut re_export_index = 0;
            for re_export in &module_data.re_exports {
              match &re_export.name {
                ReExportName::Named(name) => {
                  re_export_index += 1;
                  let temp_name = format!("_packReExport{}", re_export_index);
                  final_module.body.push(ModuleItem::Stmt(Stmt::Decl(
                    Decl::Var(Box::new(const_var_decl(
                      temp_name.clone(),
                      Expr::Member(MemberExpr {
                        span: DUMMY_SP,
                        obj: ident(re_export.module_id.to_code_string()).into(),
                        prop: ident(name.local_name.clone()).into(),
                      }),
                    ))),
                  )));
                  final_module.body.push(export_x_as_y(
                    temp_name,
                    name.export_name().to_string(),
                  ));
                  export_names.insert(name.export_name());
                }
                ReExportName::Namespace(name) => {
                  re_export_index += 1;
                  let temp_name = format!("_packReExport{}", re_export_index);
                  final_module.body.push(
                    const_var_decl(
                      temp_name.clone(),
                      Expr::Ident(ident(re_export.module_id.to_code_string())),
                    )
                    .into(),
                  );
                  final_module
                    .body
                    .push(export_x_as_y(temp_name, name.to_string()));
                  export_names.insert(name);
                }
                ReExportName::All => {
                  // handle these when all done
                }
              }
            }
            for re_export in &module_data.re_exports {
              if matches!(re_export.name, ReExportName::All) {
                let re_export_names =
                  context.module_data.get_export_names(&re_export.specifier);
                for name in &re_export_names {
                  if !export_names.contains(&name) {
                    re_export_index += 1;
                    let temp_name = format!("_packReExport{}", re_export_index);
                    final_module.body.push(
                      const_var_decl(
                        temp_name.clone(),
                        Expr::Member(member_x_y(
                          re_export.module_id.to_code_string(),
                          name.to_string(),
                        )),
                      )
                      .into(),
                    );
                    final_module
                      .body
                      .push(export_x_as_y(temp_name, name.to_string()));
                  }
                }
              }
            }
          } else {
            let mut stmts = module
              .body
              .into_iter()
              .map(|item| match item {
                ModuleItem::ModuleDecl(_) => unreachable!(),
                ModuleItem::Stmt(stmt) => stmt,
              })
              .collect::<Vec<_>>();
            let code_string = module_data.id.to_code_string();
            let mut export_names = HashSet::with_capacity(
              module_data.exports.len() + module_data.re_exports.len(),
            );
            for export in &module_data.exports {
              stmts.push(object_define_property(
                code_string.clone(),
                export.export_name().to_string(),
                ident(export.local_name.clone()).into(),
              ));
              export_names.insert(export.export_name());
            }
            for re_export in &module_data.re_exports {
              match &re_export.name {
                ReExportName::Named(name) => {
                  stmts.push(object_define_property(
                    code_string.clone(),
                    name.export_name().to_string(),
                    member_x_y(
                      re_export.module_id.to_code_string(),
                      name.local_name.to_string(),
                    )
                    .into(),
                  ));
                  export_names.insert(name.export_name());
                }
                ReExportName::Namespace(name) => {
                  stmts.push(object_define_property(
                    code_string.clone(),
                    name.to_string(),
                    ident(re_export.module_id.to_code_string()).into(),
                  ));
                  export_names.insert(name);
                }
                ReExportName::All => {
                  // handle these when all done
                }
              }
            }
            for re_export in &module_data.re_exports {
              if matches!(re_export.name, ReExportName::All) {
                let re_export_names =
                  context.module_data.get_export_names(&re_export.specifier);
                for name in &re_export_names {
                  if !export_names.contains(&name) {
                    stmts.push(object_define_property(
                      code_string.clone(),
                      name.clone(),
                      member_x_y(
                        re_export.module_id.to_code_string(),
                        name.clone(),
                      )
                      .into(),
                    ));
                  }
                }
              }
            }
            let fn_expr = FnExpr {
              ident: Some(ident(specifier_id)),
              function: Box::new(Function {
                params: Vec::new(),
                decorators: Vec::new(),
                span: DUMMY_SP,
                body: Some(BlockStmt {
                  span: DUMMY_SP,
                  stmts,
                }),
                is_generator: false,
                is_async: module_data.has_tla,
                type_params: None,
                return_type: None,
              }),
            };
            let iife = Expr::Call(CallExpr {
              span: DUMMY_SP,
              callee: Callee::Expr(Box::new(Expr::Paren(ParenExpr {
                span: DUMMY_SP,
                expr: Box::new(Expr::Fn(fn_expr)),
              }))),
              args: vec![],
              type_args: None,
            });
            let expr = if module_data.has_tla {
              Expr::Await(AwaitExpr {
                span: DUMMY_SP,
                arg: Box::new(iife),
              })
            } else {
              iife
            };
            final_module
              .body
              .push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: expr.into(),
              })));
          }
        }
      }
    }
    Result::<(), anyhow::Error>::Ok(())
  })?;

  final_text.push_str(&print_program(
    &final_module,
    &source_map,
    &global_comments,
  )?);

  Ok(final_text)
}

struct HasAwaitKeywordVisitor {
  found: bool,
}

impl HasAwaitKeywordVisitor {
  fn has_await_keyword(node: &impl VisitWith<HasAwaitKeywordVisitor>) -> bool {
    let mut visitor = HasAwaitKeywordVisitor { found: false };
    node.visit_with(&mut visitor);
    visitor.found
  }
}

impl Visit for HasAwaitKeywordVisitor {
  fn visit_function(&mut self, _n: &Function) {
    // stop
  }

  fn visit_arrow_expr(&mut self, _n: &ArrowExpr) {
    // stop
  }

  fn visit_class_method(&mut self, _n: &ClassMethod) {
    // stop
  }

  fn visit_await_expr(&mut self, _n: &AwaitExpr) {
    self.found = true;
  }
}

fn analyze_esm_module(
  esm: &EsmModule,
  context: &mut Context,
) -> Result<(), Diagnostic> {
  let module_specifier = &esm.specifier;
  let parsed_source = context.parser.parse_module(
    module_specifier,
    esm.source.clone(),
    esm.media_type,
  )?;
  let is_root_module = context.graph.roots[0] == *module_specifier;
  let mut module = (*parsed_source.module()).clone();

  let mut replace_ids = HashMap::new();
  let mut found_tla = false;
  // analyze the top level declarations
  for module_item in &module.body {
    match module_item {
      ModuleItem::Stmt(stmt) => {
        if !found_tla && HasAwaitKeywordVisitor::has_await_keyword(stmt) {
          found_tla = true;
        }
      }
      ModuleItem::ModuleDecl(decl) => match decl {
        ModuleDecl::Import(import) => {
          if import.type_only {
            continue;
          }

          let value: &str = &import.src.value;
          match context
            .graph
            .resolve_dependency(value, module_specifier, false)
          {
            Some(dep_specifier) => {
              let dep_module_id =
                context.module_data.get_mut(&dep_specifier).id;
              for import_specifier in &import.specifiers {
                match import_specifier {
                  ImportSpecifier::Default(default_specifier) => {
                    replace_ids.insert(
                      default_specifier.local.to_id(),
                      vec![
                        dep_module_id.to_code_string(),
                        "default".to_string(),
                      ],
                    );
                  }
                  ImportSpecifier::Namespace(namespace_specifier) => {
                    replace_ids.insert(
                      namespace_specifier.local.to_id(),
                      vec![dep_module_id.to_code_string()],
                    );
                  }
                  ImportSpecifier::Named(named_specifier) => {
                    if !named_specifier.is_type_only {
                      replace_ids.insert(
                        named_specifier.local.to_id(),
                        vec![
                          dep_module_id.to_code_string(),
                          named_specifier
                            .imported
                            .as_ref()
                            .map(|i| match i {
                              ModuleExportName::Str(_) => todo!(),
                              ModuleExportName::Ident(ident) => {
                                ident.sym.to_string()
                              }
                            })
                            .unwrap_or_else(|| {
                              named_specifier.local.sym.to_string()
                            }),
                        ],
                      );
                    }
                  }
                }
              }
            }
            None => {
              todo!();
            }
          }
        }
        ModuleDecl::ExportDefaultDecl(_)
        | ModuleDecl::ExportDefaultExpr(_)
        | ModuleDecl::ExportNamed(_)
        | ModuleDecl::ExportAll(_)
        | ModuleDecl::TsImportEquals(_)
        | ModuleDecl::TsExportAssignment(_)
        | ModuleDecl::TsNamespaceExport(_) => {}
        ModuleDecl::ExportDecl(decl) => {
          if !found_tla && HasAwaitKeywordVisitor::has_await_keyword(decl) {
            found_tla = true;
          }
        }
      },
    }
  }

  {
    let module_data = context.module_data.get_mut(module_specifier);
    module_data.has_tla = found_tla;
  }

  // analyze the exports separately after because they rely on knowing
  // the imports regardless of order
  for module_item in &module.body {
    match module_item {
      ModuleItem::Stmt(_) => {}
      ModuleItem::ModuleDecl(decl) => match decl {
        ModuleDecl::Import(_) => {
          continue;
        }
        ModuleDecl::ExportDefaultDecl(decl) => {
          if is_root_module {
            continue;
          }
          let maybe_ident = match &decl.decl {
            DefaultDecl::Class(decl) => decl.ident.as_ref(),
            DefaultDecl::Fn(decl) => decl.ident.as_ref(),
            DefaultDecl::TsInterfaceDecl(_) => continue,
          };
          match maybe_ident {
            Some(ident) => {
              context.module_data.get_mut(module_specifier).exports.push(
                ExportName {
                  export_name: Some("default".to_string()),
                  local_name: replace_ids
                    .get(&ident.to_id())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| vec![ident.sym.to_string()])
                    .join("."),
                },
              );
            }
            None => {
              context.module_data.get_mut(module_specifier).exports.push(
                ExportName {
                  export_name: Some("default".to_string()),
                  local_name: "__pack_default__".to_string(),
                },
              );
            }
          }
        }
        ModuleDecl::ExportDefaultExpr(_) => {
          context.module_data.get_mut(module_specifier).exports.push(
            ExportName {
              export_name: Some("default".to_string()),
              local_name: "__pack_default__".to_string(),
            },
          );
        }
        ModuleDecl::ExportDecl(decl) => {
          if is_root_module {
            continue;
          }
          match &decl.decl {
            Decl::Class(decl) => {
              if decl.declare {
                continue;
              }
              context.module_data.get_mut(module_specifier).exports.push(
                ExportName {
                  export_name: None,
                  local_name: decl.ident.sym.to_string(),
                },
              );
            }
            Decl::Using(_) => {}
            Decl::Fn(decl) => {
              if decl.declare {
                continue;
              }
              context.module_data.get_mut(module_specifier).exports.push(
                ExportName {
                  export_name: None,
                  local_name: decl.ident.sym.to_string(),
                },
              );
            }
            Decl::Var(decl) => {
              if decl.declare {
                continue;
              }
              let module_data = context.module_data.get_mut(module_specifier);
              for decl in &decl.decls {
                match &decl.name {
                  Pat::Array(_) => todo!("array destructuring"),
                  Pat::Assign(_) => todo!("var assignment"),
                  Pat::Ident(ident) => {
                    module_data.add_export_name(ident.id.sym.to_string());
                  }
                  Pat::Rest(_) => todo!("spread in var decl"),
                  Pat::Object(obj) => {
                    for prop in &obj.props {
                      match prop {
                        ObjectPatProp::KeyValue(kv) => {
                          match &kv.key {
                            PropName::Ident(ident) => {
                              module_data
                                .add_export_name(ident.sym.to_string());
                            }
                            PropName::Str(_) => todo!(),
                            PropName::Computed(_)
                            | PropName::BigInt(_)
                            | PropName::Num(_) => {
                              // ignore
                            }
                          }
                        }
                        ObjectPatProp::Assign(assign_prop) => {
                          module_data
                            .add_export_name(assign_prop.key.sym.to_string());
                        }
                        ObjectPatProp::Rest(rest) => match &*rest.arg {
                          Pat::Ident(ident) => {
                            module_data
                              .add_export_name(ident.id.sym.to_string());
                          }
                          Pat::Array(_) => todo!(),
                          Pat::Rest(_) => todo!(),
                          Pat::Object(_) => todo!(),
                          Pat::Assign(_) => todo!(),
                          Pat::Invalid(_) => todo!(),
                          Pat::Expr(_) => todo!(),
                        },
                      }
                    }
                  }
                  Pat::Invalid(_) => todo!(),
                  Pat::Expr(_) => todo!(),
                }
              }
            }
            Decl::TsEnum(decl) => {
              if decl.declare {
                continue;
              }
              context.module_data.get_mut(module_specifier).exports.push(
                ExportName {
                  export_name: None,
                  local_name: decl.id.sym.to_string(),
                },
              );
            }
            Decl::TsModule(decl) => {
              if decl.declare {
                continue;
              }
              if let TsModuleName::Ident(id) = &decl.id {
                // the namespace will be exported as the first id
                context.module_data.get_mut(module_specifier).exports.push(
                  ExportName {
                    export_name: None,
                    local_name: id.sym.to_string(),
                  },
                );
              }
            }
            Decl::TsInterface(_) | Decl::TsTypeAlias(_) => {}
          }
        }
        ModuleDecl::ExportNamed(decl) => {
          if decl.type_only {
            continue;
          }
          if let Some(src) = &decl.src {
            match context.graph.resolve_dependency(
              &src.value,
              module_specifier,
              false,
            ) {
              Some(dep_specifier) => {
                let dep_id = context.module_data.get_mut(&dep_specifier).id;
                let module_data = context.module_data.get_mut(module_specifier);
                for export_specifier in &decl.specifiers {
                  match export_specifier {
                    ExportSpecifier::Default(_) => {
                      todo!(); // what even is this? Maybe some babel thing or I'm not thinking atm
                    }
                    ExportSpecifier::Named(named) => {
                      if named.is_type_only {
                        continue;
                      }
                      module_data.re_exports.push(ReExport {
                        name: ReExportName::Named(ExportName {
                          export_name: named.exported.as_ref().map(|name| {
                            match name {
                              ModuleExportName::Ident(ident) => {
                                ident.sym.to_string()
                              }
                              ModuleExportName::Str(_) => todo!(),
                            }
                          }),
                          local_name: match &named.orig {
                            ModuleExportName::Ident(ident) => {
                              ident.sym.to_string()
                            }
                            ModuleExportName::Str(_) => todo!(),
                          },
                        }),
                        specifier: dep_specifier.clone(),
                        module_id: dep_id,
                      })
                    }
                    ExportSpecifier::Namespace(namespace) => {
                      module_data.re_exports.push(ReExport {
                        name: ReExportName::Namespace(match &namespace.name {
                          ModuleExportName::Ident(ident) => {
                            ident.sym.to_string()
                          }
                          ModuleExportName::Str(_) => todo!(),
                        }),
                        specifier: dep_specifier.clone(),
                        module_id: dep_id,
                      })
                    }
                  }
                }
              }
              None => {
                todo!();
              }
            }
          } else {
            // no specifier
            let module_data = context.module_data.get_mut(module_specifier);
            for export_specifier in &decl.specifiers {
              match export_specifier {
                ExportSpecifier::Named(named) => {
                  let (local_name, local_name_as_export) = {
                    match &named.orig {
                      ModuleExportName::Ident(ident) => {
                        let ident_text = ident.sym.to_string();
                        let local_name = replace_ids
                          .get(&ident.to_id())
                          .map(ToOwned::to_owned)
                          .unwrap_or_else(|| vec![ident_text.clone()])
                          .join(".");
                        let local_name_as_export = if ident_text != local_name {
                          Some(ident_text)
                        } else {
                          None
                        };
                        (local_name, local_name_as_export)
                      }
                      ModuleExportName::Str(_) => todo!(),
                    }
                  };
                  module_data.exports.push(ExportName {
                    export_name: named
                      .exported
                      .as_ref()
                      .map(|name| match name {
                        ModuleExportName::Ident(ident) => ident.sym.to_string(),
                        ModuleExportName::Str(_) => todo!(),
                      })
                      .or(local_name_as_export),
                    local_name,
                  });
                }
                ExportSpecifier::Namespace(_) | ExportSpecifier::Default(_) => {
                  unreachable!()
                }
              }
            }
          }
        }
        ModuleDecl::ExportAll(export_all) => {
          if export_all.type_only {
            continue;
          }
          match context.graph.resolve_dependency(
            &export_all.src.value,
            module_specifier,
            false,
          ) {
            Some(dep_specifier) => {
              let dep_id = context.module_data.get_mut(&dep_specifier).id;
              let module_data = context.module_data.get_mut(module_specifier);
              module_data.re_exports.push(ReExport {
                name: ReExportName::All,
                specifier: dep_specifier,
                module_id: dep_id,
              });
            }
            None => {
              todo!();
            }
          }
        }
        ModuleDecl::TsImportEquals(_)
        | ModuleDecl::TsExportAssignment(_)
        | ModuleDecl::TsNamespaceExport(_) => {}
      },
    }
  }

  // replace all the identifiers
  let mut transformer = Transformer {
    replace_ids: &replace_ids,
    is_root_module,
  };
  transformer.visit_mut_module(&mut module);
  let module_data = context.module_data.get_mut(module_specifier);
  module_data.module = Some(module);

  Ok(())
}

struct Transformer<'a> {
  replace_ids: &'a HashMap<Id, Vec<String>>,
  is_root_module: bool,
}

impl<'a> VisitMut for Transformer<'a> {
  fn visit_mut_module_items(&mut self, n: &mut Vec<ModuleItem>) {
    n.retain(|item| match item {
      ModuleItem::ModuleDecl(module_decl) => match module_decl {
        ModuleDecl::TsImportEquals(_)
        | ModuleDecl::TsExportAssignment(_)
        | ModuleDecl::ExportDefaultDecl(_)
        | ModuleDecl::ExportDefaultExpr(_)
        | ModuleDecl::ExportDecl(_) => true,
        ModuleDecl::Import(_)
        | ModuleDecl::TsNamespaceExport(_)
        | ModuleDecl::ExportNamed(_)
        | ModuleDecl::ExportAll(_) => false,
      },
      ModuleItem::Stmt(_) => true,
    });

    visit_mut_module_items(self, n);
  }

  fn visit_mut_module_item(&mut self, n: &mut ModuleItem) {
    if !self.is_root_module {
      if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
        export_default_expr,
      )) = n
      {
        self.visit_mut_expr(&mut export_default_expr.expr);
        *n = ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
          span: DUMMY_SP,
          kind: VarDeclKind::Const,
          declare: false,
          decls: Vec::from([VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
              id: ident("__pack_default__".to_string()),
              type_ann: None,
            }),
            init: Some(export_default_expr.expr.clone()),
            definite: false,
          }]),
        }))));
      } else if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(
        decl,
      )) = n
      {
        let maybe_ident = match &decl.decl {
          DefaultDecl::Class(expr) => expr.ident.as_ref(),
          DefaultDecl::Fn(expr) => expr.ident.as_ref(),
          DefaultDecl::TsInterfaceDecl(_) => None,
        };
        let maybe_expr = match &decl.decl {
          DefaultDecl::Class(decl) => Some(Expr::Class(decl.clone())),
          DefaultDecl::Fn(expr) => Some(Expr::Fn(expr.clone())),
          DefaultDecl::TsInterfaceDecl(_) => None,
        };
        if let Some(mut expr) = maybe_expr {
          self.visit_mut_expr(&mut expr);
          *n = ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            kind: VarDeclKind::Const,
            declare: false,
            decls: Vec::from([VarDeclarator {
              span: DUMMY_SP,
              name: Pat::Ident(BindingIdent {
                id: maybe_ident
                  .cloned()
                  .unwrap_or_else(|| ident("__pack_default__".to_string())),
                type_ann: None,
              }),
              init: Some(Box::new(expr)),
              definite: false,
            }]),
          }))));
        } else {
          n.take(); // remove it
        }
      } else if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(decl)) = n {
        let mut decl = decl.decl.clone();
        self.visit_mut_decl(&mut decl);
        // remove the export keyword
        *n = ModuleItem::Stmt(Stmt::Decl(decl));
      } else {
        visit_mut_module_item(self, n)
      }
    } else {
      visit_mut_module_item(self, n)
    }
  }

  fn visit_mut_object_lit(&mut self, object_lit: &mut ObjectLit) {
    for prop in &mut object_lit.props {
      match prop {
        PropOrSpread::Spread(spread) => {
          self.visit_mut_spread_element(spread);
        }
        PropOrSpread::Prop(prop) => match &mut **prop {
          Prop::Shorthand(ident) => {
            let id = ident.to_id();
            if let Some(parts) = self.replace_ids.get(&id) {
              *prop = Box::new(Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(ident.clone()),
                value: Box::new(replace_id_to_expr(parts)),
              }));
            }
          }
          Prop::KeyValue(_)
          | Prop::Assign(_)
          | Prop::Getter(_)
          | Prop::Setter(_)
          | Prop::Method(_) => {
            self.visit_mut_prop(prop);
          }
        },
      }
    }
  }

  fn visit_mut_expr(&mut self, expr: &mut Expr) {
    match expr {
      Expr::Ident(ident) => {
        let id = ident.to_id();
        if let Some(parts) = self.replace_ids.get(&id) {
          *expr = replace_id_to_expr(parts);
        } else {
          visit_mut_expr(self, expr);
        }
      }
      _ => {
        visit_mut_expr(self, expr);
      }
    }
  }
}

fn replace_id_to_expr(parts: &[String]) -> Expr {
  let mut parts = parts.iter().collect::<VecDeque<_>>();
  let mut final_expr = Expr::Ident(ident(parts.pop_front().unwrap().clone()));
  while !parts.is_empty() {
    final_expr = Expr::Member(MemberExpr {
      span: DUMMY_SP,
      obj: Box::new(final_expr),
      prop: MemberProp::Ident(ident(parts.pop_front().unwrap().clone())),
    });
  }
  final_expr
}

fn get_root_dir<'a>(
  specifiers: impl Iterator<Item = &'a ModuleSpecifier>,
) -> Option<&'a str> {
  fn get_folder(specifier: &ModuleSpecifier) -> &str {
    let specifier = specifier.as_str();
    let r_index = specifier.rfind('/').unwrap();
    &specifier[..r_index]
  }

  let mut root: Option<&str> = None;
  for specifier in specifiers.filter(|s| s.scheme() == "file") {
    let folder = get_folder(specifier);
    if root.is_none() || root.as_ref().unwrap().starts_with(folder) {
      root = Some(folder);
    }
  }
  if root == Some("file://") {
    Some("file:///")
  } else {
    root
  }
}
