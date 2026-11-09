use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
// 消息自带的按钮（名字 + 动作），消息页接回来之前先只有反序列化用。
#[allow(dead_code)]
pub struct MessageAction {
    pub name: String,
    pub action: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct Message {
    pub id: i32,
    pub title: String,
    pub content: String,
    pub author: String,
    pub time: DateTime<Utc>,
    #[serde(default)]
    pub actions: Vec<MessageAction>,
}
