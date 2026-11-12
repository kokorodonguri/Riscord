//! スラッシュコマンド: 「/」で出す候補と、選んだコマンドの引数の欄（入力欄の代わりに出す）
use gpui::assets::IconName;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::prelude::*;
use gpui::*;

use super::emoji::AcKey;
use super::*;
use crate::app::{
    filter_commands, leaf_options, needs_sub, AppCommand, CommandChoice, CommandOption,
    OPT_ATTACHMENT, OPT_BOOLEAN, OPT_CHANNEL, OPT_MENTIONABLE, OPT_ROLE, OPT_USER,
};

/// 候補の数
const SUGGEST_MAX: usize = 8;

/// 引数の種類の名前
fn kind_label(k: u8) -> &'static str {
    match k {
        3 => "文字",
        4 => "整数",
        5 => "はい/いいえ",
        6 => "ユーザー",
        7 => "チャンネル",
        8 => "ロール",
        9 => "ユーザー・ロール",
        10 => "数",
        11 => "添付ファイル",
        _ => "",
    }
}

impl Root {
    /// 入力欄が「/名前」なら、合うコマンド
    fn command_hits(&mut self) -> Option<Vec<AppCommand>> {
        let q = self.app.command_query()?.to_string();
        self.app.ensure_commands();
        let all = self.app.available_commands();
        Some(
            filter_commands(&all, &q, SUGGEST_MAX)
                .into_iter()
                .cloned()
                .collect(),
        )
    }

    /// ↑↓・Enter・Tab・Esc。候補を出していれば true（入力欄には渡さない）
    pub fn command_ac_key(
        &mut self,
        key: AcKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.app.command_query().is_none() {
            // 引数の欄で Esc ならコマンドをやめる
            if key == AcKey::Dismiss && self.app.cmd_draft.is_some() {
                self.app.cancel_command();
                cx.stop_propagation();
                cx.notify();
                return true;
            }
            return false;
        }
        let Some(hits) = self.command_hits().filter(|h| !h.is_empty()) else {
            return false;
        };
        cx.stop_propagation();
        let n = hits.len();
        let sel = self.cmd_ac_sel.min(n - 1);
        match key {
            AcKey::Up => self.cmd_ac_sel = (sel + n - 1) % n,
            AcKey::Down => self.cmd_ac_sel = (sel + 1) % n,
            AcKey::Confirm => self.pick_command(hits[sel].clone(), window, cx),
            AcKey::Dismiss => self.app.input.clear(),
        }
        cx.notify();
        true
    }

    fn pick_command(&mut self, c: AppCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.app.start_command(c);
        self.cmd_ac_sel = 0;
        self.cmd_inputs_key = String::new();
        self.ensure_cmd_inputs(window, cx);
        // 最初の引数の欄へ
        let first = self.app.cmd_draft.as_ref().and_then(|d| {
            leaf_options(&d.command, &d.path)
                .iter()
                .find(|o| !o.is_sub())
                .map(|o| o.name.clone())
        });
        if let Some(e) = first.and_then(|n| self.cmd_inputs.get(&n)) {
            e.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    /// 入力欄の上に出すコマンドの候補
    pub fn command_suggest(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let q = self.app.command_query()?.to_string();
        let hits = self.command_hits()?;
        let loading = self.app.command_index.values().any(|i| i.loading);
        let failed = self
            .app
            .command_index
            .values()
            .any(|i| i.retry_at.is_some());
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
            format!("「/{q}」に一致するコマンド"),
            12.0,
            MUTED(),
        )));
        if hits.is_empty() {
            return Some(col.child(div().px(z(6.0)).py(z(6.0)).flex().items_center().gap(z(8.0)).when(loading, |d| d.child(spinner(14.0))).child(text(
                if loading {
                    "コマンドを読み込み中…"
                } else if failed {
                    "コマンドを読み込めませんでした。しばらくしてから「/」を入力し直してください"
                } else {
                    "使えるコマンドがありません"
                },
                13.0,
                MUTED(),
            ))));
        }
        let sel = self.cmd_ac_sel.min(hits.len() - 1);
        for (i, c) in hits.into_iter().enumerate() {
            let (name, desc, app) = (c.name.clone(), c.description.clone(), c.app_name.clone());
            col = col.child(
                div()
                    .id(("cmd_ac", i))
                    .min_h(z(36.0))
                    .px(z(6.0))
                    .py(z(4.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .cursor_pointer()
                    .when(i == sel, |d| d.bg(SELECTED()))
                    .hover(|s| s.bg(SELECTED()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                line(format!("/{name}"), 14.0, WHITE())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(line(desc, 12.0, MUTED())),
                    )
                    .child(line(app, 12.0, FAINT()).flex_none().max_w(z(140.0)))
                    // 押しても入力欄のフォーカスを外さない
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_command(c.clone(), window, cx);
                        cx.notify();
                    })),
            );
        }
        Some(col)
    }

    /// 引数の欄を、今のコマンド・サブコマンドに合わせて作る
    pub fn ensure_cmd_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.app.cmd_draft.as_ref() else {
            if !self.cmd_inputs.is_empty() {
                self.cmd_inputs.clear();
                self.cmd_subs.clear();
                self.cmd_inputs_key.clear();
            }
            return;
        };
        let key = format!("{}/{}", d.command.id, d.path.join("/"));
        if self.cmd_inputs_key == key {
            return;
        }
        self.cmd_inputs_key = key;
        self.cmd_inputs.clear();
        self.cmd_subs.clear();
        let opts: Vec<CommandOption> = leaf_options(&d.command, &d.path)
            .iter()
            .filter(|o| !o.is_sub() && o.kind != OPT_BOOLEAN && o.kind != OPT_ATTACHMENT)
            .cloned()
            .collect();
        for o in opts {
            let placeholder = if o.description.is_empty() {
                kind_label(o.kind).to_string()
            } else {
                o.description.clone()
            };
            let e = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            let name = o.name.clone();
            self.cmd_subs.push(cx.subscribe_in(
                &e,
                window,
                move |this: &mut Root, state, ev: &InputEvent, _, cx| match ev {
                    InputEvent::Change => {
                        let v = state.read(cx).value().to_string();
                        this.app.set_command_value(&name, v, None);
                        cx.notify();
                    }
                    InputEvent::Focus => {
                        if let Some(d) = this.app.cmd_draft.as_mut() {
                            d.focus = Some(name.clone());
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        this.app.run_command();
                        cx.notify();
                    }
                    _ => {}
                },
            ));
            self.cmd_inputs.insert(o.name.clone(), e);
        }
    }

    /// 候補（選択肢・アプリへの問い合わせ・メンバー・チャンネル・ロール）
    fn option_candidates(&self, o: &CommandOption, typed: &str) -> Vec<CommandChoice> {
        let a = &self.app;
        let t = typed.trim().to_lowercase();
        let hit = |s: &str| t.is_empty() || s.to_lowercase().contains(&t);
        let mut out: Vec<CommandChoice> = Vec::new();
        if let Some((name, list)) = a.cmd_draft.as_ref().and_then(|d| d.suggestions.as_ref()) {
            if name == &o.name && o.autocomplete {
                return list.clone();
            }
        }
        if !o.choices.is_empty() {
            return o
                .choices
                .iter()
                .filter(|c| hit(&c.name))
                .take(SUGGEST_MAX)
                .cloned()
                .collect();
        }
        let gid = a.sel_guild.as_deref();
        if matches!(o.kind, OPT_USER | OPT_MENTIONABLE) {
            let members = gid
                .and_then(|g| a.members.get(g))
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    m.user
                        .as_ref()
                        .map(|u| (m.display().to_string(), u.id.clone()))
                });
            let dms = a
                .current_channel()
                .filter(|_| gid.is_none())
                .into_iter()
                .flat_map(|c| {
                    c.recipients
                        .iter()
                        .map(|u| (u.display().to_string(), u.id.clone()))
                });
            out.extend(
                members
                    .chain(dms)
                    .filter(|(n, _)| hit(n))
                    .take(SUGGEST_MAX)
                    .map(|(n, id)| CommandChoice {
                        name: format!("@{n}"),
                        value: id.into(),
                    }),
            );
        }
        if matches!(o.kind, OPT_ROLE | OPT_MENTIONABLE) {
            out.extend(
                gid.and_then(|g| a.roles.get(g))
                    .into_iter()
                    .flatten()
                    .filter(|r| hit(&r.name))
                    .take(SUGGEST_MAX)
                    .map(|r| CommandChoice {
                        name: format!("@{}", r.name),
                        value: r.id.clone().into(),
                    }),
            );
        }
        if o.kind == OPT_CHANNEL {
            let ok = |k: u8| {
                if o.channel_types.is_empty() {
                    k != crate::model::Channel::CATEGORY
                } else {
                    o.channel_types.contains(&k)
                }
            };
            out.extend(
                gid.and_then(|g| a.channels.get(g))
                    .into_iter()
                    .flatten()
                    .filter(|c| ok(c.kind) && c.name.as_deref().is_some_and(hit))
                    .take(SUGGEST_MAX)
                    .map(|c| CommandChoice {
                        name: format!("#{}", c.name.clone().unwrap_or_default()),
                        value: c.id.clone().into(),
                    }),
            );
        }
        out.truncate(SUGGEST_MAX);
        out
    }

    /// 入力欄の代わりに出す、コマンドの引数の欄
    pub fn command_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.ensure_cmd_inputs(window, cx);
        let Some(d) = self.app.cmd_draft.as_ref() else {
            return div();
        };
        let c = &d.command;
        let title = std::iter::once(c.name.as_str())
            .chain(d.path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let mut boxed = card(INPUT(), DIVIDER(), 8.0)
            .part(Part::ChatInput)
            .p(z(10.0))
            .flex()
            .flex_col()
            .gap(z(8.0));
        boxed = boxed.child(
            div()
                .flex()
                .items_center()
                .gap(z(8.0))
                .child(line(format!("/{title}"), 15.0, WHITE()).font_weight(FontWeight::MEDIUM))
                .child(line(c.app_name.clone(), 12.0, FAINT()).flex_1().min_w_0())
                .child(
                    icon_button("cmd_cancel", IconName::CircleX, 22.0)
                        .tooltip(tip("やめる（Esc）"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.cancel_command();
                            cx.notify();
                        })),
                ),
        );
        if !c.description.is_empty() && d.path.is_empty() {
            boxed = boxed.child(text(c.description.clone(), 13.0, MUTED()));
        }
        // サブコマンドを選ぶ
        if needs_sub(c, &d.path) {
            let mut chips = div().flex().flex_wrap().gap(z(6.0));
            for o in leaf_options(c, &d.path).iter().filter(|o| o.is_sub()) {
                let name = o.name.clone();
                chips = chips.child(
                    div()
                        .id(SharedString::from(format!("cmd_sub{}", o.name)))
                        .px(z(10.0))
                        .py(z(4.0))
                        .rounded(z(6.0))
                        .bg(WIDGET())
                        .cursor_pointer()
                        .hover(|s| s.bg(WIDGET_HOVER()))
                        .tooltip(tip(o.description.clone()))
                        .child(text(o.name.clone(), 13.0, TEXT()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(d) = this.app.cmd_draft.as_mut() {
                                d.path.push(name.clone());
                                d.values.clear();
                                d.error = None;
                            }
                            this.ensure_cmd_inputs(window, cx);
                            cx.notify();
                        })),
                );
            }
            boxed = boxed
                .child(text("サブコマンドを選んでください", 12.0, MUTED()))
                .child(chips);
        } else {
            let opts: Vec<CommandOption> = leaf_options(c, &d.path).to_vec();
            let focus = d.focus.clone();
            for o in &opts {
                let (typed, chosen) = d.values.get(&o.name).cloned().unwrap_or_default();
                let label = div()
                    .flex()
                    .items_center()
                    .gap(z(4.0))
                    .child(text(o.name.clone(), 13.0, WHITE()).font_weight(FontWeight::MEDIUM))
                    .when(o.required, |d| d.child(text("*", 13.0, RED())))
                    .child(text(kind_label(o.kind), 11.0, FAINT()));
                let editor: AnyElement = match o.kind {
                    OPT_BOOLEAN => {
                        let mut row = div().flex().gap(z(6.0));
                        for (v, l) in [(true, "はい"), (false, "いいえ")] {
                            let on = chosen.as_ref().and_then(|x| x.as_bool()) == Some(v);
                            let name = o.name.clone();
                            row = row.child(
                                div()
                                    .id(SharedString::from(format!("cmd_bool{}{v}", o.name)))
                                    .px(z(12.0))
                                    .py(z(3.0))
                                    .rounded(z(6.0))
                                    .bg(if on { BLURPLE() } else { WIDGET() })
                                    .cursor_pointer()
                                    .child(text(l, 13.0, if on { WHITE() } else { TEXT() }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.app.set_command_value(
                                            &name,
                                            l.to_string(),
                                            Some(v.into()),
                                        );
                                        cx.notify();
                                    })),
                            );
                        }
                        row.into_any_element()
                    }
                    OPT_ATTACHMENT => {
                        text("添付ファイルの引数にはまだ対応していません", 12.0, ORANGE())
                            .into_any_element()
                    }
                    _ => match self.cmd_inputs.get(&o.name) {
                        Some(e) => div()
                            .bg(CHAT())
                            .border_1()
                            .border_color(DIVIDER())
                            .rounded(z(6.0))
                            .px(z(6.0))
                            .child(Input::new(e).appearance(false).h(z(30.0)))
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                };
                let mut row = div()
                    .flex()
                    .flex_col()
                    .gap(z(4.0))
                    .child(label)
                    .child(editor);
                // 選んでいる欄の候補
                if focus.as_deref() == Some(o.name.as_str()) && o.kind != OPT_BOOLEAN {
                    let cands =
                        self.option_candidates(o, if chosen.is_some() { "" } else { &typed });
                    if !cands.is_empty() {
                        let mut chips = div().flex().flex_wrap().gap(z(4.0));
                        for (i, ch) in cands.into_iter().enumerate() {
                            let (name, input) =
                                (o.name.clone(), self.cmd_inputs.get(&o.name).cloned());
                            let on = chosen.as_ref() == Some(&ch.value);
                            chips = chips.child(
                                div()
                                    .id(SharedString::from(format!("cmd_cand{}{i}", o.name)))
                                    .px(z(8.0))
                                    .py(z(2.0))
                                    .rounded(z(6.0))
                                    .bg(if on { BLURPLE() } else { WIDGET() })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(WIDGET_HOVER()))
                                    .child(text(
                                        ch.name.clone(),
                                        12.0,
                                        if on { WHITE() } else { TEXT() },
                                    ))
                                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.app.set_command_value(
                                            &name,
                                            ch.name.clone(),
                                            Some(ch.value.clone()),
                                        );
                                        if let Some(e) = &input {
                                            let label = ch.name.clone();
                                            e.update(cx, |s, cx| s.set_value(label, window, cx));
                                        }
                                        cx.notify();
                                    })),
                            );
                        }
                        row = row.child(chips);
                    }
                }
                boxed = boxed.child(row);
            }
        }
        if let Some(e) = &d.error {
            boxed = boxed.child(text(e.clone(), 13.0, RED()));
        }
        let blocked = needs_sub(c, &d.path);
        boxed = boxed.child(
            div()
                .flex()
                .justify_end()
                .gap(z(8.0))
                .child(text("Enter で実行・Esc でやめる", 12.0, FAINT()).flex_1())
                .child(
                    fill_button("cmd_run", "実行", BLURPLE(), WHITE(), !blocked).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.run_command();
                            cx.notify();
                        }),
                    ),
                ),
        );
        boxed
    }
}
