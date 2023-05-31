use std::collections::HashMap;
use std::collections::HashSet;

use deno_ast::swc::ast::*;
use deno_ast::swc::utils::find_pat_ids;
use deno_ast::swc::visit::*;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned;

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct SymbolId(u32);

#[derive(Debug, Clone, Copy)]
pub struct ModuleId(usize);

impl ModuleId {
  pub fn to_code_string(&self) -> String {
    format!("pack{}", self.0)
  }
}

pub enum FileDepName {
  All,
  Name(String),
}

pub struct FileDep {
  pub name: FileDepName,
  pub specifier: String,
}

#[derive(Default)]
pub struct Symbol {
  // todo: store any implicit types here
  // todo: store file dependencies here and what export names are used
  is_public: bool,
  decls: Vec<SourceRange>,
  deps: HashSet<Id>,
  file_dep: Option<FileDep>,
}

impl Symbol {
  pub fn is_public(&self) -> bool {
    self.is_public
  }

  pub fn mark_public(&mut self) -> bool {
    if self.is_public {
      false
    } else {
      self.is_public = true;
      true
    }
  }

  pub fn swc_dep_ids(&self) -> impl Iterator<Item = &Id> {
    self.deps.iter()
  }

  pub fn file_dep(&self) -> Option<&FileDep> {
    self.file_dep.as_ref()
  }
}

pub struct ModuleSymbol {
  module_id: ModuleId,
  next_symbol_id: SymbolId,
  exports: HashMap<String, SymbolId>,
  // note: not all symbol ids have an swc id. For example, default exports
  swc_id_to_symbol_id: HashMap<Id, SymbolId>,
  symbols: HashMap<SymbolId, Symbol>,
}

impl ModuleSymbol {
  pub fn module_id(&self) -> ModuleId {
    self.module_id
  }

  pub fn public_source_ranges(&self) -> HashSet<SourceRange> {
    self
      .symbols
      .values()
      .filter(|symbol| symbol.is_public)
      .flat_map(|symbol| symbol.decls.clone())
      .collect()
  }

  pub fn export_symbols(&self) -> Vec<SymbolId> {
    self.exports.values().copied().collect::<Vec<_>>()
  }

  pub fn export_symbol_id(&self, name: &str) -> Option<SymbolId> {
    self.exports.get(name).copied()
  }

  pub fn symbol_id_from_swc(&self, id: &Id) -> Option<SymbolId> {
    self.swc_id_to_symbol_id.get(id).copied()
  }

  pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
    self.symbols.get(&id)
  }

  pub fn symbol_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
    self.symbols.get_mut(&id)
  }

  fn get_next_symbol_id(&mut self) -> SymbolId {
    let next_id = self.next_symbol_id;
    self.next_symbol_id = SymbolId(self.next_symbol_id.0 + 1);
    next_id
  }

  fn add_export(&mut self, id: Id, range: SourceRange) -> SymbolId {
    let symbol_id = self.ensure_symbol_for_swc_id(id.clone(), range);
    self.exports.insert(id.0.to_string(), symbol_id);
    symbol_id
  }

  fn get_symbol_from_swc_id(
    &mut self,
    id: Id,
    symbol_range: SourceRange,
  ) -> &mut Symbol {
    let symbol_id = self.ensure_symbol_for_swc_id(id, symbol_range);
    self.symbols.get_mut(&symbol_id).unwrap()
  }

  fn ensure_symbol_for_swc_id(
    &mut self,
    id: Id,
    symbol_range: SourceRange,
  ) -> SymbolId {
    let symbol_id = match self.swc_id_to_symbol_id.get(&id) {
      Some(symbol_id) => *symbol_id,
      None => {
        let symbol_id = self.get_next_symbol_id();
        self.swc_id_to_symbol_id.insert(id, symbol_id);
        symbol_id
      }
    };

    if let Some(symbol) = self.symbols.get_mut(&symbol_id) {
      symbol.decls.push(symbol_range);
    } else {
      let mut symbol = Symbol::default();
      symbol.decls.push(symbol_range);
      self.symbols.insert(symbol_id, symbol);
    }
    symbol_id
  }
}

#[derive(Default)]
pub struct ModuleAnalyzer {
  modules: HashMap<String, ModuleSymbol>,
}

impl ModuleAnalyzer {
  pub fn get(&self, specifier: &ModuleSpecifier) -> Option<&ModuleSymbol> {
    self.modules.get(specifier.as_str())
  }

  pub fn get_or_analyze(&mut self, source: &ParsedSource) -> &mut ModuleSymbol {
    if !self.modules.contains_key(source.specifier()) {
      let module = source.module();
      let mut module_symbol = ModuleSymbol {
        module_id: ModuleId(self.modules.len()),
        next_symbol_id: Default::default(),
        exports: Default::default(),
        swc_id_to_symbol_id: Default::default(),
        symbols: Default::default(),
      };
      fill_module(&mut module_symbol, module);
      self
        .modules
        .insert(source.specifier().to_string(), module_symbol);
    }
    self.modules.get_mut(source.specifier()).unwrap()
  }
}

fn fill_module(file_module: &mut ModuleSymbol, module: &Module) {
  for module_item in &module.body {
    fill_module_item(file_module, module_item);

    // now fill the file exports
    match module_item {
      ModuleItem::ModuleDecl(decl) => match decl {
        ModuleDecl::Import(_) => {
          // ignore
        }
        ModuleDecl::ExportDecl(export_decl) => match &export_decl.decl {
          Decl::Class(n) => {
            file_module.add_export(n.ident.to_id(), export_decl.range());
          }
          Decl::Fn(n) => {
            file_module.add_export(n.ident.to_id(), export_decl.range());
          }
          Decl::Var(n) => {
            for decl in &n.decls {
              let ids: Vec<Id> = find_pat_ids(&decl.name);
              for id in ids {
                file_module.add_export(id, decl.range());
              }
            }
          }
          Decl::TsInterface(n) => {
            file_module.add_export(n.id.to_id(), export_decl.range());
          }
          Decl::TsTypeAlias(n) => {
            file_module.add_export(n.id.to_id(), export_decl.range());
          }
          Decl::TsEnum(n) => {
            file_module.add_export(n.id.to_id(), export_decl.range());
          }
          Decl::TsModule(n) => match &n.id {
            TsModuleName::Ident(ident) => {
              file_module.add_export(ident.to_id(), export_decl.range());
            }
            TsModuleName::Str(_) => todo!(),
          },
        },
        ModuleDecl::ExportNamed(_)
        | ModuleDecl::ExportDefaultDecl(_)
        | ModuleDecl::ExportDefaultExpr(_)
        | ModuleDecl::ExportAll(_)
        | ModuleDecl::TsImportEquals(_)
        | ModuleDecl::TsExportAssignment(_)
        | ModuleDecl::TsNamespaceExport(_) => {
          // ignore
        }
      },
      ModuleItem::Stmt(_) => {
        // ignore
      }
    }
  }
}

fn fill_module_item(file_module: &mut ModuleSymbol, module_item: &ModuleItem) {
  match module_item {
    ModuleItem::ModuleDecl(decl) => match decl {
      ModuleDecl::Import(import_decl) => {
        for specifier in &import_decl.specifiers {
          match specifier {
            ImportSpecifier::Named(n) => {
              if let Some(imported_name) = &n.imported {
                match imported_name {
                  ModuleExportName::Ident(ident) => {
                    let local_symbol = file_module
                      .get_symbol_from_swc_id(n.local.to_id(), n.range());
                    local_symbol.deps.insert(ident.to_id());
                    let symbol = file_module
                      .get_symbol_from_swc_id(ident.to_id(), ident.range());
                    symbol.file_dep = Some(FileDep {
                      name: FileDepName::Name(ident.sym.to_string()),
                      specifier: import_decl.src.value.to_string(),
                    });
                  }
                  ModuleExportName::Str(_) => todo!(),
                }
              } else {
                let local_symbol = file_module
                  .get_symbol_from_swc_id(n.local.to_id(), n.range());
                local_symbol.file_dep = Some(FileDep {
                  name: FileDepName::Name(n.local.sym.to_string()),
                  specifier: import_decl.src.value.to_string(),
                });
              }
            }
            ImportSpecifier::Default(n) => {
              let symbol =
                file_module.get_symbol_from_swc_id(n.local.to_id(), n.range());
              symbol.file_dep = Some(FileDep {
                name: FileDepName::Name("default".to_string()),
                specifier: import_decl.src.value.to_string(),
              });
            }
            ImportSpecifier::Namespace(n) => {
              let symbol =
                file_module.get_symbol_from_swc_id(n.local.to_id(), n.range());
              symbol.file_dep = Some(FileDep {
                name: FileDepName::All,
                specifier: import_decl.src.value.to_string(),
              });
            }
          }
        }
      }
      ModuleDecl::ExportDecl(export_decl) => match &export_decl.decl {
        Decl::Class(n) => {
          let symbol = file_module
            .get_symbol_from_swc_id(n.ident.to_id(), export_decl.range());
          fill_class_decl(symbol, n);
        }
        Decl::Fn(n) => {
          let symbol = file_module
            .get_symbol_from_swc_id(n.ident.to_id(), export_decl.range());
          fill_fn_decl(symbol, n);
        }
        Decl::Var(n) => {
          for decl in &n.decls {
            let ids: Vec<Id> = find_pat_ids(&decl.name);
            for id in ids {
              let symbol = file_module.get_symbol_from_swc_id(id, decl.range());
              fill_var_declarator(symbol, decl);
            }
          }
        }
        Decl::TsInterface(n) => {
          let symbol = file_module
            .get_symbol_from_swc_id(n.id.to_id(), export_decl.range());
          fill_ts_interface(symbol, n);
        }
        Decl::TsTypeAlias(n) => {
          let symbol = file_module
            .get_symbol_from_swc_id(n.id.to_id(), export_decl.range());
          fill_ts_type_alias(symbol, n);
        }
        Decl::TsEnum(n) => {
          let symbol = file_module
            .get_symbol_from_swc_id(n.id.to_id(), export_decl.range());
          fill_ts_enum(symbol, n);
        }
        Decl::TsModule(n) => {
          fill_ts_module(file_module, export_decl.range(), n)
        }
      },
      ModuleDecl::ExportNamed(n) => {
        for specifier in &n.specifiers {
          match specifier {
            ExportSpecifier::Named(named) => {
              if let Some(exported_name) = &named.exported {
                match exported_name {
                  ModuleExportName::Ident(export_ident) => match &named.orig {
                    ModuleExportName::Ident(orig_ident) => {
                      let orig_symbol = file_module
                        .get_symbol_from_swc_id(orig_ident.to_id(), n.range());
                      orig_symbol.deps.insert(export_ident.to_id());
                      if let Some(src) = &n.src {
                        orig_symbol.file_dep = Some(FileDep {
                          name: FileDepName::Name(orig_ident.sym.to_string()),
                          specifier: src.value.to_string(),
                        });
                      }

                      let export_id = file_module
                        .add_export(export_ident.to_id(), named.range());
                      file_module
                        .symbol_mut(export_id)
                        .unwrap()
                        .deps
                        .insert(orig_ident.to_id());
                    }
                    ModuleExportName::Str(_) => todo!(),
                  },
                  ModuleExportName::Str(_) => todo!(),
                }
              } else {
                match &named.orig {
                  ModuleExportName::Ident(orig_ident) => {
                    let orig_symbol = file_module
                      .get_symbol_from_swc_id(orig_ident.to_id(), n.range());
                    if let Some(src) = &n.src {
                      orig_symbol.file_dep = Some(FileDep {
                        name: FileDepName::Name(orig_ident.sym.to_string()),
                        specifier: src.value.to_string(),
                      });
                    }
                    file_module.add_export(orig_ident.to_id(), named.range());
                  }
                  ModuleExportName::Str(_) => todo!(),
                }
              }
            }
            ExportSpecifier::Namespace(_) => todo!(),
            ExportSpecifier::Default(_) => todo!(),
          }
        }
      }
      ModuleDecl::ExportDefaultDecl(_) => todo!(),
      ModuleDecl::ExportDefaultExpr(_) => todo!(),
      ModuleDecl::ExportAll(_) => todo!(),
      ModuleDecl::TsImportEquals(_) => todo!(),
      ModuleDecl::TsExportAssignment(_) => todo!(),
      ModuleDecl::TsNamespaceExport(_) => todo!(),
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
        // ignore
      }
      Stmt::Decl(n) => {
        match n {
          Decl::Class(n) => {
            let id = n.ident.to_id();
            let symbol = file_module.get_symbol_from_swc_id(id, n.range());
            fill_class_decl(symbol, n);
          }
          Decl::Fn(n) => {
            let id = n.ident.to_id();
            let symbol = file_module.get_symbol_from_swc_id(id, n.range());
            fill_fn_decl(symbol, n);
          }
          Decl::Var(var_decl) => {
            for decl in &var_decl.decls {
              let ids: Vec<Id> = find_pat_ids(&decl.name);
              for id in ids {
                let symbol =
                  file_module.get_symbol_from_swc_id(id, decl.range());
                fill_var_declarator(symbol, decl);
              }
            }
          }
          Decl::TsInterface(n) => {
            let id = n.id.to_id();
            let symbol = file_module.get_symbol_from_swc_id(id, n.range());
            fill_ts_interface(symbol, n);
          }
          Decl::TsTypeAlias(n) => {
            let id = n.id.to_id();
            let symbol = file_module.get_symbol_from_swc_id(id, n.range());
            fill_ts_type_alias(symbol, n);
          }
          Decl::TsEnum(n) => {
            let id = n.id.to_id();
            let symbol = file_module.get_symbol_from_swc_id(id, n.range());
            fill_ts_enum(symbol, n);
          }
          Decl::TsModule(n) => {
            fill_ts_module(file_module, n.range(), n);
          }
        };
      }
    },
  }
}

struct SymbolFillVisitor<'a> {
  symbol: &'a mut Symbol,
}

impl<'a> Visit for SymbolFillVisitor<'a> {
  fn visit_ident(&mut self, n: &Ident) {
    let id = n.to_id();
    self.symbol.deps.insert(id);
  }

  fn visit_ts_import_type(&mut self, n: &TsImportType) {
    // probably need to have another map for these
    todo!();
  }

  fn visit_ts_qualified_name(&mut self, n: &TsQualifiedName) {
    todo!();
  }
}

fn fill_class_decl(symbol: &mut Symbol, n: &ClassDecl) {
  fill_class(symbol, &n.class);
}

fn fill_class(symbol: &mut Symbol, n: &Class) {
  if let Some(type_params) = &n.type_params {
    fill_ts_type_param_decl(symbol, type_params);
  }
  if let Some(expr) = &n.super_class {
    fill_expr(symbol, expr);
  }
  if let Some(type_params) = &n.super_type_params {
    fill_ts_type_param_instantiation(symbol, type_params)
  }
  for expr in &n.implements {
    fill_ts_expr_with_type_args(symbol, expr);
  }
  fill_ts_class_members(symbol, &n.body);
}

fn fill_var_declarator(symbol: &mut Symbol, n: &VarDeclarator) {
  if let Some(init) = &n.init {
    fill_expr(symbol, init);
  }
}

fn fill_fn_decl(symbol: &mut Symbol, n: &FnDecl) {
  fill_function_decl(symbol, &n.function);
}

fn fill_function_decl(symbol: &mut Symbol, n: &Function) {
  if let Some(type_params) = &n.type_params {
    fill_ts_type_param_decl(symbol, type_params);
  }
  for param in &n.params {
    fill_param(symbol, param);
  }
  if let Some(return_type) = &n.return_type {
    fill_ts_type_ann(symbol, return_type);
  }
}

fn fill_ts_interface(symbol: &mut Symbol, n: &TsInterfaceDecl) {
  todo!()
}

fn fill_ts_type_alias(symbol: &mut Symbol, n: &TsTypeAliasDecl) {
  if let Some(type_params) = &n.type_params {
    fill_ts_type_param_decl(symbol, type_params);
  }
  fill_ts_type(symbol, &*n.type_ann);
}

fn fill_ts_enum(symbol: &mut Symbol, n: &TsEnumDecl) {
  for member in &n.members {
    if let Some(init) = &member.init {
      fill_expr(symbol, init);
    }
  }
}

fn fill_ts_module(
  file_module: &mut ModuleSymbol,
  range: SourceRange,
  n: &TsModuleDecl,
) {
  let mut id = match &n.id {
    TsModuleName::Ident(ident) => ident.to_id(),
    TsModuleName::Str(_) => todo!(),
  };
  let mut symbol_id = file_module.ensure_symbol_for_swc_id(id.clone(), range);

  // fill the exported declarations
  if let Some(body) = &n.body {
    let mut current = body;
    let block = loop {
      match current {
        TsNamespaceBody::TsModuleBlock(block) => break block,
        TsNamespaceBody::TsNamespaceDecl(decl) => {
          let previous_symbol_id = symbol_id;
          let previous_id = id;
          id = decl.id.to_id();
          symbol_id =
            file_module.ensure_symbol_for_swc_id(id.clone(), decl.range());
          file_module
            .symbol_mut(previous_symbol_id)
            .unwrap()
            .deps
            .insert(id.clone());
          file_module
            .symbol_mut(symbol_id)
            .unwrap()
            .deps
            .insert(previous_id);
          current = &decl.body;
        }
      }
    };
    for item in &block.body {
      fill_module_item(file_module, item);
      let symbol = file_module.symbol_mut(symbol_id).unwrap();
      match item {
        ModuleItem::ModuleDecl(decl) => match decl {
          ModuleDecl::Import(_)
          | ModuleDecl::ExportNamed(_)
          | ModuleDecl::ExportDefaultDecl(_)
          | ModuleDecl::ExportDefaultExpr(_)
          | ModuleDecl::ExportAll(_)
          | ModuleDecl::TsImportEquals(_)
          | ModuleDecl::TsExportAssignment(_)
          | ModuleDecl::TsNamespaceExport(_) => {
            // ignore
          }
          ModuleDecl::ExportDecl(decl) => match &decl.decl {
            Decl::Class(n) => {
              symbol.deps.insert(n.ident.to_id());
            }
            Decl::Fn(n) => {
              symbol.deps.insert(n.ident.to_id());
            }
            Decl::Var(n) => {
              for decl in &n.decls {
                symbol.deps.extend(find_pat_ids(&decl.name));
              }
            }
            Decl::TsInterface(n) => {
              symbol.deps.insert(n.id.to_id());
            }
            Decl::TsTypeAlias(n) => {
              symbol.deps.insert(n.id.to_id());
            }
            Decl::TsEnum(n) => {
              symbol.deps.insert(n.id.to_id());
            }
            Decl::TsModule(n) => match &n.id {
              TsModuleName::Ident(ident) => {
                symbol.deps.insert(ident.to_id());
              }
              TsModuleName::Str(_) => todo!(),
            },
          },
        },
        ModuleItem::Stmt(_) => {
          // ignore
        }
      }
    }
  }
}

fn fill_ts_expr_with_type_args(symbol: &mut Symbol, n: &TsExprWithTypeArgs) {
  if let Some(type_args) = &n.type_args {
    fill_ts_type_param_instantiation(symbol, type_args);
  }
  fill_expr(symbol, &n.expr);
}

fn fill_ts_type_param_decl(symbol: &mut Symbol, type_params: &TsTypeParamDecl) {
  for param in &type_params.params {
    fill_ts_type_param(symbol, param);
  }
}

fn fill_ts_type_param(symbol: &mut Symbol, param: &TsTypeParam) {
  if let Some(constraint) = &param.constraint {
    fill_ts_type(symbol, constraint);
  }
  if let Some(default) = &param.default {
    fill_ts_type(symbol, default);
  }
}

fn fill_ts_type_param_instantiation(
  symbol: &mut Symbol,
  type_params: &TsTypeParamInstantiation,
) {
  for param in &type_params.params {
    fill_ts_type(symbol, param);
  }
}

fn fill_expr(symbol: &mut Symbol, n: &Expr) {
  let mut visitor = SymbolFillVisitor { symbol };
  n.visit_with(&mut visitor);
}

fn fill_ts_class_members(symbol: &mut Symbol, members: &[ClassMember]) {
  for member in members {
    match member {
      ClassMember::Constructor(ctor) => fill_ctor(symbol, ctor),
      ClassMember::Method(method) => fill_method(symbol, method),
      ClassMember::PrivateMethod(method) => fill_private_method(symbol, method),
      ClassMember::ClassProp(prop) => fill_class_prop(symbol, prop),
      ClassMember::PrivateProp(prop) => fill_private_prop(symbol, prop),
      ClassMember::TsIndexSignature(signature) => {
        fill_ts_index_signature(symbol, signature)
      }
      ClassMember::AutoAccessor(prop) => fill_auto_accessor(symbol, prop),
      ClassMember::StaticBlock(_) | ClassMember::Empty(_) => {
        // ignore
      }
    }
  }
}

fn fill_ctor(symbol: &mut Symbol, ctor: &Constructor) {
  for param in &ctor.params {
    match param {
      ParamOrTsParamProp::TsParamProp(param) => {
        fill_ts_param_prop(symbol, param)
      }
      ParamOrTsParamProp::Param(param) => fill_param(symbol, param),
    }
  }
}

fn fill_method(symbol: &mut Symbol, method: &ClassMethod) {
  if let Some(type_params) = &method.function.type_params {
    fill_ts_type_param_decl(symbol, type_params)
  }
  for param in &method.function.params {
    fill_param(symbol, param)
  }
  if let Some(return_type) = &method.function.return_type {
    fill_ts_type_ann(symbol, return_type)
  }
}

fn fill_ts_param_prop(symbol: &mut Symbol, param: &TsParamProp) {
  match &param.param {
    TsParamPropParam::Ident(ident) => {
      if let Some(type_ann) = &ident.type_ann {
        fill_ts_type_ann(symbol, &type_ann)
      }
    }
    TsParamPropParam::Assign(assign) => {
      if let Some(type_ann) = &assign.type_ann {
        fill_ts_type_ann(symbol, &type_ann)
      }
    }
  }
}

fn fill_param(symbol: &mut Symbol, param: &Param) {
  fill_pat(symbol, &param.pat);
}

fn fill_pat(symbol: &mut Symbol, pat: &Pat) {
  match pat {
    Pat::Ident(n) => {
      if let Some(type_ann) = &n.type_ann {
        fill_ts_type_ann(symbol, type_ann);
      }
    }
    Pat::Array(n) => {
      if let Some(type_ann) = &n.type_ann {
        fill_ts_type_ann(symbol, type_ann);
      }
    }
    Pat::Rest(n) => {
      if let Some(type_ann) = &n.type_ann {
        fill_ts_type_ann(symbol, type_ann);
      }
    }
    Pat::Object(n) => {
      if let Some(type_ann) = &n.type_ann {
        fill_ts_type_ann(symbol, type_ann);
      }
    }
    Pat::Assign(n) => {
      if let Some(type_ann) = &n.type_ann {
        fill_ts_type_ann(symbol, type_ann);
      }
    }
    Pat::Invalid(_) => {
      // ignore
    }
    Pat::Expr(expr) => {
      fill_expr(symbol, expr);
    }
  }
}

fn fill_ts_type_ann(symbol: &mut Symbol, type_ann: &TsTypeAnn) {
  fill_ts_type(symbol, &type_ann.type_ann)
}

fn fill_ts_type(symbol: &mut Symbol, n: &TsType) {
  let mut visitor = SymbolFillVisitor { symbol };
  n.visit_with(&mut visitor);
}

fn fill_private_method(symbol: &mut Symbol, method: &PrivateMethod) {
  // do nothing, private
}

fn fill_class_prop(symbol: &mut Symbol, prop: &ClassProp) {
  if let Some(type_ann) = &prop.type_ann {
    fill_ts_type_ann(symbol, type_ann)
  }
}

fn fill_private_prop(symbol: &mut Symbol, prop: &PrivateProp) {
  // do nothing, private properties are not emitted with their type
}

fn fill_ts_index_signature(symbol: &mut Symbol, signature: &TsIndexSignature) {
  todo!()
}

fn fill_auto_accessor(symbol: &mut Symbol, prop: &AutoAccessor) {
  todo!()
}
