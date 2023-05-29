use std::collections::HashMap;
use std::collections::HashSet;

use deno_ast::swc::ast::*;
use deno_ast::swc::utils::find_pat_ids;
use deno_ast::swc::visit::*;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned;

use super::maybe_infer_type_from_expr;

// Key point:
// Only interested in creating a graph of dependencies of the public api.
// I think probably creating a hash of swc id -> symbol should work.

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

  fn get_symbol_for_export(
    &mut self,
    id: Id,
    range: SourceRange,
  ) -> &mut Symbol {
    let symbol_id = self.ensure_symbol_for_swc_id(id.clone(), range);
    self.exports.insert(id.0.to_string(), symbol_id);
    self.symbols.get_mut(&symbol_id).unwrap()
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
                let symbol = file_module
                  .get_symbol_from_swc_id(n.local.to_id(), n.range());
                symbol.file_dep = Some(FileDep {
                  name: FileDepName::Name("default".to_string()),
                  specifier: import_decl.src.value.to_string(),
                });
              }
              ImportSpecifier::Namespace(n) => {
                let symbol = file_module
                  .get_symbol_from_swc_id(n.local.to_id(), n.range());
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
              .get_symbol_for_export(n.ident.to_id(), export_decl.range());
            fill_class_decl(symbol, n);
          }
          Decl::Fn(n) => {
            let symbol = file_module
              .get_symbol_for_export(n.ident.to_id(), export_decl.range());
            fill_fn_decl(symbol, n);
          }
          Decl::Var(n) => {
            for decl in &n.decls {
              let ids: Vec<Id> = find_pat_ids(&decl.name);
              for id in ids {
                let symbol =
                  file_module.get_symbol_for_export(id, decl.range());
                fill_var_declarator(symbol, decl);
              }
            }
          }
          Decl::TsInterface(n) => {
            let symbol = file_module
              .get_symbol_for_export(n.id.to_id(), export_decl.range());
            fill_ts_interface(symbol, n);
          }
          Decl::TsTypeAlias(n) => {
            let symbol = file_module
              .get_symbol_for_export(n.id.to_id(), export_decl.range());
            fill_ts_type_alias(symbol, n);
          }
          Decl::TsEnum(n) => {
            let symbol = file_module
              .get_symbol_for_export(n.id.to_id(), export_decl.range());
            fill_ts_enum(symbol, n);
          }
          Decl::TsModule(n) => match &n.id {
            TsModuleName::Ident(ident) => {
              let symbol = file_module
                .get_symbol_for_export(ident.to_id(), export_decl.range());
              fill_ts_module(symbol, n);
            }
            TsModuleName::Str(_) => todo!(),
          },
        },
        ModuleDecl::ExportNamed(_) => todo!(),
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
              let id = match &n.id {
                TsModuleName::Ident(ident) => ident.to_id(),
                TsModuleName::Str(_) => todo!(),
              };
              let symbol = file_module.get_symbol_from_swc_id(id, n.range());
              fill_ts_module(symbol, n);

              // todo: need to actually do this
              if let Some(body) = &n.body {
                match body {
                  TsNamespaceBody::TsModuleBlock(block) => {}
                  TsNamespaceBody::TsNamespaceDecl(_) => {}
                }
              }
              // now go down through all the children
            }
          };
        }
      },
    }
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

fn fill_ts_module(symbol: &mut Symbol, n: &TsModuleDecl) {
  // fill the exported declarations
  if let Some(body) = &n.body {
    let mut current = body;
    let block = loop {
      match current {
        TsNamespaceBody::TsModuleBlock(block) => break block,
        TsNamespaceBody::TsNamespaceDecl(decl) => current = &decl.body,
      }
    };
    for item in &block.body {
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

fn fill_ts_type(symbol: &mut Symbol, ts_type: &TsType) {
  match ts_type {
    TsType::TsKeywordType(n) => fill_ts_keyword_type(symbol, n),
    TsType::TsThisType(n) => fill_ts_this_type(symbol, n),
    TsType::TsFnOrConstructorType(n) => {
      fill_ts_fn_or_constructor_type(symbol, n)
    }
    TsType::TsTypeRef(n) => fill_ts_type_ref(symbol, n),
    TsType::TsTypeQuery(n) => fill_ts_type_query(symbol, n),
    TsType::TsTypeLit(n) => fill_ts_type_lit(symbol, n),
    TsType::TsArrayType(n) => fill_ts_array_type(symbol, n),
    TsType::TsTupleType(n) => fill_ts_tuple_type(symbol, n),
    TsType::TsOptionalType(n) => fill_ts_optional_type(symbol, n),
    TsType::TsRestType(n) => fill_ts_rest_type(symbol, n),
    TsType::TsUnionOrIntersectionType(n) => {
      fill_ts_union_or_intersection_type(symbol, n)
    }
    TsType::TsConditionalType(n) => fill_ts_conditional_type(symbol, n),
    TsType::TsInferType(n) => fill_ts_infer_type(symbol, n),
    TsType::TsParenthesizedType(n) => fill_ts_parenthesized_type(symbol, n),
    TsType::TsTypeOperator(n) => fill_ts_type_operator(symbol, n),
    TsType::TsIndexedAccessType(n) => fill_ts_indexed_access_type(symbol, n),
    TsType::TsMappedType(n) => fill_ts_mapped_type(symbol, n),
    TsType::TsLitType(n) => fill_ts_lit_type(symbol, n),
    TsType::TsTypePredicate(n) => fill_ts_type_predicate(symbol, n),
    TsType::TsImportType(n) => fill_ts_import_type(symbol, n),
  }
}

fn fill_ts_keyword_type(_symbol: &mut Symbol, _n: &TsKeywordType) {
  // nothing to do, no dependencies
}

fn fill_ts_this_type(_symbol: &mut Symbol, _n: &TsThisType) {
  // nothing to do, no dependencies
}

fn fill_ts_fn_or_constructor_type(
  symbol: &mut Symbol,
  n: &TsFnOrConstructorType,
) {
  todo!()
}

fn fill_ts_type_ref(symbol: &mut Symbol, n: &TsTypeRef) {
  match &n.type_name {
    TsEntityName::TsQualifiedName(_) => todo!(),
    TsEntityName::Ident(ident) => {
      symbol.deps.insert(ident.to_id());
    }
  }

  if let Some(type_params) = &n.type_params {
    fill_ts_type_param_instantiation(symbol, type_params)
  }
}

fn fill_ts_type_query(symbol: &mut Symbol, n: &TsTypeQuery) {
  todo!()
}

fn fill_ts_type_lit(symbol: &mut Symbol, n: &TsTypeLit) {
  todo!()
}

fn fill_ts_array_type(symbol: &mut Symbol, n: &TsArrayType) {
  todo!()
}

fn fill_ts_tuple_type(symbol: &mut Symbol, n: &TsTupleType) {
  todo!()
}

fn fill_ts_optional_type(symbol: &mut Symbol, n: &TsOptionalType) {
  todo!()
}

fn fill_ts_rest_type(symbol: &mut Symbol, n: &TsRestType) {
  todo!()
}

fn fill_ts_union_or_intersection_type(
  symbol: &mut Symbol,
  n: &TsUnionOrIntersectionType,
) {
  todo!()
}

fn fill_ts_conditional_type(symbol: &mut Symbol, n: &TsConditionalType) {
  fill_ts_type(symbol, &n.check_type);
  fill_ts_type(symbol, &n.extends_type);
  fill_ts_type(symbol, &n.true_type);
  fill_ts_type(symbol, &n.false_type);
}

fn fill_ts_infer_type(symbol: &mut Symbol, n: &TsInferType) {
  todo!()
}

fn fill_ts_parenthesized_type(symbol: &mut Symbol, n: &TsParenthesizedType) {
  todo!()
}

fn fill_ts_type_operator(symbol: &mut Symbol, n: &TsTypeOperator) {
  todo!()
}

fn fill_ts_indexed_access_type(symbol: &mut Symbol, n: &TsIndexedAccessType) {
  todo!()
}

fn fill_ts_mapped_type(symbol: &mut Symbol, n: &TsMappedType) {
  todo!()
}

fn fill_ts_lit_type(symbol: &mut Symbol, n: &TsLitType) {
  // nothing to do
}

fn fill_ts_type_predicate(symbol: &mut Symbol, n: &TsTypePredicate) {
  todo!()
}

fn fill_ts_import_type(symbol: &mut Symbol, n: &TsImportType) {
  todo!()
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
