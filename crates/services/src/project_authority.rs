use db::{Project, ProjectMemberRepo, SqliteDb};

/// Project visibility is a read policy. Ownerless Projects remain readable, but
/// that public visibility does not grant mutation authority. A non-ownerless
/// Project is readable only through its exact membership representation; the
/// canonical owner is required to have that row (INV-040).
pub async fn user_can_read_project(
    db: &SqliteDb,
    project: &Project,
    user_id: &str,
) -> Result<bool, db::DbError> {
    if project.owner_id.is_none() {
        return Ok(true);
    }
    Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_some())
}

/// Ordinary Project work is available to explicit owner/admin/member
/// memberships. Canonical identity does not bypass the membership row.
pub async fn user_can_write_project(
    db: &SqliteDb,
    project: &Project,
    user_id: &str,
) -> Result<bool, db::DbError> {
    Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_some_and(|member| matches!(member.role.as_str(), "owner" | "admin" | "member")))
}

/// Project administration is limited to an explicit owner/admin membership.
pub async fn user_can_administer_project(
    db: &SqliteDb,
    project: &Project,
    user_id: &str,
) -> Result<bool, db::DbError> {
    Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_some_and(|member| matches!(member.role.as_str(), "owner" | "admin")))
}
