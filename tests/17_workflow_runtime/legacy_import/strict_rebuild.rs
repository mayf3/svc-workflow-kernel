use super::*;

use svc_workflow::application::workflow_instance::admin_recovery::rebuild_projection;
use svc_workflow::domain::definition::digest;
use svc_workflow::domain::workflow_instance::recovery::{RebuildProjectionCommand, RecoveryError};

async fn grant_admin(fixture: &ImportFixture) {
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1, $2, $3, 'WORKFLOW_ADMIN', TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.domain)
    .bind(fixture.owner)
    .execute(&fixture.pool)
    .await
    .unwrap();
}

async fn assert_rebuild_invalid(
    fixture: &ImportFixture,
    result: &ImportLegacyWorkflowInstanceResult,
) {
    grant_admin(fixture).await;
    let error = rebuild_projection(
        &fixture.pool,
        RebuildProjectionCommand {
            principal_id: PrincipalId::from_uuid(fixture.owner),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(result.workflow_instance_id),
            expected_before_snapshot_digest: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, RecoveryError::InvalidImmutableFacts(_)));
}

async fn tamper_event(
    fixture: &ImportFixture,
    result: &ImportLegacyWorkflowInstanceResult,
    field: &str,
    value: serde_json::Value,
) {
    let mut data: serde_json::Value =
        sqlx::query_scalar("SELECT event_data FROM workflow_events WHERE event_id=$1")
            .bind(result.event_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    data[field] = value;
    let event_digest = digest::compute_json_digest(&data).unwrap();
    let mut transaction = fixture.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_events SET event_data=$2, event_data_digest=$3 WHERE event_id=$1")
        .bind(result.event_id)
        .bind(data)
        .bind(event_digest)
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
}

async fn assert_rebuild_rejects(field: &str, value: serde_json::Value) {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let result = run(&fixture).await.unwrap();
    tamper_event(&fixture, &result, field, value).await;
    assert_rebuild_invalid(&fixture, &result).await;
}

#[tokio::test]
async fn strict_rebuild_rejects_each_invalid_import_event_value() {
    assert_rebuild_rejects("legacySystem", serde_json::json!("ADC")).await;
    assert_rebuild_rejects(
        "legacyRecordId",
        serde_json::json!(Uuid::new_v4().to_string().to_uppercase()),
    )
    .await;
    assert_rebuild_rejects("legacySnapshotDigest", serde_json::json!("A".repeat(64))).await;
    assert_rebuild_rejects(
        "importedNodeId",
        serde_json::json!(Uuid::new_v4().to_string()),
    )
    .await;
    assert_rebuild_rejects("importedAt", serde_json::json!("2026-07-15T01:02:03.123Z")).await;
    assert_rebuild_rejects("creatorResolution", serde_json::json!("MIGRATION_SERVICE")).await;
}

async fn tamper_receipt_linkage(
    fixture: &ImportFixture,
    result: &ImportLegacyWorkflowInstanceResult,
    case: &str,
) {
    let mut transaction = fixture.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await
        .unwrap();
    match case {
        "null command" => {
            sqlx::query("UPDATE workflow_events SET command_id=NULL WHERE event_id=$1")
                .bind(result.event_id)
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        "wrong command type" => {
            sqlx::query(
                "UPDATE workflow_command_receipts
                 SET command_type='CREATE_WORKFLOW_INSTANCE' WHERE command_id=$1",
            )
            .bind(result.command_id)
            .execute(&mut *transaction)
            .await
            .unwrap();
        }
        "wrong principal" => {
            sqlx::query("UPDATE workflow_command_receipts SET principal_id=$2 WHERE command_id=$1")
                .bind(result.command_id)
                .bind(fixture.owner)
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        "wrong actor" => {
            let other_service = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO principals
                 (principal_id, principal_type, display_name, enabled)
                 VALUES ($1, 'SERVICE', 'other migration service', TRUE)",
            )
            .bind(other_service)
            .execute(&mut *transaction)
            .await
            .unwrap();
            sqlx::query("UPDATE workflow_events SET actor_principal_id=$2 WHERE event_id=$1")
                .bind(result.event_id)
                .bind(other_service)
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        "failed receipt" => {
            let body = serde_json::json!({"error": "invalid_input"});
            let response_digest = digest::compute_json_digest(&body).unwrap();
            sqlx::query(
                "UPDATE workflow_command_receipts
                 SET response_status=422, response_body=$2, response_digest=$3
                 WHERE command_id=$1",
            )
            .bind(result.command_id)
            .bind(body)
            .bind(response_digest)
            .execute(&mut *transaction)
            .await
            .unwrap();
        }
        "response digest" => {
            sqlx::query(
                "UPDATE workflow_command_receipts SET response_digest=$2 WHERE command_id=$1",
            )
            .bind(result.command_id)
            .bind("0".repeat(64))
            .execute(&mut *transaction)
            .await
            .unwrap();
        }
        "body id" | "body digest" | "body resolution" | "extra field" => {
            let mut body: serde_json::Value = sqlx::query_scalar(
                "SELECT response_body FROM workflow_command_receipts WHERE command_id=$1",
            )
            .bind(result.command_id)
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
            match case {
                "body id" => body["workflowInstanceId"] = Uuid::new_v4().to_string().into(),
                "body digest" => body["legacySnapshotDigest"] = "0".repeat(64).into(),
                "body resolution" => body["creatorResolution"] = "DOMAIN_OWNER_FALLBACK".into(),
                "extra field" => body["unexpected"] = true.into(),
                _ => unreachable!(),
            }
            let response_digest = digest::compute_json_digest(&body).unwrap();
            sqlx::query(
                "UPDATE workflow_command_receipts
                 SET response_body=$2, response_digest=$3 WHERE command_id=$1",
            )
            .bind(result.command_id)
            .bind(body)
            .bind(response_digest)
            .execute(&mut *transaction)
            .await
            .unwrap();
        }
        _ => unreachable!("unknown receipt linkage tamper case"),
    }
    transaction.commit().await.unwrap();
}

#[tokio::test]
async fn strict_rebuild_rejects_each_invalid_import_receipt_linkage() {
    for case in [
        "null command",
        "wrong command type",
        "wrong principal",
        "wrong actor",
        "failed receipt",
        "body id",
        "body digest",
        "body resolution",
        "extra field",
        "response digest",
    ] {
        let fixture = fixture(ImportedNodeKind::Normal).await;
        let result = run(&fixture).await.unwrap();
        tamper_receipt_linkage(&fixture, &result, case).await;
        assert_rebuild_invalid(&fixture, &result).await;
    }
}
