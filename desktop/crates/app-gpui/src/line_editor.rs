//! One editable line of text: content, selection, and IME composition.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Start,
    End,
}

#[derive(Debug, Default)]
pub struct LineEditor {
    text: String,
    /// Where the selection started. Equal to `cursor` when nothing is selected.
    anchor: usize,
    cursor: usize,
    /// Text an input method is still composing.
    marked: Option<Range<usize>>,
}

impl LineEditor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn selection_is_reversed(&self) -> bool {
        self.cursor < self.anchor
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    pub fn set_text(&mut self, text: &str) {
        *self = Self {
            text: single_line(text),
            ..Self::default()
        };
        self.move_to(self.text.len(), false);
    }

    /// Moves the cursor, dragging the selection along when `extend` is set.
    pub fn move_to(&mut self, offset: usize, extend: bool) {
        self.cursor = offset;
        if !extend {
            self.anchor = offset;
        }
    }

    pub fn move_by(&mut self, motion: Motion, extend: bool) {
        let selection = self.selection();
        let target = match motion {
            // Without shift, an arrow first collapses the selection to its edge.
            Motion::Left if !extend && !selection.is_empty() => selection.start,
            Motion::Right if !extend && !selection.is_empty() => selection.end,
            _ => self.target(motion),
        };
        self.move_to(target, extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    /// Deletes the selection, or up to where `motion` leads.
    pub fn delete(&mut self, motion: Motion) {
        if self.selection().is_empty() {
            self.cursor = self.target(motion);
        }
        self.replace(None, "");
    }

    /// Replaces `range`, else the composition, else the selection.
    pub fn replace(&mut self, range: Option<Range<usize>>, new_text: &str) -> Range<usize> {
        let range = range
            .or(self.marked.take())
            .unwrap_or_else(|| self.selection());
        let new_text = single_line(new_text);
        self.text.replace_range(range.clone(), &new_text);
        self.marked = None;
        let inserted = range.start..range.start + new_text.len();
        self.move_to(inserted.end, false);
        inserted
    }

    /// Like `replace`, but leaves the new text marked as a composition in progress.
    pub fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        selected_utf16: Option<Range<usize>>,
    ) {
        let inserted = self.replace(range, new_text);
        if let Some(selected) = selected_utf16 {
            let within = utf8_range(&self.text[inserted.clone()], &selected);
            self.anchor = inserted.start + within.start;
            self.cursor = inserted.start + within.end;
        }
        self.marked = Some(inserted).filter(|range| !range.is_empty());
    }

    pub fn utf16_to_utf8(&self, range: &Range<usize>) -> Range<usize> {
        utf8_range(&self.text, range)
    }

    pub fn utf8_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        let count = |end: usize| self.text[..end].encode_utf16().count();
        count(range.start)..count(range.end)
    }

    fn target(&self, motion: Motion) -> usize {
        let (before, after) = self.text.split_at(self.cursor);
        match motion {
            Motion::Start => 0,
            Motion::End => self.text.len(),
            Motion::Left => before
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(start, _)| start),
            Motion::Right => after
                .graphemes(true)
                .next()
                .map_or(self.cursor, |grapheme| self.cursor + grapheme.len()),
            Motion::WordLeft => before
                .unicode_word_indices()
                .next_back()
                .map_or(0, |(start, _)| start),
            Motion::WordRight => after
                .unicode_word_indices()
                .next()
                .map_or(self.text.len(), |(start, word)| {
                    self.cursor + start + word.len()
                }),
        }
    }
}

fn single_line(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

fn utf8_range(text: &str, utf16: &Range<usize>) -> Range<usize> {
    let offset = |target: usize| {
        let mut units = 0;
        text.char_indices()
            .find(|(_, c)| {
                let reached = units >= target;
                units += c.len_utf16();
                reached
            })
            .map_or(text.len(), |(index, _)| index)
    };
    offset(utf16.start)..offset(utf16.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> LineEditor {
        let mut editor = LineEditor::default();
        editor.set_text(text);
        editor
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut e = editor("hello");
        e.select_all();
        e.replace(None, "hi");
        assert_eq!((e.text(), e.cursor()), ("hi", 2));
    }

    #[test]
    fn pasted_line_breaks_become_spaces() {
        let mut e = editor("");
        e.replace(None, "a\r\nb\nc");
        assert_eq!(e.text(), "a b c");
    }

    #[test]
    fn arrows_move_by_whole_graphemes() {
        let mut e = editor("a👩‍👩‍👧é");
        let ends: Vec<usize> = (0..4)
            .map(|_| {
                e.move_by(Motion::Left, false);
                e.cursor()
            })
            .collect();
        assert_eq!(ends, ["a👩‍👩‍👧".len(), 1, 0, 0]);
        e.move_by(Motion::Right, false);
        e.move_by(Motion::Right, false);
        assert_eq!(e.cursor(), "a👩‍👩‍👧".len());
    }

    #[test]
    fn an_arrow_collapses_the_selection_to_its_edge() {
        let mut e = editor("hello");
        e.select_all();
        e.move_by(Motion::Left, false);
        assert_eq!(e.selection(), 0..0);
        e.select_all();
        e.move_by(Motion::Right, false);
        assert_eq!(e.selection(), 5..5);
    }

    #[test]
    fn shift_motion_extends_from_the_anchor_in_either_direction() {
        let mut e = editor("hello");
        e.move_to(2, false);
        e.move_by(Motion::Right, true);
        assert_eq!(e.selected_text(), "l");
        e.move_by(Motion::Start, true);
        assert_eq!(e.selected_text(), "he");
        assert!(e.selection_is_reversed());
    }

    #[test]
    fn words_are_moved_over_and_deleted_whole() {
        let mut e = editor("one two, three");
        e.delete(Motion::WordLeft);
        assert_eq!(e.text(), "one two, ");
        e.move_by(Motion::WordLeft, false);
        assert_eq!(e.cursor(), 4);
        e.delete(Motion::WordRight);
        assert_eq!(e.text(), "one , ");
        e.delete(Motion::Start);
        assert_eq!(e.text(), ", ");
    }

    #[test]
    fn deleting_at_the_edges_changes_nothing() {
        let mut e = editor("a");
        e.delete(Motion::Right);
        e.move_by(Motion::Start, false);
        e.delete(Motion::Left);
        assert_eq!(e.text(), "a");
    }

    #[test]
    fn utf16_ranges_convert_both_ways() {
        let e = editor("a😀é");
        assert_eq!(e.utf16_to_utf8(&(1..3)), 1..5);
        assert_eq!(e.utf8_to_utf16(&(1..5)), 1..3);
        assert_eq!(e.utf16_to_utf8(&(0..99)), 0.."a😀é".len());
    }

    #[test]
    fn a_composition_is_replaced_until_it_is_committed() {
        let mut e = editor("x");
        e.replace_and_mark(None, "n", None);
        e.replace_and_mark(None, "に", Some(0..1));
        assert_eq!(e.text(), "xに");
        assert_eq!(e.marked(), Some(1.."xに".len()));
        assert_eq!(e.selected_text(), "に");
        e.replace(None, "二");
        assert_eq!((e.text(), e.marked()), ("x二", None));
    }
}
