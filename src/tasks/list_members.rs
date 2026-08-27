use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::tenancy;

/// `cargo loco task list_members tenant_id:<uuid>`
pub struct ListMembers;

#[async_trait]
impl Task for ListMembers {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "list_members".to_string(),
            detail: "Lists a tenant's members: cargo loco task list_members tenant_id:<uuid>"
                .to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;

        // A CLI listing, not a paged UI: shows up to MAX_LIST_LIMIT rather than truncating
        // silently at the smaller default an operator has no way to override here.
        let members = tenancy::list_members(
            &app_context.db,
            tenant_id,
            crate::models::pagination::ListParams {
                limit: crate::models::pagination::MAX_LIST_LIMIT,
                offset: 0,
            },
        )
        .await
        .map_err(|err| Error::Message(err.to_string()))?;

        if members.is_empty() {
            println!("no members for tenant {tenant_id}");
        }

        fn mask_uuid(uuid: &Uuid) -> String {
            let s = uuid.to_string();
            let tail_len = 8usize;
            if s.len() <= tail_len {
                return "***".to_string();
            }
            format!("***{}", &s[s.len() - tail_len..])
        }

        fn mask_email(email: &str) -> String {
            match email.split_once('@') {
                Some((local, domain)) => {
                    let local_masked = if local.len() <= 2 {
                        "**".to_string()
                    } else {
                        format!("{}***{}", &local[..1], &local[local.len() - 1..])
                    };
                    format!("{local_masked}@{domain}")
                }
                None => "***".to_string(),
            }
        }

        for member in members {
            println!(
                "{}  {:<8} {}",
                mask_uuid(&member.user_id),
                member.role.as_db_str(),
                mask_email(&member.email),
            );
        }
        Ok(())
    }
}
