//! サーバーのボイスチャンネルを開いたときの画面（参加者のタイル・アクティビティ・招待・通話の操作）。
//! 選んで開いただけの状態と、実際に通話へ参加している状態を分けて描く
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::sidebar::{live_badge, speaking_avatar};
use super::*;
use crate::app::{invite_link, user_avatar_url, SidePanel};
use crate::color::Color32;
use crate::model::{Channel, User};

/// この OS で画面共有・配信の視聴ができるか（golive/media.rs の platform と同じ条件）
pub const SCREEN_SHARE: bool = cfg!(target_os = "linux");
pub fn screen_share_unsupported() -> &'static str {
    tr!(
        "この OS では画面共有にまだ対応していません",
        "Screen sharing isn't supported on this OS yet",
        "此操作系统暂不支持屏幕共享"
    )
}
fn watch_unsupported() -> &'static str {
    tr!(
        "この OS では配信の視聴にまだ対応していません",
        "Watching streams isn't supported on this OS yet",
        "此操作系统暂不支持观看直播"
    )
}

/// 中身の最大幅（広い画面でタイルが間延びしないように）
const MAX_W: f32 = 1040.0;
const GAP: f32 = 8.0;
/// 下の操作ボタンの帯の高さ（折り返さないとき）
const BAR_H: f32 = 72.0;
const HEAD_H: f32 = 48.0;

/// このチャンネルと自分の接続の関係
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Link {
    /// 参加していない（別のチャンネルにいるなら elsewhere）
    Idle {
        elsewhere: bool,
    },
    Connecting,
    Connected,
    /// 通話はつながっているが、メインのゲートウェイが再接続中
    Reconnecting,
}

pub fn link_state(app: &crate::app::App, channel_id: &str) -> Link {
    match app.voice_channel() {
        Some((_, c, false)) if c == channel_id => Link::Connecting,
        Some((_, c, true)) if c == channel_id => {
            if app.gateway_ok() {
                Link::Connected
            } else {
                Link::Reconnecting
            }
        }
        Some(_) => Link::Idle { elsewhere: true },
        None => Link::Idle { elsewhere: false },
    }
}

/// 参加者の数と使える広さから (列数, タイルの幅, 高さ) を決める。16:9 を保ち、縦に入りきらなければ縮める
pub fn tile_layout(n: usize, avail_w: f32, avail_h: f32) -> (usize, f32, f32) {
    let n = n.max(1);
    let mut cols: usize = match n {
        1 => 1,
        2..=4 => 2,
        5..=9 => 3,
        _ => 4,
    };
    let col_w = |c: usize| (avail_w - GAP * (c - 1) as f32) / c as f32;
    while cols > 1 && col_w(cols) < 180.0 {
        cols -= 1;
    }
    let rows = n.div_ceil(cols);
    let mut w = col_w(cols).max(120.0);
    let mut h = w * 9.0 / 16.0;
    let max_h = ((avail_h - GAP * (rows - 1) as f32) / rows as f32).max(110.0);
    if h > max_h {
        h = max_h;
        w = w.min(h * 16.0 / 9.0);
    }
    (cols, w.floor(), h.floor())
}

/// 参加者 1 人分
struct Face {
    id: String,
    user: Option<User>,
    mute: bool,
    deaf: bool,
    speaking: bool,
    live: bool,
    me: bool,
}

/// 要素の ID。ユーザー ID は数字なので、文字列を作らずに数で区別する
fn eid(prefix: &'static str, id: &str, i: usize) -> ElementId {
    (prefix, id.parse::<u64>().map_or(i, |v| v as usize)).into()
}

/// 下の帯の、アイコン＋文字の控えめなボタン
fn pill(id: &'static str, name: IconName, label: &'static str, show_label: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(z(36.0))
        .px(z(if show_label { 14.0 } else { 10.0 }))
        .flex()
        .flex_none()
        .items_center()
        .gap(z(8.0))
        .rounded(z(8.0))
        .bg(WIDGET())
        .cursor_pointer()
        .hover(|s| s.bg(WIDGET_HOVER()))
        .child(icon(name, 16.0, TEXT()))
        .when(show_label, |d| {
            d.child(text(label, 14.0, TEXT()).font_weight(FontWeight::MEDIUM))
        })
}

/// 丸い操作ボタン（ミュートなど）。使えないときは薄くして押せなくする
fn ctl(id: &'static str, name: IconName, bg: Color32, fg: Color32, enabled: bool) -> Stateful<Div> {
    let d = div()
        .id(id)
        .size(z(48.0))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center();
    if enabled {
        d.bg(bg)
            .cursor_pointer()
            .hover(move |s| s.bg(bg.lerp_to_gamma(Color32::WHITE, 0.12)))
            .child(icon(name, 22.0, fg))
    } else {
        d.bg(bg.gamma_multiply(0.5))
            .child(icon(name, 22.0, FAINT()))
    }
}

/// 空のアクティビティに添える絵（Discord の素材ではなく、アイコンを並べただけのもの）
fn activity_art(size: f32) -> Div {
    let tile = |name: IconName, tint: Color32, lift: f32| {
        div()
            .size(z(size))
            .flex_none()
            .relative()
            .top(z(lift))
            .rounded(z(size * 0.28))
            .bg(tint.gamma_multiply(0.22))
            .border_1()
            .border_color(tint.gamma_multiply(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(name, size * 0.5, tint))
    };
    div()
        .flex()
        .items_center()
        .gap(z(size * 0.22))
        .child(tile(IconName::Dices, ORANGE(), size * 0.12))
        .child(tile(IconName::Gamepad2, BLURPLE(), -size * 0.1))
        .child(tile(IconName::Puzzle, GREEN(), size * 0.12))
}

impl Root {
    /// チャット欄の幅（左のサーバー・チャンネル一覧、右のメンバー一覧などを除いたもの）
    fn stage_width(&self) -> f32 {
        let a = &self.app;
        let (pane_w, mem_w) = a.pane_widths();
        let mut w = a.screen_w;
        if a.screen_w >= 700.0 {
            w -= crate::app::RAIL_W + pane_w;
        }
        if a.members_inline() {
            w -= mem_w;
        }
        if a.side != SidePanel::None && a.screen_w >= 900.0 {
            w -= 360.0;
        }
        w.max(0.0)
    }

    /// ボイスチャンネルの画面（chat_column の、ヘッダーより下）
    pub(super) fn voice_stage(&mut self, c: &Channel, cx: &mut Context<Self>) -> AnyElement {
        let Some(gid) = self.app.sel_guild.clone() else {
            return div().flex_1().into_any_element();
        };
        let cid = c.id.clone();
        let link = link_state(&self.app, &cid);
        let here = matches!(link, Link::Connected | Link::Reconnecting);
        let content_w = (self.stage_width() - 32.0).clamp(160.0, MAX_W);
        let stream = self.stream_view(cx);
        let stream_h = if stream.is_some() {
            (self.pane_h * 0.55).clamp(180.0, 720.0)
        } else {
            0.0
        };
        // 縦の割り振り：ヘッダー・下の帯・余白を除いた残りを、タイルとアクティビティで分ける
        let body_h = (self.pane_h - HEAD_H - BAR_H - stream_h - 32.0).max(0.0);
        let compact_activity = body_h < 420.0 || stream.is_some();
        let activity_h = if compact_activity {
            56.0
        } else {
            (body_h * 0.32).clamp(150.0, 220.0)
        };
        let tiles_h = (body_h - activity_h - GAP * 2.0).max(110.0);

        // 背景はチャット欄と同じ（テーマの背景画像があれば敷く）
        let bg = self
            .app
            .theme_applied
            .as_ref()
            .and_then(|t| t.background.clone())
            .filter(|b| !b.window);
        let mut inner = div()
            .w(z(content_w))
            .flex_none()
            .flex()
            .flex_col()
            .gap(z(12.0));
        inner = inner.children(self.voice_notice(&gid, &cid, link, cx));
        inner = inner.child(self.voice_faces(c, &cid, here, content_w, tiles_h, cx));
        inner = inner.child(self.activity_card(content_w, activity_h, compact_activity));

        let mut col = div().flex_1().min_h_0().relative().flex().flex_col();
        if let Some(bg) = &bg {
            col = col.child(background(bg, opaque(CHAT())));
        }
        col.children(stream)
            .child(
                div()
                    .id("vc_scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .min_h_full()
                            .py(z(16.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .child(inner),
                    ),
            )
            .child(self.voice_bar(&gid, &cid, link, content_w, cx))
            .into_any_element()
    }

    /// 接続できなかった・再接続中などの知らせ（上に 1 行）
    fn voice_notice(
        &mut self,
        gid: &str,
        cid: &str,
        link: Link,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = |color: Color32| {
            div()
                .w_full()
                .px(z(12.0))
                .py(z(8.0))
                .rounded(z(8.0))
                .bg(color.gamma_multiply(0.18))
                .border_1()
                .border_color(color.gamma_multiply(0.6))
                .flex()
                .items_center()
                .gap(z(8.0))
        };
        if let Some((_, _, msg)) = self
            .app
            .voice_view
            .error
            .as_ref()
            .filter(|(_, c, _)| c == cid && matches!(link, Link::Idle { .. }))
        {
            let (g, c) = (gid.to_string(), cid.to_string());
            return Some(
                row(RED())
                    .child(icon(IconName::CircleAlert, 16.0, RED()))
                    .child(
                        line(
                            tr!(
                                "{msg}。通信状態と音声設定を確認してください",
                                "{msg}. Check your connection and audio settings",
                                "{msg}。请检查网络连接和音频设置",
                            ),
                            13.0,
                            TEXT(),
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        small_button("vc_retry", tr!("再試行", "Retry", "重试")).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.app.join_voice(Some(g.clone()), c.clone(), false);
                                cx.notify();
                            }),
                        ),
                    )
                    .into_any_element(),
            );
        }
        let msg = match link {
            Link::Reconnecting => tr!(
                "サーバーとの接続が切れました。再接続しています…",
                "Lost connection to the server. Reconnecting…",
                "与服务器的连接已断开。正在重新连接…"
            ),
            Link::Idle { .. } if !self.app.gateway_ok() => {
                tr!(
                    "サーバーにつながっていません。つながるとボイスに参加できます",
                    "Not connected to the server. You can join voice once connected",
                    "未连接服务器。连接后即可加入语音"
                )
            }
            _ => return None,
        };
        Some(
            row(ORANGE())
                .child(icon(IconName::WifiOff, 16.0, ORANGE()))
                .child(line(msg, 13.0, TEXT()).flex_1().min_w_0())
                .into_any_element(),
        )
    }

    /// 参加者のタイル（いないときは空の案内）
    fn voice_faces(
        &mut self,
        c: &Channel,
        cid: &str,
        here: bool,
        content_w: f32,
        tiles_h: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let a = &self.app;
        let me = a.me.as_ref().map(|m| m.id.as_str());
        let faces: Vec<Face> = a
            .voice_members(cid)
            .map(|v| Face {
                id: v.user_id.clone(),
                user: v.user.clone().or_else(|| a.find_user(&v.user_id)),
                mute: v.self_mute,
                deaf: v.self_deaf,
                speaking: a.is_speaking(&v.user_id),
                live: a.is_streaming(&v.user_id),
                me: me == Some(v.user_id.as_str()),
            })
            .collect();
        if faces.is_empty() {
            let h = tiles_h.min(320.0);
            return div()
                .w_full()
                .h(z(h))
                .rounded(z(8.0))
                .bg(RAIL())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(z(10.0))
                .child(
                    div()
                        .size(z(64.0))
                        .rounded_full()
                        .bg(WIDGET())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::Volume2, 30.0, MUTED())),
                )
                .child(
                    medium(c.name.clone().unwrap_or_default(), 20.0, WHITE())
                        .max_w_full()
                        .truncate(),
                )
                .child(text(
                    tr!("今は誰もいません", "No one is here right now", "现在没有人"),
                    14.0,
                    MUTED(),
                ))
                .into_any_element();
        }
        let (_, tw, th) = tile_layout(faces.len(), content_w, tiles_h);
        let av = (th * 0.36).clamp(36.0, 96.0);
        let watching = a
            .watching
            .as_ref()
            .and_then(|(k, _)| crate::golive::key_user(k))
            .map(str::to_owned);
        let mut grid = div()
            .w_full()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(z(GAP));
        for (i, f) in faces.into_iter().enumerate() {
            let name = f
                .user
                .as_ref()
                .map(|u| u.display().to_string())
                .unwrap_or_else(|| f.id.clone());
            let mut tag = div()
                .absolute()
                .left(z(8.0))
                .bottom(z(8.0))
                .max_w(z((tw - 16.0).max(40.0)))
                .px(z(8.0))
                .py(z(3.0))
                .rounded(z(6.0))
                .bg(Color32::from_black_alpha(150))
                .flex()
                .items_center()
                .gap(z(5.0));
            if f.deaf {
                tag = tag.child(icon(IconName::HeadphoneOff, 14.0, RED()));
            }
            if f.mute || f.deaf {
                tag = tag.child(icon(IconName::MicOff, 14.0, RED()));
            }
            tag = tag.child(line(name, 13.0, Color32::WHITE).min_w_0());
            let watched = watching.as_deref() == Some(f.id.as_str());
            let mut tile = div()
                .id(eid("vc_tile", &f.id, i))
                .group("vct")
                .w(z(tw))
                .h(z(th))
                .flex_none()
                .relative()
                .rounded(z(8.0))
                .overflow_hidden()
                .bg(RAIL())
                .border_2()
                .border_color(if f.speaking {
                    GREEN()
                } else {
                    Color32::TRANSPARENT
                })
                .flex()
                .items_center()
                .justify_center()
                .child(speaking_avatar(f.user.as_ref(), &f.id, av, f.speaking))
                .child(tag);
            if f.live {
                tile = tile.child(
                    div()
                        .absolute()
                        .top(z(8.0))
                        .left(z(8.0))
                        .flex()
                        .items_center()
                        .gap(z(6.0))
                        .child(live_badge())
                        .when(watched, |d| {
                            d.child(
                                div()
                                    .px(z(6.0))
                                    .rounded(z(4.0))
                                    .bg(Color32::from_black_alpha(150))
                                    .child(text(
                                        tr!("視聴中", "Watching", "观看中"),
                                        11.0,
                                        Color32::WHITE,
                                    )),
                            )
                        }),
                );
                if !f.me && !watched {
                    if !SCREEN_SHARE {
                        tile = tile.tooltip(tip(watch_unsupported()));
                    } else if here {
                        let uid = f.id.clone();
                        tile = tile
                            .cursor_pointer()
                            .tooltip(tip(tr!("配信を見る", "Watch stream", "观看直播")))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(Color32::from_black_alpha(120))
                                    .invisible()
                                    .group_hover("vct", |s| s.visible())
                                    .child(
                                        div()
                                            .px(z(14.0))
                                            .py(z(6.0))
                                            .rounded(z(8.0))
                                            .bg(WIDGET())
                                            .child(text(
                                                tr!("配信を見る", "Watch stream", "观看直播"),
                                                14.0,
                                                WHITE(),
                                            )),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.app.watch_stream(&uid);
                                cx.notify();
                            }));
                    } else {
                        tile = tile.tooltip(tip(tr!(
                            "配信を見るには、このチャンネルに参加してください",
                            "Join this channel to watch the stream",
                            "要观看直播，请加入此频道"
                        )));
                    }
                }
            }
            grid = grid.child(tile);
        }
        grid.into_any_element()
    }

    /// 下のアクティビティの枠。Riscord はまだ起動・参加ができないので、空の案内だけを出す
    fn activity_card(&self, content_w: f32, h: f32, compact: bool) -> AnyElement {
        let base = div()
            .w_full()
            .h(z(h))
            .flex_none()
            .rounded(z(8.0))
            .bg(RAIL())
            .border_1()
            .border_color(BORDER());
        if compact {
            return base
                .px(z(14.0))
                .flex()
                .items_center()
                .gap(z(10.0))
                .child(icon(IconName::Gamepad2, 20.0, MUTED()))
                .child(
                    line(
                        tr!(
                            "アクティビティはまだ始まっていません",
                            "No activity has started yet",
                            "活动尚未开始"
                        ),
                        14.0,
                        TEXT(),
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .into_any_element();
        }
        let art = (h * 0.24).clamp(32.0, 52.0);
        base.p(z(16.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(z(10.0))
            .child(activity_art(art))
            .child(medium(
                tr!(
                    "アクティビティはまだ始まっていません",
                    "No activity has started yet",
                    "活动尚未开始"
                ),
                if content_w < 420.0 { 14.0 } else { 16.0 },
                WHITE(),
            ))
            .child(
                text(
                    tr!(
                        "Riscord はアクティビティの起動・参加にまだ対応していません",
                        "Riscord doesn't support launching or joining activities yet",
                        "Riscord 尚不支持启动或加入活动"
                    ),
                    13.0,
                    MUTED(),
                )
                .text_center(),
            )
            .into_any_element()
    }

    /// 下の帯（招待・アクティビティと、通話の操作）。スクロールの外に置き、小さな窓でも消えないようにする
    fn voice_bar(
        &mut self,
        gid: &str,
        cid: &str,
        link: Link,
        content_w: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let labels = content_w >= 520.0;
        let (g1, c1) = (gid.to_string(), cid.to_string());
        let actions = div()
            .flex()
            .items_center()
            .gap(z(8.0))
            .child(
                pill(
                    "vc_invite",
                    IconName::UserPlus,
                    tr!("ボイスに招待", "Invite to Voice", "邀请加入语音"),
                    labels,
                )
                .tooltip(tip(tr!("ボイスに招待", "Invite to Voice", "邀请加入语音")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.open_voice_invite(&g1, &c1);
                    cx.notify();
                })),
            )
            .child(
                pill(
                    "vc_activity",
                    IconName::Rocket,
                    tr!("アクティビティを選ぶ", "Choose an activity", "选择活动"),
                    labels,
                )
                .tooltip(tip(tr!(
                    "アクティビティを選ぶ",
                    "Choose an activity",
                    "选择活动"
                )))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.toggle_voice_activity();
                    cx.notify();
                })),
            );
        let a = &self.app;
        let controls = match link {
            Link::Connected | Link::Reconnecting => {
                let off = a.mic_off();
                let deaf = a.settings.self_deaf;
                let sharing = a.my_stream.is_some();
                let dark = WIDGET();
                div()
                    .flex()
                    .items_center()
                    .gap(z(10.0))
                    .child(
                        ctl(
                            "vc_mic",
                            if off { IconName::MicOff } else { IconName::Mic },
                            if off { RED() } else { dark },
                            if off { Color32::WHITE } else { TEXT() },
                            true,
                        )
                        .tooltip(tip(if off {
                            tr!("ミュート解除", "Unmute", "取消静音")
                        } else {
                            tr!("ミュート", "Mute", "静音")
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.toggle_mute();
                            cx.notify();
                        })),
                    )
                    .child(
                        ctl(
                            "vc_deaf",
                            if deaf {
                                IconName::HeadphoneOff
                            } else {
                                IconName::Headphones
                            },
                            if deaf { RED() } else { dark },
                            if deaf { Color32::WHITE } else { TEXT() },
                            true,
                        )
                        .tooltip(tip(if deaf {
                            tr!("スピーカーミュート解除", "Undeafen", "取消关闭扬声器")
                        } else {
                            tr!("スピーカーミュート", "Deafen", "关闭扬声器")
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.toggle_deaf();
                            cx.notify();
                        })),
                    )
                    .child({
                        // 再接続中に配信を始めない（つながっていないのに共有中に見せない）
                        let enabled = SCREEN_SHARE && (sharing || link == Link::Connected);
                        let b = ctl(
                            "vc_share",
                            if sharing {
                                IconName::MonitorOff
                            } else {
                                IconName::MonitorUp
                            },
                            if sharing { GREEN() } else { dark },
                            if sharing { Color32::WHITE } else { TEXT() },
                            enabled,
                        )
                        .tooltip(tip(if !SCREEN_SHARE {
                            screen_share_unsupported()
                        } else if sharing {
                            tr!("画面共有を停止", "Stop sharing", "停止共享屏幕")
                        } else if enabled {
                            tr!("画面を共有", "Share screen", "共享屏幕")
                        } else {
                            tr!(
                                "再接続が終わると画面を共有できます",
                                "You can share your screen once reconnected",
                                "重新连接后即可共享屏幕"
                            )
                        }));
                        if enabled {
                            b.on_click(cx.listener(|this, _, _, cx| {
                                this.app.toggle_stream();
                                cx.notify();
                            }))
                        } else {
                            b
                        }
                    })
                    .child(
                        ctl("vc_leave", IconName::PhoneOff, RED(), Color32::WHITE, true)
                            .tooltip(tip(tr!("切断", "Disconnected", "已断开")))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.leave_voice();
                                cx.notify();
                            })),
                    )
            }
            Link::Connecting => div()
                .flex()
                .items_center()
                .gap(z(10.0))
                .child(spinner(20.0))
                .child(text(
                    tr!("接続中…", "Connecting…", "正在连接…"),
                    14.0,
                    MUTED(),
                ))
                .child(
                    ctl("vc_leave", IconName::PhoneOff, RED(), Color32::WHITE, true)
                        .tooltip(tip(tr!("接続をやめる", "Disconnect", "断开连接")))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.leave_voice();
                            cx.notify();
                        })),
                ),
            Link::Idle { elsewhere } => {
                let ok = a.gateway_ok();
                let (g, c) = (gid.to_string(), cid.to_string());
                let b = fill_button(
                    "vc_join",
                    if elsewhere {
                        tr!("このチャンネルに移動", "Move to this channel", "移到此频道")
                    } else {
                        tr!("ボイスに参加", "Join Voice", "加入语音")
                    },
                    GREEN(),
                    Color32::WHITE,
                    ok,
                );
                div().child(if ok {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        this.app.join_voice(Some(g.clone()), c.clone(), false);
                        cx.notify();
                    }))
                } else {
                    b.tooltip(tip(tr!(
                        "サーバーにつながるまでお待ちください",
                        "Wait until connected to the server",
                        "请等待连接到服务器"
                    )))
                })
            }
        };
        div()
            .flex_none()
            .w_full()
            .min_h(z(BAR_H))
            .px(z(16.0))
            .py(z(12.0))
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(z(16.0))
            .child(actions)
            .child(controls)
            .into_any_element()
    }

    /// 「ボイスに招待」の窓（リンクのコピーと、DM への送信）
    pub(super) fn voice_invite_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let inv = self.app.voice_view.invite.as_ref()?;
        let a = &self.app;
        let width = (a.screen_w - 32.0).clamp(0.0, 460.0);
        let list_h = (self.pane_h - 300.0).clamp(80.0, 320.0);
        let (cname, gname) = a.voice_place(Some(&inv.guild_id), &inv.channel_id);
        let link = inv.code.as_deref().map(invite_link);
        let mut content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .p(z(16.0))
            .flex()
            .flex_col()
            .gap(z(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(6.0))
                    .child(
                        medium(
                            tr!(
                                "「{cname}」に友達を招待",
                                "Invite friends to \"{cname}\"",
                                "邀请好友加入“{cname}”",
                            ),
                            18.0,
                            WHITE(),
                        )
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                    )
                    .child(icon_button("vc_invite_close", IconName::X, 28.0).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.close_voice_invite();
                            cx.notify();
                        }),
                    )),
            )
            .child(line(gname, 13.0, MUTED()));
        if let Some(e) = &inv.error {
            content = content.child(text(e.clone(), 13.0, RED()));
        }
        let field = div()
            .flex()
            .items_center()
            .gap(z(8.0))
            .p(z(4.0))
            .pl(z(10.0))
            .rounded(z(6.0))
            .bg(INPUT())
            .border_1()
            .border_color(BORDER())
            .child(match (&link, inv.loading) {
                (Some(l), _) => line(l.clone(), 14.0, TEXT()).flex_1().min_w_0(),
                (None, true) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(spinner(14.0))
                    .child(text(
                        tr!(
                            "招待リンクを作っています…",
                            "Creating an invite link…",
                            "正在创建邀请链接…"
                        ),
                        14.0,
                        MUTED(),
                    )),
                (None, false) => line(
                    tr!("招待リンクはありません", "No invite link", "没有邀请链接"),
                    14.0,
                    FAINT(),
                )
                .flex_1(),
            })
            .child({
                let b = fill_button(
                    "vc_invite_copy",
                    tr!("コピー", "Copy", "复制"),
                    BLURPLE(),
                    Color32::WHITE,
                    link.is_some(),
                );
                if link.is_some() {
                    b.on_click(cx.listener(|this, _, _, cx| {
                        this.app.copy_voice_invite();
                        cx.notify();
                    }))
                } else {
                    b
                }
            });
        content = content.child(field).child(text(
            tr!("DM で送る", "Send via DM", "通过私信发送"),
            13.0,
            MUTED(),
        ));
        let mut list = div()
            .id("vc_invite_list")
            .max_h(z(list_h))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(z(2.0));
        if !a.dms_loaded {
            list = list.child(spinner(16.0));
        } else if a.dms.is_empty() {
            list = list.child(text(
                tr!(
                    "送れる DM がありません。リンクをコピーして共有してください",
                    "No DMs to send to. Copy the link to share it",
                    "没有可发送的私信。请复制链接分享"
                ),
                13.0,
                FAINT(),
            ));
        }
        for (i, dm) in a.dms.iter().take(50).enumerate() {
            let label = dm.dm_label();
            let av = match dm.recipients.as_slice() {
                [u] => avatar(Some(&user_avatar_url(u)), 28.0, &label, 14.0, BLURPLE()),
                _ => avatar(
                    crate::app::dm_icon_url(dm).as_deref(),
                    28.0,
                    &label,
                    14.0,
                    BLURPLE(),
                ),
            };
            let (busy, sent, failed) = (
                inv.sending.contains(&dm.id),
                inv.sent.contains(&dm.id),
                inv.failed.contains(&dm.id),
            );
            let enabled = link.is_some() && !busy && !sent;
            let b = fill_button(
                eid("vc_send", &dm.id, i),
                if sent {
                    tr!("送信済み", "Sent", "已发送")
                } else if busy {
                    tr!("送信中…", "Sending…", "正在发送…")
                } else if failed {
                    tr!("再送信", "Resend", "重新发送")
                } else {
                    tr!("送信", "Send", "发送")
                },
                if failed { RED() } else { GREEN() },
                Color32::WHITE,
                enabled,
            )
            .h(z(30.0))
            .px(z(12.0))
            .flex_none();
            let b = if enabled {
                let id = dm.id.clone();
                b.on_click(cx.listener(move |this, _, _, cx| {
                    this.app.send_voice_invite(&id);
                    cx.notify();
                }))
            } else {
                b
            };
            list = list.child(
                div()
                    .h(z(42.0))
                    .px(z(6.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(10.0))
                    .hover(|s| s.bg(HOVER()))
                    .child(av)
                    .child(line(label, 14.0, TEXT()).flex_1().min_w_0())
                    .child(b),
            );
        }
        content = content.child(list);
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.close_voice_invite();
            cx.notify();
        });
        Some(modal("vc_invite_modal", content, close, 170).into_any_element())
    }

    /// 「アクティビティを選ぶ」の案内。起動できないものを起動できるように見せない
    pub(super) fn voice_activity_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.app.voice_view.activity_open {
            return None;
        }
        let width = (self.app.screen_w - 32.0).clamp(0.0, 440.0);
        let url = match (&self.app.sel_guild, &self.app.sel_channel) {
            (Some(g), Some(c)) => format!("https://discord.com/channels/{g}/{c}"),
            _ => "https://discord.com/app".to_string(),
        };
        let content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .p(z(16.0))
            .flex()
            .flex_col()
            .gap(z(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(medium(tr!("アクティビティを選ぶ", "Choose an activity", "选择活动"), 18.0, WHITE()).flex_1())
                    .child(icon_button("vc_activity_close", IconName::X, 28.0).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.voice_view.activity_open = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(div().py(z(8.0)).flex().justify_center().child(activity_art(44.0)))
            .child(text(
                tr!("Riscord で使えるアクティビティはまだありません。", "There are no activities available in Riscord yet.", "Riscord 中暂无可用的活动。"),
                14.0,
                TEXT(),
            ))
            .child(text(
                tr!("アクティビティは Discord の中で動く Web アプリで、Riscord は WebView を使わないため起動・参加ができません。遊ぶときは公式アプリかブラウザ版から始めてください。", "Activities are web apps running inside Discord. Riscord doesn't use a WebView, so it can't launch or join them. To play, start from the official app or the browser version.", "活动是在 Discord 内运行的网页应用。Riscord 不使用 WebView，因此无法启动或加入活动。要玩的话，请从官方应用或浏览器版开始。"),
                13.0,
                MUTED(),
            ))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(text_button("vc_activity_ok", tr!("閉じる", "Close", "关闭")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.app.voice_view.activity_open = false;
                            cx.notify();
                        },
                    )))
                    .child(
                        fill_button(
                            "vc_activity_web",
                            tr!("ブラウザ版で開く", "Open in browser", "在浏览器中打开"),
                            BLURPLE(),
                            Color32::WHITE,
                            true,
                        )
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.voice_view.activity_open = false;
            cx.notify();
        });
        Some(modal("vc_activity_modal", content, close, 170).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::tile_layout;

    #[test]
    fn one_person_fills_width_at_16_9() {
        let (cols, w, h) = tile_layout(1, 1000.0, 1000.0);
        assert_eq!(cols, 1);
        assert_eq!(w, 1000.0);
        assert_eq!(h, 562.0);
    }

    #[test]
    fn short_area_shrinks_tiles_keeping_ratio() {
        let (cols, w, h) = tile_layout(1, 1000.0, 300.0);
        assert_eq!(cols, 1);
        assert_eq!(h, 300.0);
        assert_eq!(w, 533.0);
    }

    #[test]
    fn many_people_use_more_columns_and_fit_width() {
        for n in [2, 4, 5, 9, 10, 25] {
            for avail in [240.0, 600.0, 1040.0] {
                let (cols, w, _) = tile_layout(n, avail, 600.0);
                assert!(cols >= 1 && cols <= 4);
                // 横にはみ出さない
                assert!(w * cols as f32 + 8.0 * (cols - 1) as f32 <= avail.max(120.0) + 0.5);
            }
        }
        assert_eq!(tile_layout(3, 1040.0, 600.0).0, 2);
        assert_eq!(tile_layout(6, 1040.0, 600.0).0, 3);
        assert_eq!(tile_layout(12, 1040.0, 600.0).0, 4);
        // 狭いと列を減らす
        assert_eq!(tile_layout(12, 300.0, 600.0).0, 1);
    }
}
