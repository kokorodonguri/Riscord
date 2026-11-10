//! メッセージの1行（本文のリンク・絵文字、添付画像、埋め込み、スタンプ、返信、操作ボタン）
use std::ops::Range;

use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{
    author_name, embed_big_media, emoji_url, find_links, fit_size, fmt_time, is_reply, local_dt, member_role_color, name_color, resized, resolve_mentions, rgb, split_emoji,
    user_avatar_url, Seg, StickerState,
};
use crate::color::Color32;
use crate::images::{self as imgs, Img};
use crate::model::{self, Embed, Message, Role};
use crate::net::Cmd;

const EMBED_MAX_W: f32 = 432.0;
const EMBED_THUMB: f32 = 80.0;

// コンパクト表示の列（時刻・名前・本文）の幅と、本文の開始位置
const COMPACT_PAD: f32 = 16.0;
const COMPACT_GAP: f32 = 6.0;
const COMPACT_TIME_W: f32 = 40.0;
const COMPACT_NAME_X: f32 = COMPACT_PAD + COMPACT_TIME_W + COMPACT_GAP;

/// 行の並べ方（日付の区切り・前の行とまとめるか・返信か）。これが変わると高さが変わる
pub fn row_layout(m: &Message, prev: Option<&Message>, compact: bool) -> (bool, bool, bool, bool) {
    row_layout_at(m, prev, local_dt(&m.timestamp), prev.and_then(|p| local_dt(&p.timestamp)), compact)
}

/// row_layout の、時刻を読み終えたものを受け取る版（一覧全体を回すとき、1件の時刻を2回読まないため）
pub fn row_layout_at(
    m: &Message,
    prev: Option<&Message>,
    t: Option<chrono::DateTime<chrono::Local>>,
    pt: Option<chrono::DateTime<chrono::Local>>,
    compact: bool,
) -> (bool, bool, bool, bool) {
    let new_day = match (t, pt) {
        (Some(a), Some(b)) => a.date_naive() != b.date_naive(),
        (Some(_), None) => true,
        _ => false,
    };
    let reply = is_reply(m);
    // 通話の記録は1行ずつ独立させる（前後の行とまとめない）
    let grouped = !new_day
        && !reply
        && !m.is_call()
        && !prev.is_some_and(Message::is_call)
        && prev.is_some_and(|p| p.author.id == m.author.id)
        && matches!((t, pt), (Some(a), Some(b)) if (a - b).num_minutes() < 7);
    (new_day, grouped, reply, compact)
}

/// 通話の長さ（「数秒」「5分」「1時間20分」）
fn call_length(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    match (s / 3600, s % 3600 / 60) {
        (0, 0) => "数秒".into(),
        (0, m) => format!("{m}分"),
        (h, 0) => format!("{h}時間"),
        (h, m) => format!("{h}時間{m}分"),
    }
}

/// 文字と、色や太さを変える範囲・押せるリンクの範囲（バイト位置）
#[derive(Default)]
pub struct Rich {
    pub text: String,
    pub styles: Vec<(Range<usize>, HighlightStyle)>,
    pub links: Vec<(Range<usize>, String)>,
}

impl Rich {
    fn push(&mut self, s: &str, style: Option<HighlightStyle>) {
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

    /// **…** を太字にして足す（埋め込みと同じく太く白く）。対になっていない ** は記号のまま残す
    pub fn bold(&mut self, text: &str) {
        let bold = HighlightStyle { color: Some(WHITE().into()), font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let parts: Vec<&str> = text.split("**").collect();
        // 使える区切りは偶数個まで。余った最後の ** はそのまま出す
        let paired = (parts.len() - 1) / 2 * 2;
        for (i, part) in parts.iter().enumerate() {
            if i > paired {
                self.linked("**", None);
            }
            self.linked(part, (i % 2 == 1 && i <= paired).then_some(bold));
        }
    }

    /// 押せるリンク付きの文字の要素
    pub fn element(self, id: impl Into<ElementId>) -> AnyElement {
        let styled = StyledText::new(self.text).with_highlights(self.styles);
        if self.links.is_empty() {
            return styled.into_any_element();
        }
        let (ranges, urls): (Vec<_>, Vec<_>) = self.links.into_iter().unzip();
        let urls2 = urls.clone();
        let ranges2 = ranges.clone();
        InteractiveText::new(id, styled)
            .on_click(ranges, move |i, _, cx| cx.open_url(&urls[i]))
            .tooltip(move |ix, window, cx| {
                let i = ranges2.iter().position(|r| r.contains(&ix))?;
                Some(gpui::component::tooltip::Tooltip::new(urls2[i].clone()).build(window, cx))
            })
            .into_any_element()
    }
}

/// 埋め込みの本文。URL・[文字](URL) の伏せリンク・**太字** だけ解釈する
fn embed_text(id: impl Into<ElementId>, s: &str, size: f32, color: Color32) -> Div {
    let mut r = Rich::default();
    let bold = HighlightStyle { color: Some(WHITE().into()), font_weight: Some(FontWeight::BOLD), ..Default::default() };
    for (i, part) in s.split("**").enumerate() {
        r.masked(part, (i % 2 == 1).then_some(bold));
    }
    div().text_size(z(size)).text_color(color).child(r.element(id))
}

/// 押すとブラウザで開く1行の文字
fn embed_line(id: impl Into<ElementId>, s: impl Into<SharedString>, size: f32, color: Color32, url: Option<&str>) -> AnyElement {
    let t = text(s, size, color);
    match url {
        Some(u) => {
            let u = u.to_string();
            t.id(id).cursor_pointer().hover(|s| s.underline()).on_click(move |_, _, cx| cx.open_url(&u)).into_any_element()
        }
        None => t.into_any_element(),
    }
}

/// 画像（読み込み中は同じ大きさの箱を出して、行の高さが変わらないようにする）
fn image_box(id: impl Into<ElementId>, src: &str, open: &str, size: (f32, f32), video: bool, cx: &mut Context<Root>) -> Stateful<Div> {
    let open = open.to_string();
    let d = div().id(id).w(z(size.0)).h(z(size.1)).max_w_full().rounded(z(8.0)).overflow_hidden().cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
        if video { cx.open_url(&open); } else { this.image_preview = Some(open.clone()); cx.notify(); }
    }));
    match imgs::get(src) {
        Img::Ready(i) => d.child(img(i).id("img").size_full().rounded(z(8.0)).object_fit(ObjectFit::Contain)),
        Img::Pending => d.bg(EMBED_BG()).flex().items_center().justify_center().child(spinner(20.0)),
        Img::Failed => d.bg(EMBED_BG()).flex().items_center().justify_center().child(icon(IconName::TriangleAlert, 20.0, FAINT())),
    }
}

impl Root {
    /// 開いているサーバーのロール（DM なら空）
    fn guild_roles(&self) -> &[Role] {
        self.app.sel_guild.as_ref().and_then(|g| self.app.roles.get(g)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// 画像を出したことを覚える（チャンネルを捨てるときに画像のキャッシュも解放するため）
    pub(super) fn note_image(&mut self, uri: &str) {
        let Some(cid) = &self.app.sel_channel else { return };
        // 毎フレーム見えている画像の数だけ呼ばれるので、覚え済みならキーも作らない
        if let Some(set) = self.app.channel_images.get_mut(cid) {
            if !set.contains(uri) {
                set.insert(uri.to_string());
            }
        } else {
            self.app.channel_images.entry(cid.clone()).or_default().insert(uri.to_string());
        }
    }

    pub fn message_row(&mut self, m: &Message, prev: Option<&Message>, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (new_day, grouped, reply, compact) = row_layout(m, prev, self.app.settings.compact);
        let hovered = self.hover_msg.as_deref() == Some(m.id.as_str());
        let highlight = self.app.editing.as_ref().or(self.app.reply_to.as_ref()).is_some_and(|x| x.id == m.id);
        // ロールは行ごとに複製せず、色を決める間だけ借りる（ロールの多いサーバーで見えている行の数だけ Vec を作っていた）
        let name_col = m.member.as_ref().and_then(|member| member_role_color(member, self.guild_roles())).unwrap_or(WHITE());
        let mut outer = div().flex().flex_col().w_full();
        if new_day {
            if let Some(t) = local_dt(&m.timestamp) {
                outer = outer.child(
                    div()
                        .mt(z(20.0))
                        .h(z(16.0))
                        .px(z(16.0))
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .child(div().flex_1().h(px(1.0)).bg(DIVIDER()))
                        .child(text(t.format("%Y年%-m月%-d日").to_string(), 12.0, FAINT()))
                        .child(div().flex_1().h(px(1.0)).bg(DIVIDER())),
                );
            }
        }
        if m.is_call() {
            return outer.child(self.call_row(m, name_col, compact, cx));
        }
        let mid = m.id.clone();
        let mut row = div()
            .id(SharedString::from(format!("msg{}", m.id)))
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Right, {
                let m = m.clone();
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.menu = Some((PopMenu::Message, ev.position));
                    this.menu_msg = Some(m.clone());
                    cx.stop_propagation();
                    cx.notify();
                })
            })
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h && this.hover_msg.as_deref() != Some(mid.as_str()) {
                    this.hover_msg = Some(mid.clone());
                    cx.notify();
                }
            }));
        if highlight {
            row = row.bg(Color32::from_rgba_unmultiplied(0x58, 0x65, 0xf2, 14)).child(div().absolute().left_0().top_0().bottom_0().w(px(2.0)).bg(BLURPLE()));
        } else if hovered {
            row = row.bg(MSG_HOVER());
        }
        let author = m.author.clone();
        // コンパクト表示の名前の列の幅（狭い画面では本文の幅を残す）と、本文の開始位置
        let name_w = if self.app.screen_w < 700.0 { 72.0 } else { 120.0 };
        let text_x = COMPACT_NAME_X + name_w + COMPACT_GAP;
        if compact {
            outer = outer.when(!grouped, |d| d.pt(z(if new_day { 6.0 } else { 4.0 })));
            row = row.py(px(1.0));
            if reply {
                row = row.child(self.reply_preview(m, COMPACT_NAME_X + 8.0, text_x, cx));
            }
            // 時刻・名前・本文を固定の列にそろえる。連投は名前を省き、時刻はマウスを乗せたときだけ出す
            let time = if !grouped || hovered { local_dt(&m.timestamp).map(|t| t.format("%H:%M").to_string()).unwrap_or_default() } else { String::new() };
            let name: AnyElement = if grouped {
                div().w(z(name_w)).flex_none().into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("an{}", m.id)))
                    .w(z(name_w))
                    .flex_none()
                    .truncate()
                    .text_size(z(self.app.settings.font_size))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(name_col)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let p = window.mouse_position();
                        this.app.open_profile(author.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                        cx.notify();
                    }))
                    .child(author_name(m).to_string())
                    .into_any_element()
            };
            let mut body = div().flex_1().min_w_0();
            if !m.content.is_empty() {
                body = body.child(self.inline_content(m));
            }
            // 本文のない連投（画像だけなど）は、空の行を作らない
            let head = !grouped || !m.content.is_empty();
            row = row.when(head, |r| r.child(
                div()
                    .pl(z(COMPACT_PAD))
                    .pr(z(16.0))
                    .flex()
                    .items_baseline()
                    .gap(z(COMPACT_GAP))
                    .child(div().w(z(COMPACT_TIME_W)).flex_none().flex().justify_end().child(text(time, 11.0, FAINT())))
                    .child(name)
                    .child(body),
            ));
            if !m.attachments.is_empty() || !m.sticker_items.is_empty() || m.embeds.iter().any(|e| e.visible()) {
                row = row.child(div().pl(z(text_x)).pr(z(16.0)).child(self.message_media(m, cx)));
            }
        } else {
            outer = outer.when(!grouped, |d| d.pt(z(13.0)));
            row = row.pt(z(if grouped { 1.0 } else { 4.0 })).pb(z(2.0));
            if reply {
                row = row.child(self.reply_preview(m, 36.0, 72.0, cx).mb(z(2.0)));
            }
            let left: AnyElement = if grouped {
                // まとめた行はアバターの代わりに、マウスを乗せたとき時刻を出す
                let t = if hovered { local_dt(&m.timestamp).map(|t| t.format("%H:%M").to_string()).unwrap_or_default() } else { String::new() };
                div().w(z(40.0)).h(z(22.0)).flex_none().flex().items_center().justify_center().child(text(t, 11.0, FAINT())).into_any_element()
            } else {
                let url = user_avatar_url(&m.author);
                let author = author.clone();
                avatar(Some(&url), 40.0, m.author.display(), 20.0, name_color(&m.author.id))
                    .id(SharedString::from(format!("av{}", m.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let p = window.mouse_position();
                        this.app.open_profile(author.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                        cx.notify();
                    }))
                    .into_any_element()
            };
            let mut content = div().flex_1().min_w_0().flex().flex_col().gap(z(2.0));
            if !grouped {
                let author = author.clone();
                content = content.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(z(8.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("an{}", m.id)))
                                .text_size(z(16.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(name_col)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let p = window.mouse_position();
                                    this.app.open_profile(author.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                                    cx.notify();
                                }))
                                .child(author_name(m).to_string()),
                        )
                        .child(text(fmt_time(&m.timestamp), 12.0, FAINT())),
                );
            }
            if !m.content.is_empty() {
                content = content.child(self.inline_content(m));
            }
            content = content.child(self.message_media(m, cx));
            row = row.child(div().flex().items_start().pl(z(16.0)).pr(z(16.0)).gap(z(16.0)).child(left).child(content));
        }
        if self.app.plugins.translator_enabled() {
            if let Some((_, translated)) = self.app.translator.messages.get(&(m.channel_id.clone(), m.id.clone())).filter(|(original, _)| original == &m.content) {
                row = row.child(div().pl(z(if compact { text_x } else { 72.0 })).pr(z(16.0)).child(text(format!("翻訳: {translated}"), self.app.settings.font_size, MUTED())));
            }
        }
        if hovered {
            row = row.child(self.message_toolbar(m, cx));
        }
        outer.child(row)
    }

    /// 通話の記録（不在着信か、参加した通話か、続いている通話か）
    fn call_row(&mut self, m: &Message, name_col: Color32, compact: bool, cx: &mut Context<Self>) -> Div {
        let call = m.call.as_ref();
        let ended = call.and_then(|c| c.ended_timestamp.as_deref());
        let me = self.app.me.as_ref().map(|u| u.id.as_str());
        // 自分が参加者に入っていなければ不在着信（参加者が分からないときは不在にしない）
        let missed = ended.is_some() && call.is_some_and(|c| me.is_some_and(|me| !c.participants.iter().any(|p| p == me)));
        let length = ended.and_then(|e| Some(call_length(local_dt(e)? - local_dt(&m.timestamp)?)));
        let after = match (missed, &length) {
            (true, Some(l)) => format!(" からの不在着信（{l}）"),
            (true, None) => " からの不在着信".to_string(),
            (false, Some(l)) => format!(" が通話を開始しました（{l}）"),
            (false, None) => " が通話を開始しました".to_string(),
        };
        let author = m.author.clone();
        let fs = self.app.settings.font_size;
        let name = div()
            .id(SharedString::from(format!("an{}", m.id)))
            .flex_none()
            .max_w(relative(0.5))
            .truncate()
            .text_size(z(fs))
            .font_weight(FontWeight::MEDIUM)
            .text_color(name_col)
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(cx.listener(move |this, _, window, cx| {
                let p = window.mouse_position();
                this.app.open_profile(author.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                cx.notify();
            }))
            .child(author_name(m).to_string());
        let color = if missed { MUTED() } else { GREEN() };
        // アイコンは、ふつうの行のアバターの列（幅 40）の中央に置く
        let left = if compact { div().w(z(COMPACT_TIME_W)).flex_none().flex().justify_end() } else { div().w(z(40.0)).flex_none().flex().justify_center() };
        div()
            .w_full()
            .pt(z(if compact { 4.0 } else { 13.0 }))
            .pb(z(2.0))
            .pl(z(if compact { COMPACT_PAD } else { 16.0 }))
            .pr(z(16.0))
            .flex()
            .items_center()
            .gap(z(if compact { COMPACT_GAP } else { 16.0 }))
            .child(left.child(icon(if missed { IconName::PhoneOff } else { IconName::Phone }, 18.0, color)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .child(name)
                    .child(text(after, fs, MUTED()).flex_none())
                    .child(text(fmt_time(&m.timestamp), 12.0, FAINT()).flex_none().ml(z(8.0))),
            )
    }

    /// 右上に出る操作ボタン（返信・編集・コピー・削除）
    fn message_toolbar(&mut self, m: &Message, cx: &mut Context<Self>) -> impl IntoElement {
        let mine = self.app.me.as_ref().is_some_and(|u| u.id == m.author.id);
        let mut bar = div().absolute().top(z(-16.0)).right(z(16.0)).h(z(32.0)).px(z(2.0)).flex().items_center().bg(SIDEBAR()).border_1().border_color(BORDER()).rounded(z(8.0));
        let btn = |id: &str, name: IconName, danger: bool| {
            div()
                .id(SharedString::from(id.to_string()))
                .group("tb")
                .size(z(28.0))
                .m(z(1.0))
                .rounded(z(6.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(SELECTED()))
                .child(icon(name, 18.0, MUTED()).group_hover("tb", move |s| s.text_color(if danger { RED() } else { WHITE() })))
        };
        let m1 = m.clone();
        bar = bar.child(btn("tb_reply", IconName::Reply, false).tooltip(tip("返信")).on_click(cx.listener(move |this, _, _, cx| {
            if this.app.editing.take().is_some() {
                this.app.input.clear();
            }
            this.app.reply_to = Some(m1.clone());
            this.app.focus_input = true;
            cx.notify();
        })));
        if mine {
            let m2 = m.clone();
            bar = bar.child(btn("tb_edit", IconName::Pencil, false).tooltip(tip("編集")).on_click(cx.listener(move |this, _, _, cx| {
                this.app.input = crate::emoji::shorten(&m2.content).0;
                this.app.editing = Some(m2.clone());
                this.app.reply_to = None;
                this.app.focus_input = true;
                cx.notify();
            })));
        }
        if !m.content.is_empty() {
            let t = m.content.clone();
            bar = bar.child(btn("tb_copy", IconName::Copy, false).tooltip(tip("テキストをコピー")).on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(t.clone()));
                this.app.show_toast("テキストをコピーしました");
                cx.notify();
            })));
        }
        if mine {
            let m3 = m.clone();
            bar = bar.child(btn("tb_delete", IconName::Trash, true).tooltip(tip("メッセージを削除（Shift+クリックで確認なし）")).on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.modifiers().shift {
                    this.app.send(Cmd::Delete { channel_id: m3.channel_id.clone(), id: m3.id.clone() });
                } else {
                    this.app.pending_delete = Some(m3.clone());
                }
                cx.notify();
            })));
        }
        if self.app.plugins.translator_enabled() && !m.content.trim().is_empty() {
            let message = m.clone();
            let key = (m.channel_id.clone(), m.id.clone());
            let translated = self.app.translator.messages.get(&key).is_some_and(|(original, _)| original == &m.content);
            bar = bar.child(btn("tb_translate", IconName::Settings, false).tooltip(tip(if translated { "翻訳を閉じる" } else { "Google で翻訳" })).on_click(cx.listener(move |this, _, _, cx| {
                let key = (message.channel_id.clone(), message.id.clone());
                if this.app.translator.messages.remove(&key).is_some_and(|(original, _)| original == message.content) {
                    this.app.plugins.revision = this.app.plugins.revision.wrapping_add(1);
                } else {
                    let language = this.app.plugins.translation_target();
                    let target = crate::translator::Target::Message { channel: message.channel_id.clone(), id: message.id.clone(), original: message.content.clone(), generation: this.app.plugins.generation };
                    if let Err(e) = this.app.translator.request(target, message.content.clone(), language, this.app.waker.clone()) { this.app.show_toast(&e); }
                    else { this.app.show_toast("Google で翻訳しています…"); }
                }
                cx.notify();
            })));
        }
        for (id, index, label) in self.app.plugins.message_actions() {
            let message = m.clone();
            bar = bar.child(btn(&format!("plugin-{id}-{index}"), IconName::Settings, false).tooltip(tip(label)).on_click(cx.listener(move |this, _, _, cx| {
                this.app.plugins.message_action(&id, index, &message); this.app.waker.wake(); cx.notify();
            })));
        }
        bar
    }

    /// 返信の上に出る「╭ @相手 本文」の一行。avatar_x = 本文側アバターの中心
    fn reply_preview(&mut self, m: &Message, avatar_x: f32, text_x: f32, cx: &mut Context<Self>) -> Div {
        let roles = self.guild_roles();
        // 左の曲がった線（上と左の枠を角丸にした箱の一部）
        let curve = div().absolute().left(z(avatar_x)).top(z(10.0)).w(z(text_x - avatar_x - 6.0)).h(z(12.0)).border_l_2().border_t_2().border_color(REPLY_LINE()).rounded_tl(z(6.0));
        let row = div().relative().h(z(20.0)).w_full().flex().items_center().pl(z(text_x)).pr(z(16.0)).gap(z(4.0)).child(curve);
        let Some(r) = m.referenced_message.as_deref() else {
            return row.child(text("元のメッセージは削除されました", 13.0, FAINT()));
        };
        let nc = r.member.as_ref().and_then(|member| member_role_color(member, roles)).unwrap_or(TEXT());
        let body = if !r.content.is_empty() {
            resolve_mentions(&r.content, r, &self.app.channels, roles).replace('\n', " ")
        } else if !r.attachments.is_empty() {
            "添付ファイル".to_string()
        } else if !r.sticker_items.is_empty() {
            "スタンプ".to_string()
        } else {
            String::new()
        };
        let author = r.author.clone();
        row.child(avatar(Some(&user_avatar_url(&r.author)), 16.0, r.author.display(), 8.0, name_color(&r.author.id)))
            .child(
                div()
                    .id(SharedString::from(format!("rp{}", m.id)))
                    .flex_none()
                    .text_size(z(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(nc)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let p = window.mouse_position();
                        this.app.open_profile(author.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                        cx.notify();
                    }))
                    .child(format!("@{}", author_name(r))),
            )
            .child(line(body, 13.0, MUTED()).flex_1().ml(z(2.0)))
    }

    /// 本文。カスタム絵文字があれば画像を文中に並べる
    fn inline_content(&mut self, m: &Message) -> AnyElement {
        let fs = self.app.settings.font_size;
        let edited = HighlightStyle { color: Some(MUTED().into()), ..Default::default() };
        let displayed = self.app.plugins.transform("on_display", &m.content, &m.channel_id);
        let roles = self.guild_roles();
        let content = resolve_mentions(&displayed, m, &self.app.channels, roles).into_owned();
        let segs = split_emoji(&content);
        if !segs.iter().any(|x| matches!(x, Seg::Emoji { .. })) {
            let mut r = Rich::default();
            r.bold(&content);
            if m.edited_timestamp.is_some() {
                r.push(" (編集済)", Some(edited));
            }
            return div().min_w_0().text_size(z(fs)).text_color(TEXT()).child(r.element(SharedString::from(format!("t{}", m.id)))).into_any_element();
        }
        // 絵文字だけのメッセージは大きく表示
        let only_emoji = segs.iter().all(|x| match x {
            Seg::Text(t) => t.trim().is_empty(),
            Seg::Emoji { .. } => true,
        });
        let size = if only_emoji { 48.0 } else { (fs * 1.4).round() };
        let mut row = div().w_full().min_w_0().flex().flex_wrap().items_center().gap_y(z(2.0)).text_size(z(fs)).text_color(TEXT());
        let mut urls = Vec::new();
        for (i, seg) in segs.into_iter().enumerate() {
            match seg {
                Seg::Text(t) => {
                    if !(only_emoji && t.trim().is_empty()) {
                        let mut r = Rich::default();
                        r.bold(t);
                        // 長い文は行いっぱいで折り返す（flex_wrap の中では幅を絞らないとはみ出す）
                        row = row.child(div().max_w_full().child(r.element(SharedString::from(format!("t{}_{i}", m.id)))));
                    }
                }
                Seg::Emoji { name, id, animated } => {
                    let url = emoji_url(id, animated);
                    let face = match imgs::get(&url) {
                        Img::Ready(img_) => div().size(z(size)).child(img(img_).id("img").size_full().object_fit(ObjectFit::Contain)),
                        _ => div().size(z(size)),
                    };
                    row = row.child(face.id(SharedString::from(format!("e{}_{i}", m.id))).tooltip(tip(format!(":{name}:"))).when(only_emoji, |d| d.mr(z(4.0))));
                    urls.push(url);
                }
            }
        }
        for u in urls {
            self.note_image(&u);
        }
        if m.edited_timestamp.is_some() {
            row = row.child(text(" (編集済)", 11.0, MUTED()));
        }
        row.into_any_element()
    }

    /// 添付画像・スタンプ・埋め込み画像
    fn message_media(&mut self, m: &Message, cx: &mut Context<Self>) -> Div {
        let max = (420.0, 320.0);
        let mut col = div().flex().flex_col().gap(z(4.0)).items_start();
        for (i, a) in m.attachments.iter().enumerate() {
            if a.is_image() {
                let size = fit_size(a.width, a.height, max);
                let src = resized(a.proxy_url.as_deref(), &a.url, size.unwrap_or(max));
                self.note_image(&src);
                self.note_image(&a.url);
                col = col.child(image_box(SharedString::from(format!("at{}_{i}", m.id)), &src, &a.url, size.unwrap_or(max), false, cx));
            } else if a.is_video() && cfg!(target_os = "linux") {
                col = col.child(self.video_attachment(a, &m.channel_id, &format!("{}_{i}", m.id), cx));
            } else {
                col = col.child(embed_line(SharedString::from(format!("af{}_{i}", m.id)), format!("📎 {}", a.filename), 16.0, LINK(), Some(&a.url)));
            }
        }
        for st in &m.sticker_items {
            col = col.child(self.sticker(st));
        }
        for (i, e) in m.embeds.iter().enumerate() {
            if let Some(md) = e.media() {
                let size = fit_size(md.width, md.height, max).unwrap_or(max);
                // GIF リンクは .gif 版を取って動かす。取れなかったら静止画に戻す
                let gif = e.animated_url().filter(|g| !matches!(imgs::get(g), Img::Failed));
                let src = gif.unwrap_or_else(|| resized(md.proxy_url.as_deref(), &md.url, size));
                let open = e.animated_url().unwrap_or_else(|| md.url.clone());
                self.note_image(&src);
                self.note_image(&open);
                col = col.child(image_box(SharedString::from(format!("em{}_{i}", m.id)), &src, &open, size, false, cx));
            } else if e.is_rich() {
                col = col.child(self.embed_card(e, &format!("{}_{i}", m.id), cx));
            }
        }
        col
    }

    fn video_attachment(&mut self, attachment: &model::Attachment, channel: &str, key: &str, cx: &mut Context<Self>) -> Div {
        let size = fit_size(attachment.width, attachment.height, (420.0, 320.0)).unwrap_or((420.0, 236.0));
        let source = attachment.url.clone();
        let target_channel = channel.to_owned();
        let width = attachment.width.unwrap_or(640);
        let height = attachment.height.unwrap_or(360);
        let active = self.video.as_ref().filter(|v| v.url == source && v.channel == channel);
        let frame = active.and_then(|v| v.image.clone());
        let show_play = active.is_none_or(|v| v.paused || v.ended);
        let preview = if frame.is_none() { crate::video::thumbnail(&source, width, height, &self.app.waker) } else { None };
        let has_preview = preview.is_some();
        let frame = frame.or(preview);
        let label = match active { Some(v) if v.error.is_some() => "再生できません", Some(v) if v.ended => "もう一度再生", Some(v) if v.paused => "再生", Some(v) if v.image.is_some() => "一時停止", Some(_) => "読み込み中…", None => "動画を再生" };
        let error = active.and_then(|v| v.error.clone());
        let mut screen = div().id(SharedString::from(format!("video-screen-{key}"))).w(z(size.0)).h(z(size.1)).relative().rounded(z(8.0)).overflow_hidden().bg(Color32::BLACK).cursor_pointer().flex().items_center().justify_center();
        if let Some(frame) = frame { screen = screen.child(img(frame).id("video-frame").size_full().object_fit(ObjectFit::Contain)); }
        else { screen = screen.child(div().flex().flex_col().items_center().gap(z(8.0)).child(icon(IconName::Play, 32.0, WHITE())).child(text(label, 14.0, WHITE()))); }
        if has_preview && show_play {
            screen = screen.child(div().absolute().inset_0().flex().items_center().justify_center().child(
                div().size(z(52.0)).rounded_full().bg(Color32::from_rgba_unmultiplied(0, 0, 0, 170))
                    .flex().items_center().justify_center().child(icon(IconName::Play, 28.0, WHITE()))));
        }
        let download_url = attachment.url.clone();
        let download_name = attachment.filename.clone();
        screen = screen.child(div().id(SharedString::from(format!("video-download-{key}")))
            .absolute().top(z(8.0)).right(z(8.0)).size(z(32.0)).rounded(z(6.0))
            .bg(Color32::from_rgba_unmultiplied(0, 0, 0, 180)).hover(|s| s.bg(Color32::from_rgba_unmultiplied(0, 0, 0, 230)))
            .flex().items_center().justify_center().cursor_pointer().tooltip(tip("動画をダウンロード"))
            .child(icon(IconName::Download, 18.0, WHITE()))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if this.download.is_none() { this.download = Some(crate::download::start(download_url.clone(), download_name.clone(), this.app.waker.clone())); }
                else { this.app.show_toast("別の動画をダウンロード中です"); }
                cx.notify();
            })));
        let screen = screen.tooltip(tip(label)).on_click(cx.listener(move |this, _, _, cx| {
            if let Some(player) = this.video.as_mut().filter(|v| v.url == source && v.channel == target_channel) { player.toggle(); }
            else {
                this.video = None;
                match crate::video::Player::new(target_channel.clone(), source.clone(), width, height, this.app.waker.clone()) {
                    Ok(player) => this.video = Some(player),
                    Err(error) => this.app.show_toast(&error),
                }
            }
            cx.notify();
        }));
        let mut content = div().flex().flex_col().gap(z(4.0)).child(screen);
        if let Some(error) = error { content = content.child(text(error, 13.0, RED())); }
        let link = attachment.url.clone();
        let mut controls = div().w(z(size.0)).flex().items_center().gap(z(10.0)).child(line(attachment.filename.clone(), 12.0, MUTED()).flex_1());
        if active.is_some() {
            controls = controls.child(div().id(SharedString::from(format!("video-close-{key}"))).cursor_pointer().child(text("停止", 12.0, LINK())).on_click(cx.listener(|this, _, _, cx| { this.video = None; cx.notify(); })));
        }
        controls = controls.child(div().id(SharedString::from(format!("video-open-{key}"))).cursor_pointer().child(text("開く", 12.0, LINK())).on_click(cx.listener(move |_, _, _, cx| cx.open_url(&link))));
        content.child(controls)
    }

    fn sticker(&mut self, st: &model::StickerItem) -> AnyElement {
        const BOX: f32 = 160.0;
        match self.app.stickers.get(&st.id) {
            Some(StickerState::Ready(i)) => div()
                .id(SharedString::from(format!("st{}", st.id)))
                .size(z(BOX))
                .tooltip(tip(st.name.clone()))
                .child(img(i.clone()).id("img").size_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            Some(StickerState::Failed) => text(format!("[スタンプ: {}]", st.name), 16.0, MUTED()).into_any_element(),
            state => {
                if state.is_none() {
                    self.app.stickers.insert(st.id.clone(), StickerState::Loading);
                    self.app.send(Cmd::LoadSticker { id: st.id.clone(), format: st.format_type });
                }
                div().size(z(BOX)).flex().items_center().justify_center().child(spinner(24.0)).into_any_element()
            }
        }
    }

    fn embed_icon(&mut self, icon_url: Option<&str>, proxy: Option<&str>, size: f32) -> Option<Div> {
        let u = proxy.or(icon_url)?.to_string();
        self.note_image(&u);
        Some(avatar(Some(&u), size, "", size / 2.0, Color32::TRANSPARENT))
    }

    /// タイトル・説明・フィールドなどを持つ埋め込み（リンクのプレビューや bot の埋め込み）
    fn embed_card(&mut self, e: &Embed, key: &str, cx: &mut Context<Self>) -> Div {
        let bar = e.color.filter(|&c| c != 0).map(rgb).unwrap_or(EMBED_BAR());
        let big = embed_big_media(e);
        let small_thumb = if big.is_none() { e.thumbnail.as_ref() } else { None };
        let inner = EMBED_MAX_W - 32.0;
        let mut body = div().flex_1().min_w_0().flex().flex_col().gap(z(6.0)).pt(z(2.0));
        if let Some(name) = e.provider.as_ref().and_then(|p| p.name.clone()) {
            body = body.child(embed_line(SharedString::from(format!("ep{key}")), name, 12.0, MUTED(), e.provider.as_ref().and_then(|p| p.url.as_deref())));
        }
        if let Some(a) = &e.author {
            if let Some(name) = a.name.clone() {
                let mut r = div().flex().items_center().gap(z(8.0));
                if let Some(i) = self.embed_icon(a.icon_url.as_deref(), a.proxy_icon_url.as_deref(), 24.0) {
                    r = r.child(i);
                }
                r = r.child(embed_line(SharedString::from(format!("ea{key}")), name, 14.0, WHITE(), a.url.as_deref()));
                body = body.child(r);
            }
        }
        if let Some(t) = e.title.clone() {
            let color = if e.url.is_some() { LINK() } else { WHITE() };
            body = body.child(div().font_weight(FontWeight::MEDIUM).child(embed_line(SharedString::from(format!("et{key}")), t, 16.0, color, e.url.as_deref())));
        }
        if let Some(d) = e.description.as_deref().filter(|d| !d.is_empty()) {
            body = body.child(embed_text(SharedString::from(format!("ed{key}")), d, 14.0, TEXT()));
        }
        // フィールド: inline は最大3つまで横に並べる
        let mut i = 0;
        while i < e.fields.len() {
            let mut n = 1;
            if e.fields[i].inline {
                while n < 3 && i + n < e.fields.len() && e.fields[i + n].inline {
                    n += 1;
                }
            }
            let mut r = div().flex().gap(z(8.0));
            for (j, f) in e.fields[i..i + n].iter().enumerate() {
                r = r.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(z(2.0))
                        .child(embed_text(SharedString::from(format!("efn{key}_{}", i + j)), &f.name, 14.0, WHITE()).font_weight(FontWeight::MEDIUM))
                        .child(embed_text(SharedString::from(format!("efv{key}_{}", i + j)), &f.value, 14.0, TEXT())),
                );
            }
            body = body.child(r);
            i += n;
        }
        let mut top = div().flex().items_start().gap(z(16.0)).child(body);
        if let Some(t) = small_thumb {
            let size = fit_size(t.width, t.height, (EMBED_THUMB, EMBED_THUMB)).unwrap_or((EMBED_THUMB, EMBED_THUMB));
            let src = resized(t.proxy_url.as_deref(), &t.url, size);
            self.note_image(&src);
            self.note_image(&t.url);
            top = top.child(div().mt(z(4.0)).child(image_box(SharedString::from(format!("eth{key}")), &src, &t.url, size, false, cx)));
        }
        let mut col = div().flex().flex_col().child(top);
        if let Some(md) = big {
            let size = fit_size(md.width, md.height, (inner, 300.0)).unwrap_or((inner, 300.0));
            let src = resized(md.proxy_url.as_deref(), &md.url, size);
            self.note_image(&src);
            let video = e.video.is_some() || e.kind.as_deref() == Some("video");
            let open = if video { e.url.clone().unwrap_or_else(|| md.url.clone()) } else { md.url.clone() };
            if !video { self.note_image(&open); }
            let mut b = div().relative().mt(z(8.0)).child(image_box(SharedString::from(format!("eb{key}")), &src, &open, size, video, cx));
            if video {
                // 動画はアプリ内で再生できないので、再生マークを重ねてブラウザで開く
                b = b.child(
                    div().absolute().inset_0().flex().items_center().justify_center().child(
                        div().size(z(48.0)).rounded_full().bg(Color32::from_black_alpha(170)).flex().items_center().justify_center().child(icon(IconName::Play, 22.0, WHITE())),
                    ),
                );
            }
            col = col.child(b);
        }
        // フッター
        let time = e.timestamp.as_deref().and_then(local_dt).map(|t| t.format("%Y/%m/%d %H:%M").to_string());
        let ft = e.footer.as_ref().map(|f| f.text.as_str()).filter(|t| !t.is_empty());
        let foot = match (ft, &time) {
            (Some(t), Some(d)) => Some(format!("{t} • {d}")),
            (Some(t), None) => Some(t.to_string()),
            (None, Some(d)) => Some(d.clone()),
            (None, None) => None,
        };
        if let Some(foot) = foot {
            let mut r = div().mt(z(8.0)).flex().items_center().gap(z(8.0));
            if let Some(f) = &e.footer {
                if let Some(i) = self.embed_icon(f.icon_url.as_deref(), f.proxy_icon_url.as_deref(), 20.0) {
                    r = r.child(i);
                }
            }
            col = col.child(r.child(text(foot, 12.0, MUTED())));
        }
        div()
            .max_w(z(EMBED_MAX_W))
            .bg(EMBED_BG())
            .rounded(z(4.0))
            .border_l(z(4.0))
            .border_color(bar)
            .pl(z(12.0))
            .pr(z(16.0))
            .pt(z(8.0))
            .pb(z(12.0))
            .child(col)
    }
}

#[cfg(test)]
mod tests {
    use super::{call_length, Message, Rich};

    #[test]
    fn call_length_reads_naturally() {
        let len = |s| call_length(chrono::Duration::seconds(s));
        assert_eq!(len(4), "数秒");
        assert_eq!(len(59), "数秒");
        assert_eq!(len(5 * 60 + 30), "5分");
        assert_eq!(len(3600), "1時間");
        assert_eq!(len(3600 + 20 * 60), "1時間20分");
        assert_eq!(len(-3), "数秒");
    }

    #[test]
    fn call_rows_stand_alone() {
        let msg = |id: &str, kind: u8| -> Message {
            serde_json::from_value(serde_json::json!({
                "id": id, "channel_id": "1", "author": { "id": "7", "username": "a" }, "type": kind,
                "timestamp": "2026-11-09T17:24:00+09:00",
                "call": if kind == 3 { serde_json::json!({ "participants": ["7"], "ended_timestamp": "2026-11-09T17:24:05+09:00" }) } else { serde_json::Value::Null },
            }))
            .unwrap()
        };
        let (text, call) = (msg("1", 0), msg("2", 3));
        assert_eq!(call.call.as_ref().unwrap().participants, ["7"]);
        assert!(super::row_layout(&text, Some(&text), false).1, "ふつうの連投はまとめる");
        assert!(!super::row_layout(&call, Some(&text), false).1, "通話の記録はまとめない");
        assert!(!super::row_layout(&text, Some(&call), false).1, "通話の記録の次の行は名前を出す");
    }

    #[test]
    fn bold_markers_pair_and_leave_unpaired_as_text() {
        let render = |t: &str| {
            let mut r = Rich::default();
            r.bold(t);
            let bolded: String = r.styles.iter().filter(|(_, s)| s.font_weight.is_some()).map(|(range, _)| &r.text[range.clone()]).collect();
            (r.text.clone(), bolded)
        };
        assert_eq!(render("a **b** c"), ("a b c".to_string(), "b".to_string()));
        assert_eq!(render("**x** と **y**"), ("x と y".to_string(), "xy".to_string()));
        assert_eq!(render("2 ** 3"), ("2 ** 3".to_string(), String::new()));
        assert_eq!(render("**a** **b"), ("a **b".to_string(), "a".to_string()));
        assert_eq!(render("普通"), ("普通".to_string(), String::new()));
    }
}
