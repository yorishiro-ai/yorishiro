use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy;

/// `cargo loco task create_workspace tenant_id:<uuid> name:acme-prod`
///
/// The operator-assisted second step of invite-less signup: run this, then `create_api_key`.
pub struct CreateWorkspace;

#[async_trait]
impl Task for CreateWorkspace {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_workspace".to_string(),
            detail: "Creates a workspace: cargo loco task create_workspace tenant_id:<uuid> name:acme-prod".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid =
            vars.cli_arg("tenant_id")?
                .parse()
                .map_err(|_| YorishiroError::ValidationFailed {
                    message: "tenant_id is not a valid UUID".into(),
                    details: vec![],
                    hint: "tenant_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                        .into(),
                })?;
        let name = vars.cli_arg("name")?;

        let provider = crate::services::embedding::build_embedding_provider()
            .await
            .internal()?;
        let embedding_model = provider.model_name();
        let dimensions = provider.dimensions() as i32;

        // `create_workspace` holds a transaction-scoped advisory lock across its count and insert, so it takes a transaction rather than the pool.
        let txn = app_context.db.begin().await.internal()?;
        let workspace = tenancy::create_workspace(
            &txn,
            tenant_id,
            name,
            None,
            None,
            Some((&embedding_model, dimensions)),
        )
        .await
        .internal()?;
        txn.commit().await.internal()?;

        println!("workspace id: {}", workspace.id);
        Ok(())
    }
}
