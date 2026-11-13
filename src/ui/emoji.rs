//! 絵文字のピッカーと、入力欄の :名前: の補完
use std::ops::Range;
use std::rc::Rc;

use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::emoji_url;
use crate::emoji::{self, Pick};
use crate::images::{self as imgs, Img};

/// ピッカーの 1 行に並べる数と、1 マスの大きさ
const COLS: usize = 9;
const CELL: f32 = 38.0;
/// 補完に出す候補の数
const SUGGEST_MAX: usize = 8;

/// ピッカーの 1 行（見出しか、絵文字の並び）
pub enum EmojiRow {
    Head(SharedString),
    Cells(Vec<Pick>),
}

/// ピッカーの行を作り直す目印（検索語・開いているサーバー・絵文字の版・Nitro の有無）
pub type EmojiKey = (String, Option<String>, u64, bool);

/// 補完中のキー操作
#[derive(Clone, Copy, PartialEq)]
pub enum AcKey {
    Up,
    Down,
    Confirm,
    Dismiss,
}

fn push_cells(rows: &mut Vec<EmojiRow>, picks: Vec<Pick>) {
    let mut it = picks.into_iter().peekable();
    while it.peek().is_some() {
        rows.push(EmojiRow::Cells(it.by_ref().take(COLS).collect()));
    }
}

impl Root {
    /// 入力欄の絵文字ボタン（押すとピッカーを開く・閉じる）
    pub fn toggle_emoji_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some_and(|m| m.0 == PopMenu::Emoji) {
            self.menu = None;
            return;
        }
        self.menu = Some((PopMenu::Emoji, window.mouse_position()));
        self.app.emoji_query.clear();
        self.app.ensure_guild_emojis();
        self.inp.emoji_query.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
    }

    /// 絵文字 1 つの絵（カスタムは画像、標準は文字）
    fn emoji_face(&mut self, pick: &Pick, size: f32) -> Div {
        match pick {
            Pick::Custom { id, .. } => {
                // 一覧では動かさない（アニメ絵文字も 1 枚目の画像で出す）
                let url = emoji_url(id, false);
                let face = match imgs::get(&url) {
                    Img::Ready(i) => div()
                        .size(z(size))
                        .child(img(i).size_full().object_fit(ObjectFit::Contain)),
                    _ => div().size(z(size)),
                };
                self.note_image(&url);
                face
            }
            Pick::Unicode { ch, .. } => div()
                .size(z(size))
                .flex()
                .items_center()
                .justify_center()
                .text_size(z(size * 0.85))
                .line_height(z(size))
                .child(*ch),
        }
    }

    /// ピッカーの行（検索語や絵文字が変わったときだけ作り直す）
    fn emoji_rows(&mut self) -> Rc<Vec<EmojiRow>> {
        let a = &self.app;
        let query = a.emoji_query.trim().trim_matches(':').to_string();
        let key: EmojiKey = (
            query.clone(),
            a.sel_guild.clone(),
            a.emoji_rev,
            a.has_nitro(),
        );
        if let Some((k, rows)) = &self.emoji_rows {
            if *k == key {
                return rows.clone();
            }
        }
        let mut rows = Vec::new();
        if query.is_empty() {
            for (gid, list) in a.usable_emojis() {
                let name = a
                    .guilds
                    .iter()
                    .find(|g| g.id == gid)
                    .map_or(tr!("サーバー", "Server"), |g| g.name.as_str());
                rows.push(EmojiRow::Head(name.to_string().into()));
                push_cells(
                    &mut rows,
                    list.into_iter()
                        .map(|e| Pick::Custom {
                            name: e.name.clone(),
                            id: e.id.clone(),
                            animated: e.animated,
                        })
                        .collect(),
                );
            }
            let mut group = None;
            let mut picks = Vec::new();
            for (g, p) in emoji::standard() {
                if group != Some(g) {
                    push_cells(&mut rows, std::mem::take(&mut picks));
                    rows.push(EmojiRow::Head(emoji::group_label(g).into()));
                    group = Some(g);
                }
                picks.push(p);
            }
            push_cells(&mut rows, picks);
        } else {
            let hits = a.emoji_search(&query, COLS * 12);
            if hits.is_empty() {
                rows.push(EmojiRow::Head(
                    tr!("見つかりませんでした", "Nothing found").into(),
                ));
            }
            push_cells(&mut rows, hits);
        }
        let rows = Rc::new(rows);
        self.emoji_rows = Some((key, rows.clone()));
        rows
    }

    /// 絵文字のピッカー（上に検索欄、下に一覧）
    pub fn emoji_picker(&mut self, cx: &mut Context<Self>) -> Div {
        let rows = self.emoji_rows();
        let count = rows.len();
        let grid = uniform_list(
            "emoji_grid",
            count,
            cx.processor(move |this: &mut Root, range: Range<usize>, _, cx| {
                range
                    .filter_map(|r| Some((r, rows.get(r)?)))
                    .map(|(r, row)| match row {
                        EmojiRow::Head(label) => div()
                            .h(z(CELL))
                            .px(z(4.0))
                            .flex()
                            .items_end()
                            .pb(z(4.0))
                            .child(medium(label.clone(), 12.0, MUTED()))
                            .into_any_element(),
                        EmojiRow::Cells(picks) => {
                            let mut line = div().h(z(CELL)).flex();
                            for (c, pick) in picks.iter().enumerate() {
                                let face = this.emoji_face(pick, 28.0);
                                let chosen = pick.clone();
                                line = line.child(
                                    div()
                                        .id(SharedString::from(format!("ep{r}_{c}")))
                                        .size(z(CELL))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(z(6.0))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(SELECTED()))
                                        .tooltip(tip(format!(":{}:", pick.name())))
                                        .child(face)
                                        // Shift を押しながら選ぶと、閉じずに続けて選べる
                                        .on_click(cx.listener(
                                            move |this, ev: &ClickEvent, window, cx| {
                                                this.compose_replace(
                                                    None,
                                                    chosen.insert_text(),
                                                    window,
                                                    cx,
                                                );
                                                if !ev.modifiers().shift {
                                                    this.menu = None;
                                                    this.app.focus_input = true;
                                                }
                                                cx.notify();
                                            },
                                        )),
                                );
                            }
                            line.into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();
        div()
            .bg(SIDEBAR())
            .border_1()
            .border_color(BORDER())
            .rounded(z(8.0))
            .shadow(popup_shadow())
            .w(z(CELL * COLS as f32 + 26.0))
            .h(z(420.0))
            .p(z(8.0))
            .flex()
            .flex_col()
            .gap(z(6.0))
            .child(
                div()
                    .flex_none()
                    .bg(INPUT())
                    .border_1()
                    .border_color(DIVIDER())
                    .rounded(z(8.0))
                    .px(z(6.0))
                    .child(
                        Input::new(&self.inp.emoji_query)
                            .appearance(false)
                            .h(z(32.0)),
                    ),
            )
            .child(grid)
    }

    /// ピッカーの検索欄で Enter: 先頭の候補を入れて閉じる
    pub fn pick_first_emoji(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.app.emoji_query.trim().trim_matches(':').to_string();
        if query.is_empty() {
            return;
        }
        if let Some(pick) = self.app.emoji_search(&query, 1).into_iter().next() {
            self.compose_replace(None, pick.insert_text(), window, cx);
            self.menu = None;
            self.app.focus_input = true;
            cx.notify();
        }
    }

    /// 入力欄のカーソル位置（range があればその範囲）に文字を入れる
    fn compose_replace(
        &mut self,
        range: Option<Range<usize>>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app.input = self.inp.compose.update(cx, |s, cx| {
            match range {
                Some(r) => {
                    s.set_selected_range(r, cx);
                    s.replace(text, window, cx);
                }
                None => s.insert(text, window, cx),
            }
            s.value().to_string()
        });
    }

    /// 補完の状態: 入力欄で「:xx」を打ちかけなら、その範囲・名前・候補
    fn emoji_ac(&mut self, window: &Window, cx: &App) -> Option<(Range<usize>, String, Vec<Pick>)> {
        let s = self.inp.compose.read(cx);
        if !s.focus_handle(cx).is_focused(window) {
            return None;
        }
        let (value, cursor) = (s.value(), s.cursor());
        let (start, name) = emoji::token_before(&value, cursor)?;
        if self
            .emoji_ac_dismissed
            .as_ref()
            .is_some_and(|(at, n)| *at == start && n == name)
        {
            return None;
        }
        let hits = self.app.emoji_search(name, SUGGEST_MAX);
        if hits.is_empty() {
            return None;
        }
        if self.emoji_ac_sel.0 != name {
            self.emoji_ac_sel = (name.to_string(), 0);
        }
        self.emoji_ac_sel.1 = self.emoji_ac_sel.1.min(hits.len() - 1);
        Some((start..cursor, name.to_string(), hits))
    }

    /// 補完中のキー（↑↓ で選ぶ、Tab / Enter で決める、Esc でやめる）。補完していなければ何もしない
    pub fn emoji_ac_key(&mut self, key: AcKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some((range, name, hits)) = self.emoji_ac(window, cx) else {
            return;
        };
        cx.stop_propagation();
        let n = hits.len();
        let sel = self.emoji_ac_sel.1;
        match key {
            AcKey::Up => self.emoji_ac_sel.1 = (sel + n - 1) % n,
            AcKey::Down => self.emoji_ac_sel.1 = (sel + 1) % n,
            AcKey::Confirm => {
                self.compose_replace(Some(range), hits[sel].insert_text(), window, cx)
            }
            AcKey::Dismiss => self.emoji_ac_dismissed = Some((range.start, name)),
        }
        cx.notify();
    }

    /// 入力欄の上に出す補完の候補
    pub fn emoji_suggest(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let (range, name, hits) = self.emoji_ac(window, cx)?;
        self.app.ensure_guild_emojis();
        let sel = self.emoji_ac_sel.1;
        let mut col = div()
            .mb(z(4.0))
            .p(z(6.0))
            .bg(SIDEBAR())
            .border_1()
            .border_color(BORDER())
            .rounded(z(8.0))
            .flex()
            .flex_col()
            .gap(z(1.0));
        col = col.child(div().px(z(6.0)).pb(z(4.0)).child(medium(
            tr!("「:{name}」に一致する絵文字", "Emoji matching \":{name}\"",),
            12.0,
            MUTED(),
        )));
        for (i, pick) in hits.iter().enumerate() {
            let face = self.emoji_face(pick, 22.0);
            let (chosen, range) = (pick.clone(), range.clone());
            col = col.child(
                div()
                    .id(("emoji_ac", i))
                    .h(z(32.0))
                    .px(z(6.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .cursor_pointer()
                    .when(i == sel, |d| d.bg(SELECTED()))
                    .hover(|s| s.bg(SELECTED()))
                    .child(face)
                    .child(text(format!(":{}:", pick.name()), 14.0, TEXT()))
                    // 押しても入力欄のフォーカスを外さない
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.compose_replace(Some(range.clone()), chosen.insert_text(), window, cx);
                        cx.notify();
                    })),
            );
        }
        Some(col)
    }
}
