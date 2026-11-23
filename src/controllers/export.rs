use axum::extract::State;
use axum::http::header;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified};
use crate::controllers::openapi::Routes;
use crate::models::transfer::export;

/// Line-delimited JSON export of every schema, entity, and relation belonging to the workspace, one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
pub(crate) async fn export_jsonl(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
) -> Result<impl aide::axum::IntoApiResponse, ApiError> {
    let body = export::export_jsonl_for_actor(&ctx, verified.ctx).await?;

    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body))
}

pub fn routes() -> Routes {
    Routes::new().tag("community").add(
        "/api/export.jsonl",
        aide::axum::routing::get_with(export_jsonl, |op| {
            op.with(crate::controllers::openapi::extension(
                "x-yorishiro-required-actions",
                serde_json::json!(["export"]),
            ))
            .response_with::<200, String, _>(|op| {
                op.description("Newline-delimited JSON export, at most 8 MiB and 10,000 records")
                    .with(|mut op| {
                        let response = op.inner();
                        if let Some(media) =
                            response.content.shift_remove("text/plain; charset=utf-8")
                        {
                            response
                                .content
                                .insert("application/x-ndjson".into(), media);
                        }
                        op
                    })
            })
        }),
    )
}
