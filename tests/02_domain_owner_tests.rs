#![allow(clippy::needless_borrow)]
//! Test: Domain Owner uniqueness constraint.
//!
//! Verifies that the partial unique index prevents a domain from
//! having more than one enabled DOMAIN_OWNER binding.

mod common;

#[tokio::test]
async fn test_cannot_have_two_enabled_domain_owners() {
    let pool = common::create_pool().await;
    let (principal1, domain_id) = common::seed_principal_and_domain(&pool).await;
    let principal2 = common::seed_second_principal(&pool).await;

    // Create the first domain owner binding
    common::seed_domain_owner(&pool, domain_id, principal1).await;

    // Attempt to create a second enabled DOMAIN_OWNER binding for the same domain
    let binding_id2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO domain_role_bindings (binding_id, domain_id, principal_id, role_key, enabled)
        VALUES ($1, $2, $3, 'DOMAIN_OWNER', TRUE)
        "#,
    )
    .bind(binding_id2)
    .bind(domain_id)
    .bind(principal2)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("idx_drb_single_owner") || err_str.contains("unique constraint"),
                "expected unique constraint violation, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected unique constraint violation but insert succeeded"),
    }
}

#[tokio::test]
async fn test_can_have_disabled_owner_after_enabled() {
    let pool = common::create_pool().await;
    let (principal1, domain_id) = common::seed_principal_and_domain(&pool).await;
    let principal2 = common::seed_second_principal(&pool).await;

    // Create the first domain owner binding
    common::seed_domain_owner(&pool, domain_id, principal1).await;

    // A disabled DOMAIN_OWNER should not violate the partial unique index
    let binding_id2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO domain_role_bindings (binding_id, domain_id, principal_id, role_key, enabled)
        VALUES ($1, $2, $3, 'DOMAIN_OWNER', FALSE)
        "#,
    )
    .bind(binding_id2)
    .bind(domain_id)
    .bind(principal2)
    .execute(&pool)
    .await;

    assert!(
        result.is_ok(),
        "should allow a disabled DOMAIN_OWNER binding alongside an enabled one"
    );
}
