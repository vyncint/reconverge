//! Shared span-slicing helpers for the extract and mutate subcommands.

use proc_macro2::LineColumn;

/// Byte offsets of line starts, for slicing source text by
/// `proc_macro2::LineColumn` (1-based lines, 0-based char columns).
pub struct LineOffsets(Vec<usize>);

impl LineOffsets {
    pub fn new(source: &str) -> Self {
        let mut offsets = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                offsets.push(i + 1);
            }
        }
        LineOffsets(offsets)
    }

    pub fn byte_of(&self, source: &str, pos: LineColumn) -> usize {
        let line_start = self.0[pos.line - 1];
        let line = &source[line_start..];
        let char_bytes: usize = line.chars().take(pos.column).map(char::len_utf8).sum();
        line_start + char_bytes
    }

    pub fn slice(&self, source: &str, start: LineColumn, end: LineColumn) -> String {
        source[self.byte_of(source, start)..self.byte_of(source, end)].to_string()
    }
}

pub fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == name)
    })
}
