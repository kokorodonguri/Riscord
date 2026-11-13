//! このサーバーでの自分のプロフィール（ニックネーム・自己紹介・代名詞）の編集。
use super::*;

const ADMINISTRATOR: u64 = 1 << 3;
const CHANGE_NICKNAME: u64 = 1 << 26;
const MANAGE_NICKNAMES: u64 = 1 << 27;
pub const NICK_MAX: usize = 32;
pub const BIO_MAX: usize = 190;
pub const PRONOUNS_MAX: usize = 40;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuildProfileValues {
    pub nick: String,
    pub bio: String,
    pub pronouns: String,
}

#[derive(Clone, Debug, Default)]
pub struct GuildProfileEdit {
    pub guild_id: String,
    /// 読み込んだ今の値（届くまでは None。その間は編集させない）
    pub original: Option<GuildProfileValues>,
    pub draft: GuildProfileValues,
    pub load: Option<u64>,
    /// 送信中（ニックネームとプロフィールは別の API。true = ニックネーム）
    pub pending: Vec<(u64, bool)>,
    pub error: Option<String>,
}

fn text(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// 変わった項目だけの (ニックネームの本体, プロフィールの本体)
pub fn profile_bodies(
    original: &GuildProfileValues,
    draft: &GuildProfileValues,
    nick_allowed: bool,
) -> Result<(Option<serde_json::Value>, Option<serde_json::Value>), &'static str> {
    let nick = draft.nick.trim();
    if nick.chars().count() > NICK_MAX {
        return Err(tr!(
            "ニックネームは 32 文字までです",
            "Nicknames can be up to 32 characters",
            "昵称最多 32 个字符"
        ));
    }
    if draft.bio.chars().count() > BIO_MAX {
        return Err(tr!(
            "自己紹介は 190 文字までです",
            "About Me can be up to 190 characters",
            "个人简介最多 190 个字符"
        ));
    }
    if draft.pronouns.trim().chars().count() > PRONOUNS_MAX {
        return Err(tr!(
            "代名詞は 40 文字までです",
            "Pronouns can be up to 40 characters",
            "代词最多 40 个字符"
        ));
    }
    let nick_body = (nick != original.nick.trim()).then(|| {
        // 空にするとニックネームを外す
        serde_json::json!({ "nick": if nick.is_empty() { serde_json::Value::Null } else { nick.into() } })
    });
    if nick_body.is_some() && !nick_allowed {
        return Err(tr!(
            "このサーバーでニックネームを変える権限がありません",
            "You don't have permission to change your nickname in this server",
            "你没有在此服务器中更改昵称的权限"
        ));
    }
    let mut profile = serde_json::Map::new();
    if draft.bio != original.bio {
        profile.insert("bio".into(), draft.bio.clone().into());
    }
    if draft.pronouns.trim() != original.pronouns.trim() {
        profile.insert("pronouns".into(), draft.pronouns.trim().into());
    }
    let profile_body = (!profile.is_empty()).then_some(serde_json::Value::Object(profile));
    if nick_body.is_none() && profile_body.is_none() {
        return Err(tr!("変更がありません", "No changes", "没有更改"));
    }
    Ok((nick_body, profile_body))
}

impl App {
    /// ニックネームを変えられるか（分からないときは出しておき、Discord の判定に任せる）
    pub fn can_change_nick(&self, guild_id: &str) -> bool {
        self.guild_perms(guild_id)
            .is_none_or(|p| p & (CHANGE_NICKNAME | MANAGE_NICKNAMES | ADMINISTRATOR) != 0)
    }

    pub fn open_guild_profile(&mut self, guild_id: &str) {
        let Some(me) = self.me.as_ref().map(|u| u.id.clone()) else {
            return;
        };
        if !self.guilds.iter().any(|g| g.id == guild_id) {
            return;
        }
        let load = self.role_queue(net::ApiCall::GetProfile {
            user_id: me,
            guild_id: Some(guild_id.to_owned()),
        });
        self.guild_profile = Some(GuildProfileEdit {
            guild_id: guild_id.to_owned(),
            error: load.is_none().then(|| {
                tr!(
                    "読み込めませんでした。再接続してください",
                    "Couldn't load. Please reconnect",
                    "无法加载。请重新连接"
                )
                .into()
            }),
            load,
            ..Default::default()
        });
    }

    pub fn close_guild_profile(&mut self) {
        if self
            .guild_profile
            .as_ref()
            .is_some_and(|e| e.pending.is_empty())
        {
            self.guild_profile = None;
        }
    }

    pub fn save_guild_profile(&mut self) {
        let Some(e) = self.guild_profile.as_ref() else {
            return;
        };
        let Some(original) = e.original.clone() else {
            return;
        };
        if !e.pending.is_empty() {
            return;
        }
        let gid = e.guild_id.clone();
        let (nick, profile) = match profile_bodies(&original, &e.draft, self.can_change_nick(&gid))
        {
            Ok(b) => b,
            Err(msg) => {
                self.guild_profile.as_mut().unwrap().error = Some(msg.into());
                return;
            }
        };
        let mut pending = Vec::new();
        let mut failed = false;
        if let Some(body) = nick {
            match self.role_queue(net::ApiCall::ModifyMyMember {
                guild_id: gid.clone(),
                body,
            }) {
                Some(id) => pending.push((id, true)),
                None => failed = true,
            }
        }
        if let Some(body) = profile {
            match self.role_queue(net::ApiCall::ModifyMyGuildProfile {
                guild_id: gid,
                body,
            }) {
                Some(id) => pending.push((id, false)),
                None => failed = true,
            }
        }
        let e = self.guild_profile.as_mut().unwrap();
        e.pending = pending;
        e.error = failed.then(|| {
            tr!(
                "送信できませんでした。再接続してください",
                "Couldn't send. Please reconnect",
                "无法发送。请重新连接"
            )
            .into()
        });
    }

    pub fn is_guild_profile_request(&self, id: u64) -> bool {
        self.guild_profile
            .as_ref()
            .is_some_and(|e| e.load == Some(id) || e.pending.iter().any(|(n, _)| *n == id))
    }

    pub fn guild_profile_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        let Some(e) = self.guild_profile.as_mut() else {
            return;
        };
        if e.load == Some(id) {
            e.load = None;
            match result {
                Ok(v) => {
                    let values = GuildProfileValues {
                        nick: text(&v["guild_member"]["nick"]),
                        bio: text(&v["guild_member_profile"]["bio"]),
                        pronouns: text(&v["guild_member_profile"]["pronouns"]),
                    };
                    e.draft = values.clone();
                    e.original = Some(values);
                }
                Err(err) => {
                    e.error = Some(super::pins::api_error_text(
                        &err,
                        tr!("読み込めませんでした", "Couldn't load", "无法加载"),
                    ))
                }
            }
            return;
        }
        let Some(i) = e.pending.iter().position(|(n, _)| *n == id) else {
            return;
        };
        let (_, nick) = e.pending.remove(i);
        match result {
            // 通った分は「今の値」に写す（もう一度保存しても送らない）
            Ok(_) => {
                if let Some(o) = e.original.as_mut() {
                    if nick {
                        o.nick = e.draft.nick.trim().to_owned();
                    } else {
                        o.bio = e.draft.bio.clone();
                        o.pronouns = e.draft.pronouns.trim().to_owned();
                    }
                }
            }
            Err(err) => {
                e.error = Some(super::pins::api_error_text(
                    &err,
                    tr!("保存できませんでした", "Couldn't save", "无法保存"),
                ))
            }
        }
        // 失敗したときは下書きを残し、直してもう一度保存できるようにする
        if e.pending.is_empty() && e.error.is_none() {
            let gid = e.guild_id.clone();
            self.guild_profile = None;
            // 自分のプロフィールの控えを捨て、次に開いたときに読み直す
            if let Some(me) = self.me.as_ref() {
                self.profiles.remove(&net::profile_key(&me.id, Some(&gid)));
            }
            self.show_toast(tr!(
                "サーバープロフィールを保存しました",
                "Server profile saved",
                "已保存服务器个人资料"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(nick: &str, bio: &str, pronouns: &str) -> GuildProfileValues {
        GuildProfileValues {
            nick: nick.into(),
            bio: bio.into(),
            pronouns: pronouns.into(),
        }
    }

    #[test]
    fn bodies_contain_only_changes_and_respect_limits() {
        let o = values("old", "hi", "they");
        let (n, p) = profile_bodies(&o, &values("new", "hi", "they"), true).unwrap();
        assert_eq!(n.unwrap()["nick"], "new");
        assert!(p.is_none());
        let (n, p) = profile_bodies(&o, &values("old", "hello", "they"), true).unwrap();
        assert!(n.is_none());
        assert_eq!(p.unwrap(), serde_json::json!({"bio": "hello"}));
        // 空にするとニックネームを外す
        let (n, _) = profile_bodies(&o, &values("  ", "hi", "they"), true).unwrap();
        assert!(n.unwrap()["nick"].is_null());
        assert!(profile_bodies(&o, &o, true).is_err());
        assert!(profile_bodies(&o, &values(&"a".repeat(33), "hi", "they"), true).is_err());
        assert!(profile_bodies(&o, &values("old", &"a".repeat(191), "they"), true).is_err());
        assert!(profile_bodies(&o, &values("new", "hi", "they"), false).is_err());
    }

    #[test]
    fn loads_then_saves_and_keeps_draft_on_failure() {
        let mut app = App {
            me: serde_json::from_value(serde_json::json!({"id": "10", "username": "me"})).ok(),
            guilds: vec![
                serde_json::from_value(serde_json::json!({"id": "1", "name": "g"})).unwrap(),
            ],
            ..Default::default()
        };
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.open_guild_profile("1");
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        app.guild_profile_result(
            id,
            Ok(serde_json::json!({
                "guild_member": {"nick": "n"},
                "guild_member_profile": {"bio": "b", "pronouns": null}
            })),
        );
        let e = app.guild_profile.as_mut().unwrap();
        assert_eq!(e.original, Some(values("n", "b", "")));
        e.draft.nick = "m".into();
        e.draft.pronouns = "he".into();
        app.save_guild_profile();
        let mut sent = Vec::new();
        while let Ok(Cmd::Api { id, call }) = rx.try_recv() {
            sent.push((id, call));
        }
        assert_eq!(sent.len(), 2);
        // ニックネームは通り、プロフィールは断られた
        app.guild_profile_result(sent[0].0, Ok(serde_json::Value::Null));
        assert!(app.guild_profile.as_ref().unwrap().pending.len() == 1);
        app.guild_profile_result(
            sent[1].0,
            Err("PATCH /guilds/1/profile/@me: 429 Too Many {}".into()),
        );
        let e = app.guild_profile.as_ref().unwrap();
        assert!(e.error.as_deref().unwrap().contains("多すぎ"));
        assert_eq!(e.draft, values("m", "b", "he"), "下書きは残す");
        assert_eq!(
            e.original,
            Some(values("m", "b", "")),
            "通ったニックネームは今の値に"
        );
        // もう一度保存すると、プロフィールだけ送る
        app.save_guild_profile();
        let Cmd::Api { call, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert!(matches!(call, net::ApiCall::ModifyMyGuildProfile { .. }));
        assert!(rx.try_recv().is_err());
    }
}
