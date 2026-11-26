use std::ops::Range;

use gpui::{
    Action, App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels,
    Point, Render, ShapedLine, SharedString, Style, Styled, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, px, relative, size,
};

use crate::line_editor::{LineEditor, Motion};
use crate::theme;

const CONTEXT: &str = "TextInput";
const CURSOR_WIDTH: Pixels = px(1.5);

#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = text_input, no_json)]
struct Move {
    motion: Motion,
    extend: bool,
}

#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = text_input, no_json)]
struct Delete {
    motion: Motion,
}

actions!(text_input, [SelectAll, Copy, Cut, Paste, Submit, Cancel]);

pub fn bind_keys(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    let word = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    for (key, motion) in [
        ("left".to_owned(), Motion::Left),
        ("right".to_owned(), Motion::Right),
        ("home".to_owned(), Motion::Start),
        ("end".to_owned(), Motion::End),
        (format!("{word}-left"), Motion::WordLeft),
        (format!("{word}-right"), Motion::WordRight),
    ] {
        let action = |extend| Move { motion, extend };
        let shifted = match key.rsplit_once('-') {
            Some((modifier, key)) => format!("{modifier}-shift-{key}"),
            None => format!("shift-{key}"),
        };
        cx.bind_keys([
            KeyBinding::new(&key, action(false), Some(CONTEXT)),
            KeyBinding::new(&shifted, action(true), Some(CONTEXT)),
        ]);
    }
    for (key, motion) in [
        ("backspace".to_owned(), Motion::Left),
        ("delete".to_owned(), Motion::Right),
        (format!("{word}-backspace"), Motion::WordLeft),
        (format!("{word}-delete"), Motion::WordRight),
    ] {
        cx.bind_keys([KeyBinding::new(&key, Delete { motion }, Some(CONTEXT))]);
    }
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-a"), SelectAll, Some(CONTEXT)),
        KeyBinding::new(&format!("{modifier}-c"), Copy, Some(CONTEXT)),
        KeyBinding::new(&format!("{modifier}-x"), Cut, Some(CONTEXT)),
        KeyBinding::new(&format!("{modifier}-v"), Paste, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

pub enum TextInputEvent {
    Changed,
    Submitted,
    Cancelled,
}

/// A one-line text field that fills its container edge to edge. The container shows focus.
pub struct TextInput {
    editor: LineEditor,
    placeholder: SharedString,
    focus_handle: FocusHandle,
    /// How far the text has scrolled left to keep the cursor in view.
    scroll: Pixels,
    last_layout: Option<(ShapedLine, Bounds<Pixels>)>,
    selecting: bool,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            editor: LineEditor::default(),
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
            scroll: px(0.),
            last_layout: None,
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        self.editor.text()
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.editor.set_text(text);
        self.changed(cx);
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn move_by(&mut self, action: &Move, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.move_by(action.motion, action.extend);
        cx.notify();
    }

    fn delete(&mut self, action: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.delete(action.motion);
        self.changed(cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.select_all();
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selected = self.editor.selected_text();
        if !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        self.editor.replace(None, "");
        self.changed(cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.editor.replace(None, &text);
            self.changed(cx);
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submitted);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Cancelled);
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.selecting = true;
        let offset = self.offset_at(event.position);
        self.editor.move_to(offset, event.modifiers.shift);
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let offset = self.offset_at(event.position);
            self.editor.move_to(offset, true);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn offset_at(&self, position: Point<Pixels>) -> usize {
        match &self.last_layout {
            Some((line, bounds)) if !self.editor.text().is_empty() => {
                line.closest_index_for_x(position.x - bounds.left() + self.scroll)
            }
            _ => 0,
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::move_by))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .px(px(8.))
            .bg(theme::color(theme::BACKGROUND))
            .text_size(px(12.5))
            .line_height(px(18.))
            .child(TextLine { input: cx.entity() })
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.editor.utf16_to_utf8(&range);
        actual.replace(self.editor.utf8_to_utf16(&range));
        Some(self.editor.text()[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.editor.utf8_to_utf16(&self.editor.selection()),
            reversed: self.editor.selection_is_reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let marked = self.editor.marked()?;
        Some(self.editor.utf8_to_utf16(&marked))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.editor.unmark();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.editor.utf16_to_utf8(&range));
        self.editor.replace(range, text);
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.editor.utf16_to_utf8(&range));
        self.editor.replace_and_mark(range, text, selected);
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (line, _) = self.last_layout.as_ref()?;
        let range = self.editor.utf16_to_utf8(&range);
        let x = |index| bounds.left() + line.x_for_index(index) - self.scroll;
        Some(Bounds::from_corners(
            point(x(range.start), bounds.top()),
            point(x(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_at(point);
        Some(self.editor.utf8_to_utf16(&(offset..offset)).start)
    }
}

/// Draws the text, selection, and cursor of a [`TextInput`].
struct TextLine {
    input: Entity<TextInput>,
}

struct TextLinePaint {
    line: ShapedLine,
    origin: Point<Pixels>,
    selection: PaintQuad,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextLine {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextLine {
    type RequestLayoutState = ();
    type PrepaintState = TextLinePaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> TextLinePaint {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let input = self.input.read(cx);
        let editor = &input.editor;
        let empty = editor.text().is_empty();
        let (text, color): (SharedString, _) = if empty {
            (input.placeholder.clone(), theme::color(theme::MUTED).into())
        } else {
            (editor.text().to_owned().into(), style.color)
        };
        let run = |len, underline| TextRun {
            len,
            font: style.font(),
            color,
            background_color: None,
            underline,
            strikethrough: None,
        };
        let runs: Vec<TextRun> = match editor.marked() {
            Some(marked) if !empty => [
                run(marked.start, None),
                run(
                    marked.len(),
                    Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                ),
                run(text.len() - marked.end, None),
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            _ => vec![run(text.len(), None)],
        };
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);

        let x_of = |index| {
            if empty {
                px(0.)
            } else {
                line.x_for_index(index)
            }
        };
        let cursor_x = x_of(editor.cursor());
        let width = bounds.size.width - CURSOR_WIDTH;
        let scroll = input
            .scroll
            .min(cursor_x)
            .max(cursor_x - width)
            .min((x_of(editor.text().len()) - width).max(px(0.)));

        let selected = editor.selection();
        let left = bounds.left() - scroll;
        let selection = fill(
            Bounds::from_corners(
                point(left + x_of(selected.start), bounds.top()),
                point(left + x_of(selected.end), bounds.bottom()),
            ),
            theme::tint(theme::ACCENT, 0x80),
        );
        let focused = input.focus_handle.is_focused(window);
        let cursor = focused.then(|| {
            fill(
                Bounds::new(
                    point(left + cursor_x, bounds.top() + px(2.)),
                    size(CURSOR_WIDTH, bounds.size.height - px(4.)),
                ),
                theme::color(theme::ACCENT_BRIGHT),
            )
        });
        self.input.update(cx, |input, _| input.scroll = scroll);
        TextLinePaint {
            line,
            origin: point(left, bounds.top()),
            selection,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        paint: &mut TextLinePaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(paint.selection.clone());
            let line_height = window.line_height();
            // A glyph that fails to rasterize is not worth losing the frame over.
            let _ = paint.line.paint(paint.origin, line_height, window, cx);
            if let Some(cursor) = paint.cursor.take() {
                window.paint_quad(cursor);
            }
        });
        let layout = (paint.line.clone(), bounds);
        self.input
            .update(cx, |input, _| input.last_layout = Some(layout));
    }
}
