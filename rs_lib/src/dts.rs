use std::collections::HashSet;
use std::rc::Rc;

use deno_ast::swc::ast::*;
use deno_ast::swc::common::comments::CommentKind;
use deno_ast::swc::common::comments::SingleThreadedComments;
use deno_ast::swc::common::FileName;
use deno_ast::swc::common::SourceMap;
use deno_ast::swc::common::Spanned;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::visit::*;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_ast::SourcePos;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned;
use deno_graph::type_tracer::ImportedExports;
use deno_graph::type_tracer::ModuleId;
use deno_graph::type_tracer::RootSymbol;
use deno_graph::CapturingModuleParser;
use deno_graph::ModuleGraph;

use crate::helpers::adjust_spans;
use crate::helpers::fill_leading_comments;
use crate::helpers::ident;
use crate::helpers::is_remote_specifier;
use crate::helpers::print_program;
use crate::helpers::ts_keyword_type;
use crate::Diagnostic;
use crate::Reporter;

struct LibPackTypeTraceHandler<'a, TReporter: Reporter>(&'a TReporter);

impl<'a, TReporter: Reporter> deno_graph::type_tracer::TypeTraceHandler
  for LibPackTypeTraceHandler<'a, TReporter>
{
  fn diagnostic(
    &self,
    diagnostic: deno_graph::type_tracer::TypeTraceDiagnostic,
  ) {
    self.0.diagnostic(Diagnostic {
        message: match diagnostic.kind {
            deno_graph::type_tracer::TypeTraceDiagnosticKind::UnsupportedDefaultExpr => concat!(
              "Default expressions that are not identifiers are not supported. ",
              "To work around this, extract out the expression to a variable, ",
              "type the variable, and then default export the variable declaration.",
            ).to_string(),
        },
        specifier: diagnostic.specifier,
        line_and_column: diagnostic.line_and_column.map(|line_and_column| line_and_column.into()),
      });
  }
}

pub fn pack_dts(
  graph: &ModuleGraph,
  parser: &CapturingModuleParser,
  reporter: &impl Reporter,
) -> Result<String, anyhow::Error> {
  // run the tracer
  let root_symbol = deno_graph::type_tracer::trace_public_types(
    &graph,
    &graph.roots,
    parser,
    &LibPackTypeTraceHandler(reporter),
  )?;

  let source_map = Rc::new(SourceMap::default());
  let global_comments = SingleThreadedComments::default();
  let mut final_module = Module {
    span: DUMMY_SP,
    body: vec![],
    shebang: None,
  };
  let mut remote_module_items = Vec::new();
  let mut default_remote_module_items = Vec::new();

  for graph_module in graph.modules() {
    if is_remote_specifier(graph_module.specifier()) {
      if let Some(module_symbol) = root_symbol
        .get_module_from_specifier(graph_module.specifier())
        .and_then(|m| m.esm())
      {
        let has_locally_imported_remote_default = module_symbol
          .traced_referrers()
          .iter()
          .any(|(module_id, imported_exports)| {
            let Some(module) = root_symbol.get_module_from_id(*module_id) else {
              return false;
            };
            module.specifier().scheme() == "file"
              && match imported_exports {
                ImportedExports::AllWithDefault => true,
                ImportedExports::Star => false,
                ImportedExports::Named(named) => named.contains("default"),
              }
          });
        if has_locally_imported_remote_default {
          let temp_name = format!(
            "{}DefaultImport",
            module_symbol.module_id().to_code_string()
          );
          remote_module_items.push(ModuleItem::ModuleDecl(ModuleDecl::Import(
            ImportDecl {
              span: DUMMY_SP,
              specifiers: vec![ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                imported: Some(ModuleExportName::Ident(ident(
                  "default".to_string(),
                ))),
                local: ident(temp_name.clone()),
                is_type_only: false,
              })],
              src: Box::new(Str {
                span: DUMMY_SP,
                value: graph_module.specifier().to_string().into(),
                raw: None,
              }),
              type_only: false,
              with: None,
            },
          )));
          // This is done because `import something = defaultImport` is not valid
          // because `defaultImport` is not a namespace, so instead we do:
          //   import { default as pack1DefaultImport } from "...";
          //   declare module pack1Default {
          //     export { pack1DefaultImport as __default };
          //   }
          // Then downstream code will do `import something = pack1Default.__default`
          default_remote_module_items.push(ModuleItem::Stmt(Stmt::Decl(
            Decl::TsModule(Box::new(TsModuleDecl {
              span: DUMMY_SP,
              declare: true,
              global: false,
              id: ident(module_symbol.module_id().to_default_code_string())
                .into(),
              body: Some(TsNamespaceBody::TsModuleBlock(TsModuleBlock {
                span: DUMMY_SP,
                body: Vec::from([ModuleItem::ModuleDecl(
                  ModuleDecl::ExportNamed(NamedExport {
                    span: DUMMY_SP,
                    specifiers: Vec::from([ExportSpecifier::Named(
                      ExportNamedSpecifier {
                        span: DUMMY_SP,
                        orig: ident(temp_name).into(),
                        exported: Some(ident("__default".to_string()).into()),
                        is_type_only: false,
                      },
                    )]),
                    src: None,
                    type_only: false,
                    with: None,
                  }),
                )]),
              })),
            })),
          )))
        }
        let is_locally_imported_remote = module_symbol
          .traced_referrers()
          .iter()
          .any(|(module_id, imported_exports)| {
            let Some(module) = root_symbol.get_module_from_id(*module_id) else {
              return false;
            };
            module.specifier().scheme() == "file"
              && match imported_exports {
                ImportedExports::AllWithDefault => true,
                ImportedExports::Star => true,
                ImportedExports::Named(named) => {
                  // ignore if this only imported the default import
                  named.len() > 1
                    || named.len() == 1 && !named.contains("default")
                }
              }
          });
        if is_locally_imported_remote {
          remote_module_items.push(ModuleItem::ModuleDecl(ModuleDecl::Import(
            ImportDecl {
              span: DUMMY_SP,
              specifiers: vec![ImportSpecifier::Namespace(
                ImportStarAsSpecifier {
                  span: DUMMY_SP,
                  local: ident(module_symbol.module_id().to_code_string()),
                },
              )],
              src: Box::new(Str {
                span: DUMMY_SP,
                value: graph_module.specifier().to_string().into(),
                raw: None,
              }),
              type_only: false,
              with: None,
            },
          )));
        }
      }
    } else {
      if let Some(module_symbol) = root_symbol
        .get_module_from_specifier(graph_module.specifier())
        .and_then(|m| m.esm())
      {
        let ranges = module_symbol.public_source_ranges();
        if !ranges.is_empty() || !module_symbol.traced_re_exports().is_empty() {
          let graph_module = graph_module.esm().unwrap();
          let parsed_source = module_symbol.source();

          let file_name = FileName::Url(graph_module.specifier.clone());
          let source_file = source_map.new_source_file(
            file_name,
            parsed_source.text_info().text().to_string(),
          );

          let mut module = (*parsed_source.module()).clone();
          let is_root = graph.roots.contains(&graph_module.specifier);
          let module_name = if is_root {
            None
          } else {
            Some(module_symbol.module_id().to_code_string())
          };
          // strip all the non-declaration types
          let mut dts_transformer = DtsTransformer {
            reporter,
            module_name,
            module_specifier: &graph_module.specifier,
            module_symbol,
            parsed_source: &parsed_source,
            ranges,
            graph,
            root_symbol: &root_symbol,
            append_module_items: Default::default(),
            re_export_index: 0,
          };
          module.visit_mut_with(&mut dts_transformer);

          // adjust the spans to be within the sourcemap
          adjust_spans(source_file.start_pos, &mut module);

          // Add the file's leading comments to the global comment map.
          // We don't have to deal with the trailing comments because
          // we're only interested in jsdocs
          fill_leading_comments(
            source_file.start_pos,
            &parsed_source,
            &global_comments,
            // only include js docs
            |comment| {
              comment.kind == CommentKind::Block
                && comment.text.starts_with('*')
            },
          );
          final_module.body.extend(module.body);
        }
      }
    }
  }

  final_module.body.splice(
    0..0,
    remote_module_items
      .into_iter()
      .chain(default_remote_module_items.into_iter()),
  );

  print_program(&final_module, &source_map, &global_comments)
}

struct ReExportName(String);

impl ReExportName {
  pub fn to_string(&self) -> String {
    self.0.clone()
  }

  fn into_module_item(self, export_name: String) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
      span: DUMMY_SP,
      specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
        span: DUMMY_SP,
        orig: ModuleExportName::Ident(ident(self.0)),
        exported: Some(ModuleExportName::Ident(ident(export_name))),
        is_type_only: false,
      })],
      src: None,
      type_only: false,
      with: None,
    }))
  }
}

struct DtsTransformer<'a, TReporter: Reporter> {
  reporter: &'a TReporter,
  module_name: Option<String>,
  module_specifier: &'a ModuleSpecifier,
  module_symbol: &'a deno_graph::type_tracer::EsmModuleSymbol,
  parsed_source: &'a ParsedSource,
  ranges: HashSet<SourceRange>,
  graph: &'a ModuleGraph,
  root_symbol: &'a RootSymbol,
  append_module_items: Vec<ModuleItem>,
  re_export_index: u32,
}

impl<'a, TReporter: Reporter> DtsTransformer<'a, TReporter> {
  pub fn next_re_export_name(&mut self) -> ReExportName {
    // exports should not be available in the scope of their module
    // so to work around this, give the export an obscure import name
    // then re-export it with the real name
    self.re_export_index += 1;
    ReExportName(format!("__export{}", self.re_export_index))
  }

  fn has_internal_jsdoc(&self, pos: SourcePos) -> bool {
    has_internal_jsdoc(self.parsed_source, pos)
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
    if self.module_name.is_some() {
      match &*n.expr {
        // convert:
        //   export default a;
        // to:
        //   export { a as __default };
        Expr::Ident(orig) => self.append_module_items.push(
          ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
            span: DUMMY_SP,
            specifiers: Vec::from([ExportSpecifier::Named(
              ExportNamedSpecifier {
                span: DUMMY_SP,
                orig: ModuleExportName::Ident(ident(orig.sym.to_string())),
                exported: Some(ModuleExportName::Ident(ident(
                  "__default".to_string(),
                ))),
                is_type_only: false,
              },
            )]),
            src: None,
            type_only: false,
            with: None,
          })),
        ),
        _ => {}
      }
    }
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
          .parsed_source
          .text_info()
          .line_and_column_display(n.start());
        self.reporter.diagnostic(Diagnostic {
          message: "Missing return type for function with return statement."
            .to_string(),
          specifier: self.module_specifier.clone(),
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
    if self.module_name.is_none() {
      for item in &mut n.body {
        if let ModuleItem::Stmt(Stmt::Decl(decl)) = item {
          match decl {
            Decl::Class(n) => n.declare = true,
            Decl::Fn(n) => n.declare = true,
            Decl::Var(n) => n.declare = true,
            Decl::TsModule(n) => n.declare = true,
            Decl::Using(_)
            | Decl::TsInterface(_)
            | Decl::TsTypeAlias(_)
            | Decl::TsEnum(_) => {
              // ignore
            }
          }
        }
      }
    }

    let mut insert_decls = Vec::new();
    for item in &n.body {
      if let ModuleItem::ModuleDecl(ModuleDecl::Import(import_decl)) = item {
        if !import_decl.specifiers.is_empty() {
          let maybe_specifier = self.graph.resolve_dependency(
            &import_decl.src.value,
            self.module_specifier,
            true,
          );
          if let Some(specifier) = maybe_specifier {
            if let Some(module_symbol) =
              self.root_symbol.get_module_from_specifier(&specifier)
            {
              let module_id = module_symbol.module_id();
              let is_remote = is_remote_specifier(&specifier);
              for specifier in &import_decl.specifiers {
                match specifier {
                  ImportSpecifier::Named(named) => {
                    let maybe_symbol = self
                      .module_symbol
                      .symbol_id_from_swc(&named.local.to_id())
                      .and_then(|symbol_id| {
                        self.module_symbol.symbol(symbol_id)
                      });
                    match maybe_symbol {
                      Some(symbol) if !symbol.is_public() => {
                        continue;
                      }
                      None => {
                        continue;
                      }
                      _ => {}
                    }

                    let imported_name = named
                      .imported
                      .as_ref()
                      .map(|i| match i {
                        ModuleExportName::Ident(ident) => ident.sym.to_string(),
                        ModuleExportName::Str(_) => todo!(),
                      })
                      .unwrap_or_else(|| named.local.sym.to_string());

                    insert_decls.push(ModuleItem::ModuleDecl(
                      ModuleDecl::TsImportEquals(Box::new(
                        TsImportEqualsDecl {
                          span: DUMMY_SP,
                          is_export: false,
                          is_type_only: false,
                          id: named.local.clone(),
                          module_ref: TsModuleRef::TsEntityName(
                            TsEntityName::TsQualifiedName(Box::new(
                              TsQualifiedName {
                                left: TsEntityName::Ident(ident(
                                  if is_remote && imported_name == "default" {
                                    module_id.to_default_code_string()
                                  } else {
                                    module_id.to_code_string()
                                  },
                                )),
                                right: ident(if imported_name == "default" {
                                  "__default".to_string()
                                } else {
                                  imported_name
                                }),
                              },
                            )),
                          ),
                        },
                      )),
                    ));
                  }
                  ImportSpecifier::Default(specifier) => {
                    let maybe_symbol = self
                      .module_symbol
                      .symbol_id_from_swc(&specifier.local.to_id())
                      .and_then(|symbol_id| {
                        self.module_symbol.symbol(symbol_id)
                      });
                    match maybe_symbol {
                      Some(symbol) if !symbol.is_public() => {
                        continue;
                      }
                      None => {
                        continue;
                      }
                      _ => {}
                    }

                    insert_decls.push(ModuleItem::ModuleDecl(
                      ModuleDecl::TsImportEquals(Box::new(
                        TsImportEqualsDecl {
                          span: DUMMY_SP,
                          is_export: false,
                          is_type_only: false,
                          id: specifier.local.clone(),
                          module_ref: TsModuleRef::TsEntityName(
                            TsEntityName::TsQualifiedName(Box::new(
                              TsQualifiedName {
                                left: TsEntityName::Ident(ident(
                                  if is_remote {
                                    module_id.to_default_code_string()
                                  } else {
                                    module_id.to_code_string()
                                  },
                                )),
                                // can't use `.default` because it's a reserved word,
                                // so use our custom `__default` instead
                                right: ident("__default".to_string()),
                              },
                            )),
                          ),
                        },
                      )),
                    ));
                  }
                  ImportSpecifier::Namespace(specifier) => {
                    let maybe_symbol = self
                      .module_symbol
                      .symbol_id_from_swc(&specifier.local.to_id())
                      .and_then(|symbol_id| {
                        self.module_symbol.symbol(symbol_id)
                      });
                    match maybe_symbol {
                      Some(symbol) if !symbol.is_public() => {
                        continue;
                      }
                      None => {
                        continue;
                      }
                      _ => {}
                    }
                    insert_decls.push(ModuleItem::ModuleDecl(
                      ModuleDecl::TsImportEquals(Box::new(
                        TsImportEqualsDecl {
                          span: DUMMY_SP,
                          is_export: false,
                          is_type_only: false,
                          id: specifier.local.clone(),
                          module_ref: TsModuleRef::TsEntityName(
                            TsEntityName::Ident(ident(
                              module_id.to_code_string(),
                            )),
                          ),
                        },
                      )),
                    ));
                  }
                }
              }
            }
          }
        }
      }
    }

    n.body.splice(0..0, insert_decls);

    for (name, global_symbol) in self.module_symbol.traced_re_exports() {
      let private_name = self.next_re_export_name();
      n.body
        .push(ModuleItem::ModuleDecl(ModuleDecl::TsImportEquals(
          Box::new(TsImportEqualsDecl {
            span: DUMMY_SP,
            is_export: false,
            is_type_only: false,
            id: ident(private_name.to_string()),
            module_ref: TsModuleRef::TsEntityName(
              TsEntityName::TsQualifiedName(Box::new(TsQualifiedName {
                left: TsEntityName::Ident(ident(
                  global_symbol.module_id.to_code_string(),
                )),
                right: ident(name.clone()),
              })),
            ),
          }),
        )));
      n.body.push(private_name.into_module_item(name.clone()));
    }

    visit_mut_module(self, n);

    if let Some(module_name) = self.module_name.clone() {
      let module_items = n.body.drain(..).collect::<Vec<_>>();
      n.body
        .push(ModuleItem::Stmt(Stmt::Decl(Decl::TsModule(Box::new(
          TsModuleDecl {
            span: DUMMY_SP,
            declare: true,
            global: false,
            id: TsModuleName::Ident(ident(module_name.into())),
            body: Some(TsNamespaceBody::TsModuleBlock(TsModuleBlock {
              span: DUMMY_SP,
              body: module_items,
            })),
          },
        )))));
    }
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
      let should_retain =
        item.span() == DUMMY_SP || self.ranges.contains(&item.range());
      if should_retain {
        return true;
      }

      let decl = match item {
        ModuleItem::ModuleDecl(decl) => decl.as_export_decl().map(|d| &d.decl),
        ModuleItem::Stmt(stmt) => stmt.as_decl(),
      };
      if let Some(decl) = decl {
        // check if any variable declaration individually is traced
        decl
          .as_var()
          .map(|d| {
            d.decls.iter().any(|decl| {
              decl.span() == DUMMY_SP || self.ranges.contains(&decl.range())
            })
          })
          .unwrap_or(false)
      } else if let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(named)) =
        item
      {
        named
          .specifiers
          .iter()
          .any(|s| s.span() == DUMMY_SP || self.ranges.contains(&s.range()))
      } else {
        false
      }
    });

    visit_mut_module_items(self, n);
    n.retain(|item| {
      if let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item {
        if export.specifiers.is_empty() {
          return false;
        }
      }
      if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(_)) = item {
        // don't keep export default exprs in the namespaces
        return self.module_name.is_none();
      }
      true
    });
    n.extend(self.append_module_items.drain(..));

    // todo: temporary workaround until https://github.com/microsoft/TypeScript/issues/54446 is fixed
    let should_insert_ts_under_5_2_workaround = self.module_name.is_some()
      && n.iter().all(|n| match n {
        ModuleItem::ModuleDecl(decl) => match decl {
          ModuleDecl::TsImportEquals(import_equals) => !import_equals.is_export,
          ModuleDecl::ExportNamed(_) => true,
          _ => false,
        },
        ModuleItem::Stmt(_) => false,
      });
    if should_insert_ts_under_5_2_workaround {
      // for some reason, adding a dummy declaration will fix the error
      n.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
          span: DUMMY_SP,
          name: Pat::Ident(BindingIdent {
            id: ident("__packTsUnder5_2_Workaround__".into()),
            type_ann: Some(Box::new(TsTypeAnn {
              span: DUMMY_SP,
              type_ann: Box::new(ts_keyword_type(
                TsKeywordTypeKind::TsUnknownKeyword,
              )),
            })),
          }),
          init: None,
          definite: false,
        }],
      })))))
    }
  }

  fn visit_mut_named_export(&mut self, n: &mut NamedExport) {
    n.specifiers
      .retain(|s| s.span() == DUMMY_SP || self.ranges.contains(&s.range()));
    if let Some(src) = n.src.as_ref().map(|s| s.value.to_string()) {
      let maybe_src_specifier =
        self
          .graph
          .resolve_dependency(&src, self.module_specifier, true);
      if let Some(src_specifier) = maybe_src_specifier {
        let src_module_id = self
          .root_symbol
          .get_module_from_specifier(&src_specifier)
          .unwrap()
          .module_id();
        for specifier in &mut n.specifiers {
          match specifier {
            ExportSpecifier::Named(named) => {
              let export_name = named
                .exported
                .as_ref()
                .map(|e| match e {
                  ModuleExportName::Ident(ident) => ident.sym.to_string(),
                  ModuleExportName::Str(_) => todo!(),
                })
                .clone()
                .unwrap_or_else(|| match &named.orig {
                  ModuleExportName::Ident(ident) => ident.sym.to_string(),
                  ModuleExportName::Str(_) => todo!(),
                });
              let private_name = self.next_re_export_name();
              self.append_module_items.push(ModuleItem::ModuleDecl(
                ModuleDecl::TsImportEquals(Box::new(TsImportEqualsDecl {
                  span: DUMMY_SP,
                  is_export: false,
                  is_type_only: false,
                  id: ident(private_name.to_string()),
                  module_ref: TsModuleRef::TsEntityName(
                    TsEntityName::TsQualifiedName(Box::new(TsQualifiedName {
                      left: TsEntityName::Ident(ident(
                        src_module_id.to_code_string(),
                      )),
                      right: ident(match &named.orig {
                        ModuleExportName::Ident(ident) => {
                          let name = ident.sym.to_string();
                          if name == "default" {
                            "__default".to_string()
                          } else {
                            name
                          }
                        }
                        ModuleExportName::Str(_) => todo!(),
                      }),
                    })),
                  ),
                })),
              ));
              self
                .append_module_items
                .push(private_name.into_module_item(export_name));
            }
            ExportSpecifier::Namespace(specifier) => {
              let export_name = match &specifier.name {
                ModuleExportName::Ident(ident) => ident.sym.to_string(),
                ModuleExportName::Str(_) => todo!(),
              };
              let private_name = self.next_re_export_name();
              self.append_module_items.push(ModuleItem::ModuleDecl(
                ModuleDecl::TsImportEquals(Box::new(TsImportEqualsDecl {
                  span: DUMMY_SP,
                  is_export: false,
                  is_type_only: false,
                  id: ident(private_name.to_string()),
                  module_ref: TsModuleRef::TsEntityName(TsEntityName::Ident(
                    ident(src_module_id.to_code_string()),
                  )),
                })),
              ));
              self
                .append_module_items
                .push(private_name.into_module_item(export_name));
            }
            ExportSpecifier::Default(_) => todo!(),
          }
        }
        n.specifiers.clear();
      }
    }
    n.src = None;
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
    n.decls.retain(|decl| {
      decl.span() == DUMMY_SP || self.ranges.contains(&decl.range())
    });
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

fn maybe_infer_type_from_expr(expr: &Expr) -> Option<TsType> {
  match expr {
    Expr::TsTypeAssertion(n) => Some(*n.type_ann.clone()),
    Expr::TsAs(n) => Some(*n.type_ann.clone()),
    Expr::Lit(lit) => {
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

trait ModuleIdExtensions {
  fn to_default_code_string(&self) -> String;
  fn to_code_string(&self) -> String;
}

impl ModuleIdExtensions for ModuleId {
  fn to_default_code_string(&self) -> String {
    format!("pack{}Default", self)
  }

  fn to_code_string(&self) -> String {
    format!("pack{}", self)
  }
}
