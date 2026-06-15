//! Subject (task/requirement/etc.) queries, mutations, and change stream.
//!
//! All shapes mirror `animus_subject_protocol::Subject` and the
//! `animus_control_protocol` subject request/response types so the GraphQL
//! schema faithfully represents what the CLI shows.

use animus_control_protocol::types::{
    SubjectCreateRequest, SubjectGetRequest, SubjectListRequest, SubjectNextRequest,
    SubjectStatusRequest, SubjectUpdateRequest, SubjectWatchRequest,
};
use animus_subject_protocol::{
    Subject as WireSubject, SubjectAttachment as WireAttachment, SubjectFilter, SubjectId,
    SubjectPatch, SubjectStatus as WireStatus,
};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, Subscription, ID};
use futures_util::stream::{self, Stream};

use super::client_from_ctx;

/// Normalized cross-backend subject status. Mirrors
/// [`animus_subject_protocol::SubjectStatus`].
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SubjectStatus {
    Ready,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

impl From<WireStatus> for SubjectStatus {
    fn from(s: WireStatus) -> Self {
        match s {
            WireStatus::Ready => SubjectStatus::Ready,
            WireStatus::InProgress => SubjectStatus::InProgress,
            WireStatus::Blocked => SubjectStatus::Blocked,
            WireStatus::Done => SubjectStatus::Done,
            WireStatus::Cancelled => SubjectStatus::Cancelled,
        }
    }
}

impl From<SubjectStatus> for WireStatus {
    fn from(s: SubjectStatus) -> Self {
        match s {
            SubjectStatus::Ready => WireStatus::Ready,
            SubjectStatus::InProgress => WireStatus::InProgress,
            SubjectStatus::Blocked => WireStatus::Blocked,
            SubjectStatus::Done => WireStatus::Done,
            SubjectStatus::Cancelled => WireStatus::Cancelled,
        }
    }
}

/// An attachment carried by a [`Subject`].
#[derive(SimpleObject, Default)]
pub struct SubjectAttachment {
    pub id: String,
    pub kind: String,
    pub uri: String,
    pub title: Option<String>,
    pub mime_type: Option<String>,
    /// Free-form backend metadata, serialized as a JSON string.
    pub metadata: Option<String>,
}

impl From<WireAttachment> for SubjectAttachment {
    fn from(a: WireAttachment) -> Self {
        SubjectAttachment {
            id: a.id,
            kind: a.kind,
            uri: a.uri,
            title: a.title,
            mime_type: a.mime_type,
            metadata: (!a.metadata.is_null()).then(|| a.metadata.to_string()),
        }
    }
}

/// A normalized cross-backend subject. Mirrors
/// [`animus_subject_protocol::Subject`].
#[derive(SimpleObject)]
pub struct Subject {
    pub id: ID,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub status: SubjectStatus,
    /// Backend-raw status string (e.g. `"In Review"`), when richer than the
    /// normalized bucket.
    pub native_status: Option<String>,
    /// Free-form backend status payload, serialized as a JSON string.
    pub status_metadata: Option<String>,
    /// Priority on a 0..=4 scale.
    pub priority: Option<i32>,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub parent: Option<ID>,
    pub children: Vec<ID>,
    pub url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Backend-specific custom fields, serialized as a JSON string.
    pub custom: Option<String>,
    pub attachments: Vec<SubjectAttachment>,
}

impl From<WireSubject> for Subject {
    fn from(s: WireSubject) -> Self {
        Subject {
            id: ID(s.id.as_str().to_string()),
            kind: s.kind,
            title: s.title,
            description: s.description,
            status: s.status.into(),
            native_status: s.native_status,
            status_metadata: (!s.status_metadata.is_null()).then(|| s.status_metadata.to_string()),
            priority: s.priority.map(|p| p as i32),
            assignee: s.assignee,
            labels: s.labels,
            parent: s.parent.map(|p| ID(p.as_str().to_string())),
            children: s
                .children
                .into_iter()
                .map(|c| ID(c.as_str().to_string()))
                .collect(),
            url: s.url,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            custom: (!s.custom.is_empty())
                .then(|| serde_json::to_string(&s.custom).unwrap_or_default()),
            attachments: s
                .attachments
                .into_iter()
                .map(SubjectAttachment::from)
                .collect(),
        }
    }
}

#[derive(InputObject)]
pub struct CreateSubjectInput {
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub status: Option<SubjectStatus>,
    /// Priority on a 0..=4 scale.
    pub priority: Option<i32>,
    #[graphql(default)]
    pub labels: Vec<String>,
    pub assignee: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateSubjectInput {
    pub id: ID,
    pub status: Option<SubjectStatus>,
    /// `Some(...)` sets the assignee; pass an empty string to clear it.
    pub assignee: Option<String>,
    #[graphql(default)]
    pub labels_add: Vec<String>,
    #[graphql(default)]
    pub labels_remove: Vec<String>,
    pub comment: Option<String>,
}

#[derive(SimpleObject, Default)]
pub struct SubjectChangeEvent {
    pub subject_id: ID,
    pub change: String,
    pub at: String,
}

#[derive(Default)]
pub struct SubjectQuery;

#[Object]
impl SubjectQuery {
    /// List subjects, optionally filtered by kind and/or status.
    async fn subject(
        &self,
        ctx: &Context<'_>,
        kind: Option<String>,
        status: Option<SubjectStatus>,
    ) -> Result<Vec<Subject>> {
        let client = client_from_ctx(ctx).await?;
        let filter = SubjectFilter {
            kind: kind.into_iter().collect(),
            status: status.map(WireStatus::from).into_iter().collect(),
            ..SubjectFilter::default()
        };
        let response = client
            .subject_list(SubjectListRequest { filter })
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/list failed: {e}")))?;
        Ok(response.subjects.into_iter().map(Subject::from).collect())
    }

    /// Look up a single subject by id.
    async fn subject_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Subject> {
        let client = client_from_ctx(ctx).await?;
        let subject = client
            .subject_get(SubjectGetRequest {
                id: SubjectId::new(id.to_string()),
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/get failed: {e}")))?;
        Ok(subject.into())
    }

    /// Highest-priority Ready subject, optionally restricted to a kind.
    async fn subject_next(
        &self,
        ctx: &Context<'_>,
        kind: Option<String>,
    ) -> Result<Option<Subject>> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .subject_next(SubjectNextRequest { kind })
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/next failed: {e}")))?;
        Ok(response.subject.map(Subject::from))
    }
}

#[derive(Default)]
pub struct SubjectMutation;

#[Object]
impl SubjectMutation {
    async fn create_subject(
        &self,
        ctx: &Context<'_>,
        input: CreateSubjectInput,
    ) -> Result<Subject> {
        let client = client_from_ctx(ctx).await?;
        let request = SubjectCreateRequest {
            kind: input.kind,
            title: input.title,
            body: input.body,
            status: input.status.map(WireStatus::from),
            priority: input.priority.map(|p| p.clamp(0, 4) as u8),
            labels: input.labels,
            assignee: input.assignee,
            custom: Default::default(),
        };
        let subject = client
            .subject_create(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/create failed: {e}")))?;
        Ok(subject.into())
    }

    async fn update_subject(
        &self,
        ctx: &Context<'_>,
        input: UpdateSubjectInput,
    ) -> Result<Subject> {
        let client = client_from_ctx(ctx).await?;
        let patch = SubjectPatch {
            status: input.status.map(WireStatus::from),
            assignee: input
                .assignee
                .map(|a| if a.is_empty() { None } else { Some(a) }),
            labels_add: input.labels_add,
            labels_remove: input.labels_remove,
            comment: input.comment,
            custom: Default::default(),
        };
        let subject = client
            .subject_update(SubjectUpdateRequest {
                id: SubjectId::new(input.id.to_string()),
                patch,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/update failed: {e}")))?;
        Ok(subject.into())
    }

    /// Set a subject's normalized status.
    async fn set_subject_status(
        &self,
        ctx: &Context<'_>,
        id: ID,
        status: SubjectStatus,
    ) -> Result<Subject> {
        let client = client_from_ctx(ctx).await?;
        let subject = client
            .subject_status(SubjectStatusRequest {
                id: SubjectId::new(id.to_string()),
                status: status.into(),
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/status failed: {e}")))?;
        Ok(subject.into())
    }
}

#[derive(Default)]
pub struct SubjectChangedSubscription;

#[Subscription]
impl SubjectChangedSubscription {
    async fn subject_changed(
        &self,
        ctx: &Context<'_>,
        kind: Option<String>,
    ) -> Result<impl Stream<Item = SubjectChangeEvent>> {
        let client = client_from_ctx(ctx).await?;
        let request = SubjectWatchRequest { kind, filter: None };
        let subscription = client
            .subject_watch(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("subject/watch failed: {e}")))?;
        Ok(stream::unfold(
            (subscription, client),
            |(mut sub, client)| async move {
                let event = sub.recv().await?;
                let projected = SubjectChangeEvent {
                    subject_id: ID(event.id.as_str().to_string()),
                    change: format!("{:?}", event.change_kind).to_lowercase(),
                    at: event.subject.updated_at.to_rfc3339(),
                };
                Some((projected, (sub, client)))
            },
        ))
    }
}
