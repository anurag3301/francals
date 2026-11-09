use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::Point;

/// Maps between byte offsets and LSP positions (UTF-16 columns).
#[derive(Clone, Debug, Default)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.starts[line.min(self.starts.len() - 1)]
    }

    fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());
        let line = self.line_of(offset);
        let start = self.starts[line];
        let col: usize = text[start..offset].chars().map(char::len_utf16).sum();
        Position::new(line as u32, col as u32)
    }

    pub fn range(&self, text: &str, start: usize, end: usize) -> Range {
        Range::new(self.position(text, start), self.position(text, end))
    }

    /// Byte offset for a position. Out-of-range positions are clamped.
    pub fn offset(&self, text: &str, pos: Position) -> usize {
        let line = pos.line as usize;
        if line >= self.starts.len() {
            return text.len();
        }
        let start = self.starts[line];
        let end = self.starts.get(line + 1).copied().unwrap_or(text.len());
        let mut units = 0u32;
        for (i, c) in text[start..end].char_indices() {
            if units >= pos.character || c == '\n' {
                return start + i;
            }
            units += c.len_utf16() as u32;
        }
        end
    }

    pub fn point(&self, offset: usize) -> Point {
        let line = self.line_of(offset);
        Point::new(line, offset - self.starts[line])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_ascii() {
        let text = "package a\ninterface B {\n}\n";
        let idx = LineIndex::new(text);
        let off = text.find("interface").unwrap();
        let pos = idx.position(text, off);
        assert_eq!(pos, Position::new(1, 0));
        assert_eq!(idx.offset(text, pos), off);
        assert_eq!(idx.line_count(), 4);
    }

    #[test]
    fn counts_utf16_units() {
        let text = "a😀b\nc";
        let idx = LineIndex::new(text);
        let off = text.find('b').unwrap();
        assert_eq!(idx.position(text, off), Position::new(0, 3));
        assert_eq!(idx.offset(text, Position::new(0, 3)), off);
        // Inside a surrogate pair rounds to the following character.
        assert_eq!(idx.offset(text, Position::new(0, 2)), off);
    }

    #[test]
    fn clamps_out_of_range() {
        let text = "ab\ncd";
        let idx = LineIndex::new(text);
        assert_eq!(idx.offset(text, Position::new(0, 99)), 2);
        assert_eq!(idx.offset(text, Position::new(9, 0)), text.len());
        assert_eq!(idx.position(text, 999), Position::new(1, 2));
    }

    #[test]
    fn computes_points() {
        let text = "ab\ncde";
        let idx = LineIndex::new(text);
        assert_eq!(idx.point(4), Point::new(1, 1));
    }
}
