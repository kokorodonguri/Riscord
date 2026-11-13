//! ログイン画面（QR コード・メールアドレスとパスワード・二段階認証）
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::QrSession;
use crate::color::Color32;
use crate::login;

/// 入力欄（暗い箱）
fn login_input(state: &Entity<gpui::component::input::InputState>, busy: bool) -> Div {
    div()
        .w_full()
        .bg(RAIL())
        .border_1()
        .border_color(BORDER())
        .rounded(z(6.0))
        .px(z(4.0))
        .child(
            Input::new(state)
                .appearance(false)
                .disabled(busy)
                .h(z(40.0)),
        )
}

fn label(s: &'static str) -> Div {
    text(s, 12.0, MUTED()).mb(z(6.0))
}

impl Root {
    /// 送信（Enter かボタン）
    pub fn login_submit(&mut self, cx: &mut Context<Self>) {
        let a = &mut self.app;
        if a.login.busy.is_some() {
            return;
        }
        a.login_error = None;
        if let Some(ticket) = a.login.mfa_ticket.clone() {
            if !a.login.mfa_code.trim().is_empty() {
                a.login.busy = Some(login::mfa(
                    &a.waker,
                    ticket,
                    a.login.mfa_code.trim().to_string(),
                ));
            }
        } else {
            if !a.login.email.trim().is_empty() && !a.login.password.is_empty() {
                a.login.busy = Some(login::password(
                    &a.waker,
                    a.login.email.trim().to_string(),
                    a.login.password.clone(),
                ));
            }
        }
        cx.notify();
    }

    pub fn login_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = self.app.screen_w >= 760.0;
        // つなぎ始めたら（backend あり）ホーム画面を出すので、ここに来るのは未ログインのときだけ
        let card = if wide {
            div()
                .flex()
                .flex_row()
                .items_start()
                .child(
                    div()
                        .w(z(720.0 - 240.0 - 64.0))
                        .child(self.login_form(window, cx)),
                )
                .child(div().mx(z(32.0)).w(px(1.0)).h(z(300.0)).bg(DIVIDER()))
                .child(div().w(z(240.0)).child(self.login_qr(cx)))
        } else {
            div()
                .flex()
                .flex_col()
                .child(self.login_form(window, cx))
                .child(div().my(z(24.0)).child(hline(DIVIDER())))
                .child(self.login_qr(cx))
        };
        let width = if wide {
            z(720.0)
        } else {
            z((self.app.screen_w - 64.0).clamp(0.0, 420.0))
        };
        div().id("login").size_full().bg(RAIL()).overflow_y_scroll().child(
            div().w_full().min_h_full().flex().flex_col().items_center().justify_center().py(z(24.0)).child(
                div()
                    .bg(CHAT())
                    .rounded(z(8.0))
                    .p(z(32.0))
                    .w(width + z(64.0))
                    .flex()
                    .flex_col()
                    .child(card)
                    .child(
                        div()
                            .mt(z(20.0))
                            .text_center()
                            .child(text(tr!("Riscord は非公式のクライアントです。利用は Discord の利用規約に抵触する可能性があります。", "Riscord is an unofficial client. Using it may violate Discord's Terms of Service."), 12.0, FAINT())),
                    ),
            ),
        )
    }

    /// 左側: メールアドレスとパスワード（二段階認証のコード）
    fn login_form(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let a = &self.app;
        let busy = a.login.busy.is_some();
        let (title, sub) = if a.login.mfa_ticket.is_some() {
            (tr!("二段階認証", "Two-Factor Authentication"), tr!("認証アプリに表示されている6桁のコード、またはバックアップコードを入力してください。", "Enter the 6-digit code from your authenticator app, or a backup code."))
        } else {
            (
                tr!("おかえりなさい！", "Welcome back!"),
                tr!(
                    "Discord のアカウントでログインします。",
                    "Log in with your Discord account."
                ),
            )
        };
        let mut col = div().flex().flex_col().child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .text_center()
                .mb(z(20.0))
                .child(medium(title, 24.0, WHITE()))
                .child(text(sub, 15.0, MUTED()).mt(z(6.0))),
        );
        let remember = |label_text: &'static str, on: bool, cx: &mut Context<Self>| {
            div()
                .id("remember")
                .mt(z(8.0))
                .flex()
                .items_center()
                .gap(z(8.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.remember = !this.app.remember;
                    cx.notify();
                }))
                .child(
                    div()
                        .size(z(18.0))
                        .rounded(z(4.0))
                        .border_1()
                        .border_color(BORDER())
                        .when(on, |d| d.bg(BLURPLE()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(on, |d| {
                            d.child(icon(gpui::assets::IconName::Check, 14.0, Color32::WHITE))
                        }),
                )
                .child(text(label_text, 15.0, MUTED()))
        };
        let button = |label_text: &'static str, enabled: bool, cx: &mut Context<Self>| {
            fill_button("login_btn", label_text, BLURPLE(), Color32::WHITE, enabled)
                .w_full()
                .h(z(44.0))
                .when(enabled, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.login_submit(cx)))
                })
        };
        if a.login.mfa_ticket.is_some() {
            let ready = !busy && !a.login.mfa_code.trim().is_empty();
            col = col
                .child(label(tr!("認証コード", "Authentication code")))
                .child(login_input(&self.inp.mfa, busy))
                .child(div().mt(z(20.0)).child(button(
                    if busy {
                        tr!("確認中…", "Checking…")
                    } else {
                        tr!("ログイン", "Log In")
                    },
                    ready,
                    cx,
                )))
                .child(div().mt(z(8.0)).child(
                    link_text("back", tr!("最初に戻る", "Start over")).on_click(cx.listener(
                        |this, _, _, cx| {
                            let a = &mut this.app;
                            a.login.mfa_ticket = None;
                            a.login.mfa_code.clear();
                            a.login_error = None;
                            cx.notify();
                        },
                    )),
                ));
        } else {
            let ready = !busy && !a.login.email.trim().is_empty() && !a.login.password.is_empty();
            col = col
                .child(label(tr!(
                    "メールアドレスまたは電話番号",
                    "Email or phone number"
                )))
                .child(login_input(&self.inp.email, busy))
                .child(div().h(z(16.0)))
                .child(label(tr!("パスワード", "Password")))
                .child(login_input(&self.inp.password, busy))
                .child(remember(
                    tr!(
                        "このPCにログイン情報を保存する",
                        "Save login info on this PC"
                    ),
                    a.remember,
                    cx,
                ))
                .child(div().mt(z(16.0)).child(button(
                    if busy {
                        tr!("ログイン中…", "Logging in…")
                    } else {
                        tr!("ログイン", "Log In")
                    },
                    ready,
                    cx,
                )));
        }

        if let Some(e) = &a.login_error {
            col = col.child(text(e.clone(), 14.0, RED()).mt(z(10.0)));
        }
        col
    }

    /// 右側: QR コード（スマホの Discord アプリで読み取る）
    fn login_qr(&mut self, cx: &mut Context<Self>) -> Div {
        let qr = self.app.login.qr.as_ref();
        let qr_size = 176.0;
        let code: AnyElement = match qr {
            Some(QrSession {
                scanned: Some((name, avatar_url)),
                ended: None,
                ..
            }) => {
                // 読み取られたら、スマホでの承認を待つ間はそのアカウントを出す
                div()
                    .size(z(qr_size))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(avatar(avatar_url.as_deref(), 96.0, name, 48.0, BLURPLE()))
                    .into_any_element()
            }
            Some(QrSession {
                modules: Some((w, cells)),
                ended,
                ..
            }) => {
                let (w, cells, ended) = (*w, cells.clone(), ended.is_some());
                div()
                    .size(z(qr_size))
                    .relative()
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds: Bounds<Pixels>, _, window, _| {
                                window.paint_quad(fill(bounds, Hsla::white()).corner_radii(z(8.0)));
                                let quiet = z(8.0);
                                let cell = (bounds.size.width - quiet * 2.0) / w as f32;
                                for (i, &dark) in cells.iter().enumerate() {
                                    if dark {
                                        let (x, y) = ((i % w) as f32, (i / w) as f32);
                                        let o = bounds.origin
                                            + point(quiet + cell * x, quiet + cell * y);
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                o,
                                                gpui::size(cell + px(0.3), cell + px(0.3)),
                                            ),
                                            Hsla::black(),
                                        ));
                                    }
                                }
                            },
                        )
                        .size_full(),
                    )
                    // 真ん中にロゴ（誤り訂正の範囲に収まる大きさ）
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(z(36.0))
                                    .rounded(z(6.0))
                                    .bg(Color32::WHITE)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .size(z(30.0))
                                            .rounded_full()
                                            .bg(BLURPLE())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(logo(16.0, Color32::WHITE, BLURPLE())),
                                    ),
                            ),
                    )
                    .when(ended, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(z(8.0))
                                .bg(Color32::from_black_alpha(200)),
                        )
                    })
                    .into_any_element()
            }
            _ => div()
                .size(z(qr_size))
                .rounded(z(8.0))
                .bg(RAIL())
                .flex()
                .items_center()
                .justify_center()
                .when(qr.is_none_or(|q| q.ended.is_none()), |d| {
                    d.child(spinner(28.0))
                })
                .into_any_element(),
        };
        let scanned = qr
            .and_then(|q| q.scanned.as_ref())
            .filter(|_| qr.is_some_and(|q| q.ended.is_none()))
            .map(|s| s.0.clone());
        let ended = qr.and_then(|q| q.ended.clone());
        let restart = cx.listener(|this, _, _, cx| {
            this.app.login.qr = Some(QrSession::start(&this.app.waker));
            cx.notify();
        });
        let mut col = div()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .child(code)
            .child(div().h(z(20.0)));
        match (scanned, ended) {
            (_, Some(msg)) => {
                col = col
                    .child(medium(
                        tr!("QRコードでログイン", "Log in with QR Code"),
                        20.0,
                        WHITE(),
                    ))
                    .child(text(msg, 14.0, MUTED()).mt(z(6.0)))
                    .child(
                        small_button(
                            "qr_restart",
                            tr!("QR コードを作り直す", "Regenerate QR code"),
                        )
                        .mt(z(10.0))
                        .on_click(restart),
                    );
            }
            (Some(name), None) => {
                col = col
                    .child(medium(tr!("スマホで確認してください", "Check your phone"), 20.0, WHITE()))
                    .child(text(tr!("{name} としてログインします。スマホの Discord アプリで「はい、ログインします」を押してください。", "Logging in as {name}. Tap \"Yes, log me in\" in the Discord app on your phone.",), 14.0, MUTED()).mt(z(6.0)))
                    .child(div().mt(z(8.0)).child(link_text("qr_other", tr!("別のアカウントでログイン", "Log in with a different account")).on_click(restart)));
            }
            (None, None) => {
                col = col
                    .child(medium(tr!("QRコードでログイン", "Log in with QR Code"), 20.0, WHITE()))
                    .child(text(tr!("Discord モバイルアプリでスキャンして、すぐにログインできます。パスワードの入力はいりません。", "Scan this with the Discord mobile app to log in instantly. No password needed."), 14.0, MUTED()).mt(z(6.0)));
            }
        }
        col
    }
}
