use deno_ast::swc::ast::*;
use deno_ast::swc::common::BytePos;
use deno_ast::swc::common::Span;
use deno_ast::swc::common::DUMMY_SP;
use deno_ast::swc::visit::VisitMut;
use deno_ast::swc::visit::VisitMutWith;
use deno_ast::ModuleSpecifier;

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
