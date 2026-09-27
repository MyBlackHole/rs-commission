-- The initial platform-domain migration used "meitu" for the third planned connector.
-- The intended commerce/affiliate integration is Meituan Union, so migrate the persisted
-- platform identity without rewriting the already-shipped 0002 migration.

ALTER TABLE platform_connections
    DROP CONSTRAINT platform_connections_platform_check;

UPDATE platform_connections
SET platform = 'meituan'
WHERE platform = 'meitu';

ALTER TABLE platform_connections
    ADD CONSTRAINT platform_connections_platform_check
    CHECK (platform IN ('taobao', 'douyin', 'meituan'));
