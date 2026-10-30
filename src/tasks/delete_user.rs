use loco_rs::prelude::*;

pub(crate) struct DeleteUser;

#[async_trait]
impl Task for DeleteUser {
    fn task(&self) -> TaskInfo {
        TaskInfo { name: "delete_user".into(), detail: "Delete a global account with no tenant memberships: user_id:<uuid> confirmed_user_id:<same-uuid>".into() }
    }
    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        let user_id = vars
            .cli_arg("user_id")?
            .parse::<uuid::Uuid>()
            .map_err(|_| Error::Message("user_id must be a UUID".into()))?;
        let confirmation = vars
            .cli_arg("confirmed_user_id")?
            .parse::<uuid::Uuid>()
            .map_err(|_| Error::Message("confirmed_user_id must be a UUID".into()))?;
        crate::models::user_users::delete_by_operator(&ctx.db, user_id, confirmation).await?;
        println!(
            "{}",
            serde_json::json!({"user_id":user_id,"status":"deleted"})
        );
        Ok(())
    }
}
