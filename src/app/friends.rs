//! フレンド一覧と申請の状態。通信は既存の REST / Gateway を利用する。
use super::*;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Relationship {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    pub user: User,
}

#[derive(Default)]
pub struct Friends {
    pub open: bool,
    pub tab: u8,
    pub username: String,
    pub confirm: Option<(String, u8)>,
    pub relationships: Vec<Relationship>,
    pub loaded: bool,
    pub loading: Option<(u64, u64)>,
    pub error: Option<String>,
    pub revision: u64,
    pub pending: HashMap<u64, FriendAction>,
}

#[derive(Clone, Debug)]
pub enum FriendAction {
    Request,
    Accept(String),
    AddById(String),
    Remove(String),
    Block(String),
    OpenDm(String),
}

impl FriendAction {
    pub fn user_id(&self) -> Option<&str> {
        match self {
            Self::Request => None,
            Self::Accept(id)
            | Self::AddById(id)
            | Self::Remove(id)
            | Self::Block(id)
            | Self::OpenDm(id) => Some(id),
        }
    }
}

impl App {
    pub fn fetch_friends(&mut self) {
        if self.friends.loading.is_some() {
            return;
        }
        self.next_api_id += 1;
        let id = self.next_api_id;
        let sent = self.backend.as_ref().is_some_and(|b| {
            b.cmd
                .send(Cmd::Api {
                    id,
                    call: net::ApiCall::GetRelationships,
                })
                .is_ok()
        });
        if sent {
            self.friends.loading = Some((id, self.friends.revision));
            self.friends.error = None;
        } else {
            self.friends.error = Some(
                tr!(
                    "接続できません。再接続してからお試しください",
                    "Not connected. Reconnect and try again",
                    "未连接。请重新连接后重试"
                )
                .into(),
            );
        }
    }

    pub fn friend_action(&mut self, action: FriendAction) {
        if self
            .friends
            .pending
            .values()
            .any(|p| match (p.user_id(), action.user_id()) {
                (Some(a), Some(b)) => a == b,
                (None, None) => true,
                _ => false,
            })
        {
            return;
        }
        let profile_user = |id: &str| {
            self.popup.as_ref().is_some_and(|p| p.user.id == id)
                && self.me.as_ref().is_none_or(|me| me.id != id)
        };
        let call = match &action {
            FriendAction::Request => {
                let username = self.friends.username.trim();
                if username.is_empty()
                    || username.chars().count() > 100
                    || username.contains(char::is_whitespace)
                {
                    self.friends.error = Some(
                        tr!(
                            "ユーザー名を入力してください",
                            "Enter a username",
                            "请输入用户名"
                        )
                        .into(),
                    );
                    return;
                }
                net::ApiCall::SendFriendRequest {
                    username: username.into(),
                }
            }
            FriendAction::Accept(id)
                if self
                    .friends
                    .relationships
                    .iter()
                    .any(|r| r.id == *id && r.kind == 3) =>
            {
                net::ApiCall::AddFriend(id.clone())
            }
            FriendAction::AddById(id)
                if self.friends.loaded
                    && profile_user(id)
                    && !self.friends.relationships.iter().any(|r| r.id == *id) =>
            {
                net::ApiCall::AddFriend(id.clone())
            }
            FriendAction::Remove(id)
                if self
                    .friends
                    .relationships
                    .iter()
                    .any(|r| r.id == *id && (1..=4).contains(&r.kind)) =>
            {
                net::ApiCall::RemoveRelationship(id.clone())
            }
            FriendAction::Block(id)
                if self
                    .friends
                    .relationships
                    .iter()
                    .any(|r| r.id == *id && r.kind != 2)
                    || (self.friends.loaded
                        && profile_user(id)
                        && !self.friends.relationships.iter().any(|r| r.id == *id)) =>
            {
                net::ApiCall::Block(id.clone())
            }
            FriendAction::OpenDm(id)
                if self
                    .friends
                    .relationships
                    .iter()
                    .any(|r| r.id == *id && r.kind == 1)
                    || profile_user(id) =>
            {
                if let Some(cid) = self
                    .dms
                    .iter()
                    .find(|c| c.recipients.len() == 1 && c.recipients[0].id == *id)
                    .map(|c| c.id.clone())
                {
                    self.sel_guild = None;
                    self.friends.open = false;
                    self.popup = None;
                    self.pending_open = Some(cid);
                    return;
                }
                net::ApiCall::CreateDm(vec![id.clone()])
            }
            _ => return,
        };
        self.next_api_id += 1;
        let id = self.next_api_id;
        if self
            .backend
            .as_ref()
            .is_some_and(|b| b.cmd.send(Cmd::Api { id, call }).is_ok())
        {
            self.friends.pending.insert(id, action);
            self.friends.error = None;
        } else {
            self.friends.error = Some(
                tr!(
                    "接続できません。再接続してからお試しください",
                    "Not connected. Reconnect and try again",
                    "未连接。请重新连接后重试"
                )
                .into(),
            );
            self.show_toast(tr!(
                "接続できません。再接続してからお試しください",
                "Not connected. Reconnect and try again",
                "未连接。请重新连接后重试"
            ));
        }
    }

    pub fn friends_ready(&mut self, value: &serde_json::Value) {
        if let Some(items) = value.as_array() {
            let parsed: Option<Vec<Relationship>> = items
                .iter()
                .map(|v| serde_json::from_value(v.clone()).ok())
                .collect();
            if let Some(items) = parsed.filter(|v| v.iter().all(|r| r.id == r.user.id)) {
                self.friends.relationships = items;
                self.friends.loaded = true;
                self.friends.revision = self.friends.revision.wrapping_add(1);
            }
        }
    }

    pub fn friends_event(&mut self, kind: &str, value: &serde_json::Value) {
        let Some(id) = value["id"].as_str() else {
            return;
        };
        match kind {
            "RELATIONSHIP_REMOVE" => self.friends.relationships.retain(|r| r.id != id),
            "RELATIONSHIP_ADD" | "RELATIONSHIP_UPDATE" => {
                if let Ok(r) = serde_json::from_value::<Relationship>(value.clone()) {
                    if r.id != r.user.id {
                        return;
                    }
                    if let Some(old) = self.friends.relationships.iter_mut().find(|x| x.id == r.id)
                    {
                        *old = r;
                    } else {
                        self.friends.relationships.push(r);
                    }
                } else {
                    // 部分イベントは既存リストに推測で反映せず、完全な一覧を取り直す。
                    self.friends.revision = self.friends.revision.wrapping_add(1);
                    self.fetch_friends();
                    return;
                }
            }
            _ => return,
        }
        self.friends.revision = self.friends.revision.wrapping_add(1);
    }

    pub fn friends_api_result(
        &mut self,
        id: u64,
        result: Result<serde_json::Value, String>,
    ) -> bool {
        if let Some((request, revision)) = self.friends.loading {
            if request == id {
                self.friends.loading = None;
                if revision != self.friends.revision {
                    self.fetch_friends();
                    return true;
                }
                match result {
                    Ok(value) => {
                        let before = self.friends.revision;
                        self.friends_ready(&value);
                        if before == self.friends.revision {
                            self.friends.error = Some(
                                tr!(
                                    "フレンド一覧の応答を読み取れませんでした",
                                    "Couldn't read the friends list response",
                                    "无法读取好友列表的响应"
                                )
                                .into(),
                            );
                        }
                    }
                    Err(e) => {
                        self.friends.error = Some(super::pins::api_error_text(
                            &e,
                            tr!(
                                "フレンド一覧を取得できませんでした",
                                "Couldn't load the friends list",
                                "无法获取好友列表"
                            ),
                        ))
                    }
                }
                return true;
            }
        }
        let Some(action) = self.friends.pending.remove(&id) else {
            return false;
        };
        match result {
            Err(e) => {
                let message = super::pins::api_error_text(
                    &e,
                    tr!(
                        "フレンド操作に失敗しました",
                        "The friend action failed",
                        "好友操作失败"
                    ),
                );
                self.show_toast(&message);
                self.friends.error = Some(message);
            }
            Ok(value) => match action {
                FriendAction::OpenDm(user_id) => match serde_json::from_value::<Channel>(value) {
                    Ok(channel)
                        if channel.kind == 1
                            && channel.recipients.len() == 1
                            && channel.recipients[0].id == user_id =>
                    {
                        let cid = channel.id.clone();
                        if !self.dms.iter().any(|c| c.id == cid) {
                            self.dms.insert(0, channel);
                        }
                        if self.friends.open
                            || self.popup.as_ref().is_some_and(|p| p.user.id == user_id)
                        {
                            self.sel_guild = None;
                            self.friends.open = false;
                            self.popup = None;
                            self.pending_open = Some(cid);
                        }
                    }
                    _ => {
                        self.friends.error = Some(
                            tr!(
                                "DM の応答を読み取れませんでした",
                                "Couldn't read the DM response",
                                "无法读取私信的响应"
                            )
                            .into(),
                        )
                    }
                },
                FriendAction::Request => {
                    self.friends.revision = self.friends.revision.wrapping_add(1);
                    self.friends.username.clear();
                    self.show_toast(tr!(
                        "フレンド申請を送信しました",
                        "Friend request sent",
                        "已发送好友请求"
                    ));
                    self.fetch_friends();
                }
                _ => {
                    self.friends.revision = self.friends.revision.wrapping_add(1);
                    self.show_toast(tr!(
                        "フレンド情報を更新しました",
                        "Friend info updated",
                        "已更新好友信息"
                    ));
                    self.fetch_friends();
                }
            },
        }
        true
    }
}
