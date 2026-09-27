//! PostgreSQL invariants for the external platform ingestion domain.
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn connection(pool: &PgPool, platform: &str, external: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,$2,$3,$4,'oauth',$5)",
    )
    .bind(id)
    .bind(platform)
    .bind(external)
    .bind(format!("{platform}-{external}"))
    .bind(format!("secret://platform/{id}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn raw_event(
    pool: &PgPool,
    connection: Uuid,
    stream: &str,
    external_event_id: Option<&str>,
    hash_byte: char,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO platform_raw_events
         (id,connection_id,stream,external_event_id,event_type,payload,payload_hash,platform_updated_at)
         VALUES($1,$2,$3,$4,'order.updated',$5,$6,now())",
    )
    .bind(id)
    .bind(connection)
    .bind(stream)
    .bind(external_event_id)
    .bind(json!({"order":"A100","status":"paid"}))
    .bind(hash_byte.to_string().repeat(64))
    .execute(pool)
    .await
    .unwrap();
    id
}

#[sqlx::test(migrations = "./migrations")]
async fn raw_events_are_idempotent_and_payload_is_immutable(pool: PgPool) {
    let connection = connection(&pool, "taobao", "publisher-1").await;
    let first = raw_event(&pool, connection, "orders", Some("evt-1"), 'a').await;

    let duplicate_external = sqlx::query(
        "INSERT INTO platform_raw_events
         (id,connection_id,stream,external_event_id,event_type,payload,payload_hash)
         VALUES($1,$2,'orders','evt-1','order.updated','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(connection)
    .bind("b".repeat(64))
    .execute(&pool)
    .await;
    assert!(duplicate_external.is_err());

    raw_event(&pool, connection, "orders", None, 'c').await;
    let duplicate_fallback = sqlx::query(
        "INSERT INTO platform_raw_events
         (id,connection_id,stream,event_type,payload,payload_hash)
         VALUES($1,$2,'orders','order.updated','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(connection)
    .bind("c".repeat(64))
    .execute(&pool)
    .await;
    assert!(duplicate_fallback.is_err());

    assert!(
        sqlx::query("UPDATE platform_raw_events SET payload='{"tampered":true}' WHERE id=$1")
            .bind(first)
            .execute(&pool)
            .await
            .is_err()
    );

    sqlx::query(
        "UPDATE platform_raw_events
         SET processing_status='normalized',normalizer_version=1
         WHERE id=$1",
    )
    .bind(first)
    .execute(&pool)
    .await
    .unwrap();

    let status: (String, Option<i32>) = sqlx::query_as(
        "SELECT processing_status,normalizer_version FROM platform_raw_events WHERE id=$1",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, ("normalized".into(), Some(1)));
}

#[sqlx::test(migrations = "./migrations")]
async fn observations_cannot_cross_connections_and_are_append_only(pool: PgPool) {
    let a = connection(&pool, "douyin", "shop-a").await;
    let b = connection(&pool, "douyin", "shop-b").await;
    let event_a = raw_event(&pool, a, "alliance", Some("804-a"), 'd').await;

    let cross_connection = sqlx::query(
        "INSERT INTO external_order_observations
         (id,connection_id,raw_event_id,external_order_line_id,currency,raw_status,
          source_updated_at,normalizer_version,observation_hash)
         VALUES($1,$2,$3,'line-1','CNY','PAY_SUCCESS',now(),1,$4)",
    )
    .bind(Uuid::new_v4())
    .bind(b)
    .bind(event_a)
    .bind("e".repeat(64))
    .execute(&pool)
    .await;
    assert!(cross_connection.is_err());

    let observation = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO external_order_observations
         (id,connection_id,raw_event_id,external_parent_order_id,external_order_line_id,
          currency,paid_minor,raw_status,paid_at,source_updated_at,normalizer_version,observation_hash)
         VALUES($1,$2,$3,'parent-1','line-1','CNY',10000,'PAY_SUCCESS',now(),now(),1,$4)",
    )
    .bind(observation)
    .bind(a)
    .bind(event_a)
    .bind("f".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    assert!(
        sqlx::query("UPDATE external_order_observations SET paid_minor=1 WHERE id=$1")
            .bind(observation)
            .execute(&pool)
            .await
            .is_err()
    );

    sqlx::query(
        "INSERT INTO external_orders
         (connection_id,external_order_line_id,latest_observation_id,external_parent_order_id,
          normalized_status,raw_status,currency,paid_minor,paid_at,last_platform_updated_at)
         VALUES($1,'line-1',$2,'parent-1','paid','PAY_SUCCESS','CNY',10000,now(),now())",
    )
    .bind(a)
    .bind(observation)
    .execute(&pool)
    .await
    .unwrap();

    let wrong_projection = sqlx::query(
        "INSERT INTO external_orders
         (connection_id,external_order_line_id,latest_observation_id,
          normalized_status,raw_status,currency,last_platform_updated_at)
         VALUES($1,'other-line',$2,'paid','PAY_SUCCESS','CNY',now())",
    )
    .bind(a)
    .bind(observation)
    .execute(&pool)
    .await;
    assert!(wrong_projection.is_err());

    sqlx::query(
        "UPDATE external_orders
         SET normalized_status='completed',raw_status='TRADE_FINISHED',updated_at=now()
         WHERE connection_id=$1 AND external_order_line_id='line-1'",
    )
    .bind(a)
    .execute(&pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn commission_observations_preserve_unknown_vs_zero_and_projection_ownership(pool: PgPool) {
    let connection = connection(&pool, "taobao", "publisher-2").await;
    let event = raw_event(
        &pool,
        connection,
        "commissions",
        Some("commission-event-1"),
        '1',
    )
    .await;
    let observation = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO external_commission_observations
         (id,connection_id,raw_event_id,external_commission_key,external_order_line_id,
          beneficiary_role,phase,funding_phase,currency,gross_minor,
          platform_service_fee_minor,special_service_fee_minor,net_minor,raw_status,
          source_updated_at,normalizer_version,observation_hash)
         VALUES($1,$2,$3,'line-9:publisher','line-9','publisher','settled','receivable',
                'CNY',5000,NULL,0,5000,'SETTLED',now(),1,$4)",
    )
    .bind(observation)
    .bind(connection)
    .bind(event)
    .bind("2".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    let fees: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT platform_service_fee_minor,special_service_fee_minor
         FROM external_commission_observations WHERE id=$1",
    )
    .bind(observation)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fees, (None, Some(0)));

    sqlx::query(
        "INSERT INTO external_commissions
         (connection_id,external_commission_key,latest_observation_id,external_order_line_id,
          beneficiary_role,phase,funding_phase,currency,gross_minor,
          platform_service_fee_minor,special_service_fee_minor,net_minor,raw_status,
          last_platform_updated_at)
         VALUES($1,'line-9:publisher',$2,'line-9','publisher','settled','receivable',
                'CNY',5000,NULL,0,5000,'SETTLED',now())",
    )
    .bind(connection)
    .bind(observation)
    .execute(&pool)
    .await
    .unwrap();

    let wrong_key = sqlx::query(
        "INSERT INTO external_commissions
         (connection_id,external_commission_key,latest_observation_id,external_order_line_id,
          beneficiary_role,phase,funding_phase,currency,raw_status,last_platform_updated_at)
         VALUES($1,'different-key',$2,'line-9','publisher','settled','receivable','CNY','SETTLED',now())",
    )
    .bind(connection)
    .bind(observation)
    .execute(&pool)
    .await;
    assert!(wrong_key.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn checkpoints_can_advance_without_skipping_raw_ingestion(pool: PgPool) {
    let connection = connection(&pool, "meitu", "distribution-import").await;

    sqlx::query(
        "INSERT INTO platform_sync_checkpoints
         (connection_id,stream,cursor,window_start,window_end,last_attempt_at,last_success_at)
         VALUES($1,'settlement_import','file-a:10',now()-INTERVAL '1 hour',now(),now(),now())",
    )
    .bind(connection)
    .execute(&pool)
    .await
    .unwrap();

    raw_event(&pool, connection, "settlement_import", None, '3').await;

    sqlx::query(
        "UPDATE platform_sync_checkpoints
         SET cursor='file-a:11',last_attempt_at=now(),last_success_at=now(),updated_at=now()
         WHERE connection_id=$1 AND stream='settlement_import'",
    )
    .bind(connection)
    .execute(&pool)
    .await
    .unwrap();

    let cursor: String = sqlx::query_scalar(
        "SELECT cursor FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='settlement_import'",
    )
    .bind(connection)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cursor, "file-a:11");

    let invalid_window = sqlx::query(
        "UPDATE platform_sync_checkpoints
         SET window_start=now(),window_end=now()-INTERVAL '1 hour'
         WHERE connection_id=$1 AND stream='settlement_import'",
    )
    .bind(connection)
    .execute(&pool)
    .await;
    assert!(invalid_window.is_err());
}
