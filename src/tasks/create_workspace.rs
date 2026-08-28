use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

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
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;
        let name = vars.cli_arg("name")?;
        let embedding_model = crate::services::embedding::model_name_from_env();

        let provider = crate::services::embedding::build_embedding_provider()
            .await
            .map_err(|err| Error::Message(err.to_string()))?;
        let dimensions = provider.dimensions() as i32;

        // `create_workspace` holds a transaction-scoped advisory lock across its count and insert, so it takes a transaction rather than the pool.
        let txn = app_context
            .db
            .begin()
            .await
            .map_err(|err| Error::Message(err.to_string()))?;
        let workspace = tenancy::create_workspace(
            &txn,
            tenant_id,
            name,
            None,
            None,
            Some((&embedding_model, dimensions)),
        )
        .await
        .map_err(|err| Error::Message(err.to_string()))?;
        txn.commit()
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("workspace id: {}", workspace.id);
        Ok(())
    }
}
