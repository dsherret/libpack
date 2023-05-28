use std::collections::HashMap;
use std::collections::HashSet;

use deno_ast::ModuleSpecifier;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned;
use deno_ast::swc::visit::*;
use deno_ast::swc::ast::*;
use deno_ast::ParsedSource;

// Key point:
// Only interested in creating a graph of dependencies of the public api.
// I think probably creating a hash of swc id -> symbol should work.

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct SymbolId(u32);

#[derive(Default)]
pub struct Symbol {
  // todo: store any implicit types here
  // todo: store file dependencies here and what export names are used
  is_public: bool,
  decls: Vec<SourceRange>,
  deps: HashSet<Id>,
}

impl Symbol {
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
}

#[derive(Default)]
pub struct ModuleSymbol {
  next_id: SymbolId,
  exports: HashMap<String, SymbolId>,
  // note: not all symbol ids have an swc id. For example, default exports
  swc_id_to_symbol_id: HashMap<Id, SymbolId>,
  symbols: HashMap<SymbolId, Symbol>,
}

impl ModuleSymbol {
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

  pub fn symbol(&mut self, id: SymbolId) -> Option<&mut Symbol> {
    self.symbols.get_mut(&id)
  }

  pub fn get_next_symbol_id(&mut self) -> SymbolId {
    let next_id = self.next_id;
    self.next_id = SymbolId(self.next_id.0 + 1);
    next_id
  }

  fn get_symbol_from_swc_id(&mut self, id: Id, symbol_range: SourceRange) -> &mut Symbol {
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
    self.symbols.get_mut(&symbol_id).unwrap()
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
      let module_symbol = analyze_module(module);
      self.modules.insert(source.specifier().to_string(), module_symbol);
    }
    self.modules.get_mut(source.specifier()).unwrap()
  }
}

fn analyze_module(module: &Module) -> ModuleSymbol {
  let mut file_module = ModuleSymbol::default();
  for module_item in &module.body {
    match module_item {
        ModuleItem::ModuleDecl(decl) => match decl {
            ModuleDecl::Import(_) => todo!(),
            ModuleDecl::ExportDecl(_) => todo!(),
            ModuleDecl::ExportNamed(_) => todo!(),
            ModuleDecl::ExportDefaultDecl(_) => todo!(),
            ModuleDecl::ExportDefaultExpr(_) => todo!(),
            ModuleDecl::ExportAll(_) => todo!(),
            ModuleDecl::TsImportEquals(_) => todo!(),
            ModuleDecl::TsExportAssignment(_) => todo!(),
            ModuleDecl::TsNamespaceExport(_) => todo!(),
        },
        ModuleItem::Stmt(stmt) => match stmt {
            Stmt::Block(_) |
            Stmt::Empty(_) |
            Stmt::Debugger(_) |
            Stmt::With(_) |
            Stmt::Return(_) |
            Stmt::Labeled(_) |
            Stmt::Break(_) |
            Stmt::Continue(_) |
            Stmt::If(_) |
            Stmt::Switch(_) |
            Stmt::Throw(_) |
            Stmt::Try(_) |
            Stmt::While(_) |
            Stmt::DoWhile(_) |
            Stmt::For(_) |
            Stmt::ForIn(_) |
            Stmt::ForOf(_) |
            Stmt::Expr(_) => {
              // ignore
            }
            Stmt::Decl(decl) => {
              match decl {
                Decl::Class(n) => {
                  let id = n.ident.to_id();
                  let symbol = file_module.get_symbol_from_swc_id(id, n.range());
                  fill_class_decl(symbol, n);
                },
                Decl::Fn(_) => todo!(),
                Decl::Var(_) => todo!(),
                Decl::TsInterface(_) => todo!(),
                Decl::TsTypeAlias(_) => todo!(),
                Decl::TsEnum(_) => todo!(),
                Decl::TsModule(_) => todo!(),
            }
              todo!()
            }
        },
    }
  }
  file_module
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

  fn visit_ts_qualified_name(
    &mut self,
    n: &TsQualifiedName,
  ) {
    todo!();
  }
}

fn fill_class_decl(symbol: &mut Symbol, n: &ClassDecl) {
    if let Some(type_params) = &n.class.type_params {
      fill_ts_type_param_decl(symbol, type_params);
    }
    if let Some(expr) = &n.class.super_class {
      fill_expr(symbol, expr);
    }
    if let Some(type_params) = &n.class.super_type_params {
      fill_ts_type_param_instantiation(symbol, type_params)
    }
    for expr in &n.class.implements {
      fill_ts_expr_with_type_args(symbol, expr);
    }
    fill_ts_class_members(symbol, &n.class.body);
}

fn fill_ts_expr_with_type_args(symbol: &mut Symbol, n: &TsExprWithTypeArgs) {
  if let Some(type_args) = &n.type_args {
    fill_ts_type_param_instantiation(symbol, type_args);
  }
  fill_expr(symbol, &n.expr);
}

fn fill_ts_type_param_decl(symbol: &mut Symbol, type_params: &TsTypeParamDecl) {
    todo!()
}

fn fill_ts_type_param_instantiation(symbol: &mut Symbol, type_params: &TsTypeParamInstantiation) {
    todo!()
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
        ClassMember::TsIndexSignature(signature) => fill_ts_index_signature(symbol, signature),
        ClassMember::AutoAccessor(prop) => fill_auto_accessor(symbol, prop),
        ClassMember::StaticBlock(_) | ClassMember::Empty(_) => {
          // ignore
        },
    }
  }
}

fn fill_ctor(symbol: &mut Symbol, ctor: &Constructor) {
    todo!()
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

fn fill_param(symbol: &mut Symbol, param: &Param) {
  todo!()
}

fn fill_ts_type_ann(symbol: &mut Symbol, return_type: &TsTypeAnn) {
    todo!()
}

fn fill_private_method(symbol: &mut Symbol, method: &PrivateMethod) {
    todo!()
}

fn fill_class_prop(symbol: &mut Symbol, prop: &ClassProp) {
    todo!()
}

fn fill_private_prop(symbol: &mut Symbol, prop: &PrivateProp) {
    todo!()
}

fn fill_ts_index_signature(symbol: &mut Symbol, signature: &TsIndexSignature) {
    todo!()
}

fn fill_auto_accessor(symbol: &mut Symbol, prop: &AutoAccessor) {
    todo!()
}