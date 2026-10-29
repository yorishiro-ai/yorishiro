use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use yorishiro::models::tenant_retirement_events;

#[tokio::test]
async fn retirement_evidence_survives_without_live_tenant_or_actor_rows() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let tenant_id = uuid::Uuid::new_v4();
        let actor_user_id = uuid::Uuid::new_v4();
        let event = tenant_retirement_events::ActiveModel {
            tenant_id: Set(tenant_id),
            actor_user_id: Set(Some(actor_user_id)),
            action: Set("disabled".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let stored = tenant_retirement_events::Entity::find_by_id(event.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.tenant_id, tenant_id);
        assert_eq!(stored.actor_user_id, Some(actor_user_id));
        assert_eq!(stored.action, "disabled");
        assert!(
            tenant_retirement_events::ActiveModel {
                tenant_id: Set(tenant_id),
                action: Set("unknown".into()),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .is_err()
        );
    })
    .await;
}
