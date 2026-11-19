use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use yorishiro::edition::ee::models::marketplace::{
    PublishVersionRequest, TemplateVersionRecord, TemplateVersionStatus,
};
use yorishiro::models::_entities::template_versions;

#[test]
fn version_status_values_round_trip_through_db_and_json_strings() {
    for (status, wire) in [
        (TemplateVersionStatus::Draft, "draft"),
        (TemplateVersionStatus::Pre, "pre"),
        (TemplateVersionStatus::Stable, "stable"),
    ] {
        assert_eq!(status.as_db_str(), wire);
        assert_eq!(TemplateVersionStatus::from_db_str(wire), Some(status));
        assert_eq!(serde_json::to_value(status).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<TemplateVersionStatus>(json!(wire)).unwrap(),
            status
        );
        let record = TemplateVersionRecord::try_from(template_versions::Model {
            id: Uuid::new_v4(),
            template_id: Uuid::new_v4(),
            version: 1,
            definition: json!({}),
            changelog: None,
            status: wire.to_string(),
            created_by: None,
            created_at: Utc::now().fixed_offset(),
        })
        .unwrap();
        assert_eq!(record.status, status);
    }

    let request: PublishVersionRequest =
        serde_json::from_value(json!({"definition": {}, "status": "pre"})).unwrap();
    assert_eq!(request.status, TemplateVersionStatus::Pre);
}

#[test]
fn unknown_persisted_version_status_is_an_internal_error() {
    let row = template_versions::Model {
        id: Uuid::new_v4(),
        template_id: Uuid::new_v4(),
        version: 1,
        definition: json!({}),
        changelog: None,
        status: "paused".to_string(),
        created_by: None,
        created_at: Utc::now().fixed_offset(),
    };

    let error = TemplateVersionRecord::try_from(row).unwrap_err();
    assert!(matches!(error, yorishiro::YorishiroError::Internal(_)));
}
