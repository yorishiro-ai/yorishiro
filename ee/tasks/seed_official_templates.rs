use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::ee::services::official_templates;

/// `cargo loco task seed_official_templates`
///
/// Publishes the community edition's built-in templates as official, community-visible marketplace listings.
/// Idempotent: safe to run on every deployment, including every restart.
pub struct SeedOfficialTemplates;

#[async_trait]
impl Task for SeedOfficialTemplates {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "seed_official_templates".to_string(),
            detail: "Publishes the built-in templates to the marketplace: cargo loco task seed_official_templates".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, _vars: &Vars) -> Result<()> {
        let outcome = official_templates::seed_official_templates(app_context)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!(
            "official templates: {} published, {} updated, {} unchanged",
            outcome.published.len(),
            outcome.updated.len(),
            outcome.unchanged.len()
        );
        for name in outcome.published.iter().chain(outcome.updated.iter()) {
            println!("  {name}");
        }

        Ok(())
    }
}
