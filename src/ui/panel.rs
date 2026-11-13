//! チャット右側の枠: ピン一覧（検索結果もここに出す）
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{local_dt, SidePanel, SEARCH_MAX_OFFSET, SEARCH_PAGE};
use gpui::component::input::Input;

/// 枠の上の帯（題・閉じる）
pub(super) fn panel_head(title: &str, ic: IconName, cx: &mut Context<Root>) -> Div {
    div()
        .h(z(48.0))
        .flex_none()
        .px(z(12.0))
        .flex()
        .items_center()
        .gap(z(8.0))
        .border_b_1()
        .border_color(DIVIDER())
        .child(icon(ic, 18.0, MUTED()))
        .child(
            line(title.to_string(), 15.0, WHITE())
                .font_weight(FontWeight::MEDIUM)
                .flex_1()
                .min_w_0(),
        )
        .child(
            icon_button("side_close", IconName::X, 28.0)
                .tooltip(tip(tr!("閉じる", "Close", "关闭")))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.close_side();
                    cx.notify();
                })),
        )
}

/// 枠の中央に出す一言（空・失敗・読み込み中）
fn panel_note(s: impl Into<SharedString>) -> Div {
    div()
        .p(z(16.0))
        .flex()
        .justify_center()
        .child(text(s, 14.0, MUTED()))
}

impl Root {
    /// メッセージへ飛ぶ（場所を移すなら戻る・進むの履歴にも残す）
    pub fn jump_to(&mut self, guild_id: Option<String>, channel_id: String, message_id: String) {
        let before = (self.app.sel_guild.clone(), self.app.sel_channel.clone());
        self.app.jump_to_message(guild_id, channel_id, message_id);
        self.after_move(before);
    }

    pub fn side_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let body = match self.app.side {
            SidePanel::Pins => self.pins_panel(window, cx),
            SidePanel::Search => self.search_panel(window, cx),
            SidePanel::Threads => self.threads_panel(cx),
            SidePanel::None => div(),
        };
        div()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_t_1()
            .border_color(DIVIDER())
            .bg(BG())
            .part(Part::MembersPanel)
            .child(body)
    }

    /// 一覧の行数が変わったら作り直す（同じ種類・同じ場所なら見ている位置を保つ）
    pub(super) fn sync_side_list(&mut self, key: (u8, String, u64), n: usize) {
        if self.side_key == key {
            return;
        }
        let same_place = self.side_key.0 == key.0 && self.side_key.1 == key.1;
        let top = self.side_list.logical_scroll_top();
        self.side_list.reset(n);
        if same_place {
            self.side_list.scroll_to(ListOffset {
                item_ix: top.item_ix.min(n.saturating_sub(1)),
                offset_in_item: top.offset_in_item,
            });
        }
        self.side_key = key;
    }

    fn pins_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(p) = self.app.pins.as_ref() else {
            return div();
        };
        let (n, key) = (p.items.len() + 1, (1, p.channel_id.clone(), p.rev));
        self.sync_side_list(key, n);
        let this = self.this.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(panel_head(
                tr!("ピン留めされたメッセージ", "Pinned messages", "置顶消息"),
                IconName::Pin,
                cx,
            ))
            .child(
                list(self.side_list.clone(), move |ix, window, cx| {
                    let Some(e) = this.upgrade() else {
                        return div().into_any_element();
                    };
                    e.update(cx, |r, cx| r.pin_item(ix, window, cx))
                })
                .flex_1(),
            )
    }

    fn pin_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 行を描く間だけ一覧を借り出す（message_row はピン一覧を見ない）
        let Some(p) = self.app.pins.take() else {
            return div().into_any_element();
        };
        let el = match p.items.get(ix) {
            Some(item) => {
                let m = &item.message;
                let at = local_dt(&item.pinned_at)
                    .map(|t| {
                        t.format(tr!(
                            "%Y/%m/%d %H:%M にピン留め",
                            "Pinned %b %-d, %Y %H:%M",
                            "置顶于 %Y/%m/%d %H:%M"
                        ))
                        .to_string()
                    })
                    .unwrap_or_default();
                let (gid, cid, mid) = (
                    self.app.sel_guild.clone(),
                    m.channel_id.clone(),
                    m.id.clone(),
                );
                let head = div()
                    .px(z(12.0))
                    .pt(z(8.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(line(at, 12.0, FAINT()).flex_1().min_w_0())
                    .child(
                        small_button(
                            SharedString::from(format!("pin_jump{mid}")),
                            tr!("ジャンプ", "Jump", "跳转"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.jump_to(gid.clone(), cid.clone(), mid.clone());
                            cx.notify();
                        })),
                    );
                div()
                    .px(z(8.0))
                    .pt(z(8.0))
                    .child(
                        card(CHAT(), DIVIDER(), 8.0)
                            .overflow_hidden()
                            .pb(z(8.0))
                            .child(head)
                            .child(self.message_row(m, None, window, cx)),
                    )
                    .into_any_element()
            }
            None => {
                let foot = if let Some(e) = &p.error {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(z(8.0))
                        .p(z(16.0))
                        .child(text(e.clone(), 14.0, RED()))
                        .child(
                            small_button("pins_retry", tr!("再試行", "Retry", "重试")).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.app.retry_pins();
                                    cx.notify();
                                }),
                            ),
                        )
                } else if p.loading.is_some() {
                    div()
                        .p(z(16.0))
                        .flex()
                        .justify_center()
                        .child(spinner(24.0))
                } else if p.items.is_empty() {
                    panel_note(tr!(
                        "このチャンネルにはピン留めされたメッセージがありません",
                        "This channel has no pinned messages",
                        "此频道没有置顶消息"
                    ))
                } else if p.has_more {
                    // 末尾まで来たら次のページを読む
                    self.app.pins = Some(p);
                    self.app.load_more_pins();
                    return div()
                        .p(z(16.0))
                        .flex()
                        .justify_center()
                        .child(spinner(24.0))
                        .into_any_element();
                } else {
                    div().h(z(16.0))
                };
                foot.into_any_element()
            }
        };
        if self.app.pins.is_none() {
            self.app.pins = Some(p);
        }
        el
    }
}

impl Root {
    fn search_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let a = &self.app;
        // どこを探すか（今いる場所）
        let scope = match (&a.sel_guild, a.current_channel()) {
            (Some(g), _) => a
                .guilds
                .iter()
                .find(|x| &x.id == g)
                .map(|g| {
                    tr!(
                        "サーバー「{}」を検索",
                        "Search server \"{}\"",
                        "搜索服务器“{}”",
                        g.name
                    )
                })
                .unwrap_or_default(),
            (None, Some(c)) => tr!(
                "「{}」との DM を検索",
                "Search DMs with \"{}\"",
                "搜索与“{}”的私信",
                c.dm_label()
            ),
            _ => String::new(),
        };
        let input = div()
            .mx(z(12.0))
            .mt(z(12.0))
            .bg(INPUT())
            .border_1()
            .border_color(DIVIDER())
            .rounded(z(8.0))
            .px(z(6.0))
            .flex()
            .items_center()
            .gap(z(4.0))
            .child(icon(IconName::Search, 16.0, MUTED()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.inp.search).appearance(false).h(z(32.0))),
            );
        let mut top = div().flex_none().flex().flex_col().child(input).child(
            div()
                .px(z(14.0))
                .pt(z(6.0))
                .child(line(scope, 12.0, FAINT())),
        );
        let Some(s) = a.search.as_ref() else {
            return div();
        };
        // 状態の一行（件数・ページ送り）
        let status: Option<Div> = if let Some(e) = &s.error {
            Some(
                div()
                    .px(z(14.0))
                    .py(z(8.0))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(z(8.0))
                    .child(text(e.clone(), 13.0, RED()))
                    .when(s.searched && s.scope.is_some(), |d| {
                        d.child(
                            small_button("search_retry", tr!("再試行", "Retry", "重试")).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.app.retry_search();
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
        } else if s.loading {
            Some(
                div()
                    .px(z(14.0))
                    .py(z(8.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(spinner(16.0))
                    .child(text(
                        if s.retry_at.is_some() {
                            tr!("検索の準備中…", "Preparing search…", "正在准备搜索…")
                        } else {
                            tr!("検索中…", "Searching…", "正在搜索…")
                        },
                        13.0,
                        MUTED(),
                    )),
            )
        } else if s.searched {
            let label = s.result_label();
            let can_prev = s.offset > 0;
            let can_next =
                s.offset + SEARCH_PAGE < s.total && s.offset + SEARCH_PAGE <= SEARCH_MAX_OFFSET;
            let summary = if s.partial {
                text(label, 13.0, MUTED())
            } else {
                line(label, 13.0, MUTED())
            };
            let row = div()
                .w_full()
                .flex()
                .items_center()
                .gap(z(6.0))
                .child(summary.flex_1().min_w_0())
                .when(can_prev, |d| {
                    d.child(
                        small_button("search_prev", tr!("前へ", "Previous", "上一步")).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.app.search_page(false);
                                cx.notify();
                            }),
                        ),
                    )
                })
                .when(can_next, |d| {
                    d.child(
                        small_button("search_next", tr!("次へ", "Next", "下一步")).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.app.search_page(true);
                                cx.notify();
                            }),
                        ),
                    )
                });
            Some(
                div()
                    .px(z(14.0))
                    .py(z(8.0))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(z(8.0))
                    .child(row)
                    .when(s.partial, |d| {
                        d.child(
                            small_button("search_retry_partial", tr!("再試行", "Retry", "重试"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app.retry_search();
                                    cx.notify();
                                })),
                        )
                    }),
            )
        } else {
            Some(
                div()
                    .px(z(14.0))
                    .py(z(10.0))
                    .flex()
                    .flex_col()
                    .gap(z(4.0))
                    .children(
                        [
                            tr!(
                                "from:名前 … 送った人",
                                "from:name … sender",
                                "from:名称 … 发送者"
                            ),
                            tr!(
                                "mentions:名前 … @ された人",
                                "mentions:name … person mentioned",
                                "mentions:名称 … 被 @ 的人"
                            ),
                            tr!(
                                "in:チャンネル … チャンネル（サーバーのみ）",
                                "in:channel … channel (servers only)",
                                "in:频道 … 频道（仅限服务器）"
                            ),
                            tr!(
                                "has:画像 / link / file / video / embed / sticker / poll",
                                "has:image / link / file / video / embed / sticker / poll",
                                "has:image / link / file / video / embed / sticker / poll"
                            ),
                            tr!(
                                "before: / after: / during:2026-01-31 … 日付",
                                "before: / after: / during:2026-01-31 … date",
                                "before: / after: / during:2026-01-31 … 日期"
                            ),
                            tr!(
                                "pinned:true … ピン留めされたもの",
                                "pinned:true … pinned messages",
                                "pinned:true … 已置顶的消息"
                            ),
                        ]
                        .map(|t| text(t, 12.0, MUTED())),
                    ),
            )
        };
        top = top.children(status);
        let (n, key) = (s.hits.len(), (2, format!("{:?}", s.scope), s.rev));
        self.sync_side_list(key, n);
        let this = self.this.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(panel_head(
                tr!("検索", "Search", "搜索"),
                IconName::Search,
                cx,
            ))
            .child(top)
            .child(
                list(self.side_list.clone(), move |ix, window, cx| {
                    let Some(e) = this.upgrade() else {
                        return div().into_any_element();
                    };
                    e.update(cx, |r, cx| r.search_item(ix, window, cx))
                })
                .flex_1(),
            )
    }

    fn search_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gid = self.app.search_guild();
        let Some(s) = self.app.search.take() else {
            return div().into_any_element();
        };
        let el = match s.hits.get(ix) {
            Some(hit) => {
                let m = &hit.message;
                let (gid, cid, mid) = (gid, m.channel_id.clone(), m.id.clone());
                let head = div()
                    .px(z(12.0))
                    .pt(z(8.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(
                        line(hit.place.clone(), 12.0, FAINT())
                            .font_weight(FontWeight::MEDIUM)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        small_button(
                            SharedString::from(format!("search_jump{mid}")),
                            tr!("ジャンプ", "Jump", "跳转"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.jump_to(gid.clone(), cid.clone(), mid.clone());
                            cx.notify();
                        })),
                    );
                div()
                    .px(z(8.0))
                    .pt(z(8.0))
                    .when(ix + 1 == s.hits.len(), |d| d.pb(z(8.0)))
                    .child(
                        card(CHAT(), DIVIDER(), 8.0)
                            .overflow_hidden()
                            .pb(z(8.0))
                            .child(head)
                            .child(self.message_row(m, None, window, cx)),
                    )
                    .into_any_element()
            }
            None => div().into_any_element(),
        };
        self.app.search = Some(s);
        el
    }
}
