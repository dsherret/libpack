use std::rc::Rc;

use deno_ast::swc::ast::*;
use deno_ast::swc::codegen;
use deno_ast::swc::codegen::text_writer::JsWriter;
use deno_ast::swc::codegen::Node;
use deno_ast::swc::common::comments::Comment;
use deno_ast::swc::common::comments::Comments;
use deno_ast::swc::common::comments::SingleThreadedComments;
use deno_ast::swc::common::BytePos;
use deno_ast::swc::common::SourceMap;
use deno_ast::swc::common::Span;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::visit::VisitMut;
use deno_ast::swc::visit::VisitMutWith;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;

pub fn ident(name: String) -> Ident {
  Ident {
    span: DUMMY_SP,
    sym: name.clone().into(),
    optional: false,
  }
}

pub fn ts_keyword_type(kind: TsKeywordTypeKind) -> TsType {
  TsType::TsKeywordType(TsKeywordType {
    span: DUMMY_SP,
    kind,
  })
}

pub fn export_x_as_y(x: String, y: String) -> ModuleItem {
  ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
    span: DUMMY_SP,
    specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
      span: DUMMY_SP,
      orig: ident(x).into(),
      exported: Some(ident(y).into()),
      is_type_only: false,
    })],
    src: None,
    type_only: false,
    with: None,
  }))
}

pub fn member_x_y(left: String, right: String) -> MemberExpr {
  MemberExpr {
    span: DUMMY_SP,
    obj: ident(left).into(),
    prop: ident(right).into(),
  }
}

pub fn const_var_decl(name: String, init: Expr) -> VarDecl {
  VarDecl {
    span: DUMMY_SP,
    kind: VarDeclKind::Const,
    declare: false,
    decls: vec![VarDeclarator {
      span: DUMMY_SP,
      name: ident(name).into(),
      init: Some(Box::new(init)),
      definite: false,
    }],
  }
}

pub fn object_define_property(name: String, key: String, expr: Expr) -> Stmt {
  Stmt::Expr(ExprStmt {
    span: DUMMY_SP,
    expr: Box::new(Expr::Call(CallExpr {
      span: DUMMY_SP,
      callee: Callee::Expr(Box::new(Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: ident("Object".to_string()).into(),
        prop: ident("defineProperty".to_string()).into(),
      }))),
      args: Vec::from([
        ExprOrSpread {
          expr: Box::new(Expr::Ident(ident(name))),
          spread: None,
        },
        ExprOrSpread {
          expr: Box::new(Expr::Lit(Lit::Str(Str {
            span: DUMMY_SP,
            value: key.into(),
            raw: None,
          }))),
          spread: None,
        },
        ExprOrSpread {
          expr: Box::new(Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: Vec::from([PropOrSpread::Prop(Box::new(Prop::KeyValue(
              KeyValueProp {
                key: ident("get".to_string()).into(),
                value: Box::new(Expr::Arrow(ArrowExpr {
                  span: DUMMY_SP,
                  params: Vec::new(),
                  body: Box::new(BlockStmtOrExpr::Expr(expr.into())),
                  is_async: false,
                  is_generator: false,
                  type_params: None,
                  return_type: None,
                })),
              },
            )))]),
          })),
          spread: None,
        },
      ]),
      type_args: None,
    })),
  })
}

pub fn module_has_default_export(module: &Module) -> bool {
  module.body.iter().any(|item| match item {
    ModuleItem::ModuleDecl(decl) => match decl {
      ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
        true
      }
      ModuleDecl::Import(_)
      | ModuleDecl::ExportDecl(_)
      | ModuleDecl::ExportNamed(_)
      | ModuleDecl::ExportAll(_)
      | ModuleDecl::TsImportEquals(_)
      | ModuleDecl::TsExportAssignment(_)
      | ModuleDecl::TsNamespaceExport(_) => false,
    },
    ModuleItem::Stmt(_) => false,
  })
}

pub fn is_remote_specifier(specifier: &ModuleSpecifier) -> bool {
  matches!(specifier.scheme(), "https" | "http")
}

pub fn adjust_spans(start_pos: BytePos, module: &mut Module) {
  let mut span_adjuster = SpanAdjuster { start_pos };
  module.visit_mut_with(&mut span_adjuster);
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

pub fn print_program(
  program: &impl Node,
  source_map: &Rc<SourceMap>,
  comments: &SingleThreadedComments,
) -> Result<String, anyhow::Error> {
  let mut src_map_buf = vec![];
  let mut buf = vec![];
  {
    let mut writer = Box::new(JsWriter::new(
      source_map.clone(),
      "\n",
      &mut buf,
      Some(&mut src_map_buf),
    ));
    writer.set_indent_str("  ");
    let mut config = codegen::Config::default();
    config.minify = false;
    config.ascii_only = false;
    config.omit_last_semi = false;
    config.target = deno_ast::ES_VERSION;
    let mut emitter = codegen::Emitter {
      cfg: config,
      comments: Some(comments),
      cm: source_map.clone(),
      wr: writer,
    };
    program.emit_with(&mut emitter)?;
  }
  Ok(String::from_utf8(buf)?)
}

pub fn fill_leading_comments(
  source_file_start_pos: BytePos,
  parsed_source: &ParsedSource,
  global_comments: &SingleThreadedComments,
  filter: impl Fn(&Comment) -> bool,
) {
  for (byte_pos, comment_vec) in parsed_source.comments().leading_map() {
    let byte_pos = source_file_start_pos + *byte_pos;
    for comment in comment_vec {
      if filter(comment) {
        global_comments.add_leading(
          byte_pos,
          adjusted_comment(comment, source_file_start_pos),
        );
      }
    }
  }
}

pub fn fill_trailing_comments(
  source_file_start_pos: BytePos,
  parsed_source: &ParsedSource,
  global_comments: &SingleThreadedComments,
) {
  for (byte_pos, comment_vec) in parsed_source.comments().trailing_map() {
    let byte_pos = source_file_start_pos + *byte_pos;
    for comment in comment_vec {
      global_comments.add_trailing(
        byte_pos,
        adjusted_comment(comment, source_file_start_pos),
      );
    }
  }
}

fn adjusted_comment(
  comment: &Comment,
  source_file_start_pos: BytePos,
) -> Comment {
  Comment {
    kind: comment.kind,
    span: Span::new(
      source_file_start_pos + comment.span.lo,
      source_file_start_pos + comment.span.hi,
      comment.span.ctxt,
    ),
    text: comment.text.clone(),
  }
}
