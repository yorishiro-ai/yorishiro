use sea_query::{Alias, Expr, Iden, IntoIden, OnConflict, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;

use super::get_tenant;
use super::users::Users;
use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::{MembershipRecord, MembershipRole};

#[derive(Iden)]
pub(super) enum TenantMemberships {
    Table,
    Id,
    TenantId,
    UserId,
    Role,
    CreatedAt,
}

/// Adds (or updates the role of) a user's membership in a tenant.
///
/// Takes `&mut PgConnection` (rather than `&PgPool`) so a caller can compose this with `create_user` (and anything else) in one transaction: see `create_user`'s doc comment for why.
/// Pass `&mut pool.acquire().await?` for a standalone call.
pub async fn add_member<C>(
    conn: &mut C,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    crate::models::tenancy::TenantRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    get_tenant(&mut *conn, tenant_id).await?;

    let (cols, vals) = crate::db::with_generated_id::<C, _>(
        TenantMemberships::Id,
        vec![
            TenantMemberships::TenantId.into_iden(),
            TenantMemberships::UserId.into_iden(),
            TenantMemberships::Role.into_iden(),
        ],
        vec![tenant_id.into(), user_id.into(), role.as_db_str().into()],
    );
    let (sql, values) = Query::insert()
        .into_table(C::schema_table("identity", TenantMemberships::Table))
        .columns(cols)
        .values_panic(vals)
        .on_conflict(
            OnConflict::columns([TenantMemberships::TenantId, TenantMemberships::UserId])
                .update_column(TenantMemberships::Role)
                .to_owned(),
        )
        .build_sqlx(C::builder());

    sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;
    Ok(())
}

/// Looks up a single user's role within a tenant, or `None` if they aren't a member.
pub async fn get_membership_role(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    let (sql, values) = Query::select()
        .column(TenantMemberships::Role)
        .from((Alias::new("identity"), TenantMemberships::Table))
        .and_where(Expr::col(TenantMemberships::TenantId).eq(tenant_id))
        .and_where(Expr::col(TenantMemberships::UserId).eq(user_id))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(String,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    row.map(|(role,)| {
        MembershipRole::from_db_str(&role).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown membership role in database: {role}"
            ))
        })
    })
    .transpose()
}

pub async fn list_members(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    #[derive(sqlx::FromRow)]
    struct MembershipRow {
        user_id: Uuid,
        email: String,
        display_name: Option<String>,
        role: String,
    }

    let (sql, values) = Query::select()
        .expr_as(Expr::col((Users::Table, Users::Id)), Alias::new("user_id"))
        .columns([
            (Users::Table, Users::Email),
            (Users::Table, Users::DisplayName),
        ])
        .column((TenantMemberships::Table, TenantMemberships::Role))
        .from((Alias::new("identity"), TenantMemberships::Table))
        .inner_join(
            (Alias::new("identity"), Users::Table),
            Expr::col((Users::Table, Users::Id))
                .equals((TenantMemberships::Table, TenantMemberships::UserId)),
        )
        .and_where(Expr::col((TenantMemberships::Table, TenantMemberships::TenantId)).eq(tenant_id))
        .order_by(
            (TenantMemberships::Table, TenantMemberships::CreatedAt),
            Order::Asc,
        )
        .build_sqlx(PostgresQueryBuilder);

    let rows: Vec<MembershipRow> = sqlx::query_as_with(&sql, values)
        .fetch_all(pool)
        .await
        .internal()?;

    rows.into_iter()
        .map(|row| {
            let role = MembershipRole::from_db_str(&row.role).ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "unknown membership role in database: {}",
                    row.role
                ))
            })?;
            Ok(MembershipRecord {
                user_id: row.user_id,
                email: row.email,
                display_name: row.display_name,
                role,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../tests/models/tenancy/memberships.rs"]
mod tests;
