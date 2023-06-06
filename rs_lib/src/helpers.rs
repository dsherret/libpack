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

pub fn is_remote(specifier: &ModuleSpecifier) -> bool {
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
