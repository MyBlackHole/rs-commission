//! Native session owner, independent of Tauri so it can be tested without a GUI.
//! Mutexes protect transitions only; no guard is held over network I/O.
use crate::{bridge::*, invalid, ApiClient, ClientError, Operation, PreparedWrite, Result};
use commission_types::{Actor, QuoteInput};
use serde_json::Value;
use std::sync::{Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Default)]
pub struct NativeBridge(Mutex<State>);
#[derive(Default)]
struct State {
    generation: Uuid,
    session: Option<Session>,
}
struct Session {
    id: Uuid,
    client: ApiClient,
    actor: Actor,
    pending: Option<Pending>,
}
struct Pending {
    receipt: WriteReceipt,
    write: PreparedWrite,
    phase: WritePhase,
    result: Option<Result<Value>>,
}
impl State {
    fn session(&self, id: Uuid) -> Result<&Session> {
        self.session
            .as_ref()
            .filter(|s| s.id == id)
            .ok_or_else(|| invalid("登录会话已失效"))
    }
    fn session_mut(&mut self, id: Uuid) -> Result<&mut Session> {
        self.session
            .as_mut()
            .filter(|s| s.id == id)
            .ok_or_else(|| invalid("登录会话已失效"))
    }
    fn can_replace(&self) -> bool {
        !self
            .session
            .as_ref()
            .and_then(|s| s.pending.as_ref())
            .is_some_and(|p| p.phase.unresolved())
    }
}
impl NativeBridge {
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.0
            .lock()
            .map_err(|_| invalid("客户端状态异常，请先核验原业务请求"))
    }
    pub async fn login(&self, origin: &str, token: &str) -> Result<NativeSessionInfo> {
        let client = ApiClient::new(origin, token)?;
        let generation = Uuid::new_v4();
        {
            let mut state = self.lock()?;
            if !state.can_replace() {
                return Err(invalid("存在未确认写请求，不能更换会话"));
            }
            state.generation = generation;
            state.session = None;
        }
        let actor = client.me().await?;
        let mut state = self.lock()?;
        if state.generation != generation {
            return Err(invalid("登录响应已过期"));
        }
        let info = NativeSessionInfo {
            id: generation,
            actor: actor.clone(),
        };
        state.session = Some(Session {
            id: generation,
            client,
            actor,
            pending: None,
        });
        Ok(info)
    }
    pub fn logout(&self, id: Uuid) -> Result<()> {
        let mut state = self.lock()?;
        state.session(id)?;
        if !state.can_replace() {
            return Err(invalid("请先核验未确认写请求，不能丢弃会话"));
        }
        state.generation = Uuid::new_v4();
        state.session = None;
        Ok(())
    }
    fn client(&self, id: Uuid) -> Result<ApiClient> {
        Ok(self.lock()?.session(id)?.client.clone())
    }
    pub async fn resource(&self, id: Uuid, resource: &str, offset: u32) -> Result<Value> {
        if offset > 1_000_000 {
            return Err(invalid("分页偏移过大"));
        }
        let result = self.client(id)?.resource(resource, offset).await;
        self.lock()?.session(id)?;
        result
    }
    pub async fn quote(&self, id: Uuid, input: &QuoteInput) -> Result<Value> {
        let result = self.client(id)?.quote(input).await;
        self.lock()?.session(id)?;
        result
    }
    pub async fn order(&self, id: Uuid, order_id: Uuid) -> Result<Value> {
        let result = self.client(id)?.order(order_id).await;
        self.lock()?.session(id)?;
        result
    }
    pub async fn payout(&self, id: Uuid, payout_id: Uuid) -> Result<Value> {
        let result = self.client(id)?.payout(payout_id).await;
        self.lock()?.session(id)?;
        result
    }
    pub fn prepare(
        &self,
        id: Uuid,
        operation: Operation,
        target: Option<Uuid>,
        body: &str,
    ) -> Result<WriteReceipt> {
        let mut state = self.lock()?;
        let session = state.session_mut(id)?;
        if session.pending.is_some() {
            return Err(invalid("已有待处理请求，不能覆盖"));
        }
        if !operation.allowed(&session.actor) {
            return Err(invalid("当前身份不能执行此操作"));
        }
        let write = session.client.prepare(operation, target, body)?;
        let receipt = WriteReceipt {
            id: Uuid::new_v4(),
            key: write.key().to_owned(),
            path: write.path().to_owned(),
        };
        session.pending = Some(Pending {
            receipt: receipt.clone(),
            write,
            phase: WritePhase::Prepared,
            result: None,
        });
        Ok(receipt)
    }
    pub async fn execute(&self, id: Uuid, request: Uuid) -> Result<Value> {
        let (client, write) = {
            let mut state = self.lock()?;
            let session = state.session_mut(id)?;
            let pending = session
                .pending
                .as_mut()
                .filter(|p| p.receipt.id == request)
                .ok_or_else(|| invalid("待确认请求不存在或不属于当前会话"))?;
            if let Some(result) = &pending.result {
                return result.clone();
            }
            if !pending.phase.may_send() {
                return Err(ClientError::Unknown);
            }
            pending.phase = WritePhase::Sending;
            (session.client.clone(), pending.write.clone())
        };
        let result = client.execute(&write).await;
        let mut state = self.lock()?;
        let session = state.session_mut(id)?;
        let pending = session
            .pending
            .as_mut()
            .filter(|p| p.receipt.id == request)
            .ok_or(ClientError::Unknown)?;
        pending.phase = match &result {
            Ok(_) => WritePhase::Succeeded,
            Err(e) if e.outcome_unknown() => WritePhase::Unknown,
            Err(_) => WritePhase::Rejected,
        };
        if !pending.phase.unresolved() {
            pending.result = Some(result.clone());
        }
        result
    }
    pub fn discard(&self, id: Uuid, request: Uuid) -> Result<()> {
        let mut state = self.lock()?;
        let session = state.session_mut(id)?;
        let pending = session
            .pending
            .as_ref()
            .filter(|p| p.receipt.id == request)
            .ok_or_else(|| invalid("待确认请求不存在"))?;
        if !pending.phase.may_edit() {
            return Err(invalid("结果未知或执行中的请求不能丢弃"));
        }
        session.pending = None;
        Ok(())
    }
}
