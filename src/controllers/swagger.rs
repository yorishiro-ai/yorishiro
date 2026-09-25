//! OpenAPI documentation served through Swagger UI.

use axum::Router;
use axum::http::Method;
use utoipa::openapi::path::{HttpMethod, Operation, OperationBuilder, Parameter};
use utoipa::openapi::schema::{ArrayBuilder, KnownFormat, SchemaType};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{
    Content, Info, OpenApiBuilder, Paths, Ref, Response, Responses, Schema, Type,
};
use utoipa_swagger_ui::SwaggerUi;

use super::route_inventory::{Edition, RouteEntry, RouteInventory};

const ERROR_SCHEMA: &str = "ApiErrorBody";

/// Build the document for the routes mounted by the selected edition.
pub(crate) fn openapi_for(
    inventory: &RouteInventory,
    edition: Edition,
) -> utoipa::openapi::OpenApi {
    let mut paths = Paths::new();
    for entry in inventory.public(edition) {
        paths.add_path_operation(
            &entry.path,
            vec![openapi_method(&entry.method)],
            operation(entry),
        );
    }

    let mut components = utoipa::openapi::Components::new();
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("yorishiro-api-key")
                .build(),
        ),
    );
    for (name, schema) in schemas() {
        components.schemas.insert(name.to_string(), schema);
    }

    OpenApiBuilder::new()
        .info(Info::new("Yorishiro REST API", env!("CARGO_PKG_VERSION")))
        .paths(paths)
        .components(Some(components))
        .build()
}

#[derive(Clone, Copy)]
struct RouteContract {
    request: Option<&'static str>,
    response: &'static str,
    status: &'static str,
    auth: Auth,
    query: &'static [QueryParam],
}

#[derive(Clone, Copy)]
enum Auth {
    Public,
    Scope(&'static str),
}

struct QueryParam {
    name: &'static str,
    kind: Type,
    required: bool,
    format: Option<KnownFormat>,
    enum_values: &'static [&'static str],
}

const PAGE: &[QueryParam] = &[
    QueryParam {
        name: "page",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
    QueryParam {
        name: "page_size",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
];

const NO_QUERY: &[QueryParam] = &[];
const ENTITY_LIST_QUERY: &[QueryParam] = &[
    QueryParam {
        name: "entity_type",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "filter",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "schema_version",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
    QueryParam {
        name: "page",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
    QueryParam {
        name: "page_size",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
];
const RELATION_LIST_QUERY: &[QueryParam] = &[
    QueryParam {
        name: "source_id",
        kind: Type::String,
        required: false,
        format: Some(KnownFormat::Uuid),
        enum_values: &[],
    },
    QueryParam {
        name: "target_id",
        kind: Type::String,
        required: false,
        format: Some(KnownFormat::Uuid),
        enum_values: &[],
    },
    QueryParam {
        name: "relation_type",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "status",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &["active", "deprecated", "archived"],
    },
    QueryParam {
        name: "page",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
    QueryParam {
        name: "page_size",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int32),
        enum_values: &[],
    },
];
const SEARCH_QUERY: &[QueryParam] = &[
    QueryParam {
        name: "query_text",
        kind: Type::String,
        required: true,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "entity_type",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "filter",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "limit",
        kind: Type::Integer,
        required: false,
        format: Some(KnownFormat::Int64),
        enum_values: &[],
    },
];
const OAUTH_CALLBACK_QUERY: &[QueryParam] = &[
    QueryParam {
        name: "code",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "state",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
    QueryParam {
        name: "error",
        kind: Type::String,
        required: false,
        format: None,
        enum_values: &[],
    },
];
const VERSION_QUERY: &[QueryParam] = &[QueryParam {
    name: "version",
    kind: Type::Integer,
    required: false,
    format: Some(KnownFormat::Int32),
    enum_values: &[],
}];

fn operation(entry: &RouteEntry) -> Operation {
    let contract = contract(entry);
    let mut builder = OperationBuilder::new()
        .operation_id(Some(entry.operation_id.clone()))
        .tag(match entry.edition {
            Edition::Community => "community",
            Edition::Enterprise => "enterprise",
        })
        .description(Some(if entry.gated {
            "Enterprise operation. Requires an active licence."
        } else {
            "REST operation served by Yorishiro."
        }))
        .responses(responses(contract));

    for parameter in path_parameters(&entry.path) {
        builder = builder.parameter(parameter);
    }
    for parameter in contract.query {
        builder = builder.parameter(query_parameter(parameter));
    }
    if let Some(schema) = contract.request {
        builder = builder.request_body(Some(
            utoipa::openapi::request_body::RequestBodyBuilder::new()
                .description(Some("JSON request body for this operation."))
                .required(Some(utoipa::openapi::Required::True))
                .content("application/json", Content::new(Some(schema_ref(schema))))
                .build(),
        ));
    }
    match contract.auth {
        Auth::Public => builder.security(SecurityRequirement::default()),
        Auth::Scope(scope) => builder.security(SecurityRequirement::new("bearer_auth", [scope])),
    }
    .build()
}

fn contract(entry: &RouteEntry) -> RouteContract {
    let path = entry.path.as_str();
    let method = &entry.method;
    let public = matches!(
        path,
        "/auth/signup"
            | "/auth/login"
            | "/setup"
            | "/setup/status"
            | "/auth/oauth/status"
            | "/auth/oauth/authorize"
            | "/auth/oauth/callback"
            | "/api/stripe/webhook"
    );
    let auth = if public {
        Auth::Public
    } else if path == "/api/audit-log" {
        Auth::Scope("audit")
    } else if path == "/api/system/maintenance" {
        Auth::Scope("migration")
    } else {
        Auth::Scope(scope(path, method))
    };

    let (request, response, status, query) = match (method, path) {
        (m, "/auth/signup") if *m == Method::POST => {
            (Some("SignupRequest"), "SignupResponse", "201", NO_QUERY)
        }
        (m, "/auth/login") if *m == Method::POST => {
            (Some("LoginRequest"), "LoginResponse", "200", NO_QUERY)
        }
        (m, "/setup") if *m == Method::POST => {
            (Some("SetupRequest"), "SetupResponse", "201", NO_QUERY)
        }
        (m, "/setup/status") if *m == Method::GET => (None, "SetupStatusResponse", "200", NO_QUERY),
        (m, "/api/export.jsonl") if *m == Method::GET => (None, "ExportDocument", "200", NO_QUERY),
        (m, "/api/import.jsonl") if *m == Method::POST => {
            (Some("ImportDocument"), "ImportResult", "200", NO_QUERY)
        }
        (m, "/api/search") if *m == Method::GET => (None, "SearchHitList", "200", SEARCH_QUERY),
        (m, "/api/stripe/webhook") if *m == Method::POST => {
            (Some("StripeEvent"), "EmptyResponse", "200", NO_QUERY)
        }
        (m, "/auth/oauth/authorize") if *m == Method::GET => {
            (None, "RedirectResponse", "302", NO_QUERY)
        }
        (m, "/auth/oauth/status") if *m == Method::GET => (None, "OAuthStatus", "200", NO_QUERY),
        (m, "/auth/oauth/callback") if *m == Method::GET => {
            (None, "RedirectResponse", "302", OAUTH_CALLBACK_QUERY)
        }
        (m, "/api/marketplace/{id}/fork") if *m == Method::POST => {
            (None, "ForkResponse", "201", VERSION_QUERY)
        }
        (m, "/api/schemas/active/{name}/infer-fill") if *m == Method::POST => {
            (None, "InferFillResponse", "200", NO_QUERY)
        }
        (m, "/api/migration-jobs/reindex") if *m == Method::POST => {
            (None, "ReindexResponse", "200", NO_QUERY)
        }
        (m, "/api/migration-jobs/fill-defaults") if *m == Method::POST => (
            Some("FillDefaultsRequest"),
            "FillDefaultsResponse",
            "200",
            NO_QUERY,
        ),
        (m, "/api/entities") if *m == Method::POST => {
            (Some("CreateEntityRequest"), "EntityRecord", "201", NO_QUERY)
        }
        (m, "/api/entities") if *m == Method::GET => {
            (None, "EntityRecordList", "200", ENTITY_LIST_QUERY)
        }
        (m, "/api/entities/{id}") if *m == Method::GET => (None, "EntityRecord", "200", NO_QUERY),
        (m, "/api/entities/{id}") if *m == Method::PUT => {
            (Some("UpdateEntityRequest"), "EntityRecord", "200", NO_QUERY)
        }
        (m, "/api/entities/{id}") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/migration-jobs/{job_id}/undo") if *m == Method::POST => {
            (None, "UndoReport", "200", NO_QUERY)
        }
        (m, "/api/members") if *m == Method::GET => (None, "MembershipRecordList", "200", PAGE),
        (m, "/api/members") if *m == Method::POST => (
            Some("AddMemberRequest"),
            "MembershipRecord",
            "201",
            NO_QUERY,
        ),
        (m, "/api/workspaces") if *m == Method::GET => {
            (None, "WorkspaceRecordList", "200", NO_QUERY)
        }
        (m, "/api/workspaces") if *m == Method::POST => (
            Some("CreateWorkspaceRequest"),
            "WorkspaceRecord",
            "201",
            NO_QUERY,
        ),
        (m, "/api/workspaces/{id}") if *m == Method::GET => {
            (None, "WorkspaceDetail", "200", NO_QUERY)
        }
        (m, "/api/workspaces/{id}") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/relations") if *m == Method::POST => (
            Some("CreateRelationRequest"),
            "RelationRecord",
            "201",
            NO_QUERY,
        ),
        (m, "/api/relations") if *m == Method::GET => {
            (None, "RelationRecordList", "200", RELATION_LIST_QUERY)
        }
        (m, "/api/relations/{id}") if *m == Method::GET => {
            (None, "RelationRecord", "200", NO_QUERY)
        }
        (m, "/api/relations/{id}") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/relations/{id}/status") if *m == Method::PUT => (
            Some("SetRelationStatusRequest"),
            "RelationRecord",
            "200",
            NO_QUERY,
        ),
        (m, "/api/schemas") if *m == Method::GET => (None, "SchemaSummaryList", "200", PAGE),
        (m, "/api/schemas") if *m == Method::POST => (
            Some("CreateSchemaRequest"),
            "CreateSchemaResponse",
            "201",
            NO_QUERY,
        ),
        (m, "/api/schemas/active/{name}") if *m == Method::GET => {
            (None, "SchemaRecord", "200", NO_QUERY)
        }
        (m, "/api/schemas/{schema_id}") if *m == Method::GET => {
            (None, "SchemaRecord", "200", NO_QUERY)
        }
        (m, "/api/schemas/active/{name}/entity-types/{entity_type}/json-schema")
            if *m == Method::GET =>
        {
            (None, "JsonSchema", "200", NO_QUERY)
        }
        (m, "/api/templates") if *m == Method::GET => (None, "TemplateSummaryList", "200", PAGE),
        (m, "/api/templates/{id}") if *m == Method::GET => {
            (None, "MetaSchemaDefinition", "200", NO_QUERY)
        }
        (m, "/api/audit-log") if *m == Method::GET => (None, "AuditLogRecordList", "200", PAGE),
        (m, "/api/template-library") if *m == Method::GET => {
            (None, "TemplateRecordList", "200", PAGE)
        }
        (m, "/api/template-library") if *m == Method::POST => (
            Some("CreateTemplateRequest"),
            "TemplateRecord",
            "201",
            NO_QUERY,
        ),
        (m, "/api/template-library/{id}") if *m == Method::GET => {
            (None, "TemplateRecord", "200", NO_QUERY)
        }
        (m, "/api/template-library/{id}") if *m == Method::PUT => (
            Some("UpdateTemplateRequest"),
            "TemplateRecord",
            "200",
            NO_QUERY,
        ),
        (m, "/api/template-library/{id}") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/template-library/{id}/fork") if *m == Method::POST => (
            Some("ForkTemplateRequest"),
            "TemplateRecord",
            "201",
            NO_QUERY,
        ),
        (m, "/api/system/maintenance") if *m == Method::GET => {
            (None, "MaintenanceResponse", "200", NO_QUERY)
        }
        (m, "/api/system/maintenance") if *m == Method::PUT => (
            Some("SetMaintenanceRequest"),
            "MaintenanceResponse",
            "200",
            NO_QUERY,
        ),
        (m, "/api/whoami") if *m == Method::GET => (None, "WhoAmIResponse", "200", NO_QUERY),
        (m, "/api/tenant/overview") if *m == Method::GET => {
            (None, "TenantOverview", "200", NO_QUERY)
        }
        (m, "/api/workspace/embedding-key") if *m == Method::GET => {
            (None, "EmbeddingKeyDescription", "200", NO_QUERY)
        }
        (m, "/api/workspace/embedding-key") if *m == Method::PUT => (
            Some("SetEmbeddingKeyRequest"),
            "EmptyResponse",
            "204",
            NO_QUERY,
        ),
        (m, "/api/workspace/embedding-key") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/workspace/llm-key") if *m == Method::GET => {
            (None, "LlmKeyDescription", "200", NO_QUERY)
        }
        (m, "/api/workspace/llm-key") if *m == Method::PUT => {
            (Some("SetLlmKeyRequest"), "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/workspace/llm-key") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/workspace/worker-class") if *m == Method::GET => {
            (None, "WorkerClassAssignment", "200", NO_QUERY)
        }
        (m, "/api/workspace/worker-class") if *m == Method::PUT => (
            Some("SetWorkerClassRequest"),
            "EmptyResponse",
            "204",
            NO_QUERY,
        ),
        (m, "/api/workspace/worker-class") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/workspace/entity-columns") if *m == Method::GET => {
            (None, "ColumnPreferenceList", "200", PAGE)
        }
        (m, "/api/workspace/entity-columns/{entity_type}") if *m == Method::PUT => (
            Some("SetColumnsRequest"),
            "ColumnPreference",
            "200",
            NO_QUERY,
        ),
        (m, "/api/workspace/entity-columns/{entity_type}") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        (m, "/api/inference-jobs/{job_id}") if *m == Method::GET => {
            (None, "InferJobStatus", "200", NO_QUERY)
        }
        (m, "/api/marketplace") if *m == Method::GET => {
            (None, "MarketplaceListingList", "200", PAGE)
        }
        (m, "/api/marketplace/{id}/versions") if *m == Method::GET => {
            (None, "TemplateVersionRecordList", "200", PAGE)
        }
        (m, "/api/marketplace/{id}/versions") if *m == Method::POST => (
            Some("PublishVersionRequest"),
            "TemplateVersionRecord",
            "201",
            NO_QUERY,
        ),
        (m, "/api/marketplace/{id}/reviews") if *m == Method::GET => {
            (None, "TemplateReviewRecordList", "200", PAGE)
        }
        (m, "/api/marketplace/{id}/reviews") if *m == Method::POST => (
            Some("SubmitReviewRequest"),
            "TemplateReviewRecord",
            "200",
            NO_QUERY,
        ),
        (m, "/api/marketplace/{id}/visibility") if *m == Method::PUT => (
            Some("SetVisibilityRequest"),
            "EmptyResponse",
            "204",
            NO_QUERY,
        ),
        (m, "/api/schemas/upstream-changes") if *m == Method::GET => {
            (None, "UpstreamChangeList", "200", PAGE)
        }
        (m, "/api/schemas/{schema_id}/merge-preview") if *m == Method::GET => {
            (None, "MergePlan", "200", NO_QUERY)
        }
        (m, "/api/schemas/{schema_id}/merge") if *m == Method::POST => {
            (None, "MergeResponse", "201", NO_QUERY)
        }
        (m, "/api/schema-forks") if *m == Method::GET => (None, "ForkRecordList", "200", NO_QUERY),
        (m, "/api/schema-forks") if *m == Method::POST => {
            (Some("ForkCreateInput"), "ForkRecord", "201", NO_QUERY)
        }
        (m, "/api/schema-forks/{fork_id}") if *m == Method::GET => {
            (None, "ForkRecord", "200", NO_QUERY)
        }
        (m, "/api/schema-forks/{fork_id}") if *m == Method::PUT => {
            (Some("ForkUpdateInput"), "ForkRecord", "200", NO_QUERY)
        }
        (m, "/api/schema-forks/{fork_id}") if *m == Method::DELETE => {
            (None, "EmptyResponse", "204", NO_QUERY)
        }
        _ => panic!("missing OpenAPI contract for {} {}", method, path),
    };

    RouteContract {
        request,
        response,
        status,
        auth,
        query,
    }
}

fn responses(contract: RouteContract) -> Responses {
    let mut responses = Responses::new();
    let success = if contract.status == "204" {
        Response::new("No content")
    } else if contract.status == "302" {
        Response::builder()
            .description("Redirect response.")
            .build()
    } else {
        Response::builder()
            .description("Successful response.")
            .content(
                "application/json",
                Content::new(Some(schema_ref(contract.response))),
            )
            .build()
    };
    responses
        .responses
        .insert(contract.status.into(), success.into());
    for (status, description) in [
        ("400", "The request was malformed."),
        ("401", "Authentication required or credentials invalid."),
        ("403", "The API key does not have the required scope."),
        ("404", "The requested resource or feature was not found."),
        ("409", "The request conflicts with current state."),
        ("422", "The request failed validation."),
        ("500", "The server could not complete the request."),
        ("503", "The service or a required provider is unavailable."),
    ] {
        responses.responses.insert(
            status.into(),
            Response::builder()
                .description(description)
                .content(
                    "application/json",
                    Content::new(Some(schema_ref(ERROR_SCHEMA))),
                )
                .build()
                .into(),
        );
    }
    responses
}

fn scope(path: &str, method: &Method) -> &'static str {
    match (path, method) {
        ("/api/entities", &Method::POST)
        | ("/api/entities/{id}", &Method::PUT | &Method::DELETE)
        | ("/api/relations", &Method::POST)
        | ("/api/relations/{id}", &Method::DELETE)
        | ("/api/relations/{id}/status", &Method::PUT)
        | ("/api/template-library/{id}/fork", &Method::POST)
        | ("/api/workspace/entity-columns/{entity_type}", &Method::PUT | &Method::DELETE) => {
            "write"
        }
        ("/api/entities/{job_id}/undo", &Method::POST)
        | ("/api/migration-jobs/reindex", &Method::POST)
        | ("/api/migration-jobs/fill-defaults", &Method::POST)
        | ("/api/system/maintenance", &Method::GET | &Method::PUT) => "migration",
        ("/api/schemas", &Method::POST)
        | ("/api/import.jsonl", &Method::POST)
        | ("/api/workspace/embedding-key", &Method::PUT | &Method::DELETE)
        | ("/api/workspace/llm-key", &Method::PUT | &Method::DELETE)
        | ("/api/workspace/worker-class", &Method::PUT | &Method::DELETE)
        | ("/api/schemas/{schema_id}/merge", &Method::POST)
        | ("/api/schema-forks", &Method::POST)
        | ("/api/schema-forks/{fork_id}", &Method::PUT | &Method::DELETE)
        | ("/api/marketplace/{id}/versions", &Method::POST)
        | ("/api/marketplace/{id}/visibility", &Method::PUT) => "schema",
        ("/api/template-library", &Method::POST)
        | ("/api/template-library/{id}", &Method::PUT | &Method::DELETE)
        | ("/api/marketplace/{id}/reviews", &Method::POST) => "write",
        _ => "read",
    }
}

fn query_parameter(parameter: &QueryParam) -> Parameter {
    let mut schema = utoipa::openapi::ObjectBuilder::new().schema_type(parameter.kind.clone());
    if let Some(format) = parameter.format.clone() {
        schema = schema.format(Some(utoipa::openapi::SchemaFormat::KnownFormat(format)));
    }
    if !parameter.enum_values.is_empty() {
        schema = schema.enum_values(Some(parameter.enum_values.iter().copied()));
    }
    utoipa::openapi::path::ParameterBuilder::new()
        .name(parameter.name)
        .parameter_in(utoipa::openapi::path::ParameterIn::Query)
        .required(if parameter.required {
            utoipa::openapi::Required::True
        } else {
            utoipa::openapi::Required::False
        })
        .schema(Some(schema))
        .build()
}

fn path_parameters(path: &str) -> impl Iterator<Item = Parameter> + '_ {
    path.split('{').skip(1).filter_map(|part| {
        let name = part.split('}').next()?.to_string();
        Some(
            utoipa::openapi::path::ParameterBuilder::new()
                .name(&name)
                .parameter_in(utoipa::openapi::path::ParameterIn::Path)
                .required(utoipa::openapi::Required::True)
                .schema(Some(string_schema(
                    if name.ends_with("id") || name == "id" {
                        Some("uuid")
                    } else {
                        None
                    },
                )))
                .build(),
        )
    })
}

fn openapi_method(method: &Method) -> HttpMethod {
    match *method {
        Method::GET => HttpMethod::Get,
        Method::POST => HttpMethod::Post,
        Method::PUT => HttpMethod::Put,
        Method::DELETE => HttpMethod::Delete,
        Method::PATCH => HttpMethod::Patch,
        Method::HEAD => HttpMethod::Head,
        Method::OPTIONS => HttpMethod::Options,
        Method::TRACE => HttpMethod::Trace,
        _ => panic!("unsupported OpenAPI method: {method}"),
    }
}

fn schema_ref(name: &str) -> utoipa::openapi::RefOr<Schema> {
    Ref::from_schema_name(name).into()
}

fn string_schema(format: Option<&str>) -> utoipa::openapi::RefOr<Schema> {
    let mut schema = utoipa::openapi::ObjectBuilder::new().schema_type(Type::String);
    if let Some(format) = format {
        schema = schema.format(Some(utoipa::openapi::SchemaFormat::Custom(format.into())));
    }
    schema.build().into()
}

fn uuid_schema() -> utoipa::openapi::RefOr<Schema> {
    string_schema(Some("uuid"))
}

fn date_schema() -> utoipa::openapi::RefOr<Schema> {
    utoipa::openapi::ObjectBuilder::new()
        .schema_type(Type::String)
        .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
            KnownFormat::DateTime,
        )))
        .build()
        .into()
}

fn any_schema() -> utoipa::openapi::RefOr<Schema> {
    utoipa::openapi::ObjectBuilder::new()
        .schema_type(SchemaType::AnyValue)
        .build()
        .into()
}

fn number_schema() -> utoipa::openapi::RefOr<Schema> {
    utoipa::openapi::ObjectBuilder::new()
        .schema_type(Type::Number)
        .build()
        .into()
}

fn object(
    properties: &[(&str, utoipa::openapi::RefOr<Schema>)],
    required: &[&str],
) -> utoipa::openapi::RefOr<Schema> {
    let mut builder = utoipa::openapi::ObjectBuilder::new().schema_type(Type::Object);
    for (name, schema) in properties {
        builder = builder.property(*name, schema.clone());
    }
    for name in required {
        builder = builder.required(*name);
    }
    builder.build().into()
}

fn array(item: utoipa::openapi::RefOr<Schema>) -> utoipa::openapi::RefOr<Schema> {
    ArrayBuilder::new().items(item).build().into()
}

fn schemas() -> Vec<(&'static str, utoipa::openapi::RefOr<Schema>)> {
    let uuid = uuid_schema();
    let string = string_schema(None);
    let integer: utoipa::openapi::RefOr<Schema> = utoipa::openapi::ObjectBuilder::new()
        .schema_type(Type::Integer)
        .build()
        .into();
    let boolean: utoipa::openapi::RefOr<Schema> = utoipa::openapi::ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .build()
        .into();
    let json = any_schema();
    let number = number_schema();
    let record = object(
        &[
            ("id", uuid.clone()),
            ("workspace_id", uuid.clone()),
            ("schema_id", uuid.clone()),
            ("schema_version", integer.clone()),
            ("entity_type", string.clone()),
            ("data", json.clone()),
            ("created_at", date_schema()),
            ("updated_at", date_schema()),
            ("created_by", uuid.clone()),
            ("updated_by", uuid.clone()),
        ],
        &[
            "id",
            "workspace_id",
            "schema_id",
            "schema_version",
            "entity_type",
            "data",
        ],
    );
    let relation = object(
        &[
            ("id", uuid.clone()),
            ("workspace_id", uuid.clone()),
            ("source_id", uuid.clone()),
            ("target_id", uuid.clone()),
            ("relation_type", string.clone()),
            ("properties", json.clone()),
            ("status", string.clone()),
            ("created_at", date_schema()),
        ],
        &[
            "id",
            "workspace_id",
            "source_id",
            "target_id",
            "relation_type",
            "properties",
            "status",
        ],
    );
    let schema = object(
        &[
            ("id", uuid.clone()),
            ("tenant_id", uuid.clone()),
            ("workspace_id", uuid.clone()),
            ("name", string.clone()),
            ("version", integer.clone()),
            ("definition", schema_ref("MetaSchemaDefinition")),
            ("status", string.clone()),
            ("origin_template_id", uuid.clone()),
            ("origin_status", string.clone()),
            ("origin_snapshot", schema_ref("MetaSchemaDefinition")),
            ("origin_updated_at", date_schema()),
            ("created_at", date_schema()),
        ],
        &[
            "id",
            "tenant_id",
            "workspace_id",
            "name",
            "version",
            "definition",
            "status",
        ],
    );
    let template = object(
        &[
            ("id", uuid.clone()),
            ("tenant_id", uuid.clone()),
            ("name", string.clone()),
            ("description", string.clone()),
            ("definition", schema_ref("MetaSchemaDefinition")),
            ("tags", array(string.clone())),
            ("locale", string.clone()),
            ("visibility", string.clone()),
            ("author", string.clone()),
            ("fork_of", uuid.clone()),
            ("created_by", uuid.clone()),
            ("created_at", date_schema()),
            ("updated_at", date_schema()),
        ],
        &[
            "id",
            "tenant_id",
            "name",
            "definition",
            "tags",
            "visibility",
            "created_at",
            "updated_at",
        ],
    );
    let meta_field = object(
        &[
            ("type", string.clone()),
            ("required", boolean.clone()),
            ("description", string.clone()),
            ("enum", array(string.clone())),
            ("format", string.clone()),
            ("minimum", json.clone()),
            ("maximum", json.clone()),
            ("items", json.clone()),
            ("properties", json.clone()),
        ],
        &["type"],
    );
    let meta = object(
        &[
            ("name", string.clone()),
            ("description", string.clone()),
            ("entity_types", json.clone()),
            ("relation_types", json.clone()),
        ],
        &["name", "entity_types", "relation_types"],
    );
    let error = object(
        &[(
            "error",
            object(
                &[
                    ("code", string.clone()),
                    ("message", string.clone()),
                    ("details", json.clone()),
                    ("hint", string.clone()),
                ],
                &["code", "message"],
            ),
        )],
        &["error"],
    );
    vec![
        (ERROR_SCHEMA, error),
        ("JsonValue", json.clone()),
        ("EmptyResponse", object(&[], &[])),
        ("ExportDocument", string_schema(Some("ndjson"))),
        ("ImportDocument", string_schema(Some("ndjson"))),
        (
            "SignupRequest",
            object(
                &[
                    ("invite_token", string.clone()),
                    ("email", string.clone()),
                    ("password", string.clone()),
                    ("display_name", string.clone()),
                ],
                &["password"],
            ),
        ),
        (
            "SignupResponse",
            object(
                &[
                    ("user_id", uuid.clone()),
                    ("email", string.clone()),
                    ("tenant_id", uuid.clone()),
                    ("role", string.clone()),
                    ("workspaces", array(schema_ref("WorkspaceSummary"))),
                ],
                &["user_id", "email", "tenant_id", "role", "workspaces"],
            ),
        ),
        (
            "LoginRequest",
            object(
                &[
                    ("email", string.clone()),
                    ("password", string.clone()),
                    ("workspace_id", uuid.clone()),
                ],
                &["email", "password"],
            ),
        ),
        (
            "LoginResponse",
            object(
                &[
                    ("api_key", string.clone()),
                    ("api_key_id", uuid.clone()),
                    ("workspace_id", uuid.clone()),
                    ("scope", string.clone()),
                    ("user_id", uuid.clone()),
                ],
                &["api_key", "api_key_id", "workspace_id", "scope", "user_id"],
            ),
        ),
        (
            "SetupRequest",
            object(
                &[
                    ("email", string.clone()),
                    ("password", string.clone()),
                    ("display_name", string.clone()),
                ],
                &["email", "password"],
            ),
        ),
        (
            "SetupResponse",
            object(
                &[
                    ("user_id", uuid.clone()),
                    ("email", string.clone()),
                    ("tenant_id", uuid.clone()),
                    ("workspace_id", uuid.clone()),
                    ("api_key", string.clone()),
                ],
                &["user_id", "email", "tenant_id", "workspace_id", "api_key"],
            ),
        ),
        (
            "SetupStatusResponse",
            object(&[("setup_required", boolean.clone())], &["setup_required"]),
        ),
        (
            "OAuthStatus",
            object(&[("enabled", boolean.clone())], &["enabled"]),
        ),
        (
            "CreateEntityRequest",
            object(
                &[
                    ("schema_name", string.clone()),
                    ("entity_type", string.clone()),
                    ("data", json.clone()),
                ],
                &["schema_name", "entity_type", "data"],
            ),
        ),
        (
            "UpdateEntityRequest",
            object(&[("data", json.clone())], &["data"]),
        ),
        (
            "FillDefaultsRequest",
            object(&[("schema_name", string.clone())], &["schema_name"]),
        ),
        (
            "UndoReport",
            object(
                &[
                    ("job_id", uuid.clone()),
                    ("restored", integer.clone()),
                    ("missing", integer.clone()),
                ],
                &["job_id", "restored", "missing"],
            ),
        ),
        (
            "ReindexResponse",
            object(&[("job_id", string.clone())], &["job_id"]),
        ),
        (
            "FillDefaultsResponse",
            object(
                &[
                    ("schema_name", string.clone()),
                    ("job_id", uuid.clone()),
                    ("entities_updated", integer.clone()),
                    ("fields_filled", integer.clone()),
                ],
                &["schema_name", "job_id", "entities_updated", "fields_filled"],
            ),
        ),
        (
            "CreateRelationRequest",
            object(
                &[
                    ("source_id", uuid.clone()),
                    ("target_id", uuid.clone()),
                    ("relation_type", string.clone()),
                    ("properties", json.clone()),
                ],
                &["source_id", "target_id", "relation_type"],
            ),
        ),
        (
            "SetRelationStatusRequest",
            object(&[("status", string.clone())], &["status"]),
        ),
        (
            "AddMemberRequest",
            object(
                &[("email", string.clone()), ("role", string.clone())],
                &["email", "role"],
            ),
        ),
        (
            "CreateWorkspaceRequest",
            object(
                &[
                    ("name", string.clone()),
                    ("max_entities", integer.clone()),
                    ("schema_id", uuid.clone()),
                ],
                &["name"],
            ),
        ),
        (
            "SetMaintenanceRequest",
            object(
                &[
                    ("mode", string.clone()),
                    ("retry_after", integer.clone()),
                    ("reason", string.clone()),
                ],
                &["mode"],
            ),
        ),
        (
            "CreateSchemaRequest",
            object(
                &[
                    ("name", string.clone()),
                    ("description", string.clone()),
                    ("entity_types", json.clone()),
                    ("relation_types", json.clone()),
                    ("template_id", string.clone()),
                ],
                &[],
            ),
        ),
        (
            "CreateTemplateRequest",
            object(
                &[
                    ("name", string.clone()),
                    ("description", string.clone()),
                    ("definition", schema_ref("MetaSchemaDefinition")),
                    ("tags", array(string.clone())),
                    ("locale", string.clone()),
                    ("author", string.clone()),
                ],
                &["name", "definition"],
            ),
        ),
        (
            "UpdateTemplateRequest",
            object(
                &[
                    ("name", string.clone()),
                    ("description", string.clone()),
                    ("definition", schema_ref("MetaSchemaDefinition")),
                    ("tags", array(string.clone())),
                    ("locale", string.clone()),
                ],
                &[],
            ),
        ),
        (
            "ForkTemplateRequest",
            object(&[("name", string.clone())], &["name"]),
        ),
        (
            "SetEmbeddingKeyRequest",
            object(
                &[
                    ("base_url", string.clone()),
                    ("model", string.clone()),
                    ("api_key", string.clone()),
                    ("dimensions", integer.clone()),
                    ("send_dimensions_param", boolean.clone()),
                ],
                &["base_url", "model", "api_key", "dimensions"],
            ),
        ),
        (
            "SetLlmKeyRequest",
            object(
                &[
                    ("base_url", string.clone()),
                    ("model", string.clone()),
                    ("api_key", string.clone()),
                ],
                &["base_url", "model", "api_key"],
            ),
        ),
        (
            "SetWorkerClassRequest",
            object(&[("worker_class", string.clone())], &["worker_class"]),
        ),
        (
            "SetColumnsRequest",
            object(&[("columns", array(string.clone()))], &["columns"]),
        ),
        (
            "PublishVersionRequest",
            object(
                &[
                    ("definition", json.clone()),
                    ("changelog", string.clone()),
                    ("status", string.clone()),
                ],
                &["definition"],
            ),
        ),
        (
            "SubmitReviewRequest",
            object(
                &[("rating", integer.clone()), ("comment", string.clone())],
                &["rating"],
            ),
        ),
        (
            "SetVisibilityRequest",
            object(&[("visibility", string.clone())], &["visibility"]),
        ),
        (
            "ForkCreateInput",
            object(
                &[
                    ("source_workspace_id", uuid.clone()),
                    ("source_schema_id", uuid.clone()),
                ],
                &["source_workspace_id", "source_schema_id"],
            ),
        ),
        (
            "ForkUpdateInput",
            object(
                &[
                    ("definition", schema_ref("MetaSchemaDefinition")),
                    ("action", string.clone()),
                    ("force", boolean.clone()),
                    ("expected_fork_schema_id", uuid.clone()),
                    ("expected_source_schema_id", uuid.clone()),
                ],
                &[],
            ),
        ),
        (
            "StripeEvent",
            object(
                &[
                    ("id", string.clone()),
                    ("type", string.clone()),
                    ("created", integer.clone()),
                    ("data", object(&[("object", json.clone())], &["object"])),
                ],
                &["id", "type", "created", "data"],
            ),
        ),
        ("RedirectResponse", object(&[], &[])),
        ("MetaSchemaDefinition", meta),
        (
            "WorkspaceSummary",
            object(
                &[("id", uuid.clone()), ("name", string.clone())],
                &["id", "name"],
            ),
        ),
        ("MetaSchemaField", meta_field),
        ("EntityRecord", record.clone()),
        ("EntityRecordList", array(schema_ref("EntityRecord"))),
        ("RelationRecord", relation.clone()),
        ("RelationRecordList", array(schema_ref("RelationRecord"))),
        ("SchemaRecord", schema),
        (
            "SchemaSummaryList",
            array(object(
                &[
                    ("id", uuid.clone()),
                    ("name", string.clone()),
                    ("version", integer.clone()),
                    ("status", string.clone()),
                    ("created_at", date_schema()),
                ],
                &["id", "name", "version", "status", "created_at"],
            )),
        ),
        (
            "CreateSchemaResponse",
            object(
                &[
                    ("schema", schema_ref("SchemaRecord")),
                    ("diff", json.clone()),
                ],
                &["schema", "diff"],
            ),
        ),
        (
            "WorkspaceRecord",
            object(
                &[
                    ("id", uuid.clone()),
                    ("tenant_id", uuid.clone()),
                    ("name", string.clone()),
                    ("max_entities", integer.clone()),
                    ("status", string.clone()),
                    ("embedding_model", string.clone()),
                    ("embedding_dimensions", integer.clone()),
                    ("schema_id", uuid.clone()),
                    ("created_at", date_schema()),
                ],
                &["id", "tenant_id", "name", "status", "created_at"],
            ),
        ),
        ("WorkspaceRecordList", array(schema_ref("WorkspaceRecord"))),
        (
            "WorkspaceDetail",
            object(
                &[
                    ("id", uuid.clone()),
                    ("tenant_id", uuid.clone()),
                    ("name", string.clone()),
                    ("max_entities", integer.clone()),
                    ("schema_id", uuid.clone()),
                    ("created_at", date_schema()),
                    ("entity_count", integer.clone()),
                    ("relation_count", integer.clone()),
                    ("schema_count", integer.clone()),
                ],
                &[
                    "id",
                    "tenant_id",
                    "name",
                    "created_at",
                    "entity_count",
                    "relation_count",
                    "schema_count",
                ],
            ),
        ),
        (
            "MembershipRecord",
            object(
                &[
                    ("user_id", uuid.clone()),
                    ("email", string.clone()),
                    ("display_name", string.clone()),
                    ("role", string.clone()),
                ],
                &["user_id", "email", "role"],
            ),
        ),
        (
            "MembershipRecordList",
            array(schema_ref("MembershipRecord")),
        ),
        ("TemplateRecord", template),
        ("TemplateRecordList", array(schema_ref("TemplateRecord"))),
        (
            "TemplateSummaryList",
            array(object(
                &[
                    ("id", string.clone()),
                    ("name", string.clone()),
                    ("description", string.clone()),
                ],
                &["id", "name"],
            )),
        ),
        (
            "MaintenanceResponse",
            object(
                &[
                    ("mode", string.clone()),
                    ("retry_after", integer.clone()),
                    ("reason", string.clone()),
                ],
                &["mode", "retry_after"],
            ),
        ),
        (
            "WhoAmIResponse",
            object(
                &[
                    ("workspace_id", uuid.clone()),
                    ("tenant_id", uuid.clone()),
                    ("scope", string.clone()),
                    ("user_id", uuid.clone()),
                    ("audit", boolean.clone()),
                ],
                &["workspace_id", "tenant_id", "scope", "audit"],
            ),
        ),
        (
            "AuditLogRecordList",
            array(object(
                &[
                    ("id", uuid.clone()),
                    ("workspace_id", uuid.clone()),
                    ("tenant_id", uuid.clone()),
                    ("api_key_id", uuid.clone()),
                    ("user_id", uuid.clone()),
                    ("action", string.clone()),
                    ("detail", json.clone()),
                    ("created_at", date_schema()),
                ],
                &[
                    "id",
                    "workspace_id",
                    "tenant_id",
                    "action",
                    "detail",
                    "created_at",
                ],
            )),
        ),
        (
            "SearchHitList",
            array(object(
                &[
                    ("entity", schema_ref("EntityRecord")),
                    ("distance", json.clone()),
                ],
                &["entity"],
            )),
        ),
        ("JsonSchema", json.clone()),
        (
            "ImportResult",
            object(
                &[
                    ("schemas", integer.clone()),
                    ("entities", integer.clone()),
                    ("relations", integer.clone()),
                ],
                &["schemas", "entities", "relations"],
            ),
        ),
        (
            "InferFillResponse",
            object(
                &[("job_id", string.clone()), ("status", string.clone())],
                &["job_id", "status"],
            ),
        ),
        (
            "InferJobStatus",
            object(
                &[
                    ("job_id", string.clone()),
                    ("status", string.clone()),
                    ("applied", integer.clone()),
                    ("skipped", integer.clone()),
                    ("error", string.clone()),
                ],
                &["job_id", "status"],
            ),
        ),
        (
            "LlmKeyDescription",
            object(
                &[
                    ("base_url", string.clone()),
                    ("model", string.clone()),
                    ("configured", boolean.clone()),
                ],
                &["base_url", "model", "configured"],
            ),
        ),
        (
            "EmbeddingKeyDescription",
            object(
                &[
                    ("base_url", string.clone()),
                    ("model", string.clone()),
                    ("dimensions", integer.clone()),
                    ("configured", boolean.clone()),
                ],
                &["base_url", "model", "dimensions", "configured"],
            ),
        ),
        (
            "WorkerClassAssignment",
            object(&[("worker_class", string.clone())], &["worker_class"]),
        ),
        (
            "ColumnPreference",
            object(
                &[
                    ("entity_type", string.clone()),
                    ("columns", array(string.clone())),
                ],
                &["entity_type", "columns"],
            ),
        ),
        (
            "ColumnPreferenceList",
            array(schema_ref("ColumnPreference")),
        ),
        (
            "MarketplaceListingList",
            array(object(
                &[
                    ("template_id", uuid.clone()),
                    ("name", string.clone()),
                    ("description", string.clone()),
                    ("tags", array(string.clone())),
                    ("author", string.clone()),
                    ("tenant_id", uuid.clone()),
                    ("latest_stable_version", integer.clone()),
                    ("review_count", integer.clone()),
                    ("average_rating", number.clone()),
                ],
                &["template_id", "name", "tags", "tenant_id", "review_count"],
            )),
        ),
        (
            "TemplateVersionRecordList",
            array(schema_ref("TemplateVersionRecord")),
        ),
        (
            "TemplateVersionRecord",
            object(
                &[
                    ("id", uuid.clone()),
                    ("template_id", uuid.clone()),
                    ("version", integer.clone()),
                    ("definition", json.clone()),
                    ("changelog", string.clone()),
                    ("status", string.clone()),
                    ("created_at", date_schema()),
                ],
                &[
                    "id",
                    "template_id",
                    "version",
                    "definition",
                    "status",
                    "created_at",
                ],
            ),
        ),
        (
            "TemplateReviewRecordList",
            array(schema_ref("TemplateReviewRecord")),
        ),
        (
            "TemplateReviewRecord",
            object(
                &[
                    ("id", uuid.clone()),
                    ("template_id", uuid.clone()),
                    ("tenant_id", uuid.clone()),
                    ("rating", integer.clone()),
                    ("comment", string.clone()),
                    ("created_at", date_schema()),
                    ("updated_at", date_schema()),
                ],
                &[
                    "id",
                    "template_id",
                    "tenant_id",
                    "rating",
                    "created_at",
                    "updated_at",
                ],
            ),
        ),
        (
            "ForkResponse",
            object(&[("template_id", uuid.clone())], &["template_id"]),
        ),
        (
            "UpstreamChangeList",
            array(object(
                &[
                    ("schema_id", uuid.clone()),
                    ("schema_name", string.clone()),
                    ("version", integer.clone()),
                    ("template_id", uuid.clone()),
                    ("template_name", string.clone()),
                    ("changed_at", date_schema()),
                    ("pending_notification", boolean.clone()),
                    ("summary", schema_ref("MergeDiffSummary")),
                ],
                &[
                    "schema_id",
                    "schema_name",
                    "version",
                    "template_id",
                    "template_name",
                    "changed_at",
                    "pending_notification",
                    "summary",
                ],
            )),
        ),
        (
            "MergePlan",
            object(
                &[
                    ("fields", array(schema_ref("FieldMerge"))),
                    ("summary", schema_ref("MergeDiffSummary")),
                ],
                &["fields", "summary"],
            ),
        ),
        (
            "MergeResponse",
            object(
                &[
                    ("schema", schema_ref("SchemaRecord")),
                    ("diff", json.clone()),
                    ("summary", json.clone()),
                ],
                &["schema", "diff", "summary"],
            ),
        ),
        (
            "ForkRecord",
            object(
                &[
                    ("id", uuid.clone()),
                    ("tenant_id", uuid.clone()),
                    ("workspace_id", uuid.clone()),
                    ("source_workspace_id", uuid.clone()),
                    ("source_schema_id", uuid.clone()),
                    ("source_schema_version", integer.clone()),
                    ("source_schema_name", string.clone()),
                    ("fork_schema_id", uuid.clone()),
                    ("fork_schema_version", integer.clone()),
                    ("customized", boolean.clone()),
                    ("upstream_version", integer.clone()),
                    ("definition", schema_ref("MetaSchemaDefinition")),
                    ("created_at", date_schema()),
                    ("updated_at", date_schema()),
                ],
                &[
                    "id",
                    "tenant_id",
                    "workspace_id",
                    "source_workspace_id",
                    "source_schema_id",
                    "source_schema_version",
                    "source_schema_name",
                    "fork_schema_id",
                    "fork_schema_version",
                    "customized",
                    "definition",
                    "created_at",
                    "updated_at",
                ],
            ),
        ),
        ("ForkRecordList", array(schema_ref("ForkRecord"))),
        (
            "TenantOverview",
            object(
                &[
                    ("tenant_id", uuid.clone()),
                    ("plan", string.clone()),
                    ("max_workspaces", integer.clone()),
                    ("usage", schema_ref("TenantUsage")),
                    ("members", array(schema_ref("MembershipRecord"))),
                ],
                &["tenant_id", "usage", "members"],
            ),
        ),
        (
            "TenantUsage",
            object(
                &[
                    ("tenant_id", uuid.clone()),
                    ("workspace_count", integer.clone()),
                    ("member_count", integer.clone()),
                    ("entity_count", integer.clone()),
                ],
                &[
                    "tenant_id",
                    "workspace_count",
                    "member_count",
                    "entity_count",
                ],
            ),
        ),
        (
            "FieldMerge",
            object(
                &[
                    ("entity_type", string.clone()),
                    ("field", string.clone()),
                    ("verdict", string.clone()),
                    ("detail", string.clone()),
                ],
                &["entity_type", "field", "verdict", "detail"],
            ),
        ),
        (
            "MergeDiffSummary",
            object(
                &[
                    ("total_fields", integer.clone()),
                    ("auto_add", integer.clone()),
                    ("auto_update", integer.clone()),
                    ("keep_local", integer.clone()),
                    ("conflict", integer.clone()),
                    ("has_conflicts", boolean.clone()),
                ],
                &[
                    "total_fields",
                    "auto_add",
                    "auto_update",
                    "keep_local",
                    "conflict",
                    "has_conflicts",
                ],
            ),
        ),
    ]
}

/// Mount Swagger UI and the enterprise document.
pub(crate) fn mount(router: Router, inventory: &RouteInventory) -> Router {
    let swagger = SwaggerUi::new("/docs").url(
        "/docs/openapi.json",
        openapi_for(inventory, Edition::Enterprise),
    );
    router.merge(swagger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::route_inventory::{RouteClass, RouteEntry};
    use crate::controllers::{self, route_inventory::RouteInventory};
    use loco_rs::controller::AppRoutes;
    use serde_json::Value;

    fn entry(method: Method, path: &str) -> RouteEntry {
        RouteEntry {
            path: path.to_string(),
            method,
            edition: Edition::Community,
            gated: false,
            operation_id: format!("test_{}", path.replace('/', "_")),
            class: RouteClass::Public,
        }
    }

    #[test]
    fn contract_uses_handler_auth_and_exact_success_status() {
        let oauth = contract(&entry(Method::GET, "/auth/oauth/authorize"));
        assert!(matches!(oauth.auth, Auth::Public));
        assert_eq!(oauth.status, "302");

        let entity = contract(&entry(Method::PUT, "/api/entities/{id}"));
        assert!(matches!(entity.auth, Auth::Scope("write")));
        assert_eq!(entity.request, Some("UpdateEntityRequest"));
        assert_eq!(entity.response, "EntityRecord");
    }

    #[test]
    fn schemas_do_not_contain_secret_values() {
        let document = openapi_for(&RouteInventory::default(), Edition::Enterprise);
        let json = serde_json::to_string(&document).unwrap();
        assert!(!json.contains("ysr_"));
    }

    #[test]
    fn every_composed_rest_route_has_a_contract_in_both_editions() {
        let mut inventory = RouteInventory::default();

        macro_rules! add {
            ($routes:expr, $edition:expr, $gated:expr) => {
                let routes = AppRoutes::empty().add_route($routes);
                inventory.add_group(&routes, $edition, $gated, RouteClass::Public);
            };
        }

        add!(controllers::audit_log::routes(), Edition::Community, false);
        add!(controllers::auth::routes(), Edition::Community, false);
        add!(controllers::entities::routes(), Edition::Community, false);
        add!(
            controllers::entities::migration_routes(),
            Edition::Community,
            false
        );
        add!(controllers::export::routes(), Edition::Community, false);
        add!(controllers::import::routes(), Edition::Community, false);
        add!(controllers::members::routes(), Edition::Community, false);
        add!(controllers::relations::routes(), Edition::Community, false);
        add!(controllers::schemas::routes(), Edition::Community, false);
        add!(
            controllers::schemas::template_routes(),
            Edition::Community,
            false
        );
        add!(controllers::search::routes(), Edition::Community, false);
        add!(controllers::setup::routes(), Edition::Community, false);
        add!(controllers::system::routes(), Edition::Community, false);
        add!(
            controllers::template_library::routes(),
            Edition::Community,
            false
        );
        add!(controllers::whoami::routes(), Edition::Community, false);
        add!(controllers::workspaces::routes(), Edition::Community, false);
        add!(
            crate::ee::controllers::dashboard::routes(),
            Edition::Enterprise,
            false
        );
        add!(
            crate::ee::controllers::embedding::routes(),
            Edition::Enterprise,
            false
        );
        add!(
            crate::ee::controllers::entity_columns::routes(),
            Edition::Enterprise,
            false
        );
        add!(
            crate::ee::controllers::inference::routes(),
            Edition::Enterprise,
            false
        );
        add!(
            crate::ee::controllers::inference::gated_routes(),
            Edition::Enterprise,
            true
        );
        add!(
            crate::ee::controllers::inference::inference_job_status_routes(),
            Edition::Enterprise,
            true
        );
        add!(
            crate::ee::controllers::marketplace::routes(),
            Edition::Enterprise,
            true
        );
        add!(
            crate::ee::controllers::oauth::routes(),
            Edition::Enterprise,
            true
        );
        add!(
            crate::ee::controllers::origin::routes(),
            Edition::Enterprise,
            false
        );
        add!(
            crate::ee::controllers::schema_forks::routes(),
            Edition::Enterprise,
            false
        );
        add!(
            crate::ee::controllers::stripe::routes(),
            Edition::Enterprise,
            true
        );
        add!(
            crate::ee::controllers::worker_class::routes(),
            Edition::Enterprise,
            false
        );

        inventory.validate();
        let community = openapi_for(&inventory, Edition::Community);
        let enterprise = openapi_for(&inventory, Edition::Enterprise);
        assert!(community.paths.paths.len() < enterprise.paths.paths.len());
        assert!(
            !serde_json::to_string(&community)
                .unwrap()
                .contains("/api/marketplace")
        );
        assert!(
            serde_json::to_string(&enterprise)
                .unwrap()
                .contains("/api/marketplace")
        );
    }

    #[test]
    fn enterprise_response_components_are_typed() {
        let mut inventory = RouteInventory::default();
        inventory.add_group(
            &AppRoutes::empty().add_route(crate::ee::controllers::marketplace::routes()),
            Edition::Enterprise,
            true,
            RouteClass::Public,
        );
        inventory.add_group(
            &AppRoutes::empty().add_route(crate::ee::controllers::origin::routes()),
            Edition::Enterprise,
            false,
            RouteClass::Public,
        );
        inventory.add_group(
            &AppRoutes::empty().add_route(crate::ee::controllers::schema_forks::routes()),
            Edition::Enterprise,
            false,
            RouteClass::Public,
        );
        inventory.add_group(
            &AppRoutes::empty().add_route(crate::ee::controllers::dashboard::routes()),
            Edition::Enterprise,
            false,
            RouteClass::Public,
        );
        let document = serde_json::to_value(openapi_for(&inventory, Edition::Enterprise)).unwrap();
        for name in [
            "MarketplaceListingList",
            "TemplateVersionRecord",
            "TemplateReviewRecord",
            "UpstreamChangeList",
            "MergePlan",
            "ForkRecord",
            "TenantOverview",
        ] {
            assert_ne!(
                document["components"]["schemas"][name]["type"],
                Value::Null,
                "{name} must have a concrete top-level type"
            );
        }
    }
}
