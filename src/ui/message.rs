//! メッセージの1行（本文のリンク・絵文字、添付画像、埋め込み、スタンプ、返信、操作ボタン）
use std::ops::Range;
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{
    author_name, decoration_for, embed_big_media, emoji_url, find_links, fit_size, fmt_time,
    is_reply, local_dt, member_avatar_url, member_role_color, name_color, resized,
    resolve_mentions, rgb, split_emoji, user_avatar_url, Seg, StickerState,
};
use crate::color::Color32;
use crate::images::{self as imgs, Img};
use crate::markdown as md;
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
    row_layout_at(
        m,
        prev,
        local_dt(&m.timestamp),
        prev.and_then(|p| local_dt(&p.timestamp)),
        compact,
    )
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
        (0, 0) => tr!("数秒", "a few seconds", "几秒").into(),
        (0, m) => tr!("{m}分", "{m}m", "{m} 分钟",),
        (h, 0) => tr!("{h}時間", "{h}h", "{h} 小时",),
        (h, m) => tr!("{h}時間{m}分", "{h}h {m}m", "{h} 小时 {m} 分钟",),
    }
}

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
        h.underline = Some(UnderlineStyle {
            thickness: px(1.0),
            ..Default::default()
        });
    }
    if st.strike {
        h.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.0),
            ..Default::default()
        });
    }
    if st.code || st.spoiler {
        h.background_color = Some(WIDGET().into());
    }
    Some(h)
}

impl Rich {
    /// ColorIndicator: #rrggbb をその色で太字にする（ほかの飾り・リンクと重ならない所だけ）
    pub fn color_codes(&mut self) {
        let mut add = Vec::new();
        let mut i = 0;
        while let Some(pos) = self.text[i..].find('#') {
            let s = i + pos;
            let e = s + 7;
            let ok = e <= self.text.len()
                && self.text.is_char_boundary(e)
                && self.text[s + 1..e].bytes().all(|b| b.is_ascii_hexdigit())
                && !self.text[e..].starts_with(|c: char| c.is_ascii_alphanumeric());
            if !ok {
                i = s + 1;
                continue;
            }
            let free = !self.styles.iter().any(|(r, _)| r.start < e && s < r.end)
                && !self.links.iter().any(|(r, _)| r.start < e && s < r.end);
            if free {
                if let Ok(v) = u32::from_str_radix(&self.text[s + 1..e], 16) {
                    let c = Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
                    add.push((
                        s..e,
                        HighlightStyle {
                            color: Some(c.into()),
                            font_weight: Some(FontWeight::BOLD),
                            underline: Some(UnderlineStyle {
                                thickness: px(2.0),
                                color: Some(c.into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ));
                }
            }
            i = e;
        }
        if !add.is_empty() {
            self.styles.extend(add);
            self.styles.sort_by_key(|(r, _)| r.start);
        }
    }

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
            self.push(
                url,
                Some(HighlightStyle {
                    color: Some(LINK().into()),
                    ..style.unwrap_or_default()
                }),
            );
            self.links.push((start..self.text.len(), url.to_string()));
            last = if wrapped { r.end + 1 } else { r.end };
        }
        self.push(&text[last..], style);
    }

    /// [文字](https://...) を文字だけ青くして押せるようにする。それ以外は linked に任せる
    pub fn masked(&mut self, text: &str, style: Option<HighlightStyle>) {
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            let Some(close) = rest[open..].find("](").map(|i| open + i) else {
                break;
            };
            let after = &rest[close + 2..];
            let Some(end) = after.find(')') else { break };
            let url = after[..end]
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>');
            let label = &rest[open + 1..close];
            if label.contains('[') || !(url.starts_with("https://") || url.starts_with("http://")) {
                self.linked(&rest[..open + 1], style);
                rest = &rest[open + 1..];
                continue;
            }
            self.linked(&rest[..open], style);
            let start = self.text.len();
            self.push(
                label,
                Some(HighlightStyle {
                    color: Some(LINK().into()),
                    ..style.unwrap_or_default()
                }),
            );
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
            self.push(
                &s.text,
                Some(HighlightStyle {
                    color: Some(hidden),
                    background_color: Some(hidden),
                    ..Default::default()
                }),
            );
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
    pub fn element_with(
        self,
        id: impl Into<ElementId>,
        on_spoiler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    ) -> AnyElement {
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
            styled = styled
                .with_font_family_overrides(self.mono.into_iter().map(|r| (r, family.clone())));
        }
        let spoilers = if on_spoiler.is_some() {
            self.spoilers
        } else {
            Vec::new()
        };
        if self.links.is_empty() && spoilers.is_empty() {
            return styled.into_any_element();
        }
        let (ranges, urls): (Vec<_>, Vec<_>) = self.links.into_iter().unzip();
        let urls2 = urls.clone();
        let ranges2 = ranges.clone();
        let clickable = ranges.iter().cloned().chain(spoilers).collect();
        InteractiveText::new(id, styled)
            .on_click(clickable, move |i, window, cx| match urls.get(i) {
                Some(url) if url.starts_with(MENTION_LINK) => {
                    *PENDING_MENTION.lock().unwrap() = Some(url[MENTION_LINK.len()..].to_string());
                    window.refresh();
                }
                Some(url) => cx.open_url(url),
                None => {
                    if let Some(f) = &on_spoiler {
                        f(window, cx)
                    }
                }
            })
            .tooltip(move |ix, window, cx| {
                let i = ranges2.iter().position(|r| r.contains(&ix))?;
                let label = if urls2[i].starts_with(MENTION_LINK) {
                    tr!("プロフィールを開く", "Open profile", "打开个人资料").to_string()
                } else {
                    urls2[i].clone()
                };
                Some(gpui::component::tooltip::Tooltip::new(label).build(window, cx))
            })
            .into_any_element()
    }
}

/// ClickableMentions: 押せるメンションのリンク先の印（後ろはユーザー ID）
pub const MENTION_LINK: &str = "riscord-mention:";
/// StickerEmojiPreview: 押された絵文字の大きい画像（Root の描画で開く）
pub static PENDING_PREVIEW: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// 押されたメンションのユーザー ID（Root の描画で開く）
pub static PENDING_MENTION: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// メッセージのツールバーで、1 行（slots 個）に収まるよう (並べるクイックリアクションの数, プラグインの操作の数) を決める。
/// 本体のボタン（base 個）は常に並べ、プラグインの操作があれば「…」かボタン 1 つ分を残す
pub fn toolbar_fit(slots: usize, base: usize, reacts: usize, actions: usize) -> (usize, usize) {
    let room = slots.saturating_sub(base);
    let n_reacts = reacts.min(room.saturating_sub(usize::from(actions > 0)));
    (n_reacts, room - n_reacts)
}

/// 名前の横のタグ（同梱プラグインの StaffTag・PlatformIndicators など）
pub fn tag_elements(tags: &[crate::app::bd_display::Tag], key: &str) -> Vec<AnyElement> {
    tags.iter()
        .enumerate()
        .map(|(i, t)| {
            let el = div()
                .id(SharedString::from(format!("tag{key}_{i}")))
                .flex_none()
                .max_w(z(160.0))
                .truncate()
                .text_size(z(11.0))
                .text_color(t.color)
                .when(t.pill, |d| {
                    d.px(z(5.0))
                        .rounded(z(4.0))
                        .border_1()
                        .border_color(t.color)
                        .font_weight(FontWeight::MEDIUM)
                })
                .child(t.text.clone());
            match &t.tip {
                Some(tip_text) => el.tooltip(tip(tip_text.clone())).into_any_element(),
                None => el.into_any_element(),
            }
        })
        .collect()
}

/// 返信のプレビュー用に、本文を記法を解いた1行にする。リンクは色だけで、押せない
pub fn preview_rich(content: &str, revealed: bool) -> Rich {
    let mut r = Rich::default();
    for block in md::blocks(content) {
        if !r.text.is_empty() {
            r.push(" ", None);
        }
        let spans = match block {
            md::Block::Code(code) => vec![md::Span {
                text: code,
                style: md::Style {
                    code: true,
                    ..Default::default()
                },
            }],
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
fn embed_text(id: impl Into<ElementId>, s: &str, size: f32, color: Color32) -> Div {
    let mut r = Rich::default();
    for span in md::inline(s) {
        r.span(&span, true);
    }
    div()
        .text_size(z(size))
        .text_color(color)
        .child(r.element(id))
}

/// 押すとブラウザで開く1行の文字
fn embed_line(
    id: impl Into<ElementId>,
    s: impl Into<SharedString>,
    size: f32,
    color: Color32,
    url: Option<&str>,
) -> AnyElement {
    let t = text(s, size, color);
    match url {
        Some(u) => {
            let u = u.to_string();
            t.id(id)
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(move |_, _, cx| cx.open_url(&u))
                .into_any_element()
        }
        None => t.into_any_element(),
    }
}

/// 動画・画像の右上に重ねるダウンロードのボタン（押したときの動きは呼ぶ側で付ける）
fn download_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .top(z(8.0))
        .right(z(8.0))
        .size(z(32.0))
        .rounded(z(6.0))
        .bg(Color32::from_rgba_unmultiplied(0, 0, 0, 180))
        .hover(|s| s.bg(Color32::from_rgba_unmultiplied(0, 0, 0, 230)))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(tip(label))
        .child(icon(IconName::Download, 18.0, WHITE()))
}

/// 1024 区切りで KB / MB / GB にする（Discord と同じ表し方）
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// 画像・動画以外の添付ファイル。名前を押すとブラウザで開き、右のボタンで保存する
fn file_card(id: SharedString, a: &model::Attachment, cx: &mut Context<Root>) -> Stateful<Div> {
    let open = a.url.clone();
    let url = a.url.clone();
    let name = a.filename.clone();
    let mut info = div().flex_1().min_w_0().flex().flex_col().child(
        line(a.filename.clone(), 16.0, LINK())
            .id(SharedString::from(format!("{id}-name")))
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(move |_, _, cx| cx.open_url(&open)),
    );
    if let Some(size) = a.size {
        info = info.child(text(human_size(size), 12.0, MUTED()));
    }
    div()
        .id(id.clone())
        .w(z(432.0))
        .max_w_full()
        .p(z(16.0))
        .rounded(z(8.0))
        .bg(WIDGET())
        .border_1()
        .border_color(DIVIDER())
        .flex()
        .items_center()
        .gap(z(12.0))
        .child(icon(IconName::File, 32.0, LINK()).flex_none())
        .child(info)
        .child(
            div()
                .id(SharedString::from(format!("{id}-dl")))
                .flex_none()
                .p(z(4.0))
                .rounded(z(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(HOVER()))
                .tooltip(tip(tr!("ダウンロード", "Download", "下载")))
                .child(icon(IconName::Download, 20.0, MUTED()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start_download(url.clone(), name.clone(), crate::download::Kind::File);
                    cx.notify();
                })),
        )
}

/// ダウンロードのボタンを重ねる画像の最小の大きさ（これより小さいとボタンで絵が隠れる）
const DOWNLOAD_MIN: f32 = 80.0;

/// 画像（読み込み中は同じ大きさの箱を出して、行の高さが変わらないようにする）。
/// download = 保存するときのファイル名。Some なら右上にダウンロードのボタンを出す（保存するのは open の元画像）
fn image_box(
    id: impl Into<ElementId>,
    src: &str,
    open: &str,
    size: (f32, f32),
    video: bool,
    download: Option<String>,
    cx: &mut Context<Root>,
) -> Stateful<Div> {
    let open = open.to_string();
    let d = div()
        .id(id)
        .relative()
        .w(z(size.0))
        .h(z(size.1))
        .max_w_full()
        .rounded(z(8.0))
        .overflow_hidden()
        .cursor_pointer()
        .on_click(cx.listener({
            let open = open.clone();
            move |this, _, _, cx| {
                if video {
                    cx.open_url(&open);
                } else {
                    this.open_image_preview(open.clone());
                    cx.notify();
                }
            }
        }));
    let d = match imgs::get(src) {
        Img::Ready(i) => d.child(
            img(i)
                .id("img")
                .size_full()
                .rounded(z(8.0))
                .object_fit(ObjectFit::Contain),
        ),
        Img::Pending => d
            .bg(EMBED_BG())
            .flex()
            .items_center()
            .justify_center()
            .child(spinner(20.0)),
        Img::Failed => d
            .bg(EMBED_BG())
            .flex()
            .items_center()
            .justify_center()
            .child(icon(IconName::TriangleAlert, 20.0, FAINT())),
    };
    match download.filter(|_| size.0 >= DOWNLOAD_MIN && size.1 >= DOWNLOAD_MIN) {
        Some(name) => d.child(
            download_button(
                "download",
                tr!("画像をダウンロード", "Download image", "下载图片"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                // 下の画像（プレビューを開く）には伝えない
                cx.stop_propagation();
                this.start_download(open.clone(), name.clone(), crate::download::Kind::Image);
                cx.notify();
            })),
        ),
        None => d,
    }
}

impl Root {
    /// 開いているサーバーのロール（DM なら空）
    fn guild_roles(&self) -> &[Role] {
        self.app
            .sel_guild
            .as_ref()
            .and_then(|g| self.app.roles.get(g))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// 画像を出したことを覚える（チャンネルを捨てるときに画像のキャッシュも解放するため）
    pub(super) fn note_image(&mut self, uri: &str) {
        let Some(cid) = &self.app.sel_channel else {
            return;
        };
        // 毎フレーム見えている画像の数だけ呼ばれるので、覚え済みならキーも作らない
        if let Some(set) = self.app.channel_images.get_mut(cid) {
            if !set.contains(uri) {
                set.insert(uri.to_string());
            }
        } else {
            self.app
                .channel_images
                .entry(cid.clone())
                .or_default()
                .insert(uri.to_string());
        }
    }

    pub fn message_row(
        &mut self,
        m: &Message,
        prev: Option<&Message>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (new_day, grouped, reply, compact) = row_layout(m, prev, self.app.settings.compact);
        let hovered = self.hover_msg.as_deref() == Some(m.id.as_str());
        let highlight = self
            .app
            .editing
            .as_ref()
            .or(self.app.reply_to.as_ref())
            .is_some_and(|x| x.id == m.id)
            || self.app.is_jump_flash(&m.id);
        // ロールは行ごとに複製せず、色を決める間だけ借りる（ロールの多いサーバーで見えている行の数だけ Vec を作っていた）
        let name_col = self
            .app
            .bd_name_color(
                &m.author.id,
                m.member.as_ref(),
                self.app.sel_guild.as_deref(),
            )
            .or_else(|| {
                m.member
                    .as_ref()
                    .and_then(|member| member_role_color(member, self.guild_roles()))
            })
            .unwrap_or(WHITE());
        let shown_name = self.app.shown_name(&m.author.id, author_name(m));
        let tags = self.app.bd_tags(m);
        let always_time = self.app.bd_always_time();
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
                        .child(
                            text(
                                t.format(tr!("%Y年%-m月%-d日", "%b %-d, %Y", "%Y年%-m月%-d日"))
                                    .to_string(),
                                12.0,
                                FAINT(),
                            )
                            .part(Part::MessageDate),
                        )
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
                // 描くたびに Message を丸ごと（埋め込み・返信先まで）複製しないよう、押されたときに ID で引く
                let (cid, id) = (m.channel_id.clone(), m.id.clone());
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let Some(m) = this.app.find_message(&cid, &id) else {
                        return;
                    };
                    this.menu = Some((PopMenu::Message, ev.position));
                    this.menu_msg = Some(m);
                    cx.stop_propagation();
                    cx.notify();
                })
            })
            .on_click({
                let (cid, id) = (m.channel_id.clone(), m.id.clone());
                cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    let mods = ev.modifiers();
                    let Some(m) = this.app.find_message(&cid, &id) else {
                        return;
                    };
                    if this.app.message_click(
                        m,
                        ev.click_count(),
                        mods.shift,
                        mods.control || mods.platform,
                        mods.alt,
                    ) {
                        cx.notify();
                    }
                })
            })
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h && this.hover_msg.as_deref() != Some(mid.as_str()) {
                    this.hover_msg = Some(mid.clone());
                    cx.notify();
                }
            }));
        let deleted_at = m.log.as_ref().and_then(|l| l.deleted_at.as_deref());
        if highlight {
            row = row
                .bg(Color32::from_rgba_unmultiplied(0x58, 0x65, 0xf2, 14))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.0))
                        .bg(BLURPLE()),
                );
        } else if deleted_at.is_some() {
            // MessageLogger: 削除されたメッセージは赤く残す
            let a = if hovered { 34 } else { 22 };
            row = row
                .bg(Color32::from_rgba_unmultiplied(0xf2, 0x3f, 0x43, a))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.0))
                        .bg(RED()),
                );
        } else if hovered {
            row = row.bg(MSG_HOVER());
        }
        let author = m.author.clone();
        // コンパクト表示の名前の列の幅（狭い画面では本文の幅を残す）と、本文の開始位置
        let name_w = if self.app.screen_w < 700.0 {
            72.0
        } else {
            120.0
        };
        let text_x = COMPACT_NAME_X + name_w + COMPACT_GAP;
        if compact {
            outer = outer.when(!grouped, |d| d.pt(z(if new_day { 6.0 } else { 4.0 })));
            row = row.py(px(1.0));
            if reply {
                row = row.child(self.reply_preview(m, COMPACT_NAME_X + 8.0, text_x, cx));
            }
            // 時刻・名前・本文を固定の列にそろえる。連投は名前を省き、時刻はマウスを乗せたときだけ出す
            let time = if !grouped || hovered || always_time {
                self.app.bd_time(&m.timestamp, true).unwrap_or_else(|| {
                    local_dt(&m.timestamp)
                        .map(|t| t.format("%H:%M").to_string())
                        .unwrap_or_default()
                })
            } else {
                String::new()
            };
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
                        this.app.open_profile(
                            author.clone(),
                            Bounds::new(p, size(px(0.), px(0.))),
                            false,
                        );
                        cx.notify();
                    }))
                    .child(shown_name.clone())
                    .into_any_element()
            };
            let name: AnyElement = if grouped || tags.is_empty() {
                name
            } else {
                // タグが多くても本文の幅を残す（あふれたタグは切る）
                div()
                    .flex_none()
                    .max_w(relative(0.45))
                    .overflow_hidden()
                    .flex()
                    .items_baseline()
                    .gap(z(4.0))
                    .child(name)
                    .children(tag_elements(&tags, &m.id))
                    .into_any_element()
            };
            let mut body = div().flex_1().min_w_0();
            if !m.content.is_empty() && !self.app.bd_hide_content(m) {
                body = body.child(self.inline_content(m, cx));
            }
            // 本文のない連投（画像だけなど）は、空の行を作らない
            let head = !grouped || !m.content.is_empty();
            row = row.when(head, |r| {
                r.child(
                    div()
                        .pl(z(COMPACT_PAD))
                        .pr(z(16.0))
                        .flex()
                        .items_baseline()
                        .gap(z(COMPACT_GAP))
                        .child(
                            div()
                                .w(z(COMPACT_TIME_W))
                                .flex_none()
                                .flex()
                                .justify_end()
                                .child(text(time, 11.0, FAINT()).part(Part::MessageTime)),
                        )
                        .child(name)
                        .child(body),
                )
            });
            if !m.attachments.is_empty()
                || !m.sticker_items.is_empty()
                || m.embeds.iter().any(|e| e.visible())
                || m.poll.is_some()
            {
                row = row.child(
                    div()
                        .pl(z(text_x))
                        .pr(z(16.0))
                        .child(self.media_or_collapsed(m, cx)),
                );
            }
            if let Some(r) = self.reactions_row(m, cx) {
                row = row.child(div().pl(z(text_x)).pr(z(16.0)).child(r));
            }
        } else {
            outer = outer.when(!grouped, |d| d.pt(z(13.0)));
            row = row.pt(z(if grouped { 1.0 } else { 4.0 })).pb(z(2.0));
            if reply {
                row = row.child(self.reply_preview(m, 36.0, 72.0, cx).mb(z(2.0)));
            }
            let left: AnyElement = if grouped {
                // まとめた行はアバターの代わりに、マウスを乗せたとき時刻を出す
                let t = if hovered || always_time {
                    self.app.bd_time(&m.timestamp, true).unwrap_or_else(|| {
                        local_dt(&m.timestamp)
                            .map(|t| t.format("%H:%M").to_string())
                            .unwrap_or_default()
                    })
                } else {
                    String::new()
                };
                div()
                    .w(z(40.0))
                    .h(z(22.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(text(t, 11.0, FAINT()).part(Part::MessageTime))
                    .into_any_element()
            } else {
                let url = match self.app.sel_guild.as_deref() {
                    Some(g) => member_avatar_url(g, &m.author, m.member.as_ref()),
                    None => user_avatar_url(&m.author),
                };
                let deco = decoration_for(&m.author, m.member.as_ref());
                let author = author.clone();
                with_decoration(
                    avatar(
                        Some(&url),
                        40.0,
                        m.author.display(),
                        20.0,
                        name_color(&m.author.id),
                    ),
                    40.0,
                    deco.as_deref(),
                )
                .id(SharedString::from(format!("av{}", m.id)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    let p = window.mouse_position();
                    this.app.open_profile(
                        author.clone(),
                        Bounds::new(p, size(px(0.), px(0.))),
                        false,
                    );
                    cx.notify();
                }))
                .into_any_element()
            };
            let mut content = div().flex_1().min_w_0().flex().flex_col().gap(z(2.0));
            if !grouped {
                let author = author.clone();
                content = content.child(
                    // プラグインのタグが多いときは横にはみ出さず折り返す
                    div()
                        .flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap_x(z(8.0))
                        .gap_y(z(2.0))
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
                                    this.app.open_profile(
                                        author.clone(),
                                        Bounds::new(p, size(px(0.), px(0.))),
                                        false,
                                    );
                                    cx.notify();
                                }))
                                .child(shown_name.clone()),
                        )
                        .children(tag_elements(&tags, &m.id))
                        .child(
                            text(
                                self.app
                                    .bd_time(&m.timestamp, false)
                                    .unwrap_or_else(|| fmt_time(&m.timestamp)),
                                12.0,
                                FAINT(),
                            )
                            .part(Part::MessageTime),
                        ),
                );
            }
            if !m.content.is_empty() && !self.app.bd_hide_content(m) {
                content = content.child(self.inline_content(m, cx));
            }
            content = content.children(self.peek_cards(m, cx));
            content = content.child(self.media_or_collapsed(m, cx));
            if let Some(r) = self.reactions_row(m, cx) {
                content = content.child(r);
            }
            row = row.child(
                div()
                    .flex()
                    .items_start()
                    .pl(z(16.0))
                    .pr(z(16.0))
                    .gap(z(16.0))
                    .child(left)
                    .child(content),
            );
        }
        // コマンドの結果など、自分にだけ見えているメッセージ
        if m.flags & (1 << 6) != 0 {
            row = row.child(
                div()
                    .pl(z(if compact { text_x } else { 72.0 }))
                    .pr(z(16.0))
                    .child(text(
                        tr!(
                            "あなただけに表示されています",
                            "Only you can see this",
                            "仅你可见"
                        ),
                        12.0,
                        FAINT(),
                    )),
            );
        }
        if self.app.plugins.translator_enabled() {
            if let Some((_, translated)) = self
                .app
                .translator
                .messages
                .get(&(m.channel_id.clone(), m.id.clone()))
                .filter(|(original, _)| original == &m.content)
            {
                row = row.child(
                    div()
                        .pl(z(if compact { text_x } else { 72.0 }))
                        .pr(z(16.0))
                        .child(text(
                            tr!(
                                "翻訳: {translated}",
                                "Translation: {translated}",
                                "翻译：{translated}",
                            ),
                            self.app.settings.font_size,
                            MUTED(),
                        )),
                );
            }
        }
        if let Some(log) = &m.log {
            let pad = if compact { text_x } else { 72.0 };
            let fs = self.app.settings.font_size;
            if !log.edits.is_empty() {
                let mut col = div().pl(z(pad)).pr(z(16.0)).flex().flex_col().gap(z(2.0));
                for e in &log.edits {
                    let t = local_dt(&e.at)
                        .map(|t| t.format("%-m/%-d %H:%M").to_string())
                        .unwrap_or_default();
                    col = col.child(text(
                        tr!(
                            "編集前（{t} に変更）: {}",
                            "Before edit (changed {t}): {}",
                            "编辑前（{t} 修改）：{}",
                            e.content
                        ),
                        fs - 1.0,
                        FAINT(),
                    ));
                }
                row = row.child(col);
            }
            if let Some(at) = deleted_at {
                let t = local_dt(at)
                    .map(|t| t.format("%-m/%-d %H:%M").to_string())
                    .unwrap_or_default();
                row = row.child(div().pl(z(pad)).pr(z(16.0)).child(text(
                    tr!("削除済み（{t}）", "Deleted ({t})", "已删除（{t}）",),
                    12.0,
                    RED(),
                )));
            }
        }
        if hovered {
            row = row.child(self.message_toolbar(m, cx));
        }
        outer.child(row)
    }

    /// 通話の記録（不在着信か、参加した通話か、続いている通話か）
    fn call_row(
        &mut self,
        m: &Message,
        name_col: Color32,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let call = m.call.as_ref();
        let ended = call.and_then(|c| c.ended_timestamp.as_deref());
        let me = self.app.me.as_ref().map(|u| u.id.as_str());
        // 自分が参加者に入っていなければ不在着信（参加者が分からないときは不在にしない）
        let missed = ended.is_some()
            && call.is_some_and(|c| me.is_some_and(|me| !c.participants.iter().any(|p| p == me)));
        let length = ended.and_then(|e| Some(call_length(local_dt(e)? - local_dt(&m.timestamp)?)));
        let after = match (missed, &length) {
            (true, Some(l)) => tr!(
                " からの不在着信（{l}）",
                " · missed call ({l})",
                " · 未接来电（{l}）",
            ),
            (true, None) => tr!(" からの不在着信", " · missed call", " · 未接来电").to_string(),
            (false, Some(l)) => tr!(
                " が通話を開始しました（{l}）",
                " started a call ({l})",
                " 发起了通话（{l}）",
            ),
            (false, None) => {
                tr!(" が通話を開始しました", " started a call", " 发起了通话").to_string()
            }
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
                this.app
                    .open_profile(author.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                cx.notify();
            }))
            .child(author_name(m).to_string());
        let color = if missed { MUTED() } else { GREEN() };
        // アイコンは、ふつうの行のアバターの列（幅 40）の中央に置く
        let left = if compact {
            div().w(z(COMPACT_TIME_W)).flex_none().flex().justify_end()
        } else {
            div().w(z(40.0)).flex_none().flex().justify_center()
        };
        div()
            .w_full()
            .pt(z(if compact { 4.0 } else { 13.0 }))
            .pb(z(2.0))
            .pl(z(if compact { COMPACT_PAD } else { 16.0 }))
            .pr(z(16.0))
            .flex()
            .items_center()
            .gap(z(if compact { COMPACT_GAP } else { 16.0 }))
            .child(left.child(icon(
                if missed {
                    IconName::PhoneOff
                } else {
                    IconName::Phone
                },
                18.0,
                color,
            )))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .child(name)
                    .child(text(after, fs, MUTED()).flex_none())
                    .child(
                        text(fmt_time(&m.timestamp), 12.0, FAINT())
                            .part(Part::MessageTime)
                            .flex_none()
                            .ml(z(8.0)),
                    ),
            )
    }

    /// 右上に出る操作ボタン（返信・編集・コピー・削除）
    fn message_toolbar(&mut self, m: &Message, cx: &mut Context<Self>) -> impl IntoElement {
        // 削除済みとして残しているメッセージには、返信・編集・削除をさせない
        let deleted = m.log.as_ref().is_some_and(|l| l.deleted_at.is_some());
        let mine = !deleted && self.app.me.as_ref().is_some_and(|u| u.id == m.author.id);
        let actions = self.app.plugins.message_actions();
        let reacts = if deleted {
            Vec::new()
        } else {
            self.app.quick_reacts()
        };
        // 1 行に並ぶボタンの数（幅 30 ずつ）。狭い画面や拡大時は、プラグインのボタンから「…」へ回す
        let base = usize::from(!deleted)
            + usize::from(!deleted && self.app.plugins.on("quick-mention"))
            + 2 * usize::from(mine)
            + usize::from(!m.content.is_empty())
            + usize::from(self.app.plugins.translator_enabled() && !m.content.trim().is_empty());
        let slots = ((self.stage_width() * 0.9 - 12.0) / 30.0).floor().max(4.0) as usize;
        let (n_reacts, max_inline) = toolbar_fit(slots, base, reacts.len(), actions.len());
        let mut bar = div()
            .absolute()
            .top(z(-16.0))
            .right(z(16.0))
            .min_h(z(32.0))
            .max_w(relative(0.9))
            .px(z(2.0))
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .bg(SIDEBAR())
            .border_1()
            .border_color(BORDER())
            .rounded(z(8.0))
            .part(Part::MessageToolbar);
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
                .child(icon(name, 18.0, MUTED()).group_hover("tb", move |s| {
                    s.text_color(if danger { RED() } else { WHITE() })
                }))
        };
        let m1 = m.clone();
        bar = bar.when(!deleted, |bar| {
            bar.child(
                btn("tb_reply", IconName::Reply, false)
                    .tooltip(tip(tr!("返信", "Reply", "回复")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.app.editing.take().is_some() {
                            this.app.input.clear();
                        }
                        this.app.reply_to = Some(m1.clone());
                        this.app.focus_input = true;
                        cx.notify();
                    })),
            )
        });
        if !deleted {
            for (i, emoji) in reacts.into_iter().take(n_reacts).enumerate() {
                let (cid, mid) = (m.channel_id.clone(), m.id.clone());
                let e = crate::model::ReactionEmoji {
                    id: None,
                    name: Some(emoji.clone()),
                    animated: false,
                };
                bar = bar.child(
                    div()
                        .id(SharedString::from(format!("tb_qr{i}")))
                        .size(z(28.0))
                        .m(z(1.0))
                        .rounded(z(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(SELECTED()))
                        .text_size(z(16.0))
                        .child(emoji)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.toggle_reaction(&cid, &mid, &e);
                            cx.notify();
                        })),
                );
            }
            if self.app.plugins.on("quick-mention") {
                let uid = m.author.id.clone();
                bar = bar.child(
                    btn("tb_mention", IconName::AtSign, false)
                        .tooltip(tip(tr!("メンション", "Mention", "提及")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.app.input.is_empty() && !this.app.input.ends_with(' ') {
                                this.app.input.push(' ');
                            }
                            this.app.input.push_str(&format!("<@{uid}> "));
                            this.app.focus_input = true;
                            cx.notify();
                        })),
                );
            }
        }
        if mine {
            let m2 = m.clone();
            bar = bar.child(
                btn("tb_edit", IconName::Pencil, false)
                    .tooltip(tip(tr!("編集", "Edit", "编辑")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.input = crate::emoji::shorten(&m2.content).0;
                        this.app.editing = Some(m2.clone());
                        this.app.reply_to = None;
                        this.app.focus_input = true;
                        cx.notify();
                    })),
            );
        }
        if !m.content.is_empty() {
            let t = m.content.clone();
            bar = bar.child(
                btn("tb_copy", IconName::Copy, false)
                    .tooltip(tip(tr!("テキストをコピー", "Copy Text", "复制文字")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(t.clone()));
                        this.app.show_toast(tr!(
                            "テキストをコピーしました",
                            "Text copied",
                            "已复制文字"
                        ));
                        cx.notify();
                    })),
            );
        }
        if mine {
            let m3 = m.clone();
            bar = bar.child(
                btn("tb_delete", IconName::Trash, true)
                    .tooltip(tip(tr!(
                        "メッセージを削除（Shift+クリックで確認なし）",
                        "Delete message (Shift+click to skip confirmation)",
                        "删除消息（Shift+点击跳过确认）"
                    )))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if ev.modifiers().shift {
                            this.app.send(Cmd::Delete {
                                channel_id: m3.channel_id.clone(),
                                id: m3.id.clone(),
                            });
                        } else {
                            this.app.pending_delete = Some(m3.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        if self.app.plugins.translator_enabled() && !m.content.trim().is_empty() {
            let message = m.clone();
            let key = (m.channel_id.clone(), m.id.clone());
            let translated = self
                .app
                .translator
                .messages
                .get(&key)
                .is_some_and(|(original, _)| original == &m.content);
            bar = bar.child(
                btn("tb_translate", IconName::Languages, false)
                    .tooltip(tip(if translated {
                        tr!("翻訳を閉じる", "Close translation", "关闭翻译")
                    } else {
                        tr!("Google で翻訳", "Translate with Google", "用 Google 翻译")
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let key = (message.channel_id.clone(), message.id.clone());
                        if this
                            .app
                            .translator
                            .messages
                            .remove(&key)
                            .is_some_and(|(original, _)| original == message.content)
                        {
                            this.app.plugins.revision = this.app.plugins.revision.wrapping_add(1);
                        } else {
                            let language = this.app.plugins.translation_target();
                            let target = crate::translator::Target::Message {
                                channel: message.channel_id.clone(),
                                id: message.id.clone(),
                                original: message.content.clone(),
                                generation: this.app.plugins.generation,
                                manual: true,
                            };
                            if let Err(e) = this.app.translator.request(
                                target,
                                message.content.clone(),
                                language,
                                this.app.waker.clone(),
                            ) {
                                this.app.show_toast(&e);
                            } else {
                                this.app.show_toast(tr!(
                                    "Google で翻訳しています…",
                                    "Translating with Google…",
                                    "正在用 Google 翻译…"
                                ));
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        // 全部並べるときも、ツールバーが長くなりすぎないよう 6 個までにして残りは「…」へ
        const MAX_INLINE: usize = 6;
        let inline = if self.app.plugins.on("show-all-message-buttons") {
            if actions.len() <= MAX_INLINE + 1 {
                actions.len()
            } else {
                MAX_INLINE
            }
        } else if actions.len() <= 1 {
            actions.len()
        } else {
            0
        };
        // 幅に入りきらない分も「…」へ
        let inline = if inline < actions.len() || inline > max_inline {
            inline.min(max_inline.saturating_sub(1))
        } else {
            inline
        };
        let overflow = actions.len() > inline;
        if !actions.is_empty() {
            // 本体の操作とプラグインの操作の区切り
            bar = bar.child(div().w(z(1.0)).h(z(18.0)).mx(z(3.0)).bg(BORDER()));
        }
        for (id, index, label, icon_name) in actions.into_iter().take(inline) {
            let message = m.clone();
            bar = bar.child(
                btn(
                    &format!("plugin-{id}-{index}"),
                    plugin_icon(icon_name.as_deref()),
                    false,
                )
                .tooltip(tip(label))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.plugins.message_action(&id, index, &message);
                    this.app.waker.wake();
                    cx.notify();
                })),
            );
        }
        if overflow {
            // 残りのプラグインの操作は「…」から右クリックのメニューを開く
            let (cid, id) = (m.channel_id.clone(), m.id.clone());
            bar = bar.child(
                btn("tb_more", IconName::Ellipsis, false)
                    .tooltip(tip(tr!(
                        "その他（プラグイン）",
                        "More (plugins)",
                        "更多（插件）"
                    )))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        let Some(m) = this.app.find_message(&cid, &id) else {
                            return;
                        };
                        let p = ev.position();
                        let p = if p.x == px(0.) && p.y == px(0.) {
                            window.mouse_position()
                        } else {
                            p
                        };
                        this.menu = Some((PopMenu::Message, p));
                        this.menu_msg = Some(m);
                        cx.stop_propagation();
                        cx.notify();
                    })),
            );
        }
        bar
    }

    /// 返信の上に出る「╭ @相手 本文」の一行。avatar_x = 本文側アバターの中心
    fn reply_preview(
        &mut self,
        m: &Message,
        avatar_x: f32,
        text_x: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let roles = self.guild_roles();
        // 左の曲がった線（上と左の枠を角丸にした箱の一部）
        let curve = div()
            .absolute()
            .left(z(avatar_x))
            .top(z(10.0))
            .w(z(text_x - avatar_x - 6.0))
            .h(z(12.0))
            .border_l_2()
            .border_t_2()
            .border_color(REPLY_LINE())
            .rounded_tl(z(6.0));
        let row = div()
            .relative()
            .h(z(20.0))
            .w_full()
            .flex()
            .items_center()
            .pl(z(text_x))
            .pr(z(16.0))
            .gap(z(4.0))
            .child(curve);
        let Some(r) = m.referenced_message.as_deref() else {
            return row.child(text(
                tr!(
                    "元のメッセージは削除されました",
                    "The original message was deleted",
                    "原消息已被删除"
                ),
                13.0,
                FAINT(),
            ));
        };
        let nc = r
            .member
            .as_ref()
            .and_then(|member| member_role_color(member, roles))
            .unwrap_or(TEXT());
        let mut rich = Rich::default();
        let body = if !r.content.is_empty() {
            let content = resolve_mentions(&r.content, r, &self.app.channels, roles);
            rich = preview_rich(&content, self.revealed.contains(&r.id));
            String::new()
        } else if !r.attachments.is_empty() {
            tr!("添付ファイル", "Attachment", "附件").to_string()
        } else if !r.sticker_items.is_empty() {
            tr!("スタンプ", "Sticker", "贴纸").to_string()
        } else {
            String::new()
        };
        let author = r.author.clone();
        let reply_av = match self.app.sel_guild.as_deref() {
            Some(g) => member_avatar_url(g, &r.author, r.member.as_ref()),
            None => user_avatar_url(&r.author),
        };
        row.child(avatar(
            Some(&reply_av),
            16.0,
            r.author.display(),
            8.0,
            name_color(&r.author.id),
        ))
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
                    this.app.open_profile(
                        author.clone(),
                        Bounds::new(p, size(px(0.), px(0.))),
                        false,
                    );
                    cx.notify();
                }))
                .child(format!("@{}", author_name(r))),
        )
        .child(
            if rich.text.is_empty() {
                line(body, 13.0, MUTED())
            } else {
                div()
                    .text_size(z(13.0))
                    .text_color(MUTED())
                    .min_w_0()
                    .truncate()
                    .child(rich.element(SharedString::from(format!("rb{}", m.id))))
            }
            .flex_1()
            .ml(z(2.0)),
        )
    }

    /// 本文。記法ごとの塊（文・見出し・引用・コードブロック）を上から並べる
    /// 本文を描く前の形。元（本文・チャンネル・プラグインの状態）とメンションの名前が前と同じなら控えを返す
    fn body_blocks(&mut self, m: &Message) -> Rc<[md::Block]> {
        use std::hash::{Hash, Hasher};
        let hash = |f: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f(&mut h);
            h.finish()
        };
        let src = hash(&|h| (&m.content, &m.channel_id, self.app.plugins.revision).hash(h));
        if self.body_cache.get(&m.id).is_none_or(|c| c.src != src) {
            // 表示加工（Lua）は本文かプラグインの状態が変わったときだけ呼ぶ
            let displayed = self
                .app
                .plugins
                .transform("on_display", &m.content, &m.channel_id);
            if !self.body_cache.contains_key(&m.id) {
                if self.body_cache.len() >= super::BODY_CACHE_MAX {
                    if let Some(oldest) = self.body_cache_order.pop_front() {
                        self.body_cache.remove(&oldest);
                    }
                }
                self.body_cache_order.push_back(m.id.clone());
            }
            self.body_cache.insert(
                m.id.clone(),
                super::BodyCache {
                    src,
                    displayed,
                    resolved: 0,
                    blocks: Rc::from([]),
                },
            );
        }
        // メンションの名前（チャンネル名・ロール名）は後から変わるので毎回入れ直し、結果が変わったときだけ解析し直す
        let roles = self.guild_roles();
        let c = &self.body_cache[&m.id];
        let pre = self.app.bd_pre_mentions(&c.displayed, m, roles);
        let content = resolve_mentions(&pre, m, &self.app.channels, roles);
        let resolved = hash(&|h| content.hash(h)).max(1);
        if c.resolved == resolved {
            return c.blocks.clone();
        }
        let blocks: Rc<[md::Block]> = md::blocks(&content).into();
        drop(content);
        drop(pre);
        let c = self.body_cache.get_mut(&m.id).unwrap();
        c.resolved = resolved;
        c.blocks = blocks.clone();
        blocks
    }

    fn inline_content(&mut self, m: &Message, cx: &mut Context<Self>) -> AnyElement {
        let fs = self.app.settings.font_size;
        let blocks = self.body_blocks(m);
        let edited = m.edited_timestamp.is_some();
        let revealed = self.revealed.contains(&m.id);
        let on_spoiler: Rc<dyn Fn(&mut Window, &mut App)> = {
            let (root, id) = (cx.entity().downgrade(), m.id.clone());
            Rc::new(move |_, cx| {
                let _ = root.update(cx, |this, cx| {
                    this.revealed.insert(id.clone());
                    cx.notify();
                });
            })
        };
        let last = blocks.len().saturating_sub(1);
        let mut items = Vec::with_capacity(blocks.len());
        let enhance = self.app.plugins.on("enhancecodeblocks");
        let langs = if enhance {
            crate::app::bd_display::code_langs(&m.content)
        } else {
            Vec::new()
        };
        let mut code_ix = 0;
        for (i, block) in blocks.iter().enumerate() {
            let item = match block {
                md::Block::Code(code) if enhance => {
                    let lang = langs.get(code_ix).cloned().unwrap_or_default();
                    code_ix += 1;
                    let copy = code.clone();
                    let lines = code.lines().count();
                    div()
                        .min_w_0()
                        .my(z(2.0))
                        .rounded(z(4.0))
                        .border_1()
                        .border_color(BORDER())
                        .bg(EMBED_BG())
                        .overflow_hidden()
                        .child(
                            div()
                                .px(z(8.0))
                                .py(z(3.0))
                                .flex()
                                .items_center()
                                .gap(z(8.0))
                                .bg(WIDGET())
                                .child(text(
                                    if lang.is_empty() {
                                        "text".to_string()
                                    } else {
                                        lang
                                    },
                                    11.0,
                                    MUTED(),
                                ))
                                .child(text(
                                    tr!("{lines} 行", "{lines} lines", "{lines} 行",),
                                    11.0,
                                    FAINT(),
                                ))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id(SharedString::from(format!("cb{}_{i}", m.id)))
                                        .cursor_pointer()
                                        .text_size(z(11.0))
                                        .text_color(LINK())
                                        .child(tr!("コピー", "Copy", "复制"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.app.clipboard_out = Some(copy.clone());
                                            this.app.show_toast(tr!(
                                                "コピーしました",
                                                "Copied",
                                                "已复制"
                                            ));
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .px(z(8.0))
                                .py(z(6.0))
                                .font_family(mono_family())
                                .text_size(z((fs * 0.875).round()))
                                .part(Part::MessageCode)
                                .child(StyledText::new(code.clone())),
                        )
                }
                md::Block::Code(code) => div()
                    .min_w_0()
                    .my(z(2.0))
                    .px(z(8.0))
                    .py(z(6.0))
                    .rounded(z(4.0))
                    .border_1()
                    .border_color(BORDER())
                    .bg(EMBED_BG())
                    .font_family(mono_family())
                    .text_size(z((fs * 0.875).round()))
                    .part(Part::MessageCode)
                    .child(StyledText::new(code.clone())),
                md::Block::Text { kind, spans } => {
                    let body = self.text_block(
                        &format!("t{}_{i}", m.id),
                        spans,
                        &m.mentions
                            .iter()
                            .filter(|u| {
                                m.content.contains(&format!("<@{}>", u.id))
                                    || m.content.contains(&format!("<@!{}>", u.id))
                            })
                            .map(|u| (format!("@{}", u.display()), u.id.clone()))
                            .collect::<Vec<_>>(),
                        fs,
                        edited && i == last,
                        revealed,
                        on_spoiler.clone(),
                    );
                    match kind {
                        md::Kind::Plain => body,
                        md::Kind::Heading(level) => {
                            let scale = [1.5, 1.25, 1.1][(*level as usize - 1).min(2)];
                            body.text_size(z((fs * scale).round()))
                                .text_color(WHITE())
                                .font_weight(FontWeight::BOLD)
                                .when(i > 0, |d| d.mt(z(6.0)))
                        }
                        md::Kind::Subtext => {
                            body.text_size(z((fs * 0.8).round())).text_color(MUTED())
                        }
                        md::Kind::Quote => div()
                            .min_w_0()
                            .flex()
                            .gap(z(10.0))
                            .child(div().flex_none().w(z(4.0)).rounded(z(2.0)).bg(REPLY_LINE()))
                            .child(body.flex_1()),
                    }
                }
            };
            items.push(item);
        }
        if edited && matches!(blocks.last(), Some(md::Block::Code(_))) {
            items.push(text(tr!("(編集済)", "(edited)", "(已编辑)"), 11.0, MUTED()));
        }
        let style = |d: Div| {
            d.min_w_0()
                .text_size(z(fs))
                .text_color(TEXT())
                .part(Part::MessageText)
        };
        if items.len() == 1
            && matches!(
                blocks[0],
                md::Block::Text {
                    kind: md::Kind::Plain,
                    ..
                }
            )
        {
            return style(items.pop().unwrap()).into_any_element();
        }
        style(div().flex().flex_col().gap(z(2.0)))
            .children(items)
            .into_any_element()
    }

    /// 文の塊1つ。カスタム絵文字があれば画像を文中に並べる
    fn text_block(
        &mut self,
        key: &str,
        spans: &[md::Span],
        mentions: &[(String, String)],
        fs: f32,
        edited: bool,
        revealed: bool,
        on_spoiler: Rc<dyn Fn(&mut Window, &mut App)>,
    ) -> Div {
        enum Piece<'a> {
            Text(Rich),
            Emoji {
                name: &'a str,
                id: &'a str,
                animated: bool,
            },
        }
        let clickable_mentions =
            self.app.plugins.on("clickable-mentions") || self.app.plugins.on("better-mentions");
        let color_codes = self.app.plugins.on("color-indicator");
        let emoji_preview = self.app.plugins.on("sticker-emoji-preview");
        // 飾りは絵文字をまたいで続くので、記法を解いてから絵文字で切る
        let mut pieces = Vec::new();
        let mut cur = Rich::default();
        for span in spans {
            if span.style.code {
                cur.span(span, revealed);
                continue;
            }
            for seg in split_emoji(&span.text) {
                match seg {
                    Seg::Text(t) => {
                        let mut rest = t;
                        while !rest.is_empty() && !(span.style.spoiler && !revealed) {
                            let found = mentions
                                .iter()
                                .filter_map(|(name, uid)| {
                                    rest.find(name.as_str()).map(|pos| (pos, name, uid))
                                })
                                .min_by_key(|(pos, name, _)| (*pos, std::cmp::Reverse(name.len())));
                            let Some((pos, name, uid)) = found else { break };
                            cur.span(
                                &md::Span {
                                    text: rest[..pos].to_string(),
                                    style: span.style,
                                },
                                revealed,
                            );
                            let start = cur.text.len();
                            cur.push(
                                name,
                                Some(HighlightStyle {
                                    color: Some(rgb(0x9baafa).into()),
                                    background_color: Some(rgb(0x30375b).into()),
                                    ..highlight(span.style).unwrap_or_default()
                                }),
                            );
                            if clickable_mentions {
                                cur.links
                                    .push((start..cur.text.len(), format!("{MENTION_LINK}{uid}")));
                            }
                            rest = &rest[pos + name.len()..];
                        }
                        cur.span(
                            &md::Span {
                                text: rest.to_string(),
                                style: span.style,
                            },
                            revealed,
                        );
                    }
                    Seg::Emoji { name, id, animated } => {
                        if color_codes {
                            cur.color_codes();
                        }
                        pieces.push(Piece::Text(std::mem::take(&mut cur)));
                        pieces.push(Piece::Emoji { name, id, animated });
                    }
                }
            }
        }
        if color_codes {
            cur.color_codes();
        }
        if pieces.is_empty() {
            if edited {
                cur.push(
                    tr!(" (編集済)", " (edited)", " (已编辑)"),
                    Some(HighlightStyle {
                        color: Some(MUTED().into()),
                        ..Default::default()
                    }),
                );
            }
            return div()
                .min_w_0()
                .child(cur.element_with(SharedString::from(key.to_string()), Some(on_spoiler)));
        }
        pieces.push(Piece::Text(cur));
        // 絵文字だけのメッセージは大きく表示
        let only_emoji = pieces.iter().all(|p| match p {
            Piece::Text(r) => r.text.trim().is_empty(),
            Piece::Emoji { .. } => true,
        });
        let size = if only_emoji { 48.0 } else { (fs * 1.4).round() };
        let mut row = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_y(z(2.0));
        let mut urls = Vec::new();
        for (i, piece) in pieces.into_iter().enumerate() {
            match piece {
                Piece::Text(r) => {
                    if !(r.text.is_empty() || only_emoji) {
                        // 長い文は行いっぱいで折り返す（flex_wrap の中では幅を絞らないとはみ出す）
                        row = row.child(div().max_w_full().child(r.element_with(
                            SharedString::from(format!("{key}_{i}")),
                            Some(on_spoiler.clone()),
                        )));
                    }
                }
                Piece::Emoji { name, id, animated } => {
                    let url = emoji_url(id, animated);
                    let face = match imgs::get(&url) {
                        Img::Ready(img_) => div().size(z(size)).child(
                            img(img_)
                                .id("img")
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        ),
                        _ => div().size(z(size)),
                    };
                    let big = format!(
                        "https://cdn.discordapp.com/emojis/{id}.{}?size=512",
                        if animated { "gif" } else { "png" }
                    );
                    row = row.child(
                        face.id(SharedString::from(format!("{key}_e{i}")))
                            .tooltip(tip(format!(":{name}:")))
                            .when(only_emoji, |d| d.mr(z(4.0)))
                            .when(emoji_preview, |d| {
                                d.cursor_pointer().on_click(move |_, window, _| {
                                    *PENDING_PREVIEW.lock().unwrap() = Some(big.clone());
                                    window.refresh();
                                })
                            }),
                    );
                    urls.push(url);
                }
            }
        }
        for u in urls {
            self.note_image(&u);
        }
        if edited {
            row = row.child(text(
                tr!(" (編集済)", " (edited)", " (已编辑)"),
                11.0,
                MUTED(),
            ));
        }
        row
    }

    /// メッセージの下に並ぶリアクション。押すと自分の分を付け外しする
    fn reactions_row(&mut self, m: &Message, cx: &mut Context<Self>) -> Option<Div> {
        if m.reactions.is_empty() {
            return None;
        }
        let mut row = div().flex().flex_wrap().gap(z(4.0)).pt(z(4.0));
        for (i, r) in m.reactions.iter().enumerate() {
            let name = r.emoji.name.clone().unwrap_or_default();
            let face: AnyElement = match &r.emoji.id {
                Some(id) => {
                    let url = emoji_url(id, r.emoji.animated);
                    self.note_image(&url);
                    let d = div().size(z(18.0));
                    match imgs::get(&url) {
                        Img::Ready(i) => {
                            d.child(img(i).id("img").size_full().object_fit(ObjectFit::Contain))
                        }
                        _ => d,
                    }
                    .into_any_element()
                }
                None => text(name.clone(), 15.0, WHITE()).into_any_element(),
            };
            let (bg, border, count_color) = if r.me {
                (BLURPLE().gamma_multiply(0.25), BLURPLE(), WHITE())
            } else {
                (WIDGET(), Color32::TRANSPARENT, TEXT())
            };
            let tip_text = if r.emoji.id.is_some() {
                format!(":{name}:")
            } else {
                name
            };
            let (cid, mid, emoji) = (m.channel_id.clone(), m.id.clone(), r.emoji.clone());
            row = row.child(
                div()
                    .id(SharedString::from(format!("re{}_{i}", m.id)))
                    .h(z(26.0))
                    .px(z(6.0))
                    .rounded(z(8.0))
                    .border_1()
                    .border_color(border)
                    .bg(bg)
                    .hover(move |s| s.border_color(if r.me { BLURPLE() } else { BORDER() }))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(z(6.0))
                    .tooltip(tip(tip_text))
                    .child(face)
                    .child(text(r.count.to_string(), 14.0, count_color))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.toggle_reaction(&cid, &mid, &emoji);
                        cx.notify();
                    })),
            );
        }
        Some(row)
    }

    /// 添付画像・スタンプ・埋め込み画像
    /// PeekMessageLinks: 本文のメッセージリンクの先を、引用のように下に出す
    fn peek_cards(&mut self, m: &Message, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let peeks = self.app.peek_links(m);
        peeks
            .into_iter()
            .enumerate()
            .map(|(i, (cid, mid, found))| {
                let body: AnyElement = match &found {
                    Some(p) => div()
                        .flex()
                        .flex_col()
                        .gap(z(2.0))
                        .child(
                            text(
                                self.app.shown_name(&p.author.id, author_name(p)),
                                13.0,
                                WHITE(),
                            )
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(text(crate::notify::excerpt(&p.content, 300), 13.0, TEXT()))
                        .into_any_element(),
                    None => text(
                        tr!(
                            "リンク先のメッセージを読み込み中か、見られません",
                            "Loading the linked message, or it isn't available",
                            "正在加载链接的消息，或无法查看"
                        ),
                        12.0,
                        MUTED(),
                    )
                    .into_any_element(),
                };
                let guild = self.app.guild_of_channel(&cid);
                div()
                    .id(SharedString::from(format!("peek{}_{i}", m.id)))
                    .max_w(z(520.0))
                    .mt(z(4.0))
                    .p(z(8.0))
                    .rounded(z(6.0))
                    .border_l_4()
                    .border_color(REPLY_LINE())
                    .bg(EMBED_BG())
                    .cursor_pointer()
                    .child(body)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app
                            .jump_to_message(guild.clone(), cid.clone(), mid.clone());
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// CollapseEmbeds: 埋め込み・添付を畳めるようにする
    fn media_or_collapsed(&mut self, m: &Message, cx: &mut Context<Self>) -> AnyElement {
        let count = m.attachments.len() + m.embeds.iter().filter(|e| e.visible()).count();
        if !self.app.plugins.on("collapse-embeds") || count == 0 {
            return self.message_media(m, cx).into_any_element();
        }
        let default_closed = self.app.plugins.opt_bool("collapse-embeds", "collapsed");
        let flipped = self.collapsed_media.contains(&m.id);
        let closed = default_closed != flipped;
        let id = m.id.clone();
        let toggle = div()
            .id(SharedString::from(format!("collapse{}", m.id)))
            .cursor_pointer()
            .text_size(z(12.0))
            .text_color(MUTED())
            .hover(|s| s.text_color(TEXT()))
            .child(if closed {
                tr!(
                    "▶ 埋め込み・添付を表示（{count}）",
                    "▶ Show embeds and attachments ({count})",
                    "▶ 显示嵌入和附件（{count}）",
                )
            } else {
                tr!("▼ 畳む", "▼ Collapse", "▼ 折叠").to_string()
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed_media.remove(&id) {
                    this.collapsed_media.insert(id.clone());
                }
                cx.notify();
            }));
        let mut col = div().flex().flex_col().gap(z(2.0)).child(toggle);
        if !closed {
            col = col.child(self.message_media(m, cx));
        }
        col.into_any_element()
    }

    fn message_media(&mut self, m: &Message, cx: &mut Context<Self>) -> Div {
        let max = if self.app.opt_large_images() {
            (640.0, 480.0)
        } else {
            (420.0, 320.0)
        };
        let mut col = div().flex().flex_col().gap(z(4.0)).items_start();
        if let Some(poll) = &m.poll {
            let mut card = div()
                .max_w(z(420.0))
                .p(z(12.0))
                .rounded(z(8.0))
                .bg(WIDGET())
                .flex()
                .flex_col()
                .gap(z(8.0))
                .child(text(
                    poll.question.text.clone().unwrap_or_default(),
                    16.0,
                    WHITE(),
                ));
            for answer in &poll.answers {
                let result = poll
                    .results
                    .as_ref()
                    .and_then(|r| r.answer_counts.iter().find(|c| c.id == answer.answer_id));
                let emoji = answer
                    .poll_media
                    .emoji
                    .as_ref()
                    .and_then(|e| e.name.as_deref())
                    .unwrap_or("");
                let label = format!(
                    "{} {}",
                    emoji,
                    answer.poll_media.text.as_deref().unwrap_or("")
                );
                let count = result
                    .map(|r| {
                        tr!(
                            "{} 票{}",
                            "{} votes{}",
                            "{} 票{}",
                            r.count,
                            if r.me_voted {
                                tr!(" · 回答済み", " · Voted", " · 已投票")
                            } else {
                                ""
                            }
                        )
                    })
                    .unwrap_or_default();
                let cid = m.channel_id.clone();
                let mid = m.id.clone();
                let answer_id = answer.answer_id;
                let pending = self
                    .app
                    .pending_poll_votes
                    .contains(&(cid.clone(), mid.clone()));
                let ended = poll
                    .expiry
                    .as_deref()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .is_some_and(|end| end <= chrono::Utc::now());
                let enabled =
                    poll.results.as_ref().is_some_and(|r| !r.is_finalized) && !pending && !ended;
                card = card.child(
                    div()
                        .id(SharedString::from(format!("poll-{}-{answer_id}", m.id)))
                        .flex()
                        .gap(z(12.0))
                        .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                        .child(text(label, 14.0, TEXT()).flex_1())
                        .child(text(count, 12.0, MUTED()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.app.toggle_poll_vote(&cid, &mid, answer_id);
                                cx.notify();
                            }
                        })),
                );
            }
            if poll.results.is_none() {
                card = card.child(text(
                    tr!(
                        "投票結果を取得できませんでした",
                        "Couldn't load poll results",
                        "无法获取投票结果"
                    ),
                    12.0,
                    MUTED(),
                ));
            }
            col = col.child(card);
        }
        for (i, a) in m.attachments.iter().enumerate() {
            if a.is_image() {
                let size = fit_size(a.width, a.height, max);
                let src = if self.app.plugins.on("uncompressed-images") {
                    a.url.clone()
                } else {
                    resized(a.proxy_url.as_deref(), &a.url, size.unwrap_or(max))
                };
                self.note_image(&src);
                self.note_image(&a.url);
                col = col.child(image_box(
                    SharedString::from(format!("at{}_{i}", m.id)),
                    &src,
                    &a.url,
                    size.unwrap_or(max),
                    false,
                    Some(a.filename.clone()),
                    cx,
                ));
            } else if a.is_video() && cfg!(target_os = "linux") {
                col = col.child(self.video_attachment(
                    a,
                    &m.channel_id,
                    &format!("{}_{i}", m.id),
                    cx,
                ));
            } else {
                col = col.child(file_card(
                    SharedString::from(format!("af{}_{i}", m.id)),
                    a,
                    cx,
                ));
            }
        }
        for st in &m.sticker_items {
            col = col.child(self.sticker(st));
        }
        for (i, e) in m.embeds.iter().enumerate() {
            if let Some(md) = e.media() {
                let size = fit_size(md.width, md.height, max).unwrap_or(max);
                // GIF リンクは .gif 版を取って動かす。取れなかったら静止画に戻す
                let gif = e
                    .animated_url()
                    .filter(|g| !matches!(imgs::get(g), Img::Failed));
                let src = gif.unwrap_or_else(|| resized(md.proxy_url.as_deref(), &md.url, size));
                let open = e.animated_url().unwrap_or_else(|| md.url.clone());
                self.note_image(&src);
                self.note_image(&open);
                let name = crate::download::name_from_url(&open);
                col = col.child(image_box(
                    SharedString::from(format!("em{}_{i}", m.id)),
                    &src,
                    &open,
                    size,
                    false,
                    Some(name),
                    cx,
                ));
            } else if e.is_rich() {
                col = col.child(self.embed_card(e, &format!("{}_{i}", m.id), cx));
            }
        }
        col
    }

    fn video_attachment(
        &mut self,
        attachment: &model::Attachment,
        channel: &str,
        key: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let size =
            fit_size(attachment.width, attachment.height, (420.0, 320.0)).unwrap_or((420.0, 236.0));
        let source = attachment.url.clone();
        let target_channel = channel.to_owned();
        let width = attachment.width.unwrap_or(640);
        let height = attachment.height.unwrap_or(360);
        let active = self
            .video
            .as_ref()
            .filter(|v| v.url == source && v.channel == channel);
        let frame = active.and_then(|v| v.image.clone());
        let show_play = active.is_none_or(|v| v.paused || v.ended);
        let preview = if frame.is_none() {
            crate::video::thumbnail(&source, width, height, &self.app.waker)
        } else {
            None
        };
        let has_preview = preview.is_some();
        let frame = frame.or(preview);
        let label = match active {
            Some(v) if v.error.is_some() => tr!("再生できません", "Can't play", "无法播放"),
            Some(v) if v.ended => tr!("もう一度再生", "Play again", "再次播放"),
            Some(v) if v.paused => tr!("再生", "Play", "播放"),
            Some(v) if v.image.is_some() => tr!("一時停止", "Pause", "暂停"),
            Some(_) => tr!("読み込み中…", "Loading…", "正在加载…"),
            None => tr!("動画を再生", "Play video", "播放视频"),
        };
        let error = active.and_then(|v| v.error.clone());
        let mut screen = div()
            .id(SharedString::from(format!("video-screen-{key}")))
            .w(z(size.0))
            .h(z(size.1))
            .relative()
            .rounded(z(8.0))
            .overflow_hidden()
            .bg(Color32::BLACK)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center();
        if let Some(frame) = frame {
            screen = screen.child(
                img(frame)
                    .id("video-frame")
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
        } else {
            screen = screen.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(z(8.0))
                    .child(icon(IconName::Play, 32.0, WHITE()))
                    .child(text(label, 14.0, WHITE())),
            );
        }
        if has_preview && show_play {
            screen = screen.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(z(52.0))
                            .rounded_full()
                            .bg(Color32::from_rgba_unmultiplied(0, 0, 0, 170))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(IconName::Play, 28.0, WHITE())),
                    ),
            );
        }
        let download_url = attachment.url.clone();
        let download_name = attachment.filename.clone();
        screen = screen.child(
            download_button(
                SharedString::from(format!("video-download-{key}")),
                tr!("動画をダウンロード", "Download video", "下载视频"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.start_download(
                    download_url.clone(),
                    download_name.clone(),
                    crate::download::Kind::Video,
                );
                cx.notify();
            })),
        );
        let screen = screen
            .tooltip(tip(label))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(player) = this
                    .video
                    .as_mut()
                    .filter(|v| v.url == source && v.channel == target_channel)
                {
                    player.toggle();
                } else {
                    this.video = None;
                    match crate::video::Player::new(
                        target_channel.clone(),
                        source.clone(),
                        width,
                        height,
                        this.app.waker.clone(),
                    ) {
                        Ok(player) => this.video = Some(player),
                        Err(error) => this.app.show_toast(&error),
                    }
                }
                cx.notify();
            }));
        let mut content = div().flex().flex_col().gap(z(4.0)).child(screen);
        if let Some(error) = error {
            content = content.child(text(error, 13.0, RED()));
        }
        let link = attachment.url.clone();
        let mut controls = div()
            .w(z(size.0))
            .flex()
            .items_center()
            .gap(z(10.0))
            .child(line(attachment.filename.clone(), 12.0, MUTED()).flex_1());
        if active.is_some() {
            controls = controls.child(
                div()
                    .id(SharedString::from(format!("video-close-{key}")))
                    .cursor_pointer()
                    .child(text(tr!("停止", "Stop", "停止"), 12.0, LINK()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.video = None;
                        cx.notify();
                    })),
            );
        }
        controls = controls.child(
            div()
                .id(SharedString::from(format!("video-open-{key}")))
                .cursor_pointer()
                .child(text(tr!("開く", "Open", "打开"), 12.0, LINK()))
                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&link))),
        );
        content.child(controls)
    }

    fn sticker(&mut self, st: &model::StickerItem) -> AnyElement {
        const BOX: f32 = 160.0;
        match self.app.stickers.get(&st.id) {
            Some(StickerState::Ready(i)) => div()
                .id(SharedString::from(format!("st{}", st.id)))
                .size(z(BOX))
                .tooltip(tip(st.name.clone()))
                .when(
                    self.app.plugins.on("sticker-emoji-preview") && st.format_type != 3,
                    |d| {
                        let big = format!(
                            "https://media.discordapp.net/stickers/{}.{}?size=512",
                            st.id,
                            if st.format_type == 4 { "gif" } else { "png" }
                        );
                        d.cursor_pointer().on_click(move |_, window, _| {
                            *PENDING_PREVIEW.lock().unwrap() = Some(big.clone());
                            window.refresh();
                        })
                    },
                )
                .child(
                    img(i.clone())
                        .id("img")
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Some(StickerState::Failed) => text(
                tr!("[スタンプ: {}]", "[Sticker: {}]", "[贴纸：{}]", st.name),
                16.0,
                MUTED(),
            )
            .into_any_element(),
            state => {
                if state.is_none() {
                    self.app
                        .stickers
                        .insert(st.id.clone(), StickerState::Loading);
                    self.app.send(Cmd::LoadSticker {
                        id: st.id.clone(),
                        format: st.format_type,
                    });
                }
                div()
                    .size(z(BOX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(spinner(24.0))
                    .into_any_element()
            }
        }
    }

    fn embed_icon(
        &mut self,
        icon_url: Option<&str>,
        proxy: Option<&str>,
        size: f32,
    ) -> Option<Div> {
        let u = proxy.or(icon_url)?.to_string();
        self.note_image(&u);
        Some(avatar(Some(&u), size, "", size / 2.0, Color32::TRANSPARENT))
    }

    /// タイトル・説明・フィールドなどを持つ埋め込み（リンクのプレビューや bot の埋め込み）
    fn embed_card(&mut self, e: &Embed, key: &str, cx: &mut Context<Self>) -> Div {
        let bar = e.color.filter(|&c| c != 0).map(rgb).unwrap_or(EMBED_BAR());
        let big = embed_big_media(e);
        let small_thumb = if big.is_none() {
            e.thumbnail.as_ref()
        } else {
            None
        };
        let inner = EMBED_MAX_W - 32.0;
        let mut body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(z(6.0))
            .pt(z(2.0));
        if let Some(name) = e.provider.as_ref().and_then(|p| p.name.clone()) {
            body = body.child(embed_line(
                SharedString::from(format!("ep{key}")),
                name,
                12.0,
                MUTED(),
                e.provider.as_ref().and_then(|p| p.url.as_deref()),
            ));
        }
        if let Some(a) = &e.author {
            if let Some(name) = a.name.clone() {
                let mut r = div().flex().items_center().gap(z(8.0));
                if let Some(i) =
                    self.embed_icon(a.icon_url.as_deref(), a.proxy_icon_url.as_deref(), 24.0)
                {
                    r = r.child(i);
                }
                r = r.child(embed_line(
                    SharedString::from(format!("ea{key}")),
                    name,
                    14.0,
                    WHITE(),
                    a.url.as_deref(),
                ));
                body = body.child(r);
            }
        }
        if let Some(t) = e.title.clone() {
            let color = if e.url.is_some() { LINK() } else { WHITE() };
            body = body.child(div().font_weight(FontWeight::MEDIUM).child(embed_line(
                SharedString::from(format!("et{key}")),
                t,
                16.0,
                color,
                e.url.as_deref(),
            )));
        }
        if let Some(d) = e.description.as_deref().filter(|d| !d.is_empty()) {
            body = body.child(embed_text(
                SharedString::from(format!("ed{key}")),
                d,
                14.0,
                TEXT(),
            ));
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
                        .child(
                            embed_text(
                                SharedString::from(format!("efn{key}_{}", i + j)),
                                &f.name,
                                14.0,
                                WHITE(),
                            )
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(embed_text(
                            SharedString::from(format!("efv{key}_{}", i + j)),
                            &f.value,
                            14.0,
                            TEXT(),
                        )),
                );
            }
            body = body.child(r);
            i += n;
        }
        let mut top = div().flex().items_start().gap(z(16.0)).child(body);
        if let Some(t) = small_thumb {
            let size = fit_size(t.width, t.height, (EMBED_THUMB, EMBED_THUMB))
                .unwrap_or((EMBED_THUMB, EMBED_THUMB));
            let src = resized(t.proxy_url.as_deref(), &t.url, size);
            self.note_image(&src);
            self.note_image(&t.url);
            top = top.child(div().mt(z(4.0)).child(image_box(
                SharedString::from(format!("eth{key}")),
                &src,
                &t.url,
                size,
                false,
                None,
                cx,
            )));
        }
        let mut col = div().flex().flex_col().child(top);
        if let Some(md) = big {
            let size = fit_size(md.width, md.height, (inner, 300.0)).unwrap_or((inner, 300.0));
            let src = resized(md.proxy_url.as_deref(), &md.url, size);
            self.note_image(&src);
            let video = e.video.is_some() || e.kind.as_deref() == Some("video");
            let open = if video {
                e.url.clone().unwrap_or_else(|| md.url.clone())
            } else {
                md.url.clone()
            };
            if !video {
                self.note_image(&open);
            }
            // 動画の埋め込みはサムネイルなので、ダウンロードは画像のときだけ
            let name = (!video).then(|| crate::download::name_from_url(&open));
            let mut b = div().relative().mt(z(8.0)).child(image_box(
                SharedString::from(format!("eb{key}")),
                &src,
                &open,
                size,
                video,
                name,
                cx,
            ));
            if video {
                // 動画はアプリ内で再生できないので、再生マークを重ねてブラウザで開く
                b = b.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(z(48.0))
                                .rounded_full()
                                .bg(Color32::from_black_alpha(170))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(IconName::Play, 22.0, WHITE())),
                        ),
                );
            }
            col = col.child(b);
        }
        // フッター
        let time = e
            .timestamp
            .as_deref()
            .and_then(local_dt)
            .map(|t| t.format("%Y/%m/%d %H:%M").to_string());
        let ft = e
            .footer
            .as_ref()
            .map(|f| f.text.as_str())
            .filter(|t| !t.is_empty());
        let foot = match (ft, &time) {
            (Some(t), Some(d)) => Some(format!("{t} • {d}")),
            (Some(t), None) => Some(t.to_string()),
            (None, Some(d)) => Some(d.clone()),
            (None, None) => None,
        };
        if let Some(foot) = foot {
            let mut r = div().mt(z(8.0)).flex().items_center().gap(z(8.0));
            if let Some(f) = &e.footer {
                if let Some(i) =
                    self.embed_icon(f.icon_url.as_deref(), f.proxy_icon_url.as_deref(), 20.0)
                {
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
            .map(|d| {
                // 左の縦線は埋め込みごとの色なので、テーマで変えるのは地と角の丸みだけ
                let st = part_style(Part::MessageEmbed);
                d.when_some(st.bg, |d, c| d.bg(c))
                    .when_some(st.radius, |d, r| d.rounded(z(r)))
                    .when_some(st.opacity, |d, o| d.opacity(o))
            })
            .pl(z(12.0))
            .pr(z(16.0))
            .pt(z(8.0))
            .pb(z(12.0))
            .child(col)
    }
}

#[cfg(test)]
mod tests {
    /// メッセージのツールバーは 1 行に収め、入りきらないプラグインの操作は「…」へ回す
    #[test]
    fn toolbar_fits_one_row() {
        use super::toolbar_fit;
        // 広い画面: クイックリアクション 10 個と操作 6 個がそのまま並ぶ
        assert_eq!(toolbar_fit(50, 6, 10, 6), (10, 34));
        // 320px 相当（slots = 9）: 本体 6 個のほかは「…」の 1 つ分だけ
        assert_eq!(toolbar_fit(9, 6, 10, 6), (2, 1));
        // 操作がなければ、残りはすべてリアクションに使える
        assert_eq!(toolbar_fit(9, 6, 10, 0), (3, 0));
        // 本体のボタンだけで埋まっていても負にならない
        assert_eq!(toolbar_fit(4, 6, 10, 6), (0, 0));
    }
    use super::{call_length, preview_rich, Message};

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
        assert!(
            super::row_layout(&text, Some(&text), false).1,
            "ふつうの連投はまとめる"
        );
        assert!(
            !super::row_layout(&call, Some(&text), false).1,
            "通話の記録はまとめない"
        );
        assert!(
            !super::row_layout(&text, Some(&call), false).1,
            "通話の記録の次の行は名前を出す"
        );
    }

    #[test]
    fn reply_preview_is_one_line_without_markers() {
        let r = preview_rich("# 見出し\n**太字** と `code`\n```rust\nlet x = 1;\n```\n<:blob:123> https://example.com", true);
        assert_eq!(
            r.text,
            "見出し 太字 と code let x = 1; :blob: https://example.com"
        );
        assert!(r.links.is_empty(), "プレビューのリンクは押せない");
        assert_eq!(r.mono.len(), 2);
        let hidden = preview_rich("||ひみつ||", false);
        assert_eq!(hidden.spoilers.len(), 1);
    }
}
