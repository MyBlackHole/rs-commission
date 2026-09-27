use crate::platform::{douyin::DouyinMessage, PlatformError, Result};
use serde_json::Value;

pub fn parse_webhook(body: &[u8]) -> Result<Vec<DouyinMessage>> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| PlatformError::invalid(format!("抖店消息 body JSON 无效：{e}")))?;
    let messages = value
        .as_array()
        .ok_or_else(|| PlatformError::invalid("抖店消息 body 必须是数组"))?;
    if messages.is_empty() {
        return Err(PlatformError::invalid("抖店消息 body 不能为空数组"));
    }

    messages
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| PlatformError::invalid("抖店消息项必须是对象"))?;
            let tag = stringish(
                object
                    .get("tag")
                    .ok_or_else(|| PlatformError::invalid("抖店消息缺少 tag"))?,
            )
            .ok_or_else(|| PlatformError::invalid("抖店消息 tag 格式无效"))?;
            let msg_id = stringish(
                object
                    .get("msg_id")
                    .ok_or_else(|| PlatformError::invalid("抖店消息缺少 msg_id"))?,
            )
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| PlatformError::invalid("抖店消息 msg_id 格式无效"))?;
            let data = object
                .get("data")
                .cloned()
                .ok_or_else(|| PlatformError::invalid("抖店消息缺少 data"))?;
            Ok(DouyinMessage { tag, msg_id, data })
        })
        .collect()
}

fn stringish(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        _ => None,
    }
}
