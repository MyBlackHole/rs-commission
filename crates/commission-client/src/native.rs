//! Native session owner, independent of Tauri so it can be tested without a GUI.
//! Mutexes protect transitions only; no guard is held over network I/O.
use crate::{
    bridge::{NativeSessionInfo, RecoveredWriteReceipt, WritePhase, WriteReceipt},
    invalid, ApiClient, ClientError, Operation, PersistedWrite, PreparedWrite, Result,
};
use commission_types::{Actor, QuoteInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;

const STORE_VERSION: u8 = 1;

pub struct NativeBridge {
    state: Mutex<State>,
    store_path: Option<PathBuf>,
}
#[derive(Default)]
struct State {
    generation: Uuid,
    session: Option<Session>,
    recovery: Option<StoredPending>,
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
#[derive(Clone, Serialize, Deserialize)]
struct StoredPending {
    version: u8,
    origin: String,
    actor_id: Uuid,
    attempted: bool,
    write: PersistedWrite,
}

impl Default for NativeBridge {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            store_path: None,
        }
    }
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
}

impl NativeBridge {
    pub fn persistent(path: PathBuf) -> Result<Self> {
        let recovery = Self::load_store(&path)?;
        Ok(Self {
            state: Mutex::new(State {
                recovery,
                ..State::default()
            }),
            store_path: Some(path),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| invalid("客户端状态异常，请先核验原业务请求"))
    }

    fn next_path(path: &Path) -> PathBuf {
        path.with_extension("next")
    }

    fn decode_store(bytes: &[u8]) -> Result<StoredPending> {
        let pending: StoredPending = serde_json::from_slice(bytes)
            .map_err(|_| invalid("本地待恢复请求损坏；请先核验服务器状态"))?;
        if pending.version != STORE_VERSION {
            return Err(invalid("本地待恢复请求版本不兼容；请先核验服务器状态"));
        }
        Ok(pending)
    }

    fn load_store(path: &Path) -> Result<Option<StoredPending>> {
        match fs::read(path) {
            Ok(bytes) => return Self::decode_store(&bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid("无法读取本地待恢复请求")),
        }
        let next = Self::next_path(path);
        match fs::read(next) {
            Ok(bytes) => Self::decode_store(&bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(invalid("无法读取本地待恢复请求")),
        }
    }

    fn write_store(&self, pending: &StoredPending) -> Result<()> {
        let Some(path) = &self.store_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| invalid("无法创建本地恢复目录"))?;
        }
        let next = Self::next_path(path);
        let bytes = serde_json::to_vec(pending).map_err(|_| invalid("无法编码本地待恢复请求"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&next)
            .map_err(|_| invalid("无法保存本地待恢复请求"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| invalid("无法持久化本地待恢复请求"))?;
        if fs::rename(&next, path).is_err() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(invalid("无法替换本地待恢复请求")),
            }
            fs::rename(&next, path).map_err(|_| invalid("无法提交本地待恢复请求"))?;
        }
        Ok(())
    }

    fn clear_store(&self) -> Result<()> {
        let Some(path) = &self.store_path else {
            return Ok(());
        };
        for candidate in [path.clone(), Self::next_path(path)] {
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(invalid("无法清除本地待恢复请求")),
            }
        }
        Ok(())
    }

    pub async fn login(&self, origin: &str, token: &str) -> Result<NativeSessionInfo> {
        let client = ApiClient::new(origin, token)?;
        let generation = Uuid::new_v4();
        {
            let mut state = self.lock()?;
            if state
                .session
                .as_ref()
                .and_then(|s| s.pending.as_ref())
                .is_some()
            {
                return Err(invalid("存在待处理请求，不能更换会话"));
            }
            if state
                .recovery
                .as_ref()
                .is_some_and(|p| p.origin != client.origin())
            {
                return Err(invalid("存在其他服务地址的待恢复请求，不能更换服务"));
            }
            state.generation = generation;
            state.session = None;
        }

        let actor = client.me().await?;
        let mut state = self.lock()?;
        if state.generation != generation {
            return Err(invalid("登录响应已过期"));
        }
        if state
            .recovery
            .as_ref()
            .is_some_and(|p| p.actor_id != actor.id)
        {
            return Err(invalid("待恢复请求属于其他登录身份，不能继续当前会话"));
        }

        let pending = match state.recovery.as_ref() {
            Some(stored) => {
                let write = client.restore(&stored.write)?;
                Some(Pending {
                    receipt: WriteReceipt {
                        id: Uuid::new_v4(),
                        key: write.key().to_owned(),
                        path: write.path().to_owned(),
                    },
                    write,
                    phase: if stored.attempted {
                        WritePhase::Unknown
                    } else {
                        WritePhase::Prepared
                    },
                    result: None,
                })
            }
            None => None,
        };
        let info = NativeSessionInfo {
            id: generation,
            actor: actor.clone(),
        };
        state.session = Some(Session {
            id: generation,
            client,
            actor,
            pending,
        });
        Ok(info)
    }

    pub fn logout(&self, id: Uuid) -> Result<()> {
        let mut state = self.lock()?;
        let session = state.session(id)?;
        if session.pending.is_some() {
            return Err(invalid("请先处理待恢复写请求，不能丢弃会话"));
        }
        state.generation = Uuid::new_v4();
        state.session = None;
        Ok(())
    }

    pub fn recover(&self, id: Uuid) -> Result<Option<RecoveredWriteReceipt>> {
        let state = self.lock()?;
        let session = state.session(id)?;
        Ok(session
            .pending
            .as_ref()
            .map(|pending| RecoveredWriteReceipt {
                receipt: pending.receipt.clone(),
                phase: pending.phase,
            }))
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
        if state.recovery.is_some() {
            return Err(invalid("已有待恢复请求，不能覆盖"));
        }
        let session = state.session(id)?;
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
        let stored = StoredPending {
            version: STORE_VERSION,
            origin: session.client.origin().to_owned(),
            actor_id: session.actor.id,
            attempted: false,
            write: write.persisted(),
        };
        self.write_store(&stored)?;
        state.recovery = Some(stored);
        state.session_mut(id)?.pending = Some(Pending {
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
            let (client, write) = {
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
                (session.client.clone(), pending.write.clone())
            };

            let mut stored = state
                .recovery
                .clone()
                .ok_or_else(|| invalid("持久化待恢复请求不存在"))?;
            if stored.write.key != write.key() {
                return Err(invalid("待恢复请求与内存请求不一致"));
            }
            stored.attempted = true;
            self.write_store(&stored)?;
            state.recovery = Some(stored);
            state
                .session_mut(id)?
                .pending
                .as_mut()
                .filter(|p| p.receipt.id == request)
                .ok_or(ClientError::Unknown)?
                .phase = WritePhase::Sending;
            (client, write)
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
        let pending = state
            .session(id)?
            .pending
            .as_ref()
            .filter(|p| p.receipt.id == request)
            .ok_or_else(|| invalid("待确认请求不存在"))?;
        if !pending.phase.may_edit() {
            return Err(invalid("结果未知或执行中的请求不能丢弃"));
        }
        self.clear_store()?;
        state.recovery = None;
        state.session_mut(id)?.pending = None;
        Ok(())
    }
}
