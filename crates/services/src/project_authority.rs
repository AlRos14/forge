use db::{Project, ProjectMemberRepo, SqliteDb};

/// Project visibility is a read policy. Ownerless Projects remain readable, but
/// that public visibility does not grant mutation authority.
pub async fn user_can_read_project(
    db: &SqliteDb,
    project: &Project,
    user_id: &str,
) -> Result<bool, db::DbError> {
    if project.owner_id.is_none() || project.owner_id.as_deref() == Some(user_id) {
        return Ok(true);
    }
    Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_some())
}

/// Ordinary Project work is available to explicit members, admins, and owners.
/// A Project owner remains authoritative even if its best-effort membership row
/// is absent.
pub async fn user_can_write_project(
    db: &SqliteDb,
    project: &Project,
    user_id: &str,
) -> Result<bool, db::DbError> {
    if project.owner_id.as_deref() == Some(user_id) {
        return Ok(true);
    }
    Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_some_and(|member| matches!(member.role.as_str(), "owner" | "admin" | "member")))
}

/// Project administration is limited to the canonical Project owner or an
/// explicit owner/admin Project membership.
pub async fn user_can_administer_project(
    db: &SqliteDb,
    project: &Project,
    user_id: &str,
) -> Result<bool, db::DbError> {
    if project.owner_id.as_deref() == Some(user_id) {
        return Ok(true);
    }
    Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_some_and(|member| matches!(member.role.as_str(), "owner" | "admin")))
}
