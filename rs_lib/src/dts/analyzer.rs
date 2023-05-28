// todo: probably remove all this. Instead there should be a way to lazily get a symbol table for a module

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use deno_ast::swc::ast::*;
use deno_ast::swc::codegen::Node;
use deno_ast::swc::visit::Visit;
use deno_ast::swc::visit::VisitWith;
use deno_ast::ModuleSpecifier;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;

#[derive(Debug, Clone, Copy)]
struct ModuleId(usize);

impl ModuleId {
  pub fn to_code_string(&self) -> String {
    format!("pack{}", self.0)
  }
}

struct Declaration {
  id: Id,
  deps: Vec<Id>,
  import: Option<ModuleId>,
}

struct ExportName {
  local_name: Id,
  export_name: Option<String>,
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

#[derive(Default)]
struct ModuleDataCollection {
  module_data: HashMap<ModuleSpecifier, ModuleData>,
}

impl ModuleDataCollection {
  pub fn get_mut(&mut self, specifier: &ModuleSpecifier) -> &mut ModuleData {
    let next_id = self.module_data.len();
    self
      .module_data
      .entry(specifier.clone())
      .or_insert_with(|| ModuleData {
        id: ModuleId(next_id),
        exports: Default::default(),
        re_exports: Default::default(),
        imports: Default::default(),
        declarations: Default::default(),
      })
  }
}

struct ModuleData {
  id: ModuleId,
  exports: Vec<ExportName>,
  re_exports: Vec<ReExport>,
  imports: HashMap<ModuleSpecifier, Vec<Id>>,
  declarations: HashMap<Id, Declaration>,
}

struct Context<'a> {
  collection: ModuleDataCollection,
  graph: &'a ModuleGraph,
  parser: &'a CapturingModuleParser<'a>,
  pending_specifiers: VecDeque<ModuleSpecifier>,
}

fn analyze_module(
  specifier: &ModuleSpecifier,
  module: &Module,
  context: &mut Context,
) {
  for item in &module.body {
    match item {
      ModuleItem::ModuleDecl(decl) => match decl {
        ModuleDecl::Import(import) => {
          let value: &str = &import.src.value;
          match context.graph.resolve_dependency(value, specifier, false) {
            Some(dep_specifier) => {
              for specifier in &import.specifiers {
                match specifier {
                  ImportSpecifier::Named(named) => {
                    named.local.to_id();
                  }
                  ImportSpecifier::Default(default) => {}
                  ImportSpecifier::Namespace(namespace) => {}
                }
              }
            }
            None => {
              todo!();
            }
          }
        }
        ModuleDecl::ExportDecl(ExportDecl { decl, .. }) => match decl {
          Decl::Class(_) => todo!(),
          Decl::Fn(_) => todo!(),
          Decl::Var(_) => todo!(),
          Decl::TsInterface(decl) => analyze_interface_decl(
            decl,
            &mut context.collection.get_mut(specifier),
          ),
          Decl::TsTypeAlias(_) => todo!(),
          Decl::TsEnum(_) => todo!(),
          Decl::TsModule(_) => todo!(),
        },
        ModuleDecl::ExportNamed(_) => todo!(),
        ModuleDecl::TsImportEquals(_) => todo!(),
        ModuleDecl::TsExportAssignment(_) => todo!(),
        ModuleDecl::TsNamespaceExport(_) => todo!(),
        ModuleDecl::ExportDefaultDecl(ExportDefaultDecl { decl, .. }) => {
          match decl {
            DefaultDecl::Class(_) => todo!(),
            DefaultDecl::Fn(_) => todo!(),
            DefaultDecl::TsInterfaceDecl(decl) => {
              let module_data = context.collection.get_mut(specifier);
              module_data.exports.push(ExportName {
                local_name: decl.id.to_id(),
                export_name: Some("default".to_string()),
              });
              analyze_interface_decl(decl, module_data);
            }
          }
        }
        ModuleDecl::ExportDefaultExpr(_) => todo!(),
        ModuleDecl::ExportAll(_) => todo!(),
      },
      ModuleItem::Stmt(stmt) => match stmt {
        Stmt::Block(_)
        | Stmt::Empty(_)
        | Stmt::Debugger(_)
        | Stmt::With(_)
        | Stmt::Return(_)
        | Stmt::Labeled(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::If(_)
        | Stmt::Switch(_)
        | Stmt::Throw(_)
        | Stmt::Try(_)
        | Stmt::While(_)
        | Stmt::DoWhile(_)
        | Stmt::For(_)
        | Stmt::ForIn(_)
        | Stmt::ForOf(_)
        | Stmt::Expr(_) => {
          // do nothing
        }
        Stmt::Decl(decl) => match decl {
          Decl::Class(_) => todo!(),
          Decl::Fn(_) => todo!(),
          Decl::Var(_) => todo!(),
          Decl::TsInterface(decl) => analyze_interface_decl(
            decl,
            &mut context.collection.get_mut(specifier),
          ),
          Decl::TsTypeAlias(_) => todo!(),
          Decl::TsEnum(_) => todo!(),
          Decl::TsModule(_) => todo!(),
        },
      },
    }
  }
}

fn analyze_interface_decl(
  decl: &TsInterfaceDecl,
  module_data: &mut ModuleData,
) {
  let mut collector = IdCollector::default();
  decl.extends.visit_with(&mut collector);
  decl.body.visit_with(&mut collector);
  decl.type_params.visit_with(&mut collector);
  module_data.declarations.insert(
    decl.id.to_id(),
    Declaration {
      id: decl.id.to_id(),
      deps: collector.ids.into_iter().collect(),
      import: None,
    },
  );
}

#[derive(Default)]
struct IdCollector {
  ids: HashSet<Id>,
}

impl Visit for IdCollector {
  fn visit_ident(&mut self, ident: &Ident) {
    self.ids.insert(ident.to_id());
  }
}
