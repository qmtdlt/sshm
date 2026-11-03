use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default)]
pub struct Input {
    pub value: String,
    pub cursor: usize,
}

impl Input {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }

    fn previous(&self) -> usize {
        self.value[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.cursor
            + self.value[self.cursor..]
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }

    pub fn insert(&mut self, text: &str) {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(4096_usize.saturating_sub(self.value.chars().count()))
            .collect();
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') => self.cursor = 0,
                KeyCode::Char('e') => self.cursor = self.value.len(),
                KeyCode::Char('u') => {
                    self.value.drain(..self.cursor);
                    self.cursor = 0;
                }
                KeyCode::Char('k') => self.value.truncate(self.cursor),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(&ch.to_string())
            }
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            KeyCode::Backspace => {
                let previous = self.previous();
                self.value.drain(previous..self.cursor);
                self.cursor = previous;
            }
            KeyCode::Delete => {
                self.value.drain(self.cursor..self.next());
            }
            _ => {}
        }
    }

    pub fn visible(&self, width: usize) -> (String, u16) {
        let width = width.saturating_sub(1);
        let mut start = 0;
        while self.value[start..self.cursor].width() > width {
            start += self.value[start..]
                .graphemes(true)
                .next()
                .map_or(1, str::len);
        }
        let mut visible = String::new();
        for grapheme in self.value[start..].graphemes(true) {
            if visible.width() + grapheme.width() > width {
                break;
            }
            visible.push_str(grapheme);
        }
        (visible, self.value[start..self.cursor].width() as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_unicode_and_clips_by_terminal_columns() {
        let mut input = Input::new("a中文e\u{301}");
        input.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.value, "a中文");
        let (text, col) = input.visible(4);
        assert_eq!(text, "文");
        assert_eq!(col, 2);
        input.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        input.insert("X\n\u{1b}");
        assert_eq!(input.value, "a中X文");
    }
}
