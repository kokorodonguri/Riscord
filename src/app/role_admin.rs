//! サーバーロール管理。Discord が最終的な権限判定を行い、送信前にも検証する。
use super::*;

const MANAGE_ROLES: u128 = 1 << 28;
const MANAGE_GUILD: u128 = 1 << 5;
const ADMINISTRATOR: u128 = 1 << 3;

#[derive(Clone)]
pub struct RoleDraft {
    pub original: Option<Role>,
    pub name: String,
    pub permissions: u128,
    pub hoist: bool,
    pub color: u32,
}

pub struct RoleAdmin {
    pub guild_id: String,
    pub roles: Option<Vec<Role>>,
    pub my_roles: Option<Vec<String>>,
    pub roles_request: Option<(u64, u64)>,
    pub member_request: Option<(u64, u64)>,
    pub owner_id: Option<String>,
    pub owner_request: Option<(u64, u64)>,
    pub revision: u64,
    pub mutation: Option<u64>,
    pub draft: Option<RoleDraft>,
    pub confirm_delete: Option<String>,
    pub error: Option<String>,
    pub target: Option<MemberRoleTarget>,
    pub editing_guild: bool,
    pub guild_edit: Option<GuildEdit>,
    pub channel_permissions: Option<ChannelPermissionView>,
}

#[derive(Clone, PartialEq)]
pub struct GuildConfig {
    pub name: String,
    pub description: Option<Option<String>>,
    pub verification: Option<u64>,
    pub notifications: Option<u64>,
    pub content_filter: Option<u64>,
    pub afk_timeout: Option<u64>,
}

pub struct GuildEdit {
    pub original: GuildConfig,
    pub latest: Option<GuildConfig>,
    pub name: String,
    pub description: String,
    pub verification: Option<u64>,
    pub notifications: Option<u64>,
    pub content_filter: Option<u64>,
    pub afk_timeout: Option<u64>,
}

impl GuildEdit {
    fn new(config: GuildConfig) -> Self {
        Self {
            name: config.name.clone(),
            description: config
                .description
                .as_ref()
                .and_then(|s| s.clone())
                .unwrap_or_default(),
            verification: config.verification,
            notifications: config.notifications,
            content_filter: config.content_filter,
            afk_timeout: config.afk_timeout,
            latest: Some(config.clone()),
            original: config,
        }
    }
}

pub struct MemberRoleTarget {
    pub user_id: String,
    pub name: String,
    pub roles: Option<Vec<String>>,
    pub member_request: Option<(u64, u64)>,
}

impl RoleAdmin {
    pub(super) fn new(guild_id: String) -> Self {
        Self {
            guild_id,
            roles: None,
            my_roles: None,
            roles_request: None,
            member_request: None,
            owner_id: None,
            owner_request: None,
            revision: 0,
            mutation: None,
            draft: None,
            confirm_delete: None,
            error: None,
            target: None,
            editing_guild: false,
            guild_edit: None,
            channel_permissions: None,
        }
    }
}

fn parse_guild_config(value: &serde_json::Value) -> Option<GuildConfig> {
    let name = value.get("name")?.as_str()?.to_owned();
    if name.is_empty() {
        return None;
    }
    let description = match value.get("description") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(v.as_str()?.to_owned())),
    };
    let number = |field: &str| -> Option<Option<u64>> {
        match value.get(field) {
            None | Some(serde_json::Value::Null) => Some(None),
            Some(v) => Some(Some(v.as_u64()?)),
        }
    };
    Some(GuildConfig {
        name,
        description,
        verification: number("verification_level")?,
        notifications: number("default_message_notifications")?,
        content_filter: number("explicit_content_filter")?,
        afk_timeout: number("afk_timeout")?,
    })
}

fn parse_admin_roles(value: serde_json::Value, guild_id: &str) -> Option<Vec<Role>> {
    let values = value.as_array()?;
    if values.is_empty() {
        return None;
    }
    let mut ids = std::collections::HashSet::new();
    let mut roles = Vec::with_capacity(values.len());
    for item in values {
        let id = item.get("id")?.as_str()?;
        if id.is_empty()
            || !ids.insert(id.to_owned())
            || !item.get("managed")?.is_boolean()
            || !item.get("position")?.is_i64()
            || !item.get("permissions")?.is_string()
        {
            return None;
        }
        roles.push(serde_json::from_value::<Role>(item.clone()).ok()?);
    }
    roles.iter().any(|r| r.id == guild_id).then_some(roles)
}

pub(super) fn parse_admin_member(value: serde_json::Value, own_id: &str) -> Option<Member> {
    if !value
        .get("roles")?
        .as_array()?
        .iter()
        .all(|v| v.as_str().is_some())
    {
        return None;
    }
    if let Some(user) = value.get("user") {
        if user.get("id")?.as_str()? != own_id {
            return None;
        }
    }
    serde_json::from_value(value).ok()
}

impl App {
    pub(super) fn role_queue(&mut self, call: net::ApiCall) -> Option<u64> {
        self.next_api_id += 1;
        let id = self.next_api_id;
        self.backend
            .as_ref()
            .filter(|b| b.cmd.send(Cmd::Api { id, call }).is_ok())
            .map(|_| id)
    }

    pub fn open_role_admin(&mut self, guild_id: &str) {
        if !self.guilds.iter().any(|g| g.id == guild_id) {
            return;
        }
        self.role_admin = Some(RoleAdmin::new(guild_id.to_owned()));
        self.reload_role_admin();
    }

    pub fn open_member_role_admin(&mut self, guild_id: &str, user: &User) {
        if !self.guilds.iter().any(|g| g.id == guild_id) || user.id.is_empty() {
            return;
        }
        let mut admin = RoleAdmin::new(guild_id.to_owned());
        admin.target = Some(MemberRoleTarget {
            user_id: user.id.clone(),
            name: user.display().to_owned(),
            roles: None,
            member_request: None,
        });
        self.role_admin = Some(admin);
        self.reload_role_admin();
    }

    pub fn open_guild_editor(&mut self, guild_id: &str) {
        if !self.guilds.iter().any(|g| g.id == guild_id) {
            return;
        }
        let mut admin = RoleAdmin::new(guild_id.to_owned());
        admin.editing_guild = true;
        self.role_admin = Some(admin);
        self.reload_role_admin();
    }

    pub fn reload_role_admin(&mut self) {
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let guild = admin.guild_id.clone();
        let revision = admin.revision;
        if let Some(admin) = self.role_admin.as_mut() {
            admin.error = None;
        }
        let admin = self.role_admin.as_ref().unwrap();
        let need_roles = admin.roles_request.is_none();
        let need_member = admin.member_request.is_none();
        let need_owner = admin.owner_request.is_none();
        let target = admin
            .target
            .as_ref()
            .map(|target| (target.user_id.clone(), target.member_request.is_none()));
        let user_id = self.me.as_ref().map(|u| u.id.clone());
        if need_roles {
            let queued = self.role_queue(net::ApiCall::GetRoles(guild.clone()));
            if let Some(admin) = self.role_admin.as_mut() {
                admin.roles = None;
                admin.roles_request = queued.map(|id| (id, revision));
                if queued.is_none() {
                    admin.error = Some("接続できません。再接続してからお試しください".into());
                }
            }
        }
        if need_member {
            let queued = user_id.and_then(|user_id| {
                self.role_queue(net::ApiCall::GetMember {
                    guild_id: guild.clone(),
                    user_id,
                })
            });
            if let Some(admin) = self.role_admin.as_mut() {
                admin.my_roles = None;
                admin.member_request = queued.map(|id| (id, revision));
                if queued.is_none() {
                    admin.error = Some("自分の権限を確認できませんでした".into());
                }
            }
        }
        if let Some((user_id, need_target)) = target {
            if need_target {
                let queued = self.role_queue(net::ApiCall::GetMember {
                    guild_id: guild.clone(),
                    user_id,
                });
                if let Some(admin) = self.role_admin.as_mut() {
                    if let Some(target) = admin.target.as_mut() {
                        target.roles = None;
                        target.member_request = queued.map(|id| (id, revision));
                    }
                    if queued.is_none() {
                        admin.error = Some("対象メンバーを取得できませんでした".into());
                    }
                }
            }
        }
        if need_owner {
            let queued = self.role_queue(net::ApiCall::GetGuild(guild));
            if let Some(admin) = self.role_admin.as_mut() {
                admin.owner_id = None;
                if let Some(edit) = admin.guild_edit.as_mut() {
                    edit.latest = None;
                }
                admin.owner_request = queued.map(|id| (id, revision));
                if queued.is_none() {
                    admin.error = Some("サーバーの所有者を確認できませんでした".into());
                }
            }
        }
        self.reload_channel_permissions();
    }

    /// 権限と最高ロールは必ず読み直したデータで計算する。
    pub(super) fn role_authority(&self) -> Option<(u128, Option<i64>, bool)> {
        let admin = self.role_admin.as_ref()?;
        if admin.roles_request.is_some()
            || admin.member_request.is_some()
            || admin.owner_request.is_some()
        {
            return None;
        }
        let guild = self.guilds.iter().find(|g| g.id == admin.guild_id)?;
        let roles = admin.roles.as_ref()?;
        let mine = admin.my_roles.as_ref()?;
        let everyone = roles.iter().find(|r| r.id == guild.id)?;
        let mut permissions = everyone.permissions.parse::<u128>().ok()?;
        let mut highest = everyone.position;
        for id in mine {
            let role = roles.iter().find(|r| &r.id == id)?;
            permissions |= role.permissions.parse::<u128>().ok()?;
            highest = highest.max(role.position);
        }
        let owner = admin.owner_id.as_deref()? == self.me.as_ref()?.id;
        if owner || permissions & ADMINISTRATOR != 0 {
            permissions = u128::MAX;
        }
        Some((permissions, if owner { None } else { Some(highest) }, owner))
    }

    pub fn can_toggle_role_permission(&self, bit: u128) -> bool {
        self.role_authority()
            .is_some_and(|(allowed, _, _)| bit.count_ones() == 1 && allowed & bit != 0)
    }

    pub fn can_manage_roles(&self) -> bool {
        self.role_authority()
            .is_some_and(|(p, _, _)| p & MANAGE_ROLES != 0)
    }

    pub fn can_manage_guild(&self) -> bool {
        self.role_authority()
            .is_some_and(|(p, _, _)| p & MANAGE_GUILD != 0)
    }

    pub fn guild_draft_editable(&self) -> bool {
        self.can_manage_guild()
            && self.role_admin.as_ref().is_some_and(|admin| {
                admin.editing_guild
                    && admin.mutation.is_none()
                    && admin
                        .guild_edit
                        .as_ref()
                        .is_some_and(|d| d.latest.as_ref() == Some(&d.original))
            })
    }

    pub fn reset_guild_draft(&mut self) {
        if let Some(admin) = self.role_admin.as_mut() {
            if admin.mutation.is_none() {
                if let Some(current) = admin.guild_edit.as_ref().and_then(|d| d.latest.clone()) {
                    admin.guild_edit = Some(GuildEdit::new(current));
                    admin.error = None;
                }
            }
        }
    }

    pub fn save_guild_settings(&mut self) {
        if !self.guild_draft_editable() {
            return;
        }
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(draft) = admin.guild_edit.as_ref() else {
            return;
        };
        let name = draft.name.trim();
        if !(2..=100).contains(&name.chars().count()) || draft.description.chars().count() > 300 {
            self.role_admin.as_mut().unwrap().error =
                Some("サーバー名は2〜100文字、説明は300文字以内で入力してください".into());
            return;
        }
        let mut body = serde_json::Map::new();
        if name != draft.original.name {
            body.insert("name".into(), name.into());
        }
        if let Some(original) = &draft.original.description {
            if draft.description != original.as_deref().unwrap_or("") {
                body.insert(
                    "description".into(),
                    if draft.description.trim().is_empty() {
                        serde_json::Value::Null
                    } else {
                        draft.description.clone().into()
                    },
                );
            }
        }
        for (key, old, new, valid) in [
            (
                "verification_level",
                draft.original.verification,
                draft.verification,
                &[0, 1, 2, 3, 4][..],
            ),
            (
                "default_message_notifications",
                draft.original.notifications,
                draft.notifications,
                &[0, 1][..],
            ),
            (
                "explicit_content_filter",
                draft.original.content_filter,
                draft.content_filter,
                &[0, 1, 2][..],
            ),
            (
                "afk_timeout",
                draft.original.afk_timeout,
                draft.afk_timeout,
                &[60, 300, 900, 1800, 3600][..],
            ),
        ] {
            if old != new {
                let Some(value) = new.filter(|v| old.is_some() && valid.contains(v)) else {
                    self.role_admin.as_mut().unwrap().error =
                        Some("設定値が不正です。再読み込みしてください".into());
                    return;
                };
                body.insert(key.into(), value.into());
            }
        }
        if body.is_empty() {
            self.show_toast("変更はありません");
            return;
        }
        let guild_id = admin.guild_id.clone();
        self.submit_role_action(net::ApiCall::ModifyGuild {
            guild_id,
            body: body.into(),
        });
    }

    pub fn can_manage_role(&self, role: &Role) -> bool {
        let Some((permissions, highest, _)) = self.role_authority() else {
            return false;
        };
        let Some(admin) = self.role_admin.as_ref() else {
            return false;
        };
        if permissions & MANAGE_ROLES == 0 || role.managed != Some(false) {
            return false;
        }
        role.id == admin.guild_id || highest.is_none_or(|position| role.position < position)
    }

    pub fn can_assign_member_role(&self, role: &Role) -> bool {
        let Some(admin) = self.role_admin.as_ref() else {
            return false;
        };
        let Some(target) = admin.target.as_ref() else {
            return false;
        };
        if admin.mutation.is_some()
            || target.member_request.is_some()
            || role.id == admin.guild_id
            || !self.can_manage_role(role)
        {
            return false;
        }
        let (Some(owner_id), Some(target_roles), Some(roles), Some(me)) = (
            admin.owner_id.as_deref(),
            target.roles.as_ref(),
            admin.roles.as_ref(),
            self.me.as_ref(),
        ) else {
            return false;
        };
        if target.user_id == owner_id && me.id != owner_id {
            return false;
        }
        let Some((permissions, highest, owner)) = self.role_authority() else {
            return false;
        };
        if !target_roles.contains(&role.id) {
            let Ok(role_permissions) = role.permissions.parse::<u128>() else {
                return false;
            };
            if role_permissions & !permissions != 0 {
                return false;
            }
        }
        if owner || target.user_id == me.id {
            return true;
        }
        let Some(highest) = highest else {
            return false;
        };
        target_roles.iter().all(|id| {
            roles
                .iter()
                .find(|r| &r.id == id)
                .is_some_and(|r| r.position < highest)
        })
    }

    pub fn toggle_member_role(&mut self, role_id: &str) {
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let (Some(target), Some(role)) = (
            admin.target.as_ref(),
            admin
                .roles
                .as_ref()
                .and_then(|roles| roles.iter().find(|r| r.id == role_id)),
        ) else {
            return;
        };
        if !self.can_assign_member_role(role) {
            return;
        }
        let Some(target_roles) = target.roles.as_ref() else {
            return;
        };
        let guild_id = admin.guild_id.clone();
        let user_id = target.user_id.clone();
        let role_id = role_id.to_owned();
        let call = if target_roles.contains(&role_id) {
            net::ApiCall::RemoveMemberRole {
                guild_id,
                user_id,
                role_id,
            }
        } else {
            net::ApiCall::AddMemberRole {
                guild_id,
                user_id,
                role_id,
            }
        };
        self.submit_role_action(call);
    }

    pub fn edit_role(&mut self, role_id: Option<&str>) {
        if !self.can_manage_roles() {
            return;
        }
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        if admin.mutation.is_some() {
            return;
        }
        let original = if let Some(id) = role_id {
            let Some(role) = admin
                .roles
                .as_ref()
                .and_then(|r| r.iter().find(|r| r.id == id))
            else {
                return;
            };
            if !self.can_manage_role(role) {
                return;
            }
            Some(role.clone())
        } else {
            None
        };
        let Some(permissions) = original
            .as_ref()
            .map_or(Some(0), |r| r.permissions.parse::<u128>().ok())
        else {
            self.role_admin.as_mut().unwrap().error =
                Some("権限の値が大きすぎるか不正なため編集できません".into());
            return;
        };
        let draft = RoleDraft {
            name: original.as_ref().map_or(String::new(), |r| r.name.clone()),
            hoist: original.as_ref().is_some_and(|r| r.hoist),
            color: original.as_ref().map_or(0, |r| r.color),
            permissions,
            original,
        };
        let admin = self.role_admin.as_mut().unwrap();
        admin.error = None;
        admin.confirm_delete = None;
        admin.draft = Some(draft);
    }

    pub fn role_draft_editable(&self) -> bool {
        if !self.can_manage_roles() {
            return false;
        }
        self.role_admin
            .as_ref()
            .and_then(|a| a.draft.as_ref())
            .is_some_and(|d| {
                d.original
                    .as_ref()
                    .is_none_or(|r| self.can_manage_role(r) && self.current_role_matches(r))
            })
    }

    pub fn toggle_role_permission(&mut self, bit: u128) {
        if !self.role_draft_editable() {
            return;
        }
        let Some((allowed, _, _)) = self.role_authority() else {
            return;
        };
        if bit.count_ones() != 1 || bit & allowed == 0 {
            return;
        }
        if let Some(admin) = self.role_admin.as_mut() {
            if admin.mutation.is_none() {
                if let Some(draft) = admin.draft.as_mut() {
                    draft.permissions ^= bit;
                }
            }
        }
    }

    fn current_role_matches(&self, original: &Role) -> bool {
        self.role_admin
            .as_ref()
            .and_then(|a| a.roles.as_ref())
            .and_then(|roles| roles.iter().find(|r| r.id == original.id))
            .is_some_and(|r| {
                r.permissions == original.permissions
                    && r.name == original.name
                    && r.color == original.color
                    && r.hoist == original.hoist
                    && r.position == original.position
                    && r.managed == original.managed
            })
    }

    pub fn save_role(&mut self) {
        let Some((allowed, _, _)) = self.role_authority() else {
            return;
        };
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(draft) = admin.draft.as_ref() else {
            return;
        };
        if admin.mutation.is_some() || allowed & MANAGE_ROLES == 0 {
            return;
        }
        let name = draft.name.trim();
        if name.is_empty() || name.chars().count() > 100 || draft.color > 0xffffff {
            self.role_admin.as_mut().unwrap().error =
                Some("ロール名は1〜100文字で入力してください".into());
            return;
        }
        let old = draft
            .original
            .as_ref()
            .and_then(|r| r.permissions.parse::<u128>().ok())
            .unwrap_or(0);
        if (old ^ draft.permissions) & !allowed != 0 {
            self.role_admin.as_mut().unwrap().error =
                Some("自分が持たない権限は変更できません".into());
            return;
        }
        let guild_id = admin.guild_id.clone();
        let call = if let Some(role) = &draft.original {
            if !self.can_manage_role(role) || !self.current_role_matches(role) {
                self.role_admin.as_mut().unwrap().error =
                    Some("ロールが更新されました。一覧を再読み込みしてください".into());
                return;
            }
            let mut body = serde_json::Map::new();
            if role.id != guild_id && name != role.name {
                body.insert("name".into(), name.into());
            }
            if draft.permissions != old {
                body.insert("permissions".into(), draft.permissions.to_string().into());
            }
            if role.id != guild_id && draft.color != role.color {
                body.insert("color".into(), draft.color.into());
            }
            if role.id != guild_id && draft.hoist != role.hoist {
                body.insert("hoist".into(), draft.hoist.into());
            }
            if body.is_empty() {
                self.role_admin.as_mut().unwrap().draft = None;
                return;
            }
            net::ApiCall::ModifyRole {
                guild_id,
                role_id: role.id.clone(),
                body: body.into(),
            }
        } else {
            net::ApiCall::CreateRole {
                guild_id,
                body: serde_json::json!({"name":name,"permissions":draft.permissions.to_string(),"color":draft.color,"hoist":draft.hoist}),
            }
        };
        self.submit_role_action(call);
    }

    pub fn delete_role(&mut self, id: &str) {
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(role) = admin
            .roles
            .as_ref()
            .and_then(|r| r.iter().find(|r| r.id == id))
        else {
            return;
        };
        if id == admin.guild_id || admin.mutation.is_some() || !self.can_manage_role(role) {
            return;
        }
        self.submit_role_action(net::ApiCall::DeleteRole {
            guild_id: admin.guild_id.clone(),
            role_id: id.to_owned(),
        });
    }

    pub fn reorder_role(&mut self, id: &str, other: &str) {
        let Some(admin) = self.role_admin.as_ref() else {
            return;
        };
        let Some(roles) = admin.roles.as_ref() else {
            return;
        };
        let (Some(role), Some(target)) = (
            roles.iter().find(|r| r.id == id),
            roles.iter().find(|r| r.id == other),
        ) else {
            return;
        };
        if admin.mutation.is_some()
            || role.id == admin.guild_id
            || target.id == admin.guild_id
            || role.position == target.position
            || !self.can_manage_role(role)
            || !self.can_manage_role(target)
        {
            return;
        }
        self.submit_role_action(net::ApiCall::ReorderRoles {
            guild_id: admin.guild_id.clone(),
            positions: serde_json::json!([{"id":id,"position":target.position}]),
        });
    }

    pub(super) fn submit_role_action(&mut self, call: net::ApiCall) {
        let queued = self.role_queue(call);
        if let Some(admin) = self.role_admin.as_mut() {
            admin.mutation = queued;
            admin.error = if queued.is_some() {
                None
            } else {
                Some("接続できません。再接続してからお試しください".into())
            };
        }
    }

    pub fn role_admin_event(&mut self, guild_id: &str) {
        if let Some(admin) = self.role_admin.as_mut().filter(|a| a.guild_id == guild_id) {
            admin.revision = admin.revision.wrapping_add(1);
            admin.roles = None;
            admin.my_roles = None;
            admin.owner_id = None;
            if let Some(edit) = admin.guild_edit.as_mut() {
                edit.latest = None;
            }
            admin.confirm_delete = None;
            if let Some(target) = admin.target.as_mut() {
                target.roles = None;
            }
            if let Some(channel) = admin.channel_permissions.as_mut() {
                channel.snapshot = None;
                channel.parent = None;
            }
            if admin.mutation.is_none() {
                self.reload_role_admin();
            }
        }
    }

    pub fn is_role_admin_request(&self, id: u64) -> bool {
        self.role_admin.as_ref().is_some_and(|a| {
            a.roles_request.is_some_and(|(n, _)| n == id)
                || a.member_request.is_some_and(|(n, _)| n == id)
                || a.owner_request.is_some_and(|(n, _)| n == id)
                || a.mutation == Some(id)
                || a.channel_permissions
                    .as_ref()
                    .is_some_and(|channel| channel.is_request(id))
                || a.target
                    .as_ref()
                    .is_some_and(|target| target.member_request.is_some_and(|(n, _)| n == id))
        })
    }

    pub fn role_admin_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        if self
            .role_admin
            .as_ref()
            .and_then(|a| a.channel_permissions.as_ref())
            .is_some_and(|channel| channel.is_request(id))
        {
            self.channel_permission_result(id, result);
            return;
        }
        let Some(admin) = self.role_admin.as_mut() else {
            return;
        };
        if admin.roles_request.is_some_and(|(n, _)| n == id) {
            let (_, revision) = admin.roles_request.take().unwrap();
            if revision != admin.revision {
                self.reload_role_admin();
                return;
            }
            match result
                .ok()
                .and_then(|v| parse_admin_roles(v, &admin.guild_id))
            {
                Some(roles) => {
                    self.roles.insert(admin.guild_id.clone(), roles.clone());
                    self.member_ver = self.member_ver.wrapping_add(1);
                    admin.roles = Some(roles);
                }
                None => {
                    admin.error =
                        Some("ロール一覧を取得できませんでした。再読み込みしてください".into())
                }
            }
        } else if admin.member_request.is_some_and(|(n, _)| n == id) {
            let (_, revision) = admin.member_request.take().unwrap();
            if revision != admin.revision {
                self.reload_role_admin();
                return;
            }
            match self
                .me
                .as_ref()
                .and_then(|me| result.ok().and_then(|v| parse_admin_member(v, &me.id)))
            {
                Some(member) => {
                    self.my_roles
                        .insert(admin.guild_id.clone(), member.roles.clone());
                    admin.my_roles = Some(member.roles);
                }
                None => {
                    admin.error =
                        Some("自分のロールを取得できませんでした。再読み込みしてください".into())
                }
            }
        } else if admin
            .target
            .as_ref()
            .is_some_and(|t| t.member_request.is_some_and(|(n, _)| n == id))
        {
            let target = admin.target.as_mut().unwrap();
            let (_, revision) = target.member_request.take().unwrap();
            if revision != admin.revision {
                self.reload_role_admin();
                return;
            }
            match result
                .ok()
                .and_then(|v| parse_admin_member(v, &target.user_id))
            {
                Some(member) => {
                    if let Some(cached) =
                        self.members.get_mut(&admin.guild_id).and_then(|members| {
                            members
                                .iter_mut()
                                .find(|m| m.user.as_ref().is_some_and(|u| u.id == target.user_id))
                        })
                    {
                        cached.roles = member.roles.clone();
                        self.member_ver = self.member_ver.wrapping_add(1);
                    }
                    target.roles = Some(member.roles);
                }
                None => admin.error = Some("対象メンバーのロールを取得できませんでした".into()),
            }
        } else if admin.owner_request.is_some_and(|(n, _)| n == id) {
            let (_, revision) = admin.owner_request.take().unwrap();
            if revision != admin.revision {
                self.reload_role_admin();
                return;
            }
            let value = result.ok();
            admin.owner_id = value.as_ref().and_then(|value| {
                if value["id"].as_str()? != admin.guild_id {
                    return None;
                }
                value["owner_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
            });
            if admin.owner_id.is_none() {
                admin.error = Some("サーバーの所有者を確認できませんでした".into());
            } else if admin.editing_guild {
                let config = value.as_ref().and_then(parse_guild_config);
                if let Some(config) = config {
                    if let Some(guild) = self.guilds.iter_mut().find(|g| g.id == admin.guild_id) {
                        guild.name = config.name.clone();
                        if let Some(icon) = value.as_ref().and_then(|v| v.get("icon")) {
                            if icon.is_null() || icon.is_string() {
                                guild.icon = icon.as_str().map(str::to_owned);
                            }
                        }
                    }
                    if let Some(edit) = admin.guild_edit.as_mut() {
                        edit.latest = Some(config);
                    } else {
                        admin.guild_edit = Some(GuildEdit::new(config));
                    }
                } else {
                    admin.error = Some("サーバー設定を読み取れませんでした".into());
                }
            }
        } else if admin.mutation == Some(id) {
            admin.mutation = None;
            match result {
                Ok(_) => {
                    admin.draft = None;
                    if admin.editing_guild {
                        admin.guild_edit = None;
                    }
                    let channel_guild = if let Some(channel) = admin.channel_permissions.as_mut() {
                        channel.draft = None;
                        Some(admin.guild_id.clone())
                    } else {
                        None
                    };
                    admin.confirm_delete = None;
                    admin.revision = admin.revision.wrapping_add(1);
                    let toast = if channel_guild.is_some() {
                        "チャンネル権限を更新しました"
                    } else if admin.editing_guild {
                        "サーバー設定を更新しました"
                    } else {
                        "ロールを更新しました"
                    };
                    self.show_toast(toast);
                    if let Some(guild) = channel_guild {
                        self.send(Cmd::LoadGuildChannels(guild));
                    }
                    self.reload_role_admin();
                }
                Err(e) => {
                    let what = if admin.channel_permissions.is_some() {
                        "チャンネル権限を更新できませんでした"
                    } else if admin.editing_guild {
                        "サーバー設定を更新できませんでした"
                    } else {
                        "ロールを更新できませんでした"
                    };
                    admin.error = Some(super::pins::api_error_text(&e, what))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn fixture() -> (App, UnboundedReceiver<Cmd>) {
        let mut app = App::default();
        app.me =
            Some(serde_json::from_value(serde_json::json!({"id":"10","username":"me"})).unwrap());
        app.guilds = vec![serde_json::from_value(
            serde_json::json!({"id":"1","name":"guild","owner":false}),
        )
        .unwrap()];
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        (app, rx)
    }

    fn role(id: &str, position: i64, permissions: u128, managed: bool) -> serde_json::Value {
        serde_json::json!({"id":id,"name":format!("role-{id}"),"position":position,
            "permissions":permissions.to_string(),"color":0,"hoist":false,"managed":managed})
    }

    fn ready(app: &mut App, rx: &mut UnboundedReceiver<Cmd>) {
        app.open_role_admin("1");
        for _ in 0..3 {
            let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
                panic!("expected API")
            };
            let value = match &call {
                net::ApiCall::GetRoles(_) => serde_json::json!([
                    role("1", 0, 0, false),
                    role("2", 10, (1 << 28) | (1 << 11), false),
                    role("3", 2, 1u128 << 100, false),
                    role("4", 12, 0, false),
                    role("5", 1, 0, true)
                ]),
                net::ApiCall::GetMember { .. } => {
                    serde_json::json!({"user":{"id":"10","username":"me"},"roles":["2"]})
                }
                net::ApiCall::GetGuild(_) => serde_json::json!({"id":"1","owner_id":"99"}),
                _ => panic!("unexpected call"),
            };
            app.on_api_result(id, call, Ok(value));
        }
    }

    #[test]
    fn permissions_preserve_unknown_bits_and_enforce_hierarchy() {
        let (mut app, mut rx) = fixture();
        ready(&mut app, &mut rx);
        assert!(app.can_manage_roles());
        let roles = app.role_admin.as_ref().unwrap().roles.as_ref().unwrap();
        assert!(app.can_manage_role(roles.iter().find(|r| r.id == "3").unwrap()));
        assert!(!app.can_manage_role(roles.iter().find(|r| r.id == "4").unwrap()));
        assert!(!app.can_manage_role(roles.iter().find(|r| r.id == "5").unwrap()));
        app.delete_role("1");
        app.delete_role("4");
        app.delete_role("5");
        assert!(rx.try_recv().is_err());
        app.edit_role(Some("3"));
        app.toggle_role_permission(1 << 3);
        assert_eq!(
            app.role_admin
                .as_ref()
                .unwrap()
                .draft
                .as_ref()
                .unwrap()
                .permissions,
            1u128 << 100
        );
        app.toggle_role_permission(1 << 11);
        app.save_role();
        let Cmd::Api {
            call: net::ApiCall::ModifyRole { body, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("expected update")
        };
        assert_eq!(
            body["permissions"],
            ((1u128 << 100) | (1 << 11)).to_string()
        );
        assert!(body.get("name").is_none());
    }

    #[test]
    fn permission_changes_are_rechecked_and_failed_requests_keep_draft() {
        let (mut app, mut rx) = fixture();
        ready(&mut app, &mut rx);
        app.edit_role(Some("3"));
        app.role_admin
            .as_mut()
            .unwrap()
            .draft
            .as_mut()
            .unwrap()
            .permissions |= 1 << 3;
        app.save_role();
        assert!(rx.try_recv().is_err());
        assert!(app.role_admin.as_ref().unwrap().error.is_some());
        app.role_admin
            .as_mut()
            .unwrap()
            .draft
            .as_mut()
            .unwrap()
            .permissions &= !(1 << 3);
        app.role_admin
            .as_mut()
            .unwrap()
            .draft
            .as_mut()
            .unwrap()
            .name = "new".into();
        app.save_role();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!("expected update")
        };
        app.on_api_result(
            id,
            call,
            Err("PATCH /guilds/1/roles/3: 403 Forbidden".into()),
        );
        let admin = app.role_admin.as_ref().unwrap();
        assert!(admin.mutation.is_none());
        assert_eq!(admin.draft.as_ref().unwrap().name, "new");
        assert!(admin.error.as_ref().unwrap().contains("権限"));
    }

    #[test]
    fn role_gateway_change_disables_edit_until_fresh_data_arrives() {
        let (mut app, mut rx) = fixture();
        ready(&mut app, &mut rx);
        app.edit_role(Some("3"));
        app.role_admin_event("1");
        app.save_role();
        let calls: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| matches!(
            c,
            Cmd::Api {
                call: net::ApiCall::GetRoles(_)
                    | net::ApiCall::GetMember { .. }
                    | net::ApiCall::GetGuild(_),
                ..
            }
        )));
        assert!(!app.can_manage_roles());
    }

    #[test]
    fn stale_draft_and_invalid_reorder_are_not_sent() {
        let (mut app, mut rx) = fixture();
        ready(&mut app, &mut rx);
        app.edit_role(Some("3"));
        app.role_admin
            .as_mut()
            .unwrap()
            .draft
            .as_mut()
            .unwrap()
            .name = "edited".into();
        app.role_admin
            .as_mut()
            .unwrap()
            .roles
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r.id == "3")
            .unwrap()
            .position = 3;
        app.save_role();
        assert!(rx.try_recv().is_err());
        assert!(app.role_admin.as_ref().unwrap().error.is_some());
        app.reorder_role("3", "4");
        app.reorder_role("3", "5");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn new_role_defaults_to_no_permissions() {
        let (mut app, mut rx) = fixture();
        ready(&mut app, &mut rx);
        app.edit_role(None);
        app.role_admin
            .as_mut()
            .unwrap()
            .draft
            .as_mut()
            .unwrap()
            .name = "new role".into();
        app.save_role();
        let Cmd::Api {
            call: net::ApiCall::CreateRole { body, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("expected create")
        };
        assert_eq!(body["permissions"], "0");
    }

    fn ready_target(
        app: &mut App,
        rx: &mut UnboundedReceiver<Cmd>,
        target_roles: &[&str],
        owner: &str,
    ) {
        let user: User =
            serde_json::from_value(serde_json::json!({"id":"11","username":"target"})).unwrap();
        app.open_member_role_admin("1", &user);
        for _ in 0..4 {
            let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
                panic!("expected API")
            };
            let value = match &call {
                net::ApiCall::GetRoles(_) => serde_json::json!([
                    role("1", 0, 0, false),
                    role("2", 10, (1 << 28) | (1 << 11), false),
                    role("3", 2, 0, false),
                    role("4", 12, 0, false),
                    role("5", 1, 0, true),
                    role("6", 3, 0, false)
                ]),
                net::ApiCall::GetMember { user_id, .. } if user_id == "10" => {
                    serde_json::json!({"user":{"id":"10","username":"me"},"roles":["2"]})
                }
                net::ApiCall::GetMember { user_id, .. } if user_id == "11" => {
                    serde_json::json!({"user":{"id":"11","username":"target"},"roles":target_roles})
                }
                net::ApiCall::GetGuild(_) => serde_json::json!({"id":"1","owner_id":owner}),
                _ => panic!("unexpected call"),
            };
            app.on_api_result(id, call, Ok(value));
        }
    }

    #[test]
    fn member_roles_use_targeted_routes_and_keep_state_on_failure() {
        let (mut app, mut rx) = fixture();
        ready_target(&mut app, &mut rx, &["3"], "99");
        app.toggle_member_role("3");
        app.toggle_member_role("6");
        let Cmd::Api {
            id,
            call: call @ net::ApiCall::RemoveMemberRole { .. },
        } = rx.try_recv().unwrap()
        else {
            panic!("expected removal")
        };
        assert!(rx.try_recv().is_err());
        app.on_api_result(
            id,
            call,
            Err("DELETE /guilds/1/members/11/roles/3: 403 Forbidden".into()),
        );
        let admin = app.role_admin.as_ref().unwrap();
        assert_eq!(
            admin.target.as_ref().unwrap().roles.as_ref().unwrap(),
            &["3"]
        );
        assert!(admin.error.as_ref().unwrap().contains("権限"));
        app.toggle_member_role("6");
        assert!(
            matches!(rx.try_recv(), Ok(Cmd::Api { call: net::ApiCall::AddMemberRole { guild_id, user_id, role_id }, .. }) if guild_id == "1" && user_id == "11" && role_id == "6")
        );
    }

    #[test]
    fn member_role_edit_denies_owner_higher_and_managed_roles() {
        let (mut app, mut rx) = fixture();
        ready_target(&mut app, &mut rx, &["3"], "11");
        app.toggle_member_role("6");
        assert!(rx.try_recv().is_err(), "owner must not be modified");
        app.role_admin.as_mut().unwrap().owner_id = Some("99".into());
        app.toggle_member_role("4");
        app.toggle_member_role("5");
        assert!(
            rx.try_recv().is_err(),
            "higher and managed roles must be rejected"
        );
        app.role_admin
            .as_mut()
            .unwrap()
            .target
            .as_mut()
            .unwrap()
            .roles = Some(vec!["4".into()]);
        app.toggle_member_role("6");
        assert!(rx.try_recv().is_err(), "higher target must not be modified");
        app.role_admin
            .as_mut()
            .unwrap()
            .target
            .as_mut()
            .unwrap()
            .roles = Some(vec!["unknown".into()]);
        app.toggle_member_role("6");
        assert!(rx.try_recv().is_err(), "unknown hierarchy must fail closed");
    }

    #[test]
    fn member_role_grant_cannot_add_permissions_the_actor_lacks() {
        let (mut app, mut rx) = fixture();
        ready_target(&mut app, &mut rx, &[], "99");
        app.role_admin
            .as_mut()
            .unwrap()
            .roles
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r.id == "3")
            .unwrap()
            .permissions = (1u128 << 3).to_string();
        app.toggle_member_role("3");
        assert!(
            rx.try_recv().is_err(),
            "must not grant an administrator role"
        );
        app.role_admin
            .as_mut()
            .unwrap()
            .target
            .as_mut()
            .unwrap()
            .roles = Some(vec!["3".into()]);
        app.toggle_member_role("3");
        assert!(matches!(rx.try_recv(), Ok(Cmd::Api {
            call: net::ApiCall::RemoveMemberRole { role_id, .. }, ..
        }) if role_id == "3"));
    }

    #[test]
    fn guild_owner_can_manage_their_own_nonmanaged_roles() {
        let (mut app, mut rx) = fixture();
        ready_target(&mut app, &mut rx, &[], "10");
        app.role_admin
            .as_mut()
            .unwrap()
            .target
            .as_mut()
            .unwrap()
            .user_id = "10".into();
        app.toggle_member_role("6");
        assert!(matches!(rx.try_recv(), Ok(Cmd::Api {
            call: net::ApiCall::AddMemberRole { user_id, role_id, .. }, ..
        }) if user_id == "10" && role_id == "6"));
    }

    #[test]
    fn successful_member_role_change_reloads_before_next_action() {
        let (mut app, mut rx) = fixture();
        ready_target(&mut app, &mut rx, &[], "99");
        app.toggle_member_role("3");
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!("expected add")
        };
        app.on_api_result(id, call, Ok(serde_json::Value::Null));
        app.toggle_member_role("6");
        let calls: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(calls.len(), 4);
        assert!(calls.iter().all(|c| matches!(
            c,
            Cmd::Api {
                call: net::ApiCall::GetRoles(_)
                    | net::ApiCall::GetMember { .. }
                    | net::ApiCall::GetGuild(_),
                ..
            }
        )));
        assert!(app
            .role_admin
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .roles
            .is_none());
    }

    #[test]
    fn target_member_change_invalidates_authority_and_reloads() {
        let (mut app, mut rx) = fixture();
        ready_target(&mut app, &mut rx, &[], "99");
        app.on_gateway_event(
            "GUILD_MEMBER_UPDATE",
            serde_json::json!({"guild_id":"1","user":{"id":"11"},"roles":["4"]}),
        );
        app.toggle_member_role("3");
        let calls: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(calls.len(), 4);
        assert!(calls.iter().all(|c| matches!(
            c,
            Cmd::Api {
                call: net::ApiCall::GetRoles(_)
                    | net::ApiCall::GetMember { .. }
                    | net::ApiCall::GetGuild(_),
                ..
            }
        )));
    }

    #[test]
    fn guild_owner_is_revalidated_for_server_role_management() {
        let (mut app, mut rx) = fixture();
        app.guilds[0].owner = true;
        ready(&mut app, &mut rx);
        let admin = app.role_admin.as_ref().unwrap();
        let higher = admin
            .roles
            .as_ref()
            .unwrap()
            .iter()
            .find(|r| r.id == "4")
            .unwrap();
        assert!(
            !app.can_manage_role(higher),
            "stale owner flag cannot bypass fetched ownership"
        );
    }

    fn guild_data(name: &str) -> serde_json::Value {
        serde_json::json!({"id":"1","owner_id":"99","name":name,"icon":null,"description":null,
            "verification_level":1,"default_message_notifications":1,"explicit_content_filter":2,"afk_timeout":300,
            "features":["COMMUNITY"],"system_channel_flags":42})
    }

    fn ready_guild_editor(app: &mut App, rx: &mut UnboundedReceiver<Cmd>, data: serde_json::Value) {
        app.open_guild_editor("1");
        for _ in 0..3 {
            let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
                panic!("expected API")
            };
            let value = match &call {
                net::ApiCall::GetRoles(_) => {
                    serde_json::json!([role("1", 0, 0, false), role("2", 10, 1 << 5, false)])
                }
                net::ApiCall::GetMember { .. } => {
                    serde_json::json!({"user":{"id":"10","username":"me"},"roles":["2"]})
                }
                net::ApiCall::GetGuild(_) => data.clone(),
                _ => panic!("unexpected call"),
            };
            app.on_api_result(id, call, Ok(value));
        }
    }

    #[test]
    fn guild_edit_sends_only_changes_and_keeps_draft_on_failure() {
        let (mut app, mut rx) = fixture();
        ready_guild_editor(&mut app, &mut rx, guild_data("Before"));
        assert!(app.can_manage_guild());
        assert!(!app.can_manage_roles());
        let edit = app
            .role_admin
            .as_mut()
            .unwrap()
            .guild_edit
            .as_mut()
            .unwrap();
        edit.name = "After".into();
        edit.verification = Some(3);
        app.save_guild_settings();
        let Cmd::Api {
            id,
            call: call @ net::ApiCall::ModifyGuild { .. },
        } = rx.try_recv().unwrap()
        else {
            panic!("expected PATCH")
        };
        assert!(
            matches!(&call, net::ApiCall::ModifyGuild { body, .. } if *body == serde_json::json!({"name":"After","verification_level":3}))
        );
        app.on_api_result(id, call, Err("PATCH /guilds/1: 403 Forbidden".into()));
        let admin = app.role_admin.as_ref().unwrap();
        assert_eq!(admin.guild_edit.as_ref().unwrap().name, "After");
        assert!(admin.error.as_ref().unwrap().contains("権限"));
    }

    #[test]
    fn guild_edit_rejects_invalid_choices_and_stale_settings() {
        let (mut app, mut rx) = fixture();
        ready_guild_editor(&mut app, &mut rx, guild_data("Before"));
        app.role_admin
            .as_mut()
            .unwrap()
            .guild_edit
            .as_mut()
            .unwrap()
            .verification = Some(99);
        app.save_guild_settings();
        assert!(rx.try_recv().is_err());
        app.role_admin
            .as_mut()
            .unwrap()
            .guild_edit
            .as_mut()
            .unwrap()
            .verification = Some(1);
        app.role_admin
            .as_mut()
            .unwrap()
            .guild_edit
            .as_mut()
            .unwrap()
            .name = "Local".into();
        app.on_gateway_event(
            "GUILD_UPDATE",
            serde_json::json!({"id":"1","name":"Remote"}),
        );
        for _ in 0..3 {
            let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
                panic!("expected reload")
            };
            let value = match &call {
                net::ApiCall::GetRoles(_) => {
                    serde_json::json!([role("1", 0, 0, false), role("2", 10, 1 << 5, false)])
                }
                net::ApiCall::GetMember { .. } => {
                    serde_json::json!({"user":{"id":"10","username":"me"},"roles":["2"]})
                }
                net::ApiCall::GetGuild(_) => guild_data("Remote"),
                _ => panic!("unexpected call"),
            };
            app.on_api_result(id, call, Ok(value));
        }
        assert_eq!(app.guilds[0].name, "Remote");
        assert!(!app.guild_draft_editable());
        app.save_guild_settings();
        assert!(rx.try_recv().is_err());
        app.reset_guild_draft();
        assert_eq!(
            app.role_admin
                .as_ref()
                .unwrap()
                .guild_edit
                .as_ref()
                .unwrap()
                .name,
            "Remote"
        );
        assert!(app.guild_draft_editable());
    }

    #[test]
    fn guild_settings_requires_manage_guild_permission() {
        let (mut app, mut rx) = fixture();
        ready_guild_editor(&mut app, &mut rx, guild_data("Before"));
        app.role_admin
            .as_mut()
            .unwrap()
            .roles
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r.id == "2")
            .unwrap()
            .permissions = "0".into();
        assert!(!app.guild_draft_editable());
        app.role_admin
            .as_mut()
            .unwrap()
            .guild_edit
            .as_mut()
            .unwrap()
            .name = "After".into();
        app.save_guild_settings();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn missing_managed_field_and_missing_member_roles_are_rejected() {
        let mut bad = role("1", 0, 0, false);
        bad.as_object_mut().unwrap().remove("managed");
        assert!(parse_admin_roles(serde_json::json!([bad]), "1").is_none());
        assert!(parse_admin_member(
            serde_json::json!({"user":{"id":"10","username":"me"}}),
            "10"
        )
        .is_none());
        assert!(parse_admin_member(
            serde_json::json!({"user":{"id":"11","username":"other"},"roles":[]}),
            "10"
        )
        .is_none());
    }
}
