//! チャンネルごとのロール・メンバー権限上書き。
use super::role_admin::{parse_admin_member, RoleAdmin};
use super::*;

const ADMINISTRATOR: u128 = 1 << 3;
const VIEW_CHANNEL: u128 = 1 << 10;
const MANAGE_ROLES: u128 = 1 << 28;
pub const CHANNEL_PERMISSION_BITS: &[u32] = &[
    0, 4, 6, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20, 21, 22, 23, 24, 25, 28, 29, 31, 32, 33, 34,
    35, 36, 37, 38, 39, 42, 45, 46, 48, 49, 50, 51, 52,
];

fn supported_mask() -> u128 {
    CHANNEL_PERMISSION_BITS
        .iter()
        .fold(0, |mask, bit| mask | (1u128 << bit))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelOverwrite {
    pub id: String,
    pub kind: u8,
    pub allow: u128,
    pub deny: u128,
}

#[derive(Clone, Debug)]
pub struct ChannelSnapshot {
    pub id: String,
    pub name: String,
    pub kind: u8,
    pub parent_id: Option<String>,
    pub overwrites: Vec<ChannelOverwrite>,
}

pub struct OverwriteDraft {
    pub id: String,
    pub kind: u8,
    pub original: Option<ChannelOverwrite>,
    pub allow: u128,
    pub deny: u128,
    pub member_verified_revision: Option<u64>,
}

pub struct ChannelPermissionView {
    pub channel_id: String,
    pub snapshot: Option<ChannelSnapshot>,
    pub parent: Option<ChannelSnapshot>,
    pub channel_request: Option<(u64, u64)>,
    pub parent_request: Option<(u64, u64, String)>,
    pub member_request: Option<(u64, u64, String)>,
    pub draft: Option<OverwriteDraft>,
    pub member_id: String,
    pub confirm_delete: bool,
}

impl ChannelPermissionView {
    fn new(channel_id: String) -> Self {
        Self {
            channel_id,
            snapshot: None,
            parent: None,
            channel_request: None,
            parent_request: None,
            member_request: None,
            draft: None,
            member_id: String::new(),
            confirm_delete: false,
        }
    }

    pub fn is_request(&self, id: u64) -> bool {
        self.channel_request.is_some_and(|(n, _)| n == id)
            || self
                .parent_request
                .as_ref()
                .is_some_and(|(n, _, _)| *n == id)
            || self
                .member_request
                .as_ref()
                .is_some_and(|(n, _, _)| *n == id)
    }

    pub fn affected_channel(&self, id: &str) -> bool {
        self.channel_id == id
            || self.snapshot.as_ref().and_then(|s| s.parent_id.as_deref()) == Some(id)
            || self.parent.as_ref().is_some_and(|p| p.id == id)
            || self
                .parent_request
                .as_ref()
                .is_some_and(|(_, _, parent)| parent == id)
    }

    pub fn affected_member(&self, id: &str) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|d| d.kind == 1 && d.id == id)
            || self
                .member_request
                .as_ref()
                .is_some_and(|(_, _, member)| member == id)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && id
            .parse::<u64>()
            .is_ok_and(|n| n > 0 && n.to_string() == id)
}

fn parse_channel(
    value: &serde_json::Value,
    guild_id: &str,
    expected_id: &str,
) -> Option<ChannelSnapshot> {
    if value.get("id")?.as_str()? != expected_id || value.get("guild_id")?.as_str()? != guild_id {
        return None;
    }
    let kind = u8::try_from(value.get("type")?.as_u64()?).ok()?;
    if !matches!(kind, 0 | 2 | 4 | 5 | 13 | 15 | 16) {
        return None;
    }
    let parent_id = match value.get("parent_id") {
        Some(serde_json::Value::Null) => None,
        Some(value) => {
            let id = value.as_str()?;
            if !valid_id(id) || id == expected_id {
                return None;
            }
            Some(id.to_owned())
        }
        None if kind == 4 => None,
        None => return None,
    };
    if kind == 4 && parent_id.is_some() {
        return None;
    }
    let values = value.get("permission_overwrites")?.as_array()?;
    let mut ids = std::collections::HashSet::new();
    let mut overwrites = Vec::with_capacity(values.len());
    for value in values {
        let id = value.get("id")?.as_str()?;
        let kind = u8::try_from(value.get("type")?.as_u64()?).ok()?;
        if !valid_id(id) || !matches!(kind, 0 | 1) || !ids.insert(id.to_owned()) {
            return None;
        }
        let allow = value.get("allow")?.as_str()?.parse().ok()?;
        let deny = value.get("deny")?.as_str()?.parse().ok()?;
        overwrites.push(ChannelOverwrite {
            id: id.to_owned(),
            kind,
            allow,
            deny,
        });
    }
    Some(ChannelSnapshot {
        id: expected_id.to_owned(),
        name: value
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_owned(),
        kind,
        parent_id,
        overwrites,
    })
}

fn apply_overwrites(
    base: u128,
    snapshot: &ChannelSnapshot,
    guild_id: &str,
    mine: &[String],
    me: &str,
) -> u128 {
    if base & ADMINISTRATOR != 0 {
        return u128::MAX;
    }
    let mut result = base;
    if let Some(o) = snapshot
        .overwrites
        .iter()
        .find(|o| o.kind == 0 && o.id == guild_id)
    {
        result = (result & !o.deny) | o.allow;
    }
    let (mut deny, mut allow) = (0u128, 0u128);
    for o in snapshot
        .overwrites
        .iter()
        .filter(|o| o.kind == 0 && o.id != guild_id && mine.contains(&o.id))
    {
        deny |= o.deny;
        allow |= o.allow;
    }
    result = (result & !deny) | allow;
    if let Some(o) = snapshot
        .overwrites
        .iter()
        .find(|o| o.kind == 1 && o.id == me)
    {
        result = (result & !o.deny) | o.allow;
    }
    result
}

impl App {
    /// 権限画面への入口を出すか。権限がまだ分からないときは出しておき、開いた先で確かめる
    pub fn may_edit_channel_permissions(&self, guild_id: &str, channel_id: &str) -> bool {
        self.channel_perms(Some(guild_id), channel_id)
            .is_none_or(|p| u128::from(p) & (MANAGE_ROLES | ADMINISTRATOR) != 0)
    }

    pub fn open_channel_permissions(&mut self, guild_id: &str, channel_id: &str) {
        let Some(channel) = self.find_guild_channel(guild_id, channel_id) else {
            return;
        };
        if !valid_id(guild_id)
            || !valid_id(channel_id)
            || !matches!(channel.kind, 0 | 2 | 4 | 5 | 13 | 15 | 16)
        {
            return;
        }
        let mut admin = RoleAdmin::new(guild_id.to_owned());
        admin.channel_permissions = Some(ChannelPermissionView::new(channel_id.to_owned()));
        self.role_admin = Some(admin);
        self.reload_role_admin();
    }

    pub(super) fn reload_channel_permissions(&mut self) {
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(view) = admin.channel_permissions.as_ref() else {
            return;
        };
        if view.channel_request.is_some() || view.parent_request.is_some() {
            return;
        }
        let (channel_id, revision) = (view.channel_id.clone(), admin.revision);
        let queued = self.role_queue(net::ApiCall::GetChannel(channel_id));
        if let Some(admin) = self.role_admin.as_mut() {
            if let Some(view) = admin.channel_permissions.as_mut() {
                view.snapshot = None;
                view.parent = None;
                view.channel_request = queued.map(|id| (id, revision));
            }
            if queued.is_none() {
                admin.error = Some(
                    tr!(
                        "チャンネル権限を取得できませんでした",
                        "Couldn't load channel permissions"
                    )
                    .into(),
                );
            }
        }
    }

    fn channel_context(&self) -> Option<(u128, u128, Option<i64>, bool)> {
        let admin = self.role_admin.as_ref()?;
        let view = admin.channel_permissions.as_ref()?;
        if admin.mutation.is_some()
            || view.channel_request.is_some()
            || view.parent_request.is_some()
            || view.member_request.is_some()
        {
            return None;
        }
        let channel = view.snapshot.as_ref()?;
        let (base, highest, owner) = self.role_authority()?;
        let mine = admin.my_roles.as_ref()?;
        let me = self.me.as_ref()?;
        let effective = apply_overwrites(base, channel, &admin.guild_id, mine, &me.id);
        if effective & (MANAGE_ROLES | VIEW_CHANNEL) != (MANAGE_ROLES | VIEW_CHANNEL) {
            return None;
        }
        let parent_permissions = match &channel.parent_id {
            Some(id) => {
                let parent = view
                    .parent
                    .as_ref()
                    .filter(|p| &p.id == id && p.kind == 4)?;
                apply_overwrites(base, parent, &admin.guild_id, mine, &me.id)
            }
            None => base,
        };
        let admin_user = owner || base & ADMINISTRATOR != 0;
        let explicit_manage = channel.overwrites.iter().any(|o| {
            o.allow & MANAGE_ROLES != 0
                && ((o.kind == 0 && (o.id == admin.guild_id || mine.contains(&o.id)))
                    || (o.kind == 1 && o.id == me.id))
        });
        let mut editable = if admin_user || explicit_manage {
            u128::MAX
        } else {
            base | parent_permissions
        };
        if !admin_user {
            editable &= !MANAGE_ROLES;
        }
        Some((effective, editable, highest, owner))
    }

    pub fn can_manage_channel_permissions(&self) -> bool {
        self.channel_context().is_some()
    }

    fn valid_overwrite_target(&self, id: &str, kind: u8) -> bool {
        let Some(admin) = self.role_admin.as_ref() else {
            return false;
        };
        if !valid_id(id) {
            return false;
        }
        match kind {
            0 => {
                let Some((_, _, highest, owner)) = self.channel_context() else {
                    return false;
                };
                admin
                    .roles
                    .as_ref()
                    .and_then(|r| r.iter().find(|r| r.id == id))
                    .is_some_and(|r| {
                        r.managed == Some(false)
                            && (r.id == admin.guild_id
                                || owner
                                || highest.is_none_or(|h| r.position < h))
                    })
            }
            1 => admin
                .channel_permissions
                .as_ref()
                .and_then(|v| v.snapshot.as_ref())
                .is_some_and(|s| s.overwrites.iter().any(|o| o.id == id && o.kind == 1)),
            _ => false,
        }
    }

    pub fn can_select_channel_target(&self, id: &str, kind: u8) -> bool {
        self.valid_overwrite_target(id, kind)
    }

    pub fn edit_channel_overwrite(&mut self, id: &str, kind: u8) {
        if !self.valid_overwrite_target(id, kind) {
            return;
        }
        self.set_channel_draft(id, kind, None);
    }

    fn set_channel_draft(&mut self, id: &str, kind: u8, verified_revision: Option<u64>) {
        let Some(view) = self
            .role_admin
            .as_mut()
            .and_then(|a| a.channel_permissions.as_mut())
        else {
            return;
        };
        let Some(snapshot) = view.snapshot.as_ref() else {
            return;
        };
        let original = snapshot.overwrites.iter().find(|o| o.id == id).cloned();
        if original.as_ref().is_some_and(|o| o.kind != kind) {
            return;
        }
        view.draft = Some(OverwriteDraft {
            id: id.into(),
            kind,
            allow: original.as_ref().map_or(0, |o| o.allow),
            deny: original.as_ref().map_or(0, |o| o.deny),
            original,
            member_verified_revision: verified_revision,
        });
        view.confirm_delete = false;
    }

    pub fn verify_channel_member(&mut self) {
        if !self.can_manage_channel_permissions() {
            return;
        }
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(view) = admin.channel_permissions.as_ref() else {
            return;
        };
        let id = view.member_id.trim().to_owned();
        if !valid_id(&id) {
            self.role_admin.as_mut().unwrap().error = Some(
                tr!(
                    "メンバー ID は数字で入力してください",
                    "Enter the member ID as a number"
                )
                .into(),
            );
            return;
        }
        if let Some(existing) = view
            .snapshot
            .as_ref()
            .and_then(|s| s.overwrites.iter().find(|o| o.id == id))
        {
            if existing.kind == 1 {
                self.edit_channel_overwrite(&id, 1);
            } else {
                self.role_admin.as_mut().unwrap().error = Some(
                    tr!(
                        "その ID はロールに使用されています",
                        "That ID is used by a role"
                    )
                    .into(),
                );
            }
            return;
        }
        let (guild_id, revision) = (admin.guild_id.clone(), admin.revision);
        let queued = self.role_queue(net::ApiCall::GetMember {
            guild_id,
            user_id: id.clone(),
        });
        if let Some(admin) = self.role_admin.as_mut() {
            if let Some(view) = admin.channel_permissions.as_mut() {
                view.member_request = queued.map(|n| (n, revision, id));
            }
            if queued.is_none() {
                admin.error = Some(
                    tr!(
                        "メンバーを確認できませんでした",
                        "Couldn't verify the member"
                    )
                    .into(),
                );
            }
        }
    }

    pub fn can_toggle_channel_permission(&self, bit: u128) -> bool {
        if bit.count_ones() != 1 || !CHANNEL_PERMISSION_BITS.contains(&bit.trailing_zeros()) {
            return false;
        }
        let Some((_, editable, _, _)) = self.channel_context() else {
            return false;
        };
        editable & bit != 0 && self.channel_draft_editable()
    }

    pub fn channel_draft_editable(&self) -> bool {
        if self.channel_context().is_none() {
            return false;
        }
        let Some(view) = self
            .role_admin
            .as_ref()
            .and_then(|a| a.channel_permissions.as_ref())
        else {
            return false;
        };
        let (Some(snapshot), Some(draft)) = (&view.snapshot, &view.draft) else {
            return false;
        };
        let now = snapshot.overwrites.iter().find(|o| o.id == draft.id);
        let member_verified = draft.kind != 1
            || draft.original.is_some()
            || draft.member_verified_revision == self.role_admin.as_ref().map(|a| a.revision);
        now == draft.original.as_ref()
            && valid_id(&draft.id)
            && draft
                .original
                .as_ref()
                .is_none_or(|original| original.kind == draft.kind)
            && member_verified
            && (draft.kind == 1 || self.valid_overwrite_target(&draft.id, draft.kind))
    }

    pub fn set_channel_permission(&mut self, bit: u128, state: u8) {
        if state > 2 || !self.can_toggle_channel_permission(bit) {
            return;
        }
        let Some(draft) = self
            .role_admin
            .as_mut()
            .and_then(|a| a.channel_permissions.as_mut())
            .and_then(|v| v.draft.as_mut())
        else {
            return;
        };
        draft.allow &= !bit;
        draft.deny &= !bit;
        if state == 1 {
            draft.allow |= bit;
        }
        if state == 2 {
            draft.deny |= bit;
        }
    }

    pub fn can_delete_channel_overwrite(&self) -> bool {
        if !self.channel_draft_editable() {
            return false;
        }
        let Some((_, editable, _, _)) = self.channel_context() else {
            return false;
        };
        self.role_admin
            .as_ref()
            .and_then(|a| a.channel_permissions.as_ref())
            .and_then(|view| view.draft.as_ref())
            .and_then(|draft| draft.original.as_ref())
            .is_some_and(|original| (original.allow | original.deny) & !editable == 0)
    }

    pub fn save_channel_overwrite(&mut self) {
        if !self.channel_draft_editable() {
            return;
        }
        let Some((_, editable, _, _)) = self.channel_context() else {
            return;
        };
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(view) = admin.channel_permissions.as_ref() else {
            return;
        };
        let Some(draft) = view.draft.as_ref() else {
            return;
        };
        let (old_allow, old_deny) = draft
            .original
            .as_ref()
            .map_or((0, 0), |o| (o.allow, o.deny));
        let changed = (old_allow ^ draft.allow) | (old_deny ^ draft.deny);
        if changed & !(editable & supported_mask()) != 0
            || (draft.allow & draft.deny) & changed != 0
        {
            return;
        }
        if draft.allow == old_allow && draft.deny == old_deny {
            self.show_toast(tr!("変更はありません", "No changes"));
            return;
        }
        let body = serde_json::json!({"type": draft.kind, "allow":draft.allow.to_string(), "deny":draft.deny.to_string()});
        self.submit_role_action(net::ApiCall::EditPermissions {
            channel_id: view.channel_id.clone(),
            overwrite_id: draft.id.clone(),
            body,
        });
    }

    pub fn delete_channel_overwrite(&mut self) {
        if !self.channel_draft_editable() {
            return;
        }
        let Some((_, editable, _, _)) = self.channel_context() else {
            return;
        };
        let Some(view) = self
            .role_admin
            .as_ref()
            .and_then(|a| a.channel_permissions.as_ref())
        else {
            return;
        };
        let Some(draft) = view.draft.as_ref() else {
            return;
        };
        let Some(original) = draft.original.as_ref() else {
            return;
        };
        if (original.allow | original.deny) & !editable != 0 {
            self.role_admin.as_mut().unwrap().error =
                Some(tr!("自分が変更できない権限を含むため、上書きを削除できません", "You can't delete this overwrite because it includes permissions you can't change").into());
            return;
        }
        self.submit_role_action(net::ApiCall::DeletePermission {
            channel_id: view.channel_id.clone(),
            overwrite_id: draft.id.clone(),
        });
    }

    pub(super) fn channel_permission_result(
        &mut self,
        id: u64,
        result: Result<serde_json::Value, String>,
    ) {
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(view) = admin.channel_permissions.as_ref() else {
            return;
        };
        let guild_id = admin.guild_id.clone();
        let revision = admin.revision;
        if view.channel_request.is_some_and(|(n, _)| n == id) {
            let old_revision = self
                .role_admin
                .as_mut()
                .unwrap()
                .channel_permissions
                .as_mut()
                .unwrap()
                .channel_request
                .take()
                .unwrap()
                .1;
            if old_revision != revision {
                self.reload_role_admin();
                return;
            }
            let expected = self
                .role_admin
                .as_ref()
                .unwrap()
                .channel_permissions
                .as_ref()
                .unwrap()
                .channel_id
                .clone();
            let value = match result {
                Ok(value) => value,
                Err(e) => {
                    self.role_admin.as_mut().unwrap().error = Some(super::pins::api_error_text(
                        &e,
                        tr!(
                            "チャンネル権限を取得できませんでした",
                            "Couldn't load channel permissions"
                        ),
                    ));
                    return;
                }
            };
            let parsed = parse_channel(&value, &guild_id, &expected);
            let Some(snapshot) = parsed else {
                self.role_admin.as_mut().unwrap().error = Some(
                    tr!(
                        "チャンネル権限を読み取れませんでした",
                        "Couldn't read channel permissions"
                    )
                    .into(),
                );
                return;
            };
            let parent = snapshot.parent_id.clone();
            self.role_admin
                .as_mut()
                .unwrap()
                .channel_permissions
                .as_mut()
                .unwrap()
                .snapshot = Some(snapshot);
            if let Some(parent_id) = parent {
                let queued = self.role_queue(net::ApiCall::GetChannel(parent_id.clone()));
                let admin = self.role_admin.as_mut().unwrap();
                admin.channel_permissions.as_mut().unwrap().parent_request =
                    queued.map(|n| (n, revision, parent_id));
                if queued.is_none() {
                    admin.error = Some(
                        tr!(
                            "親カテゴリの権限を確認できませんでした",
                            "Couldn't check the parent category's permissions"
                        )
                        .into(),
                    );
                }
            }
        } else if view
            .parent_request
            .as_ref()
            .is_some_and(|(n, _, _)| *n == id)
        {
            let (_, old_revision, parent_id) = self
                .role_admin
                .as_mut()
                .unwrap()
                .channel_permissions
                .as_mut()
                .unwrap()
                .parent_request
                .take()
                .unwrap();
            if old_revision != revision {
                self.reload_role_admin();
                return;
            }
            let value = match result {
                Ok(value) => value,
                Err(e) => {
                    self.role_admin.as_mut().unwrap().error = Some(super::pins::api_error_text(
                        &e,
                        tr!(
                            "親カテゴリの権限を取得できませんでした",
                            "Couldn't load the parent category's permissions"
                        ),
                    ));
                    return;
                }
            };
            let parsed = parse_channel(&value, &guild_id, &parent_id).filter(|s| s.kind == 4);
            let admin = self.role_admin.as_mut().unwrap();
            if parsed.is_none() {
                admin.error = Some(
                    tr!(
                        "親カテゴリの権限を確認できませんでした",
                        "Couldn't check the parent category's permissions"
                    )
                    .into(),
                );
            }
            admin.channel_permissions.as_mut().unwrap().parent = parsed;
        } else if view
            .member_request
            .as_ref()
            .is_some_and(|(n, _, _)| *n == id)
        {
            let (_, old_revision, user_id) = self
                .role_admin
                .as_mut()
                .unwrap()
                .channel_permissions
                .as_mut()
                .unwrap()
                .member_request
                .take()
                .unwrap();
            if old_revision != revision {
                self.reload_role_admin();
                return;
            }
            let value = match result {
                Ok(value) => value,
                Err(e) => {
                    self.role_admin.as_mut().unwrap().error = Some(super::pins::api_error_text(
                        &e,
                        tr!(
                            "メンバーを確認できませんでした",
                            "Couldn't verify the member"
                        ),
                    ));
                    return;
                }
            };
            if parse_admin_member(value, &user_id).is_some()
                && self.can_manage_channel_permissions()
            {
                self.set_channel_draft(&user_id, 1, Some(revision));
            } else {
                self.role_admin.as_mut().unwrap().error = Some(
                    tr!(
                        "サーバーのメンバーを確認できませんでした",
                        "Couldn't verify server members"
                    )
                    .into(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    #[test]
    fn permission_entry_is_hidden_only_when_manage_roles_is_known_missing() {
        let mut app = App::default();
        app.me =
            Some(serde_json::from_value(serde_json::json!({"id":"10","username":"me"})).unwrap());
        app.guilds = vec![serde_json::from_value(
            serde_json::json!({"id":"1","name":"guild","permissions":"1024"}),
        )
        .unwrap()];
        app.channels.insert(
            "1".into(),
            vec![serde_json::from_value(serde_json::json!({"id":"20","type":0})).unwrap()],
        );
        // 自分のロールが分からないうちは出しておく
        assert!(app.may_edit_channel_permissions("1", "20"));
        app.my_roles.insert("1".into(), Vec::new());
        assert!(!app.may_edit_channel_permissions("1", "20"));
        app.guilds[0].permissions = Some((MANAGE_ROLES | VIEW_CHANNEL).to_string());
        assert!(app.may_edit_channel_permissions("1", "20"));
        app.guilds[0].permissions = Some(ADMINISTRATOR.to_string());
        assert!(app.may_edit_channel_permissions("1", "20"));
    }

    fn role(id: &str, position: i64, permissions: u128, managed: bool) -> serde_json::Value {
        serde_json::json!({"id":id,"name":format!("role-{id}"),"position":position,
            "permissions":permissions.to_string(),"color":0,"hoist":false,"managed":managed})
    }

    fn channel(
        id: &str,
        kind: u8,
        parent: Option<&str>,
        overwrites: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({"id":id,"guild_id":"1","type":kind,"name":"test","parent_id":parent,"permission_overwrites":overwrites})
    }

    fn fixture(
        child: serde_json::Value,
        parent: serde_json::Value,
    ) -> (App, UnboundedReceiver<Cmd>) {
        let mut app = App::default();
        app.me =
            Some(serde_json::from_value(serde_json::json!({"id":"10","username":"me"})).unwrap());
        app.guilds =
            vec![serde_json::from_value(serde_json::json!({"id":"1","name":"guild"})).unwrap()];
        app.channels.insert(
            "1".into(),
            vec![
                serde_json::from_value(serde_json::json!({"id":"20","type":0,"parent_id":"30"}))
                    .unwrap(),
            ],
        );
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.open_channel_permissions("1", "20");
        for _ in 0..5 {
            let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
                panic!("expected API")
            };
            let value = match &call {
                net::ApiCall::GetRoles(_) => serde_json::json!([
                    role("1", 0, 0, false),
                    role("2", 10, MANAGE_ROLES | VIEW_CHANNEL | (1 << 11), false),
                    role("3", 2, 0, false),
                    role("4", 12, 0, false),
                    role("5", 1, 0, true)
                ]),
                net::ApiCall::GetMember { .. } => {
                    serde_json::json!({"user":{"id":"10","username":"me"},"roles":["2"]})
                }
                net::ApiCall::GetGuild(_) => serde_json::json!({"id":"1","owner_id":"99"}),
                net::ApiCall::GetChannel(channel_id) if channel_id == "20" => child.clone(),
                net::ApiCall::GetChannel(channel_id) if channel_id == "30" => parent.clone(),
                _ => panic!("unexpected call"),
            };
            app.on_api_result(id, call, Ok(value));
        }
        (app, rx)
    }

    fn standard(high: u128) -> (serde_json::Value, serde_json::Value) {
        (
            channel(
                "20",
                0,
                Some("30"),
                serde_json::json!([
                    {"id":"3","type":0,"allow":high.to_string(),"deny":"0"}
                ]),
            ),
            channel("30", 4, None, serde_json::json!([])),
        )
    }

    #[test]
    fn overwrites_follow_discord_precedence() {
        let value = channel(
            "20",
            0,
            None,
            serde_json::json!([
                {"id":"1","type":0,"allow":"0","deny":"1024"},
                {"id":"2","type":0,"allow":"1024","deny":"2048"},
                {"id":"3","type":0,"allow":"2048","deny":"0"},
                {"id":"10","type":1,"allow":"0","deny":"1024"}
            ]),
        );
        let parsed = parse_channel(&value, "1", "20").unwrap();
        let p = apply_overwrites(
            VIEW_CHANNEL | (1 << 11),
            &parsed,
            "1",
            &["2".into(), "3".into()],
            "10",
        );
        assert_eq!(p & VIEW_CHANNEL, 0);
        assert_ne!(p & (1 << 11), 0);
        assert_eq!(
            apply_overwrites(ADMINISTRATOR, &parsed, "1", &[], "10"),
            u128::MAX
        );
    }

    #[test]
    fn editing_preserves_unknown_bits_and_restricts_hierarchy() {
        let (child, parent) = standard(1u128 << 100);
        let (mut app, mut rx) = fixture(child, parent);
        assert!(app.can_manage_channel_permissions());
        assert!(!app.can_select_channel_target("4", 0));
        assert!(!app.can_select_channel_target("5", 0));
        app.edit_channel_overwrite("3", 0);
        assert!(!app.can_toggle_channel_permission(1u128 << 100));
        app.set_channel_permission(1 << 11, 1);
        app.set_channel_permission(1u128 << 100, 2);
        app.save_channel_overwrite();
        let Cmd::Api {
            call:
                net::ApiCall::EditPermissions {
                    channel_id,
                    overwrite_id,
                    body,
                },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("expected overwrite")
        };
        assert_eq!((channel_id.as_str(), overwrite_id.as_str()), ("20", "3"));
        assert_eq!(
            body,
            serde_json::json!({"type":0,"allow":((1u128 << 100) | (1 << 11)).to_string(),"deny":"0"})
        );
    }

    #[test]
    fn editing_is_denied_when_channel_removes_manage_permission() {
        let (mut child, parent) = standard(0);
        child["permission_overwrites"] =
            serde_json::json!([{"id":"1","type":0,"allow":"0","deny":MANAGE_ROLES.to_string()}]);
        let (mut app, mut rx) = fixture(child, parent);
        assert!(!app.can_manage_channel_permissions());
        app.edit_channel_overwrite("3", 0);
        app.save_channel_overwrite();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn channel_events_invalidate_drafts_before_save() {
        let (child, parent) = standard(0);
        let (mut app, mut rx) = fixture(child, parent);
        app.edit_channel_overwrite("3", 0);
        app.set_channel_permission(1 << 11, 1);
        app.on_gateway_event(
            "CHANNEL_UPDATE",
            serde_json::json!({"id":"30","guild_id":"1"}),
        );
        app.save_channel_overwrite();
        let calls: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(calls.len(), 4);
        assert!(calls.iter().all(|c| matches!(
            c,
            Cmd::Api {
                call: net::ApiCall::GetRoles(_)
                    | net::ApiCall::GetMember { .. }
                    | net::ApiCall::GetGuild(_)
                    | net::ApiCall::GetChannel(_),
                ..
            }
        )));
        assert!(!app.channel_draft_editable());
    }

    #[test]
    fn new_member_must_be_verified_and_server_events_invalidate_it() {
        let (child, parent) = standard(0);
        let (mut app, mut rx) = fixture(child, parent);
        app.role_admin
            .as_mut()
            .unwrap()
            .channel_permissions
            .as_mut()
            .unwrap()
            .member_id = "11".into();
        app.verify_channel_member();
        let Cmd::Api {
            id,
            call: call @ net::ApiCall::GetMember { .. },
        } = rx.try_recv().unwrap()
        else {
            panic!("expected verification")
        };
        app.on_api_result(
            id,
            call,
            Ok(serde_json::json!({"user":{"id":"11","username":"target"},"roles":[]})),
        );
        assert!(app.channel_draft_editable());
        app.on_gateway_event(
            "GUILD_MEMBER_REMOVE",
            serde_json::json!({"guild_id":"1","user":{"id":"11"}}),
        );
        assert!(!app.channel_draft_editable());
        app.save_channel_overwrite();
        let calls: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(calls.len(), 4);
    }

    #[test]
    fn a_member_response_for_a_different_user_is_rejected() {
        let (child, parent) = standard(0);
        let (mut app, mut rx) = fixture(child, parent);
        app.role_admin
            .as_mut()
            .unwrap()
            .channel_permissions
            .as_mut()
            .unwrap()
            .member_id = "11".into();
        app.verify_channel_member();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!("expected verification")
        };
        app.on_api_result(
            id,
            call,
            Ok(serde_json::json!({"user":{"id":"12","username":"other"},"roles":[]})),
        );
        let admin = app.role_admin.as_ref().unwrap();
        assert!(admin.channel_permissions.as_ref().unwrap().draft.is_none());
        assert!(admin.error.is_some());
    }

    #[test]
    fn missing_or_invalid_parent_never_enables_editing() {
        let (child, _) = standard(0);
        let invalid_parent = channel("30", 4, None, serde_json::json!([]));
        let mut invalid_parent = invalid_parent;
        invalid_parent["guild_id"] = "2".into();
        let (mut app, mut rx) = fixture(child, invalid_parent);
        assert!(!app.can_manage_channel_permissions());
        app.edit_channel_overwrite("3", 0);
        assert!(rx.try_recv().is_err());
        assert!(app.role_admin.as_ref().unwrap().error.is_some());
    }

    #[test]
    fn administrator_still_cannot_target_a_higher_role() {
        let (child, parent) = standard(0);
        let (mut app, _rx) = fixture(child, parent);
        app.role_admin
            .as_mut()
            .unwrap()
            .roles
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r.id == "2")
            .unwrap()
            .permissions = ADMINISTRATOR.to_string();
        assert!(app.can_manage_channel_permissions());
        assert!(!app.can_select_channel_target("4", 0));
    }

    #[test]
    fn unsupported_bits_cannot_be_changed_even_by_an_administrator() {
        let (child, parent) = standard(0);
        let (mut app, mut rx) = fixture(child, parent);
        app.role_admin
            .as_mut()
            .unwrap()
            .roles
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r.id == "2")
            .unwrap()
            .permissions = ADMINISTRATOR.to_string();
        app.edit_channel_overwrite("3", 0);
        assert!(!app.can_toggle_channel_permission(1 << 5));
        app.role_admin
            .as_mut()
            .unwrap()
            .channel_permissions
            .as_mut()
            .unwrap()
            .draft
            .as_mut()
            .unwrap()
            .allow |= 1 << 5;
        app.save_channel_overwrite();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn cannot_delete_overwrite_containing_permissions_actor_cannot_change() {
        let (child, parent) = standard(1u128 << 100);
        let (mut app, mut rx) = fixture(child, parent);
        app.edit_channel_overwrite("3", 0);
        assert!(!app.can_delete_channel_overwrite());
        app.delete_channel_overwrite();
        assert!(rx.try_recv().is_err());
        assert!(app.role_admin.as_ref().unwrap().error.is_some());
    }

    #[test]
    fn failure_keeps_channel_draft_and_success_requests_authoritative_refresh() {
        let (child, parent) = standard(0);
        let (mut app, mut rx) = fixture(child, parent);
        app.edit_channel_overwrite("3", 0);
        app.set_channel_permission(1 << 11, 2);
        app.save_channel_overwrite();
        let Cmd::Api {
            id,
            call: call @ net::ApiCall::EditPermissions { .. },
        } = rx.try_recv().unwrap()
        else {
            panic!("expected update")
        };
        app.on_api_result(
            id,
            call,
            Err("PUT /channels/20/permissions/3: 403 Forbidden".into()),
        );
        let admin = app.role_admin.as_ref().unwrap();
        assert!(admin.error.as_ref().unwrap().contains("権限"));
        assert_eq!(
            admin
                .channel_permissions
                .as_ref()
                .unwrap()
                .draft
                .as_ref()
                .unwrap()
                .deny,
            1 << 11
        );
        app.save_channel_overwrite();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!("expected retry")
        };
        app.on_api_result(id, call, Ok(serde_json::Value::Null));
        let cmds: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(cmds.len(), 5);
        assert!(cmds
            .iter()
            .any(|c| matches!(c, Cmd::LoadGuildChannels(gid) if gid == "1")));
        assert!(app
            .role_admin
            .as_ref()
            .unwrap()
            .channel_permissions
            .as_ref()
            .unwrap()
            .snapshot
            .is_none());
    }

    #[test]
    fn parser_rejects_unknown_large_or_cross_guild_overwrites() {
        let (child, _) = standard(0);
        assert!(parse_channel(&child, "2", "20").is_none());
        let mut invalid = child.clone();
        invalid["permission_overwrites"][0]["id"] = "../bad".into();
        assert!(parse_channel(&invalid, "1", "20").is_none());
        invalid = child.clone();
        invalid["permission_overwrites"][0]["allow"] =
            "340282366920938463463374607431768211456".into();
        assert!(parse_channel(&invalid, "1", "20").is_none());
        invalid = child.clone();
        invalid["permission_overwrites"][0]["type"] = 8.into();
        assert!(parse_channel(&invalid, "1", "20").is_none());
        invalid = child.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .remove("permission_overwrites");
        assert!(parse_channel(&invalid, "1", "20").is_none());
    }
}
