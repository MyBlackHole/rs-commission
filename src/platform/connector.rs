#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Taobao,
    Douyin,
    Meitu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    OrderPull,
    OrderWebhook,
    RefundPull,
    RefundWebhook,
    CommissionPull,
    CommissionWebhook,
    SettlementPull,
    SettlementWebhook,
    ProductSearch,
    PromotionLink,
    PromotionPosition,
    FileImport,
}

pub trait PlatformConnector {
    fn platform(&self) -> Platform;
    fn capabilities(&self) -> &'static [Capability];
}
