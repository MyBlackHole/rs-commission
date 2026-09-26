//! Wire contracts for the constrained native bridge. Never serialize bearer tokens
//! or PreparedWrite; the WebView only receives opaque session/request handles.
use commission_types::Actor;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct NativeSessionInfo {
    pub id: Uuid,
    pub actor: Actor,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct WriteReceipt {
    pub id: Uuid,
    pub key: String,
    pub path: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePhase {
    Editing,
    Preparing,
    Prepared,
    Sending,
    Unknown,
    Succeeded,
    Rejected,
}
impl WritePhase {
    pub fn may_send(self) -> bool {
        matches!(self, Self::Prepared | Self::Unknown)
    }
    pub fn unresolved(self) -> bool {
        matches!(self, Self::Preparing | Self::Sending | Self::Unknown)
    }
    pub fn may_edit(self) -> bool {
        !self.unresolved()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_must_not_be_replaced_by_a_new_write() {
        assert!(WritePhase::Unknown.may_send());
        assert!(!WritePhase::Unknown.may_edit());
        assert!(WritePhase::Unknown.unresolved());
        assert!(!WritePhase::Sending.may_send());
        assert!(WritePhase::Prepared.may_edit());
        assert!(WritePhase::Succeeded.may_edit());
    }
}
