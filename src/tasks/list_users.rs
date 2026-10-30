use loco_rs::prelude::*;

pub(crate) struct ListUsers;

#[async_trait]
impl Task for ListUsers {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "list_users".into(),
            detail: "List global accounts without secrets: [limit:1..100] [offset:0]".into(),
        }
    }
    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        let limit = vars
            .cli_arg("limit")
            .ok()
            .unwrap_or("100")
            .parse::<u64>()
            .map_err(|_| Error::Message("limit must be an unsigned integer".into()))?;
        let offset = vars
            .cli_arg("offset")
            .ok()
            .unwrap_or("0")
            .parse::<u64>()
            .map_err(|_| Error::Message("offset must be an unsigned integer".into()))?;
        let users = crate::models::user_users::list_by_operator(&ctx.db, limit, offset).await?;
        for user in users {
            println!(
                "{}",
                serde_json::json!({"id":user.id,"email":user.email,"display_name":user.display_name,"created_at":user.created_at})
            );
        }
        Ok(())
    }
}
