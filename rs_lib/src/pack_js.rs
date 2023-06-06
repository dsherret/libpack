use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;

use deno_ast::swc::ast::Id;
use deno_ast::swc::ast::*;
use deno_ast::swc::common::comments::SingleThreadedComments;
use deno_ast::swc::common::util::take::Take;
use deno_ast::swc::common::FileName;
use deno_ast::swc::common::Mark;
use deno_ast::swc::common::SourceMap;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::visit::*;
use deno_ast::Diagnostic;
use deno_ast::EmitOptions;
use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_ast::ParseParams;
use deno_ast::SourceTextInfo;
use deno_graph::CapturingModuleParser;
use deno_graph::EsmModule;
use deno_graph::ModuleGraph;
use deno_graph::ModuleParser;
use deno_graph::WalkOptions;

use crate::helpers::adjust_spans;
use crate::helpers::fill_leading_comments;
use crate::helpers::fill_trailing_comments;
use crate::helpers::ident;
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
  let mut ordered_specifiers: Vec<(&ModuleSpecifier, &deno_graph::Module)> =
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
    match module {
      deno_graph::Module::Esm(esm) => {
        ordered_specifiers.push((specifier, module));
        if options.include_remote || is_file {
          analyze_esm_module(esm, &mut context)?;
        }
      }
      deno_graph::Module::Json(_) => {
        ordered_specifiers.push((specifier, module));
      }
      _ => {
        todo!();
      }
    }
  }

  let root_dir = get_root_dir(ordered_specifiers.iter().map(|(s, _)| *s));
  let mut final_text = String::new();
  for (specifier, module) in &ordered_specifiers {
    if specifier.scheme() != "file" {
      let module_data = context.module_data.get_mut(specifier);
      final_text.push_str(&format!(
        "import * as {} from \"{}\";\n",
        module_data.id.to_code_string(),
        specifier.to_string(),
      ));
    } else {
      if let deno_graph::Module::Esm(_) = module {
        let export_names = context.module_data.get_export_names(specifier);
        let module_data = context.module_data.get_mut(specifier);
        if export_names.is_empty() || context.graph.roots[0] == **specifier {
          continue;
        }
        final_text.push_str(&format!(
          "const {} = {{\n",
          module_data.id.to_code_string()
        ));
        for name in export_names {
          final_text.push_str(&format!("  {}: undefined,\n", name));
        }
        final_text.push_str("};\n");
      } else if let deno_graph::Module::Json(json) = module {
        let module_data = context.module_data.get_mut(specifier);
        if !final_text.is_empty() {
          final_text.push('\n');
        }
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
    for (specifier, module) in ordered_specifiers.iter().rev() {
      if !options.include_remote && specifier.scheme() != "file" {
        continue;
      }

      if let deno_graph::Module::Esm(esm) = module {
        let source = &esm.source;
        // eprintln!("PACKING: {}", specifier);
        let module_text = {
          let parsed_source = context.parser.parse_module(
            &esm.specifier,
            esm.source.clone(),
            esm.media_type,
          )?;
          // todo: do a single transpile for everything
          let module_data = context.module_data.get_mut(specifier);
          let mut module = module_data.module.take().unwrap();
          let source_map = Rc::new(SourceMap::default());
          let top_level_mark = Mark::fresh(Mark::root());
          let source_file = source_map.new_source_file(
            FileName::Url(esm.specifier.clone()),
            source.to_string(),
          );
          adjust_spans(source_file.start_pos, &mut module);
          let global_comments = SingleThreadedComments::default();
          fill_leading_comments(
            source_file.start_pos,
            &parsed_source,
            &global_comments,
            |_| true,
          );
          fill_trailing_comments(source_file.start_pos, &parsed_source, &global_comments);
          let program = deno_ast::fold_program(
            Program::Module(module),
            &EmitOptions::default(),
            source_map.clone(),
            &global_comments,
            top_level_mark,
            parsed_source.diagnostics(),
          )?;
          print_program(
            &program,
            &source_map,
            &global_comments,
          )?
        };
        let module_data = context.module_data.get(specifier).unwrap();
        let module_text = module_text.trim();
        if !module_text.is_empty()
          || !module_data.exports.is_empty()
          || !module_data.re_exports.is_empty()
        {
          if !final_text.is_empty() {
            final_text.push('\n');
          }
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
          final_text.push_str(&format!("// {}\n", displayed_specifier));
          if *specifier == &roots[0] {
            final_text.push_str(&module_text);
            final_text.push_str("\n");
          } else {
            if module_data.has_tla {
              final_text.push_str("await (async () => {\n");
            } else {
              final_text.push_str("(() => {\n");
            }
            if !module_text.is_empty() {
              final_text.push_str(&format!("{}\n", module_text));
            }
            let code_string = module_data.id.to_code_string();
            let mut export_names = HashSet::with_capacity(
              module_data.exports.len() + module_data.re_exports.len(),
            );
            for export in &module_data.exports {
              final_text.push_str(&format!(
                "Object.defineProperty({}, \"{}\", {{ get: () => {} }});\n",
                code_string,
                export.export_name(),
                export.local_name
              ));
              export_names.insert(export.export_name());
            }
            for re_export in &module_data.re_exports {
              match &re_export.name {
                ReExportName::Named(name) => {
                  final_text.push_str(&format!(
                    "Object.defineProperty({}, \"{}\", {{ get: () => {}.{} }});\n",
                    code_string,
                    name.export_name(),
                    re_export.module_id.to_code_string(),
                    name.local_name,
                  ));
                  export_names.insert(name.export_name());
                }
                ReExportName::Namespace(name) => {
                  final_text.push_str(&format!(
                    "Object.defineProperty({}, \"{}\", {{ get: () => {}.{} }});\n",
                    code_string,
                    name,
                    re_export.module_id.to_code_string(),
                    name,
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
                    final_text.push_str(&format!(
                    "Object.defineProperty({}, \"{}\", {{ get: () => {}.{} }});\n",
                    code_string,
                    name,
                    re_export.module_id.to_code_string(),
                    name
                  ));
                  }
                }
              }
            }
            final_text.push_str("})();\n");
          }
        }
      }
    }
    Result::<(), anyhow::Error>::Ok(())
  })?;

  Ok(final_text)
}

struct HasAwaitKeywordVisitor {
  found: bool,
}

impl HasAwaitKeywordVisitor {
  fn has_await_keyword(node: &Stmt) -> bool {
    let mut visitor = HasAwaitKeywordVisitor { found: false };
    visitor.visit_stmt(node);
    visitor.found
  }
}

impl Visit for HasAwaitKeywordVisitor {
  fn visit_function(&mut self, n: &Function) {
    // stop
  }

  fn visit_arrow_expr(&mut self, n: &ArrowExpr) {
    // stop
  }

  fn visit_class_method(&mut self, n: &ClassMethod) {
    // stop
  }

  fn visit_decl(&mut self, n: &Decl) {
    // stop
  }

  fn visit_await_expr(&mut self, n: &AwaitExpr) {
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
        | ModuleDecl::ExportDecl(_)
        | ModuleDecl::ExportNamed(_)
        | ModuleDecl::ExportAll(_)
        | ModuleDecl::TsImportEquals(_)
        | ModuleDecl::TsExportAssignment(_)
        | ModuleDecl::TsNamespaceExport(_) => {}
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
                  Pat::Array(_) => todo!(),
                  Pat::Assign(_) => todo!(),
                  Pat::Ident(ident) => {
                    module_data.add_export_name(ident.id.sym.to_string());
                  }
                  Pat::Rest(_) => todo!(),
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
        eprintln!("IDENT: {}", ident.sym.to_string());
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

fn emit_script(file_text: &str) -> String {
  // todo: skip emitting jsx

  // use swc for now because emitting enums is actually quite complicated
  deno_ast::parse_module(ParseParams {
    specifier: "file:///mod.ts".to_string(),
    text_info: SourceTextInfo::new(file_text.into()),
    media_type: MediaType::TypeScript,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .unwrap()
  .transpile(&EmitOptions {
    source_map: false,
    inline_source_map: false,
    ..Default::default()
  })
  .unwrap()
  .text
}
