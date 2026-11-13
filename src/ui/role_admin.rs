//! サーバーのロールを編集するダイアログ。
use gpui::assets::IconName;
use gpui::component::input::{Input, Textarea};
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::color::Color32;

pub(super) const PERMISSIONS: &[(u32, crate::i18n::Tr)] = &[
    (0, trc!("招待を作成", "Create Invite", "创建邀请")),
    (1, trc!("メンバーをキック", "Kick Members", "踢出成员")),
    (2, trc!("メンバーを BAN", "Ban Members", "封禁成员")),
    (3, trc!("管理者", "Administrator", "管理员")),
    (4, trc!("チャンネルを管理", "Manage Channels", "管理频道")),
    (5, trc!("サーバーを管理", "Manage Server", "管理服务器")),
    (6, trc!("リアクションを追加", "Add Reactions", "添加反应")),
    (7, trc!("監査ログを見る", "View Audit Log", "查看审核日志")),
    (8, trc!("優先スピーカー", "Priority Speaker", "优先发言人")),
    (9, trc!("画面を共有", "Video", "共享屏幕")),
    (10, trc!("チャンネルを見る", "View Channels", "查看频道")),
    (11, trc!("メッセージを送信", "Send Messages", "发送消息")),
    (
        12,
        trc!(
            "読み上げメッセージ",
            "Send Text-to-Speech Messages",
            "发送文字转语音消息"
        ),
    ),
    (13, trc!("メッセージを管理", "Manage Messages", "管理消息")),
    (14, trc!("リンクを埋め込み", "Embed Links", "嵌入链接")),
    (15, trc!("ファイルを添付", "Attach Files", "附加文件")),
    (
        16,
        trc!("履歴を読む", "Read Message History", "阅读消息历史"),
    ),
    (
        17,
        trc!("全員にメンション", "Mention Everyone", "提及所有人"),
    ),
    (
        18,
        trc!("外部の絵文字", "Use External Emoji", "使用外部表情"),
    ),
    (
        19,
        trc!(
            "サーバーインサイト",
            "View Server Insights",
            "查看服务器洞察"
        ),
    ),
    (20, trc!("ボイスに接続", "Connect", "连接语音")),
    (21, trc!("発言", "Speak", "发言")),
    (22, trc!("メンバーをミュート", "Mute Members", "将成员静音")),
    (
        23,
        trc!(
            "メンバーのスピーカーをミュート",
            "Deafen Members",
            "关闭成员扬声器"
        ),
    ),
    (24, trc!("メンバーを移動", "Move Members", "移动成员")),
    (
        25,
        trc!("音声検出を使用", "Use Voice Activity", "使用语音检测"),
    ),
    (
        26,
        trc!(
            "自分のニックネームを変更",
            "Change Nickname",
            "修改自己的昵称"
        ),
    ),
    (
        27,
        trc!("ニックネームを管理", "Manage Nicknames", "管理昵称"),
    ),
    (28, trc!("ロールを管理", "Manage Roles", "管理身份组")),
    (
        29,
        trc!("Webhook を管理", "Manage Webhooks", "管理 Webhook"),
    ),
    (
        30,
        trc!("絵文字などを管理", "Manage Expressions", "管理表情等"),
    ),
    (
        31,
        trc!(
            "アプリコマンドを使用",
            "Use Application Commands",
            "使用应用命令"
        ),
    ),
    (32, trc!("発言をリクエスト", "Request to Speak", "请求发言")),
    (33, trc!("イベントを管理", "Manage Events", "管理活动")),
    (34, trc!("スレッドを管理", "Manage Threads", "管理子区")),
    (
        35,
        trc!(
            "公開スレッドを作成",
            "Create Public Threads",
            "创建公开子区"
        ),
    ),
    (
        36,
        trc!(
            "非公開スレッドを作成",
            "Create Private Threads",
            "创建私密子区"
        ),
    ),
    (
        37,
        trc!("外部のスタンプ", "Use External Stickers", "使用外部贴纸"),
    ),
    (
        38,
        trc!(
            "スレッドに送信",
            "Send Messages in Threads",
            "在子区中发送消息"
        ),
    ),
    (
        39,
        trc!("アクティビティを使用", "Use Activities", "使用活动"),
    ),
    (
        40,
        trc!("メンバーをタイムアウト", "Time Out Members", "禁言成员"),
    ),
    (
        41,
        trc!(
            "収益化インサイト",
            "View Creator Monetization Analytics",
            "查看创作者收益分析"
        ),
    ),
    (
        42,
        trc!("サウンドボードを使用", "Use Soundboard", "使用音效板"),
    ),
    (
        43,
        trc!("絵文字などを作成", "Create Expressions", "创建表情等"),
    ),
    (44, trc!("イベントを作成", "Create Events", "创建活动")),
    (
        45,
        trc!("外部サウンド", "Use External Sounds", "使用外部音效"),
    ),
    (
        46,
        trc!(
            "ボイスメッセージを送信",
            "Send Voice Messages",
            "发送语音消息"
        ),
    ),
    (
        48,
        trc!(
            "ボイスチャンネルのステータス",
            "Set Voice Channel Status",
            "设置语音频道状态"
        ),
    ),
    (49, trc!("投票を作成", "Create Polls", "创建投票")),
    (
        50,
        trc!("外部アプリを使用", "Use External Apps", "使用外部应用"),
    ),
    (51, trc!("メッセージをピン留め", "Pin Messages", "置顶消息")),
    (
        52,
        trc!("低速モードを無視", "Bypass Slow Mode", "无视慢速模式"),
    ),
];

impl Root {
    pub fn role_admin_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // サーバー設定の画面の中で出しているときは、そちらで描く
        if self.app.server_settings.is_some() {
            return None;
        }
        self.role_admin_view(false, cx)
    }

    /// ロール・サーバー設定・チャンネル権限の中身。embedded = サーバー設定の画面の右側に出す（枠と題を付けない）
    pub fn role_admin_view(
        &mut self,
        embedded: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let a = self.app.role_admin.as_ref()?;
        let width = (self.app.screen_w - 32.0).clamp(0.0, 620.0);
        let height = (self.pane_h - 120.0).clamp(100.0, 600.0);
        let busy = a.mutation.is_some();
        let loading = a.roles_request.is_some()
            || a.member_request.is_some()
            || a.owner_request.is_some()
            || a.target
                .as_ref()
                .is_some_and(|t| t.member_request.is_some())
            || a.channel_permissions.as_ref().is_some_and(|v| {
                v.channel_request.is_some()
                    || v.parent_request.is_some()
                    || v.member_request.is_some()
            });
        let allowed = if a.channel_permissions.is_some() {
            self.app.can_manage_channel_permissions()
        } else if a.editing_guild {
            self.app.can_manage_guild()
        } else {
            self.app.can_manage_roles()
        };
        let mut content = if embedded {
            div().flex_1().min_w_0().flex().flex_col().gap(z(8.0))
        } else {
            card(SIDEBAR(), BORDER(), 8.0)
                .w(z(width))
                .max_h(z((self.pane_h - 32.0).max(100.0)))
                .p(z(16.0))
                .flex()
                .flex_col()
                .gap(z(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(z(6.0))
                        .child(
                            medium(
                                if a.channel_permissions.is_some() {
                                    tr!("チャンネル権限", "Channel Permissions", "频道权限")
                                } else if a.editing_guild {
                                    tr!("サーバー設定", "Server Settings", "服务器设置")
                                } else if a.target.is_some() {
                                    tr!("メンバーのロール", "Member Roles", "成员身份组")
                                } else {
                                    tr!("サーバーのロール", "Server Roles", "服务器身份组")
                                },
                                20.0,
                                WHITE(),
                            )
                            .flex_1(),
                        )
                        .child(icon_button("roles_close", IconName::X, 28.0).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this
                                    .app
                                    .role_admin
                                    .as_ref()
                                    .is_some_and(|a| a.mutation.is_none())
                                {
                                    this.app.role_admin = None;
                                }
                                cx.notify();
                            }),
                        )),
                )
        };
        if let Some(error) = &a.error {
            content = content.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(z(6.0))
                    .items_center()
                    .child(text(error.clone(), 13.0, RED()).flex_1().min_w_0())
                    .child(
                        text_button("roles_retry", tr!("再読み込み", "Reload", "重新加载"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.reload_role_admin();
                                cx.notify();
                            })),
                    ),
            );
        }
        if loading {
            content = content.child(div().child(text(
                tr!(
                    "設定と権限を読み込み中…",
                    "Loading settings and permissions…",
                    "正在加载设置和权限…"
                ),
                14.0,
                MUTED(),
            )));
        } else if !allowed {
            content = content.child(text(
                if a.channel_permissions.is_some() {
                    tr!("チャンネルの権限を管理できないか、必要な情報を確認できませんでした。", "You can't manage this channel's permissions, or the required info couldn't be verified.", "你无法管理此频道的权限，或无法确认所需信息。")
                } else if a.editing_guild {
                    tr!("サーバーを管理する権限がないか、権限を確認できませんでした。", "You don't have permission to manage the server, or your permissions couldn't be verified.", "你没有管理服务器的权限，或无法确认你的权限。")
                } else {
                    tr!("ロールを管理する権限がないか、権限を確認できませんでした。", "You don't have permission to manage roles, or your permissions couldn't be verified.", "你没有管理身份组的权限，或无法确认你的权限。")
                },
                14.0,
                MUTED(),
            ));
        }
        if a.channel_permissions.is_some() {
            content = content.child(self.channel_permissions_form(height, cx));
            return Some(self.role_admin_frame(content, embedded, cx));
        }
        if a.editing_guild {
            if let Some(draft) = &a.guild_edit {
                let editable = self.app.guild_draft_editable();
                let stale = draft
                    .latest
                    .as_ref()
                    .is_some_and(|latest| latest != &draft.original);
                if stale {
                    content = content.child(div().flex().flex_wrap().items_center().gap(z(8.0))
                        .child(text(tr!("サーバー設定が変更されました。最新の内容を読み直してください。", "Server settings have changed. Please reload the latest version.", "服务器设置已更改。请重新加载最新内容。"), 13.0, RED()).flex_1().min_w_0())
                        .child(text_button("guild_reset", tr!("最新の内容を使う", "Use latest", "使用最新内容")).on_click(cx.listener(|this, _, _, cx| { this.app.reset_guild_draft(); cx.notify(); }))));
                }
                let mut form = div()
                    .id("guild-settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .max_h(z(height))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(z(8.0))
                    .child(text(
                        tr!("サーバー名", "Server name", "服务器名称"),
                        13.0,
                        MUTED(),
                    ))
                    .child(
                        div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                            Input::new(&self.inp.guild_name)
                                .appearance(false)
                                .disabled(!editable)
                                .h(z(34.0)),
                        ),
                    );
                if draft.original.description.is_some() {
                    form = form
                        .child(text(
                            tr!("サーバーの説明", "Server description", "服务器说明"),
                            13.0,
                            MUTED(),
                        ))
                        .child(
                            div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                                Textarea::new(&self.inp.guild_description)
                                    .appearance(false)
                                    .disabled(!editable),
                            ),
                        );
                }
                let groups: [(&str, u8, Option<u64>, &[(u64, &str)]); 4] = [
                    (
                        tr!("認証レベル", "Verification Level", "验证等级"),
                        0,
                        draft.verification,
                        &[
                            (0, tr!("なし", "None", "无")),
                            (1, tr!("低", "Low", "低")),
                            (2, tr!("中", "Medium", "中")),
                            (3, tr!("高", "High", "高")),
                            (4, tr!("最高", "Highest", "最高")),
                        ],
                    ),
                    (
                        tr!("既定の通知", "Default Notifications", "默认通知"),
                        1,
                        draft.notifications,
                        &[
                            (0, tr!("すべて", "All", "全部")),
                            (1, tr!("メンションのみ", "Only mentions", "仅提及")),
                        ],
                    ),
                    (
                        tr!(
                            "不適切な画像のフィルター",
                            "Explicit Image Filter",
                            "不当图片过滤"
                        ),
                        2,
                        draft.content_filter,
                        &[
                            (0, tr!("無効", "Don't scan", "不扫描")),
                            (
                                1,
                                tr!(
                                    "ロールのないメンバー",
                                    "Members without roles",
                                    "无身份组的成员"
                                ),
                            ),
                            (2, tr!("全メンバー", "All members", "所有成员")),
                        ],
                    ),
                    (
                        tr!("離席タイムアウト", "AFK Timeout", "挂机超时"),
                        3,
                        draft.afk_timeout,
                        &[
                            (60, tr!("1分", "1 min", "1 分钟")),
                            (300, tr!("5分", "5 min", "5 分钟")),
                            (900, tr!("15分", "15 min", "15 分钟")),
                            (1800, tr!("30分", "30 min", "30 分钟")),
                            (3600, tr!("1時間", "1 hour", "1 小时")),
                        ],
                    ),
                ];
                for (label, field, current, options) in groups {
                    let Some(current) = current else { continue };
                    form = form.child(text(label, 13.0, MUTED()));
                    let mut choices = div().flex().flex_wrap().gap(z(5.0));
                    if !options.iter().any(|(value, _)| *value == current) {
                        choices = choices.child(text(
                            tr!("現在: {current}", "Current: {current}", "当前：{current}",),
                            12.0,
                            MUTED(),
                        ));
                    }
                    for &(value, label) in options {
                        choices = choices.child(
                            fill_button(
                                format!("guild-setting-{field}-{value}"),
                                label,
                                if value == current {
                                    BLURPLE()
                                } else {
                                    WIDGET()
                                },
                                WHITE(),
                                editable,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this.app.guild_draft_editable() {
                                        if let Some(d) = this
                                            .app
                                            .role_admin
                                            .as_mut()
                                            .and_then(|a| a.guild_edit.as_mut())
                                        {
                                            match field {
                                                0 => d.verification = Some(value),
                                                1 => d.notifications = Some(value),
                                                2 => d.content_filter = Some(value),
                                                3 => d.afk_timeout = Some(value),
                                                _ => {}
                                            }
                                        }
                                    }
                                    cx.notify();
                                },
                            )),
                        );
                    }
                    form = form.child(choices);
                }
                content = content.child(form).child(
                    div().flex().justify_end().gap(z(8.0)).child(
                        fill_button(
                            "guild_save",
                            if busy {
                                tr!("保存中…", "Saving…", "正在保存…")
                            } else {
                                tr!("変更を保存", "Save Changes", "保存更改")
                            },
                            BLURPLE(),
                            WHITE(),
                            editable,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.save_guild_settings();
                            cx.notify();
                        })),
                    ),
                );
            }
            return Some(self.role_admin_frame(content, embedded, cx));
        }
        if let Some(target) = &a.target {
            content = content.child(text(target.name.clone(), 15.0, WHITE()));
            if a.owner_id.as_deref() == Some(target.user_id.as_str())
                && self
                    .app
                    .me
                    .as_ref()
                    .is_none_or(|me| me.id != target.user_id)
            {
                content = content.child(text(
                    tr!(
                        "サーバー所有者のロールは変更できません。",
                        "The server owner's roles can't be changed.",
                        "无法更改服务器所有者的身份组。"
                    ),
                    13.0,
                    MUTED(),
                ));
            }
            if busy {
                content = content.child(text(
                    tr!("ロールを更新中…", "Updating roles…", "正在更新身份组…"),
                    13.0,
                    MUTED(),
                ));
            }
            if let (Some(roles), Some(assigned)) = (&a.roles, &target.roles) {
                let mut ordered = roles.clone();
                ordered.sort_by(|x, y| y.position.cmp(&x.position).then_with(|| x.id.cmp(&y.id)));
                let mut list = div()
                    .id("member-role-list")
                    .flex_1()
                    .min_h_0()
                    .max_h(z(height))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(z(3.0));
                for role in ordered.iter().filter(|r| r.id != a.guild_id) {
                    let editable = !busy && self.app.can_assign_member_role(role);
                    let assigned = assigned.contains(&role.id);
                    let id = role.id.clone();
                    let label = role.name.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("member-role-{id}")))
                            .min_h(z(36.0))
                            .px(z(8.0))
                            .rounded(z(4.0))
                            .flex()
                            .items_center()
                            .gap(z(8.0))
                            .when(editable, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                            .when(!editable, |d| d.opacity(0.55))
                            .child(text(if assigned { "☑" } else { "☐" }, 16.0, TEXT()))
                            .child(line(label, 14.0, TEXT()).flex_1().min_w_0())
                            .when(role.managed != Some(false), |d| {
                                d.child(text(
                                    tr!(
                                        "連携 / 管理不可",
                                        "Integration / unmanageable",
                                        "集成 / 无法管理"
                                    ),
                                    11.0,
                                    MUTED(),
                                ))
                            })
                            .when(!editable && role.managed == Some(false) && !busy, |d| {
                                d.child(text(
                                    tr!(
                                        "権限または階層により変更不可",
                                        "Can't change due to permissions or hierarchy",
                                        "因权限或层级限制无法更改"
                                    ),
                                    11.0,
                                    MUTED(),
                                ))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if editable {
                                    this.app.toggle_member_role(&id);
                                }
                                cx.notify();
                            })),
                    );
                }
                content = content.child(list);
            }
            return Some(self.role_admin_frame(content, embedded, cx));
        }
        if let Some(draft) = &a.draft {
            let editable = self.app.role_draft_editable();
            if !editable && !loading {
                content = content.child(text(tr!("ロールが変更されたか、編集権限がありません。キャンセルして一覧を確認してください。", "The role was changed, or you lack permission to edit it. Cancel and check the list.", "身份组已被更改，或你没有编辑权限。请取消并查看列表。"), 13.0, RED()));
            }
            let everyone = draft.original.as_ref().is_some_and(|r| r.id == a.guild_id);
            let mut form = div()
                .id("role-edit-scroll")
                .flex_1()
                .min_h_0()
                .max_h(z(height))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(z(8.0))
                .child(medium(
                    if draft.original.is_some() {
                        tr!("ロールを編集", "Edit Role", "编辑身份组")
                    } else {
                        tr!("ロールを作成", "Create Role", "创建身份组")
                    },
                    16.0,
                    WHITE(),
                ))
                .child(
                    div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                        Input::new(&self.inp.role_name)
                            .appearance(false)
                            .disabled(busy || everyone || !editable)
                            .h(z(34.0)),
                    ),
                )
                .child(text(tr!("色", "Color", "颜色"), 13.0, MUTED()));
            let mut colors = div().flex().flex_wrap().gap(z(6.0));
            for color in [
                0u32, 0x5865f2, 0x57f287, 0xfee75c, 0xeb459e, 0xed4245, 0xffffff,
            ] {
                let bg = if color == 0 {
                    WIDGET()
                } else {
                    Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8)
                };
                colors = colors.child(
                    div()
                        .id(("role-color", color as usize))
                        .size(z(26.0))
                        .rounded_full()
                        .bg(bg)
                        .when(draft.color == color, |d| d.border_2().border_color(WHITE()))
                        .when(!busy && !everyone && editable, |d| d.cursor_pointer())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !busy && !everyone && this.app.role_draft_editable() {
                                if let Some(d) =
                                    this.app.role_admin.as_mut().and_then(|a| a.draft.as_mut())
                                {
                                    d.color = color;
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            form = form
                .child(colors)
                .child(
                    text_button(
                        "role_hoist",
                        if draft.hoist {
                            tr!(
                                "☑ メンバーを分けて表示",
                                "☑ Display members separately",
                                "☑ 分组显示成员"
                            )
                        } else {
                            tr!(
                                "☐ メンバーを分けて表示",
                                "☐ Display members separately",
                                "☐ 分组显示成员"
                            )
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy && !everyone && this.app.role_draft_editable() {
                            if let Some(d) =
                                this.app.role_admin.as_mut().and_then(|a| a.draft.as_mut())
                            {
                                d.hoist = !d.hoist;
                            }
                        }
                        cx.notify();
                    })),
                )
                .child(text(
                    tr!(
                        "権限（自分が持つ権限のみ変更可能）",
                        "Permissions (you can only change permissions you have)",
                        "权限（只能更改自己拥有的权限）"
                    ),
                    13.0,
                    MUTED(),
                ))
                .child(text(
                    tr!(
                        "管理者を有効にすると、チャンネルの権限設定をすべて無視します。",
                        "Enabling Administrator ignores all channel permission settings.",
                        "启用管理员后，将忽略所有频道权限设置。"
                    ),
                    12.0,
                    MUTED(),
                ));
            for &(index, label) in PERMISSIONS {
                let bit = 1u128 << index;
                let enabled = editable && !busy && self.app.can_toggle_role_permission(bit);
                let on = draft.permissions & bit != 0;
                form = form.child(
                    div()
                        .id(("role-perm", index as usize))
                        .min_h(z(28.0))
                        .px(z(6.0))
                        .rounded(z(4.0))
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                        .when(!enabled, |d| d.opacity(0.5))
                        .child(text(if on { "☑" } else { "☐" }, 15.0, TEXT()))
                        .child(text(label.s(), 14.0, TEXT()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.app.toggle_role_permission(bit);
                            }
                            cx.notify();
                        })),
                );
            }
            content = content.child(form).child(
                div()
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(
                        text_button("role_cancel", tr!("キャンセル", "Cancel", "取消")).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(a) = this.app.role_admin.as_mut() {
                                    if a.mutation.is_none() {
                                        a.draft = None;
                                    }
                                }
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        fill_button(
                            "role_save",
                            if busy {
                                tr!("保存中…", "Saving…", "正在保存…")
                            } else {
                                tr!("保存", "Save", "保存")
                            },
                            BLURPLE(),
                            WHITE(),
                            editable && !busy,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.save_role();
                            cx.notify();
                        })),
                    ),
            );
        } else if let Some(roles) = &a.roles {
            let mut ordered = roles.clone();
            ordered.sort_by(|x, y| y.position.cmp(&x.position).then_with(|| x.id.cmp(&y.id)));
            let mut list = div()
                .id("role-list-scroll")
                .flex_1()
                .min_h_0()
                .max_h(z(height))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(z(3.0));
            for (i, role) in ordered.iter().enumerate() {
                let editable = !busy && self.app.can_manage_role(role);
                let name = role.name.clone();
                let rid = role.id.clone();
                let mut row = div()
                    .min_h(z(38.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(z(4.0))
                    .px(z(4.0))
                    .border_b_1()
                    .border_color(DIVIDER())
                    .child(line(name, 14.0, TEXT()).flex_1().min_w(z(90.0)));
                if role.managed != Some(false) {
                    row = row.child(text(
                        tr!(
                            "連携 / 管理不可",
                            "Integration / unmanageable",
                            "集成 / 无法管理"
                        ),
                        11.0,
                        MUTED(),
                    ));
                }
                if editable {
                    let id = rid.clone();
                    row = row.child(
                        text_button(format!("role-edit-{id}"), tr!("編集", "Edit", "编辑"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.app.edit_role(Some(&id));
                                cx.notify();
                            })),
                    );
                    if role.id != a.guild_id {
                        for (label, other) in [
                            ("↑", i.checked_sub(1).and_then(|n| ordered.get(n))),
                            ("↓", ordered.get(i + 1)),
                        ] {
                            if let Some(target) = other.filter(|r| {
                                r.id != a.guild_id
                                    && r.position != role.position
                                    && self.app.can_manage_role(r)
                            }) {
                                let id = rid.clone();
                                let target_id = target.id.clone();
                                row = row.child(
                                    text_button(format!("role-move-{label}-{id}"), label).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.app.reorder_role(&id, &target_id);
                                            cx.notify();
                                        }),
                                    ),
                                );
                            }
                        }
                        let id = rid.clone();
                        let confirm = a.confirm_delete.as_deref() == Some(id.as_str());
                        row = row.child(
                            text_button(
                                format!("role-delete-{id}"),
                                if confirm {
                                    tr!("削除を確定", "Confirm delete", "确认删除")
                                } else {
                                    tr!("削除", "Delete", "删除")
                                },
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this
                                        .app
                                        .role_admin
                                        .as_ref()
                                        .and_then(|a| a.confirm_delete.as_deref())
                                        == Some(id.as_str())
                                    {
                                        this.app.delete_role(&id);
                                    } else if let Some(a) = this.app.role_admin.as_mut() {
                                        a.confirm_delete = Some(id.clone());
                                    }
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
                list = list.child(row);
            }
            content = content.child(list).child(
                fill_button(
                    "role_create",
                    tr!("ロールを作成", "Create Role", "创建身份组"),
                    BLURPLE(),
                    WHITE(),
                    allowed && !busy,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.edit_role(None);
                    cx.notify();
                })),
            );
        }
        Some(self.role_admin_frame(content, embedded, cx))
    }

    fn role_admin_frame(&self, content: Div, embedded: bool, cx: &mut Context<Self>) -> AnyElement {
        if embedded {
            return content.into_any_element();
        }
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if this
                .app
                .role_admin
                .as_ref()
                .is_some_and(|a| a.mutation.is_none())
            {
                this.app.role_admin = None;
            }
            cx.notify();
        });
        modal("role-admin-modal", content, close, 170).into_any_element()
    }
}
