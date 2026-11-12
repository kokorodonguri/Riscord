//! リッチテキスト（リンク・スポイラー・コード・Markdown スパン）の組み立てと描画要素化。
use std::ops::Range;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{find_links, split_emoji, Seg};
use crate::color::Color32;
use crate::markdown as md;

/// 文字と、色や太さを変える範囲・押せるリンクの範囲（バイト位置）
#[derive(Default)]
pub struct Rich {
    pub text: String,
    pub styles: Vec<(Range<usize>, HighlightStyle)>,
    pub links: Vec<(Range<usize>, String)>,
    /// 等幅フォントで出す範囲（`コード`）
    pub mono: Vec<Range<usize>>,
    /// 押すまで隠す範囲（||スポイラー||）
    pub spoilers: Vec<Range<usize>>,
}

/// 記法の飾りを、文字の色や太さに直す
fn highlight(st: md::Style) -> Option<HighlightStyle> {
    if st == md::Style::default() {
        return None;
    }
    let mut h = HighlightStyle::default();
    if st.bold {
        h.color = Some(WHITE().into());
        h.font_weight = Some(FontWeight::BOLD);
    }
    if st.italic {
        h.font_style = Some(FontStyle::Italic);
    }
    if st.underline {
        h.underline = Some(UnderlineStyle { thickness: px(1.0), ..Default::default() });
    }
    if st.strike {
        h.strikethrough = Some(StrikethroughStyle { thickness: px(1.0), ..Default::default() });
    }
    if st.code || st.spoiler {
        h.background_color = Some(WIDGET().into());
    }
    Some(h)
}

impl Rich {
    pub(super) fn push(&mut self, s: &str, style: Option<HighlightStyle>) {
        let start = self.text.len();
        self.text.push_str(s);
        if let Some(st) = style {
            if !s.is_empty() {
                self.styles.push((start..self.text.len(), st));
            }
        }
    }

    /// text を足す。URL は色を変えて押せるようにする。埋め込みを抑止する <URL> の山かっこは本家と同じく表示しない
    pub fn linked(&mut self, text: &str, style: Option<HighlightStyle>) {
        let mut last = 0;
        for r in find_links(text) {
            let wrapped = text[..r.start].ends_with('<') && text[r.end..].starts_with('>');
            let before = if wrapped { r.start - 1 } else { r.start };
            self.push(&text[last..before], style);
            let start = self.text.len();
            let url = &text[r.clone()];
            self.push(url, Some(HighlightStyle { color: Some(LINK().into()), ..style.unwrap_or_default() }));
            self.links.push((start..self.text.len(), url.to_string()));
            last = if wrapped { r.end + 1 } else { r.end };
        }
        self.push(&text[last..], style);
    }

    /// [文字](https://...) を文字だけ青くして押せるようにする。それ以外は linked に任せる
    pub fn masked(&mut self, text: &str, style: Option<HighlightStyle>) {
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
            let after = &rest[close + 2..];
            let Some(end) = after.find(')') else { break };
            let url = after[..end].trim().trim_start_matches('<').trim_end_matches('>');
            let label = &rest[open + 1..close];
            if label.contains('[') || !(url.starts_with("https://") || url.starts_with("http://")) {
                self.linked(&rest[..open + 1], style);
                rest = &rest[open + 1..];
                continue;
            }
            self.linked(&rest[..open], style);
            let start = self.text.len();
            self.push(label, Some(HighlightStyle { color: Some(LINK().into()), ..style.unwrap_or_default() }));
            self.links.push((start..self.text.len(), url.to_string()));
            rest = &after[end + 1..];
        }
        self.linked(rest, style);
    }

    /// 記法を解いた文字を足す。`コード` の中はリンクにしない。revealed でなければスポイラーは塗りつぶす
    pub fn span(&mut self, s: &md::Span, revealed: bool) {
        let start = self.text.len();
        let style = highlight(s.style);
        if s.style.spoiler && !revealed {
            let hidden: Hsla = WIDGET().into();
            self.push(&s.text, Some(HighlightStyle { color: Some(hidden), background_color: Some(hidden), ..Default::default() }));
            self.spoilers.push(start..self.text.len());
        } else if s.style.code {
            self.push(&s.text, style);
            self.mono.push(start..self.text.len());
        } else {
            self.masked(&s.text, style);
        }
    }

    /// 押せるリンク付きの文字の要素
    pub fn element(self, id: impl Into<ElementId>) -> AnyElement {
        self.element_with(id, None)
    }

    /// on_spoiler は、隠れているスポイラーを押したときに呼ぶ
    pub fn element_with(self, id: impl Into<ElementId>, on_spoiler: Option<Rc<dyn Fn(&mut Window, &mut App)>>) -> AnyElement {
        let id = id.into();
        if self.links.is_empty()
            && (self.spoilers.is_empty() || on_spoiler.is_none())
            && self.mono.is_empty()
            && self.styles.is_empty()
        {
            return gpui::base::SelectableText::new(id, self.text).into_any_element();
        }
        let mut styled = StyledText::new(self.text).with_highlights(self.styles);
        if !self.mono.is_empty() {
            let family = mono_family();
            styled = styled.with_font_family_overrides(self.mono.into_iter().map(|r| (r, family.clone())));
        }
        let spoilers = if on_spoiler.is_some() { self.spoilers } else { Vec::new() };
        if self.links.is_empty() && spoilers.is_empty() {
            return styled.into_any_element();
        }
        let (ranges, urls): (Vec<_>, Vec<_>) = self.links.into_iter().unzip();
        let urls2 = urls.clone();
        let ranges2 = ranges.clone();
        let clickable = ranges.iter().cloned().chain(spoilers).collect();
        InteractiveText::new(id, styled)
            .on_click(clickable, move |i, window, cx| match urls.get(i) {
                Some(url) => cx.open_url(url),
                None => {
                    if let Some(f) = &on_spoiler {
                        f(window, cx)
                    }
                }
            })
            .tooltip(move |ix, window, cx| {
                let i = ranges2.iter().position(|r| r.contains(&ix))?;
                Some(gpui::component::tooltip::Tooltip::new(urls2[i].clone()).build(window, cx))
            })
            .into_any_element()
    }
}

/// 返信のプレビュー用に、本文を記法を解いた1行にする。リンクは色だけで、押せない
pub(super) fn preview_rich(content: &str, revealed: bool) -> Rich {
    let mut r = Rich::default();
    for block in md::blocks(content) {
        if !r.text.is_empty() {
            r.push(" ", None);
        }
        let spans = match block {
            md::Block::Code(code) => vec![md::Span { text: code, style: md::Style { code: true, ..Default::default() } }],
            md::Block::Text { spans, .. } => spans,
        };
        for mut span in spans {
            // カスタム絵文字は画像を並べられないので :名前: で出す
            span.text = split_emoji(&span.text)
                .into_iter()
                .map(|seg| match seg {
                    Seg::Text(t) => t.replace('\n', " "),
                    Seg::Emoji { name, .. } => format!(":{name}:"),
                })
                .collect();
            r.span(&span, revealed);
        }
    }
    r.links.clear();
    r
}

/// 埋め込みの本文。URL・[文字](URL) の伏せリンクと、文中の記法（太字・`コード` など）を解釈する
pub(super) fn embed_text(id: impl Into<ElementId>, s: &str, size: f32, color: Color32) -> Div {
    let mut r = Rich::default();
    for span in md::inline(s) {
        r.span(&span, true);
    }
    div().text_size(z(size)).text_color(color).child(r.element(id))
}

/// 押すとブラウザで開く1行の文字
pub(super) fn embed_line(id: impl Into<ElementId>, s: impl Into<SharedString>, size: f32, color: Color32, url: Option<&str>) -> AnyElement {
    let t = text(s, size, color);
    match url {
        Some(u) => {
            let u = u.to_string();
            t.id(id).cursor_pointer().hover(|s| s.underline()).on_click(move |_, _, cx| cx.open_url(&u)).into_any_element()
        }
        None => t.into_any_element(),
    }
}
