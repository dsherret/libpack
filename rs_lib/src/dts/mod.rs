use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;
use std::string::FromUtf8Error;

use deno_ast::SourceRange;
use deno_ast::swc::ast::*;
use deno_ast::swc::codegen;
use deno_ast::swc::codegen::text_writer::JsWriter;
use deno_ast::swc::codegen::Node;
use deno_ast::swc::common::BytePos;
use deno_ast::swc::common::FileName;
use deno_ast::swc::common::SourceMap;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::common::Span;
use deno_ast::swc::common::comments::Comment;
use deno_ast::swc::common::comments::Comments;
use deno_ast::swc::common::comments::SingleThreadedComments;
use deno_ast::swc::visit::*;
use deno_ast::ModuleSpecifier;
use deno_ast::SourceMapConfig;
use deno_ast::SourceRangedForSpanned;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;
use deno_graph::ModuleParser;

use crate::dts::binder::ModuleAnalyzer;

mod analyzer;
mod binder;
mod tracer;

// 1. Do a first analysis pass. Collect all "id"s that should be maintained.
// 2. Visit all modules found in the analysis pass and transform using swc
//    to a dts file

pub fn pack_dts(
  graph: &ModuleGraph,
  parser: &CapturingModuleParser,
) -> Result<String, anyhow::Error> {

  // run the tracer
  let module_analyzer = tracer::trace(graph, parser)?;

  //let mut dts_transformed_files = HashMap::new();
  let source_map = Rc::new(SourceMap::default());
  let global_comments = SingleThreadedComments::default();
  let mut final_module = Module {
    span: DUMMY_SP,
    body: vec![],
    shebang: None,
  };

  for graph_module in graph.modules() {
    if let Some(module_symbol) = module_analyzer.get(graph_module.specifier()) {
      let ranges = module_symbol.public_source_ranges();
      if !ranges.is_empty() {
        let graph_module = graph_module.esm().unwrap();
        // todo: consolidate with other code that does this
        let parsed_source = parser.parse_module(
          &graph_module.specifier,
          graph_module.source.clone(),
          graph_module.media_type,
        )?;

        let file_name = FileName::Url(graph_module.specifier.clone());
        let source_file = source_map
          .new_source_file(file_name, parsed_source.text_info().text().to_string());

        let mut module = (*parsed_source.module()).clone();
        let is_root = graph.roots.contains(&graph_module.specifier);
        // strip all the non-declaration types
        let mut dts_transformer = DtsTransformer {
          ranges,
        };
        module.visit_mut_with(&mut dts_transformer);

        // adjust the spans to be within the sourcemap
        let mut span_adjuster = SpanAdjuster {
          start_pos: source_file.start_pos,
        };
        module.visit_mut_with(&mut span_adjuster);

        // add the file's comments to the global comment map
        for (byte_pos, comment_vec) in parsed_source.comments().leading_map() {
          let byte_pos = source_file.start_pos + *byte_pos;
          for comment in comment_vec {
            global_comments.add_leading(byte_pos, Comment {
              kind: comment.kind,
              span: Span::new(source_file.start_pos + comment.span.lo, source_file.start_pos + comment.span.hi, comment.span.ctxt),
              text: comment.text.clone(),
            });
          }
        }
        final_module.body.extend(module.body);
      }
    }
  }

  let mut src_map_buf = vec![];
  let mut buf = vec![];
  {
    let writer = Box::new(JsWriter::new(
      source_map.clone(),
      "\n",
      &mut buf,
      Some(&mut src_map_buf),
    ));
    let config = codegen::Config {
      minify: false,
      ascii_only: false,
      omit_last_semi: false,
      target: deno_ast::ES_VERSION,
    };
    let mut emitter = codegen::Emitter {
      cfg: config,
      comments: Some(&global_comments),
      cm: source_map.clone(),
      wr: writer,
    };
    final_module.emit_with(&mut emitter)?;
  }
  Ok(String::from_utf8(buf)?)
}

struct DtsTransformer {
  ranges: HashSet<SourceRange>,
}

impl VisitMut for DtsTransformer {
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
    n.body.retain(|member| match member {
      ClassMember::Constructor(_) => true,
      ClassMember::Method(method) => {
        method.accessibility != Some(Accessibility::Private)
      }
      ClassMember::ClassProp(prop) => {
        prop.accessibility != Some(Accessibility::Private)
      }
      ClassMember::TsIndexSignature(_) => true,
      ClassMember::PrivateProp(_)
      | ClassMember::PrivateMethod(_)
      | ClassMember::Empty(_)
      | ClassMember::StaticBlock(_) => false,
      ClassMember::AutoAccessor(_) => true,
    });
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
    n.value = None;
    if n.type_ann.is_none() {
      n.type_ann = Some(Box::new(TsTypeAnn {
        span: DUMMY_SP,
        type_ann: Box::new(TsType::TsKeywordType(TsKeywordType {
          span: DUMMY_SP,
          kind: TsKeywordTypeKind::TsUnknownKeyword,
        })),
      }));
    }
    visit_mut_class_prop(self, n)
  }

  fn visit_mut_computed_prop_name(&mut self, n: &mut ComputedPropName) {
    visit_mut_computed_prop_name(self, n)
  }

  fn visit_mut_constructor(&mut self, n: &mut Constructor) {
    n.body = None;
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
    visit_mut_export_default_expr(self, n)
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
    // insert a void type for explicit return types
    if n.return_type.is_none() {
      // todo: this should go into if statements and other things as well
      let is_last_return = n
        .body
        .as_ref()
        .and_then(|b| b.stmts.last())
        .map(|last_stmt| matches!(last_stmt, Stmt::Return(..)))
        .unwrap_or(false);

      if !is_last_return {
        // todo: add filename with line and column number
        eprintln!("Warning: no return type. Using void.");
      }

      n.return_type = Some(Box::new(TsTypeAnn {
        span: DUMMY_SP,
        type_ann: Box::new(TsType::TsKeywordType(TsKeywordType {
          span: DUMMY_SP,
          kind: TsKeywordTypeKind::TsVoidKeyword,
        })),
      }));
    }
    n.body = None;
    visit_mut_function(self, n)
  }

  fn visit_mut_getter_prop(&mut self, n: &mut GetterProp) {
    visit_mut_getter_prop(self, n)
  }

  fn visit_mut_ident(&mut self, n: &mut Ident) {
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
    visit_mut_module(self, n)
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
    n.retain(|item| {
      match item {
        ModuleItem::ModuleDecl(decl) => {
          // todo: remove some of these
          true
        }
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
          | Stmt::Expr(_) => false,
          Stmt::Decl(_) => true,
        },
      }
    });
    n.retain(|item| {
      self.ranges.contains(&item.range())
    });
    visit_mut_module_items(self, n)
  }

  fn visit_mut_named_export(&mut self, n: &mut NamedExport) {
    visit_mut_named_export(self, n)
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

  fn visit_mut_var_decl_kind(&mut self, n: &mut deno_ast::view::VarDeclKind) {
    visit_mut_var_decl_kind(self, n)
  }

  fn visit_mut_var_decl_or_expr(&mut self, n: &mut VarDeclOrExpr) {
    visit_mut_var_decl_or_expr(self, n)
  }

  fn visit_mut_var_decl_or_pat(&mut self, n: &mut VarDeclOrPat) {
    visit_mut_var_decl_or_pat(self, n)
  }

  fn visit_mut_var_declarator(&mut self, n: &mut VarDeclarator) {
    n.definite = false;
    n.init = None;
    visit_mut_var_declarator(self, n)
  }

  fn visit_mut_var_declarators(&mut self, n: &mut Vec<VarDeclarator>) {
    visit_mut_var_declarators(self, n)
  }
}

struct SpanAdjuster {
  start_pos: BytePos,
}

impl VisitMut for SpanAdjuster {
  fn visit_mut_span(&mut self, span: &mut Span) {
    if !span.is_dummy() {
      // adjust the span to be within the source map
      span.lo = self.start_pos + span.lo;
      span.hi = self.start_pos + span.hi;
    }
  }
}