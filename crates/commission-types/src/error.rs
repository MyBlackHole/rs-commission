#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("当前角色无权执行此操作")]
    Forbidden,
    #[error("记录不存在或不可访问")]
    NotFound,
    #[error("业务计算溢出")]
    Internal,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn invalid(message: impl Into<String>) -> Self { Self::Invalid(message.into()) }
}
