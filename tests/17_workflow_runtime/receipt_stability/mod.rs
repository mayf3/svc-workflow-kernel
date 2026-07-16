mod create;
mod transition;

use super::*;

async fn receipt(
    pool: &PgPool,
    principal_id: Uuid,
    key: &str,
) -> (String, i32, serde_json::Value, String) {
    sqlx::query_as(
        "SELECT receipt_status::text, response_status, response_body, response_digest \
         FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(key)
    .fetch_one(pool)
    .await
    .expect("completed deterministic failure receipt")
}
